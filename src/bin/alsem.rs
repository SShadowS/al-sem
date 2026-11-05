//! `alsem` — the future al-sem production CLI (the Rust port). Stage 1 ships the
//! `analyze` GATE path: `analyze <ws> --preset <name> --format sarif` byte-matches
//! the al-sem TS CLI's SARIF gate goldens.
//!
//! This is a NEW bin, separate from `aldump` (the R0 differential producer). It does
//! NOT touch the LSP shipping code.
//!
//! Output discipline: SARIF goes to stdout; errors go to stderr. The SARIF is written
//! followed by a trailing newline, matching al-sem's
//! `process.stdout.write(`${formatSarif(...)}\n`)`.

use std::io::IsTerminal;
use std::process::ExitCode;

/// The analyze path is allocation-bound end to end: an attributed 8020 profile
/// put **16.6 % of the whole run inside `free()` alone** (`gate.teardown` 13.8 %
/// plus `context.ctx_drop` 2.8 %), on top of allocation costs spread through
/// every other span. Swapping the platform allocator is therefore a global
/// multiplier on this workload, not a micro-optimization.
///
/// **Scoped to this binary on purpose.** A `#[global_allocator]` is
/// per-executable, so the library, the LSP server (`src/main.rs` — a long-lived
/// process with different allocator trade-offs, and unmeasured here), the benches
/// and every test target keep the platform default.
///
/// This does NOT excuse allocation churn — it makes a future churn regression
/// *cheaper*, and therefore harder to see. Keep counting allocations.
///
/// `ReleasingMiMalloc` is `mimalloc::MiMalloc` for every command but
/// `engine-host`, which turns on the release of freed large blocks (and,
/// under `--low-mem`, turns transparent huge pages off): `al_sem::release_alloc`,
/// the resident child's per-refresh RSS creep.
#[global_allocator]
static GLOBAL: al_sem::release_alloc::ReleasingMiMalloc = al_sem::release_alloc::ReleasingMiMalloc;

/// mimalloc's `purge_delay` option: freed OS pages are held for N milliseconds
/// before being returned (default 10). `alsem` sets it to 0 — purge immediately.
///
/// **This is load-bearing, not tuning.** With the default delay mimalloc is a
/// peak-RSS REGRESSION on a small workspace: DO measured 1,598 MB under the
/// platform allocator and 1,717 MB under mimalloc (+7.4 %). At 0 it measures
/// 1,577 MB — below the platform allocator — and BC Base App 8020 drops from
/// 5,320 MB to ~4,945 MB. The cost is real and paid knowingly: immediate purging
/// gives back roughly 7–9 % of the wall-clock win.
///
/// `libmimalloc-sys` exports no constant for it (`mi_option_eager_commit_delay`,
/// its lower neighbour, is behind that crate's `v2` feature and so is unusable
/// here), so it is derived from `mi_option_use_numa_nodes`, which IS exported
/// unconditionally. Both the v2 and the v3 `mimalloc.h` run
/// `… purge_delay, use_numa_nodes, …`, so `purge_delay` is exactly one slot
/// below it. The const assertions pin the surrounding numbering against the two
/// unconditionally-exported neighbours; if the binding's option numbering ever
/// shifts, this fails to COMPILE rather than silently setting a different option.
///
/// The semantic check is the measurement: a wrong index cannot move peak RSS, and
/// this moves it to exactly the figure the `MIMALLOC_PURGE_DELAY=0` env-var
/// control run produces.
const MI_OPTION_PURGE_DELAY: libmimalloc_sys::mi_option_t =
    libmimalloc_sys::mi_option_use_numa_nodes - 1;
const _: () = assert!(
    libmimalloc_sys::mi_option_use_numa_nodes == 16
        && libmimalloc_sys::mi_option_limit_os_alloc
            == libmimalloc_sys::mi_option_use_numa_nodes + 1,
    "libmimalloc-sys' option numbering shifted — re-derive purge_delay's index \
     from mimalloc.h before trusting MI_OPTION_PURGE_DELAY"
);

use std::path::{Path, PathBuf};

use al_sem::engine::batch::admission::{JobKind, admit};
use al_sem::engine::batch::job::{
    Failed, HandOff, Job, ViewUniverse, WorkspaceRead, WorkspaceReader,
};
use al_sem::engine::batch::manifest::{JobPaths, check_write_sets};
use al_sem::engine::batch::output::{JobOutput, WriteAction, replay};
use al_sem::engine::batch::plan_cx::{PlanCx, PlanReads};
use al_sem::engine::batch::worker::{
    WorkerBatch, WorkerInput, WorkerJob, Workers, report_peak, run_workers,
};
use al_sem::engine::batch::{JobLine, JobStreams, run_batch, run_standalone};
use al_sem::engine::gate::cache_prune::{format_prune_report, prune_cache};
use al_sem::engine::gate::events::{EventsChainsOptions, EventsFanoutOptions, EventsJob};
use al_sem::engine::gate::exit_code::{exit, parse_fail_on};
use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::presets::{preset_names, resolve_analyze_detectors};
use al_sem::engine::gate::run::{AnalyzeArgs, AnalyzeJob, OutputFormat};
use al_sem::engine::gate::version::driver_version;
use al_sem::engine::l4::effect_query_cli::{Direction, QueryJob};
use al_sem::engine::l5::digest_cli::{
    ChangedAutoDetect, DiffRoot, DigestJob, DigestRequest, auto_detect_changed_with,
};
use al_sem::engine::l5::event_flow::Scope as EventScope;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintJob, FingerprintOptions, ShardMode, SpecifiedFlags, default_format,
    normalize_witness, reads_single_app, reject_illegal_combos, reject_inventory_only_combos,
    reject_shard_without_out, validate_roots,
};
use al_sem::engine::l5::prove::{ProveJob, parse_question, question_ids};
use al_sem::program::inputs::{FileInput, RequestInputs, WorkspaceNeeds};
use clap::{Parser, Subcommand};
use std::sync::Arc;

const SEVERITY_VALUES: &[&str] = &["critical", "high", "medium", "low", "info"];

#[derive(Parser)]
#[command(
    name = "al-sem",
    about = "Static semantic analysis engine for Microsoft Business Central AL code (Rust port)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze an AL workspace and emit findings (Stage 1: --format sarif).
    Analyze(AnalyzeCli),
    /// Digest an AL workspace: changed-root effect summary (cli-b/b1).
    Digest(DigestCli),
    /// Prove an absence-safety question about a single routine (cli-b/b2).
    Prove(ProveCli),
    /// Explain a routine: its own rules (checks with their conditions and
    /// messages), what it writes, commits and raises, the page actions and
    /// tests that reach it, and what the engine could not see.
    Explain(ExplainCli),
    /// Fingerprint an AL workspace: root-classification capability summary (cli-b/b3).
    Fingerprint(FingerprintCli),
    /// Diff two capability snapshots (or workspaces): ABI/schema/events/capabilities/permissions (cli-b/b4).
    Diff(DiffCli),
    /// Event fan-out and chain reports (cli-c/c1).
    Events(EventsCli),
    /// Policy check + explain over rootClassifications + capabilities (cli-c/c2).
    Policy(PolicyCli),
    /// Inspect and maintain the dependency cache (cli-c/c3).
    Cache(CacheCli),
    /// Query the db-effect store: which routines touch a table, and what a
    /// routine's transitive db effects are.
    Query(QueryCli),
    /// Write the program's may-execute call graph as JSONL (schema
    /// `al-sem.callgraph` v1), for diff-scoped test selection.
    Graph(GraphCli),
    /// Which tests to run first for a change: the test scope (the
    /// workspace's own [Test] procedures and the sibling test apps, named,
    /// configured or discovered), each changed routine's status, the ranked
    /// tests that reach it, and whether the selection is complete (when not:
    /// run the whole suite as well, with the reasons).
    Tests(TestsCli),
    /// The impact of a change on the current revision: every changed line
    /// accounted first, then per changed routine who reaches it (callers,
    /// page actions by caption, event publishers, entries, external callers),
    /// its current effects and its tests. Current effects only: no base is
    /// built, so there is no before/after.
    Impact(ImpactCli),
    /// Run the commands of a JSON manifest: jobs of one batch on one
    /// workspace share one build. Prints one JSON line per job.
    ///
    /// One batch runs in this process. Two or more run in worker processes,
    /// at most four at once and at most two when a job is `policy check`: as
    /// many as fit a 28 GiB budget at the largest measured peak of the run's
    /// job kinds. The peak table is calibrated on one large workspace (CDO);
    /// a larger workspace needs it re-measured. The budget holds only for
    /// manifests no heavier than the measured shapes: many large-root digests
    /// on one workspace, or many large outputs that wait for a later job, can
    /// exceed it.
    Batch(BatchCli),
    /// The engine child of a context-server front end (internal): speaks the
    /// engine protocol on stdin/stdout (`al_sem::engine::host`).
    #[command(hide = true, name = "engine-host")]
    EngineHost(EngineHostCli),
    /// Ask one command through an engine child, as a front end does
    /// (internal): `alsem engine-ask -- <command line>` prints the answer's
    /// exact stdout and stderr and exits with its exit code.
    #[command(hide = true, name = "engine-ask")]
    EngineAsk(EngineAskCli),
    /// Serve the Model Context Protocol on stdin/stdout, for AI agents
    /// (Claude Code and others): explain, prove, digest and query as tools.
    ///
    /// The server speaks JSON-RPC 2.0, one JSON object per line; its logs go
    /// to stderr. Each tool call is run as the CLI command it names, by an
    /// engine child process (one per workspace and universe), and answers
    /// the CLI's exact output with its exit code: inline up to
    /// `maxInlineBytes` (default 64 KiB), else as a file. Tools:
    /// `alsem_explain` (default format human), `alsem_prove`, `alsem_digest`
    /// (always a file, with a summary), `alsem_query_effects`,
    /// `alsem_query_touches`, `alsem_tests` (a composite request: the
    /// production engine and one test-universe engine per test app),
    /// `alsem_impact` (a composite request too: the change accounted, its
    /// routines' callers, current effects and tests), `alsem_status`,
    /// `alsem_refresh`.
    ///
    /// Files go only into a private folder the server creates under
    /// `--output-root`; a tool's `out` must be inside it. The engine child
    /// keeps the built model: the first call on a workspace builds it, later
    /// calls answer from it, and an edit is picked up by an incremental
    /// refresh before the next answer (with the default `fresh`).
    ///
    /// Register it with Claude Code:
    /// `claude mcp add alsem -- alsem mcp --workspace <ws>`.
    Mcp(McpCli),
}

/// `McpCli` — arguments for `alsem mcp`.
#[derive(Parser)]
struct McpCli {
    /// The workspace of a tool call that names none (a call's `workspace`
    /// input overrides it).
    #[arg(long = "workspace")]
    workspace: Option<String>,

    /// Analyse workspaces alone by default (a call's `singleApp` overrides
    /// it). The default loads the dependencies each workspace requires.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// The default freshness of a call: fresh (refresh first when an input
    /// changed) | latest-built (answer at once from the last build, marked
    /// stale, and refresh in the background) | verify (hash every input).
    #[arg(long = "freshness", default_value = "fresh")]
    freshness: String,

    /// The folder the server's output folder is created in (default
    /// `~/.al-sem/mcp-out`). Each server run makes its own private folder
    /// there; folders older than 24 hours are removed at start.
    #[arg(long = "output-root")]
    output_root: Option<String>,

    /// Each engine child's memory budget (bytes, or with K, M or G; default
    /// 6G cross-app, 2G single-app), capped by what --total-budget leaves.
    #[arg(long = "memory-budget")]
    memory_budget: Option<String>,

    /// Start each engine child with --low-mem: it keeps no dependency syntax
    /// trees between revisions (less memory; a declaration-level edit parses
    /// the dependency files again).
    #[arg(long = "low-mem", default_value_t = false)]
    low_mem: bool,

    /// Seconds an engine child stays with nothing to do before it ends (the
    /// next call starts a new one, which builds again); 0 never.
    #[arg(long = "idle-timeout", default_value_t = al_sem::engine::resident::budget::DEFAULT_IDLE_TIMEOUT_SECS)]
    idle_timeout: u64,

    /// The most engine children alive at once (one per workspace and
    /// universe); a call for another ends the least recently used idle one.
    #[arg(long = "max-children", default_value_t = al_sem::mcp::DEFAULT_MAX_CHILDREN)]
    max_children: usize,

    /// The memory all engine children may budget together (bytes, or with
    /// K, M or G); default: --max-children times the per-child budget.
    #[arg(long = "total-budget")]
    total_budget: Option<String>,
}

/// `EngineHostCli` — arguments for `alsem engine-host` (context-server
/// design §7).
#[derive(Parser)]
struct EngineHostCli {
    /// The memory budget (bytes, or with K, M or G): every build, fill and
    /// request's trees reserve their estimated peak first, and evict or
    /// refuse (`budget`) when it does not fit. Default 6G cross-app, 2G
    /// single-app.
    #[arg(long = "memory-budget")]
    memory_budget: Option<String>,

    /// Keep no dependency syntax trees between revisions: less memory, and a
    /// declaration-level edit parses the dependency files again (cross-app
    /// only; a single-app session holds none either way).
    #[arg(long = "low-mem", default_value_t = false)]
    low_mem: bool,

    /// Build the sited graph (the may-execute graph, one edge per site) with
    /// every revision, for the questions about a routine's callers
    /// (agent-tasks design §1.2 M0-1).
    #[arg(long = "graph", default_value_t = false)]
    graph: bool,

    /// End after this many seconds with no request and no work (the front
    /// end starts a new child on its next request); 0 never ends.
    #[arg(long = "idle-timeout", default_value_t = al_sem::engine::resident::budget::DEFAULT_IDLE_TIMEOUT_SECS)]
    idle_timeout: u64,

    /// Test override of the grace after the idle notice (milliseconds).
    #[arg(long = "idle-grace-ms", hide = true)]
    idle_grace_ms: Option<u64>,

    /// Test override of the pre-model estimate's bytes per `.al` byte.
    #[arg(long = "estimate-per-al-byte", hide = true)]
    estimate_per_al_byte: Option<u64>,

    /// Test override of the pre-model estimate's floor (bytes).
    #[arg(long = "estimate-floor", hide = true)]
    estimate_floor: Option<u64>,
}

/// `EngineAskCli` — arguments for `alsem engine-ask`.
#[derive(Parser)]
struct EngineAskCli {
    /// The inline stdout cap in bytes; a larger stdout comes back as a file.
    #[arg(long = "max-inline")]
    max_inline: Option<u64>,

    /// Print the `session/ask` params this front end would send (its
    /// decisions and request files included) as JSON, and exit.
    #[arg(long = "params")]
    params: bool,

    /// The ask's freshness: fresh | latest-built | verify.
    #[arg(long = "freshness", default_value = "fresh")]
    freshness: String,

    /// The command line to ask, after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    argv: Vec<String>,
}

/// `BatchCli` — arguments for `alsem batch <manifest>` (#277 D2.1).
#[derive(Parser)]
struct BatchCli {
    /// The manifest: `{"batches": [{"jobs": [...]}]}` or `{"jobs": [...]}`; a job
    /// is `{"argv": [...], "stdout": <path>, "stderr": <path>}`, both paths optional.
    manifest: String,

    /// The directory of the jobs' default stdout/stderr files (default: the
    /// manifest's directory).
    #[arg(long = "out-dir")]
    out_dir: Option<String>,

    /// Run batch `b` alone, as a worker of a parent run (#277 D2.5).
    #[arg(long, hide = true)]
    worker: Option<u32>,
}

/// `GraphCli` — arguments for `alsem graph <ws>`.
#[derive(Parser)]
struct GraphCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Write the JSONL to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Keep every routine of an app loaded from local source (a source-folder
    /// dependency) through pruning: true or false. Default: true when such an
    /// app is loaded (`meta.apps[].sourceRoot` names its folder).
    #[arg(long = "keep-local-sources")]
    keep_local_sources: Option<bool>,
}

/// `TestsCli` — arguments for `alsem tests <ws>` (agent-tasks design §4.2).
#[derive(Parser, Clone)]
struct TestsCli {
    /// Path to the AL workspace root (the production app).
    workspace: String,

    /// A changed routine (the selector grammar of `explain --routine`).
    /// Repeatable.
    #[arg(long = "routine")]
    routine: Vec<String>,

    /// A changed file (workspace-relative): every routine in it. Repeatable.
    #[arg(long = "file")]
    file: Vec<String>,

    /// A unified diff (a file, or `-` for stdin): the routines it touches.
    #[arg(long = "diff")]
    diff: Option<String>,

    /// The folder the diff's paths are relative to (as `digest --diff-root`).
    #[arg(long = "diff-root")]
    diff_root: Option<String>,

    /// A sibling test app's folder (repeatable). Replaces `testApps` in
    /// `.al-sem.json` and auto-discovery.
    #[arg(long = "test-app")]
    test_app: Vec<String>,

    /// Which tests `run first` lists: local (default: every hop in the
    /// workspace or a test app), dependency (also paths through dependency
    /// code), all (also paths over open sites).
    #[arg(long = "tier", default_value = "local")]
    tier: String,

    /// List at most N tests in `runFirst` (the counts stay whole).
    #[arg(long = "max-tests")]
    max_tests: Option<usize>,

    /// Output format: human | json (`al-sem.tests/1`).
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Resolve the changed routines in the workspace alone.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Write to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Internal: one part of the answer (`roots` in the production universe,
    /// `select` in a test universe), as JSON.
    #[arg(long = "part", hide = true)]
    part: Option<String>,

    /// Internal (`--part select`): the production app's folder, injected as a
    /// source dependency of the test app.
    #[arg(long = "production-root", hide = true)]
    production_root: Option<String>,

    /// Internal (`--part select`).
    #[arg(long = "production-app-id", hide = true)]
    production_app_id: Option<String>,

    /// Internal (`--part select`).
    #[arg(long = "test-app-id", hide = true)]
    test_app_id: Option<String>,

    /// Internal (`--part select`): how the app was found.
    #[arg(long = "found", hide = true)]
    found: Option<String>,

    /// Internal (`--part select`): a changed root's StableRoutineId.
    #[arg(long = "root-id", hide = true)]
    root_id: Vec<String>,

    /// Internal (`--part select`): a routine probed apart (impact's nearest
    /// tested caller); it never joins the selection.
    #[arg(long = "probe-id", hide = true)]
    probe_id: Vec<String>,
}

/// `ImpactCli` — arguments for `alsem impact <ws>` (agent-tasks design §8).
#[derive(Parser, Clone)]
struct ImpactCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// A changed routine (the selector grammar of `explain --routine`).
    /// Repeatable.
    #[arg(long = "routine")]
    routine: Vec<String>,

    /// A changed file (workspace-relative): every routine in it. Repeatable.
    #[arg(long = "file")]
    file: Vec<String>,

    /// A unified diff (a file, or `-` for stdin): every changed line.
    #[arg(long = "diff")]
    diff: Option<String>,

    /// The folder the diff's paths are relative to (as `digest --diff-root`).
    #[arg(long = "diff-root")]
    diff_root: Option<String>,

    /// Compare the working tree with the merge base of REV and HEAD
    /// (committed, staged, unstaged and untracked .al files).
    #[arg(long = "base")]
    base: Option<String>,

    /// A sibling test app's folder (repeatable), as `alsem tests --test-app`.
    #[arg(long = "test-app")]
    test_app: Vec<String>,

    /// Do not compute the tests (no test universe is built).
    #[arg(long = "no-tests", default_value_t = false)]
    no_tests: bool,

    /// Show at most N routines in the human output (default 20; the JSON
    /// holds every one).
    #[arg(long = "max-roots")]
    max_roots: Option<usize>,

    /// More of each list per routine (callers, entries, publishers) and
    /// every hunk outside routines and every diagnostic.
    #[arg(long = "detail", default_value_t = false)]
    detail: bool,

    /// Assume no app outside the program depends on these apps: report no
    /// external callers.
    #[arg(long = "no-dependents", default_value_t = false)]
    no_dependents: bool,

    /// Output format: human | json (`al-sem.impact/1`).
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Resolve in the workspace alone.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Write to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Internal: one part of the answer (`main` or `up`), as JSON.
    #[arg(long = "part", hide = true)]
    part: Option<String>,

    /// Internal (`--part up`): a changed root's StableRoutineId.
    #[arg(long = "root-id", hide = true)]
    root_id: Vec<String>,
}

impl ImpactCli {
    fn request(&self) -> al_sem::engine::impact::run::ImpactRequest {
        al_sem::engine::impact::run::ImpactRequest {
            workspace: self.workspace.clone(),
            routines: self.routine.clone(),
            files: self.file.clone(),
            diff: self.diff.is_some(),
            diff_root: self.diff_root.clone(),
            base: self.base.clone(),
            test_apps: self.test_app.clone(),
            single_app: self.single_app,
            tests: !self.no_tests,
            assume_no_dependents: self.no_dependents,
            detail: self.detail,
        }
    }
}

/// The selector and mode rules of `alsem impact` (spec §12.1), shared with
/// the MCP front end's wording: exactly one of routines, files, diff, base.
fn impact_modes(t: &ImpactCli) -> Result<(), String> {
    let modes = [
        !t.routine.is_empty(),
        !t.file.is_empty(),
        t.diff.is_some(),
        t.base.is_some(),
    ];
    if t.base.is_some() && t.diff.is_some() {
        return Err("al-sem: impact: --base with --diff is a before/after delta (milestone B2), not available yet; give one of them".into());
    }
    if modes.iter().filter(|m| **m).count() != 1 {
        return Err(
            "al-sem: impact: give exactly one of --routine, --file, --diff or --base".into(),
        );
    }
    if t.diff_root.is_some() && t.diff.is_none() {
        return Err("al-sem: impact: --diff-root goes with --diff".into());
    }
    Ok(())
}

impl TestsCli {
    /// The test universe a select part runs over.
    fn universe(&self) -> al_sem::engine::test_select::universe::TestUniverse {
        use al_sem::engine::test_select::universe::TestUniverse;
        let prod_id = self.production_app_id.clone().unwrap_or_default();
        match &self.production_root {
            Some(p) => TestUniverse::new(
                Path::new(&self.workspace),
                self.test_app_id.as_deref().unwrap_or(""),
                Path::new(p),
                &prod_id,
                self.found.as_deref().unwrap_or("auto"),
            ),
            None => TestUniverse::workspace(Path::new(&self.workspace), &prod_id),
        }
    }

    fn request(&self) -> al_sem::engine::test_select::run::TestsRequest {
        al_sem::engine::test_select::run::TestsRequest {
            workspace: self.workspace.clone(),
            routines: self.routine.clone(),
            files: self.file.clone(),
            diff: self.diff.clone(),
            diff_root: self.diff_root.clone(),
            test_apps: self.test_app.clone(),
            single_app: self.single_app,
        }
    }
}

/// `QueryCli` — top-level `query` subcommand group (the db-effect query surface
/// over `l4::effect_query::DbEffectQuery`).
#[derive(Parser)]
struct QueryCli {
    #[command(subcommand)]
    command: QueryCommands,
}

#[derive(Subcommand)]
enum QueryCommands {
    /// Is a table touched by any DB action, transitively, up or down the call stack?
    Touches(QueryTouchesCli),
    /// A routine's complete transitive db-effect list, with `via` provenance.
    Effects(QueryEffectsCli),
}

/// `QueryTouchesCli` — arguments for `alsem query touches <ws> --table T`.
#[derive(Parser)]
struct QueryTouchesCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Table selector: a table NUMBER or NAME, an internal table id, or the
    /// literal `unknown` (the "effect is real, target table unresolved" bucket —
    /// not a table).
    #[arg(long = "table")]
    table: String,

    /// Scope the answer to one routine: internal id, StableRoutineId,
    /// `Object.Routine`, or a bare routine name. Omit for the workspace-global
    /// list (count reported first; the list is complete, never truncated).
    #[arg(long = "from")]
    from: Option<String>,

    /// down | up | both. `up` is the ancestor-scoped query — which transitive
    /// callers touch the table through OTHER branches. Requires --from.
    #[arg(long = "direction", default_value = "both")]
    direction: String,

    /// Every witness, the callers' included, and no fold. By default a caller
    /// list is counted (and, when the routine does not touch the table itself,
    /// one row per touching caller), and human lists are folded.
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    /// Name every transitive caller (depth and location) without building
    /// their witnesses: the consumers of the touch, which `--all` lists only
    /// with every witness.
    #[arg(long = "callers", default_value_t = false)]
    callers: bool,

    /// Output format: human | json. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so a dependency's tables are selectable.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Human output: the cited source line ±2 under each location.
    #[arg(long = "show-source", default_value_t = false)]
    show_source: bool,
}

/// `QueryEffectsCli` — arguments for `alsem query effects <ws> --routine R`.
#[derive(Parser)]
struct QueryEffectsCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Routine selector: internal id, StableRoutineId, `Object.Routine`, or a
    /// bare routine name.
    #[arg(long = "routine")]
    routine: String,

    /// Output format: human | json. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so effects on a dependency's tables resolve.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Every operation in the human output (one line each), every table and
    /// every loaded dependency app; the JSON always holds every row.
    #[arg(long = "all", default_value_t = false)]
    all: bool,

    /// Human output: the cited source line ±2 under each location.
    #[arg(long = "show-source", default_value_t = false)]
    show_source: bool,
}

/// `PolicyCli` — top-level `policy` subcommand group.
#[derive(Parser)]
struct PolicyCli {
    #[command(subcommand)]
    command: PolicyCommands,
}

#[derive(Subcommand)]
enum PolicyCommands {
    /// Check an AL workspace against a policy (bundled default, auto-detected, or --policy).
    Check(PolicyCheckCli),
    /// Explain a single policy rule (rule summary + normalized AST).
    Explain(PolicyExplainCli),
}

/// `PolicyCheckCli` — arguments for `alsem policy check <ws>`.
#[derive(Parser)]
struct PolicyCheckCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Path to an explicit policy YAML file (overrides auto-detect / default).
    #[arg(long = "policy")]
    policy: Option<String>,

    /// Disable policy entirely (policySource "disabled", 0 rules).
    #[arg(long = "no-policy", default_value_t = false)]
    no_policy: bool,

    /// Output format: human | json | sarif. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Exit 1 if any analyzer error-severity diagnostic.
    #[arg(long, default_value_t = false)]
    strict: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so rules see a dependency's table names and
    /// the facts its code adds, at more memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,
}

/// `PolicyExplainCli` — arguments for `alsem policy explain <rule>`.
#[derive(Parser)]
struct PolicyExplainCli {
    /// The rule id to explain.
    rule: String,

    /// Path to the AL workspace root (for auto-detecting al-sem.policy.yaml). Defaults to ".".
    #[arg(long = "workspace", default_value = ".")]
    workspace: String,

    /// Path to an explicit policy YAML file (overrides auto-detect / default).
    #[arg(long = "policy")]
    policy: Option<String>,

    /// Output format: IGNORED (explain is always human + the AST block). Accepted
    /// for CLI compatibility with `policy check`.
    #[arg(long = "format", default_value = "human")]
    format: String,
}

/// `CacheCli` — top-level `cache` subcommand group.
#[derive(Parser)]
struct CacheCli {
    #[command(subcommand)]
    command: CacheCommands,
}

#[derive(Subcommand)]
enum CacheCommands {
    /// Remove dependency cache entries this build can no longer use.
    Prune(CachePruneCli),
}

/// `CachePruneCli` — arguments for `alsem cache prune`.
#[derive(Parser)]
struct CachePruneCli {
    /// Override the dependency cache directory (default ~/.al-sem/cache/).
    #[arg(long = "dep-cache-dir")]
    dep_cache_dir: Option<String>,

    /// Report what would be removed without deleting anything.
    #[arg(long = "dry-run", default_value_t = false)]
    dry_run: bool,
}

/// `EventsCli` — top-level `events` subcommand group.
#[derive(Parser)]
struct EventsCli {
    #[command(subcommand)]
    command: EventsCommands,
}

#[derive(Subcommand)]
enum EventsCommands {
    /// Event fan-out report: publishers → subscriber counts + coverage.
    Fanout(EventsFanoutCli),
    /// Event chain report: transitive walk from each publisher root.
    Chains(EventsChainsCli),
}

/// `EventsFanoutCli` — arguments for `alsem events fanout <ws>`.
#[derive(Parser)]
struct EventsFanoutCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Output format: human | json. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Scope: primary (default) | all.
    #[arg(long = "scope", default_value = "primary")]
    scope: String,

    /// Coverage policy: warn (default) | strict | ignore.
    #[arg(long = "coverage-policy", default_value = "warn")]
    coverage_policy: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Exit 1 if any analyzer error-severity diagnostic.
    #[arg(long, default_value_t = false)]
    strict: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so a dependency's events are seen, at more
    /// memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,
}

/// `EventsChainsCli` — arguments for `alsem events chains <ws>`.
#[derive(Parser)]
struct EventsChainsCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Output format: human | json. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Scope: primary (default) | all.
    #[arg(long = "scope", default_value = "primary")]
    scope: String,

    /// Coverage policy: warn (default) | strict | ignore.
    #[arg(long = "coverage-policy", default_value = "warn")]
    coverage_policy: String,

    /// Maximum chain depth (0..256). Default 16.
    #[arg(long = "max-depth")]
    max_depth: Option<usize>,

    /// Maximum chain node budget. Default 1024.
    #[arg(long = "max-nodes")]
    max_nodes: Option<usize>,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Exit 1 if any analyzer error-severity diagnostic.
    #[arg(long, default_value_t = false)]
    strict: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so a dependency's events are seen, at more
    /// memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,
}

/// `DiffCli` — arguments for `alsem diff <old> <new>`.
#[derive(Parser)]
struct DiffCli {
    /// Old side: a snapshot file (.json/.cbor/.cbor.gz) or a workspace directory.
    old: String,

    /// New side: a snapshot file (.json/.cbor/.cbor.gz) or a workspace directory.
    new: String,

    /// Output format: human | json | sarif. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Coverage policy: loose | strict. Strict drops findings under incomplete coverage.
    #[arg(long = "coverage-policy", default_value = "loose")]
    coverage_policy: String,

    /// Path to a rename-overlay JSON ({oldStableId: newStableId}).
    #[arg(long = "renames")]
    renames: Option<String>,

    /// Exit 1 if any finding is at/above this severity: critical|high|medium|low|info.
    #[arg(long = "fail-on")]
    fail_on: Option<String>,

    /// Exit 1 on any error-severity analyzer diagnostic (workspace mode).
    #[arg(long = "strict", default_value_t = false)]
    strict: bool,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,
}

#[derive(Parser)]
struct AnalyzeCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Path to the .alpackages directory (reserved; source-only Stage 1).
    #[arg(long)]
    alpackages: Option<String>,

    /// Pin timestamps / version for byte-stable output (reserved; SARIF is already stable).
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Drop findings below this severity: critical|high|medium|low|info.
    #[arg(long = "min-severity")]
    min_severity: Option<String>,

    /// Comma-separated allow-list of detector ids. Mutually exclusive with --preset.
    #[arg(long = "detector")]
    detector: Option<String>,

    /// Run a named detector bundle (e.g. transaction-integrity).
    #[arg(long = "preset")]
    preset: Option<String>,

    /// primary (default) drops findings anchored in a dependency; all keeps them.
    #[arg(long = "scope", default_value = "primary")]
    scope: String,

    /// Cap output at the first N findings (after filtering).
    #[arg(long = "limit")]
    limit: Option<usize>,

    /// Output format: sarif | pr-summary | terminal | json | html | auto.
    /// `auto` resolves to `terminal` when stdout is a TTY, else `json`.
    #[arg(long = "format", default_value = "auto")]
    format: String,

    /// Pin the SARIF driver.version (e.g. gate-sarif-v1) for byte-stable output.
    #[arg(long = "sarif-version-override")]
    sarif_version_override: Option<String>,

    /// Exit 1 if any kept finding is at/above this severity: critical|high|medium|low|info.
    #[arg(long = "fail-on")]
    fail_on: Option<String>,

    /// Make a degraded dependency-coverage preflight FAIL (exit 4).
    #[arg(long = "require-dependencies", default_value_t = false)]
    require_dependencies: bool,

    /// Path to a baseline file: findings whose fingerprint is listed are suppressed.
    #[arg(long = "baseline")]
    baseline: Option<String>,

    /// Rewrite the --baseline file from the current findings (the new floor), then apply it.
    #[arg(long = "update-baseline", default_value_t = false)]
    update_baseline: bool,

    /// Group terminal output by: object | routine | table | detector | file.
    /// Only acted on when the resolved format is `terminal`.
    #[arg(long = "group-by")]
    group_by: Option<String>,

    /// Opt-in: augment each finding in `--format json` with `evidencePath` (the call
    /// chain) and a POSITION-derived `enclosingMember`/`originatingObject` discriminator
    /// on its `primaryLocation`, state `lineBase: 1` (every line 1-based), and bump the
    /// envelope `schemaVersion` to `2.0.0`. OFF by default — the default analyze output is byte-identical to today.
    #[arg(long = "with-evidence", default_value_t = false)]
    with_evidence: bool,

    /// Analyse the workspace alone. The default analyses it with the dependencies it
    /// requires (their code the workspace reaches, and around the dependency events it
    /// subscribes to): cross-app findings such as d13/d16/d17, at more memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,
}

/// `ProveCli` — arguments for `alsem prove <ws> <routine> <question>`.
#[derive(Parser)]
struct ProveCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Routine selector: display name or StableRoutineId.
    routine: String,

    /// Prove question: may-commit | commits-on-success-path | writes-table:<name>
    ///   | publishes-event:<name> | reaches-ui | throws-error. `reaches-ui` is
    ///   interactive UI (Message, Confirm, StrMenu, Page/Report Run or RunModal,
    ///   Hyperlink, UploadIntoStream/DownloadFromStream/Upload/Download, a
    ///   Dialog's Open/Update); an Error is `throws-error`'s.
    question: String,

    /// Output format: json | human. Defaults to json.
    #[arg(long = "format", default_value = "json")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so effects on a dependency's tables resolve,
    /// at more memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Assume the loaded world is all there is (G24): an interface call
    /// dispatches only to the implementations loaded, which the cone already
    /// follows, so its open-world row stops blocking. A site with an
    /// implementer that could not be resolved still blocks. A result that
    /// relies on it carries `assumptions: ["closed-world"]`, and its
    /// obligations count the waived rows in `closedWorldRows`.
    ///
    /// Without it, prove assumes only that no event subscriber exists beyond
    /// the loaded ones (which includes the loggers behind System Application's
    /// `Telemetry Logger`, registered by subscribers). An answer that holds
    /// only under that carries `assumptions: ["loaded-subscribers"]`.
    #[arg(long = "closed-world", default_value_t = false)]
    closed_world: bool,

    /// Human output: list every blocker. The default groups the blockers by
    /// kind with their counts and the first few of each, workspace ones
    /// first. The JSON always lists every one.
    #[arg(long, default_value_t = false)]
    detail: bool,
}

/// `ExplainCli` — arguments for `alsem explain <ws>` (intent-evidence spec
/// §4.4). Exactly one of `--routine`, `--diff`, or `--table` with `--field`.
#[derive(Parser)]
struct ExplainCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// A routine: display name, `<owner>::<routine>` or stable id (as `prove`
    /// reads it). Repeatable: one tree serves every routine, one card per
    /// routine found (sorted, each once: a selector given twice, or two naming
    /// one routine, give one card), a diagnostic per selector that names none
    /// or several. Exit 0 when any card, 2 when none. One selector writes the
    /// single-routine document; two or more write `"selectors": [..]` in its
    /// place.
    #[arg(long = "routine")]
    routine: Vec<String>,

    /// A unified diff (a file, or `-` for stdin): one card per routine it
    /// changes, the routines `digest --diff` resolves.
    #[arg(long = "diff")]
    diff: Option<String>,

    /// The folder the diff's paths are relative to (as `digest --diff-root`).
    #[arg(long = "diff-root")]
    diff_root: Option<String>,

    /// With `--field`: a table name, number or id (as `query touches`).
    #[arg(long = "table")]
    table: Option<String>,

    /// With `--table`: a field name or number. One card per workspace routine
    /// that writes it (assignment, `Validate`, `ModifyAll`).
    #[arg(long = "field")]
    field: Option<String>,

    /// Write the majority rules as `al-sem.policy.yaml` pins (policy version
    /// 2) instead of cards: of `--table/--field` (with the rules below the
    /// bar commented out), or, alone, of every field the workspace writes.
    #[arg(long = "emit-policy", default_value_t = false)]
    emit_policy: bool,

    /// Output format: human | json. Defaults to human.
    #[arg(long = "format", default_value = "human")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), at more memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Every list complete, no fold: every table, entry point, page action,
    /// dependency gap and event-group name; up to 3 paths per item.
    #[arg(long = "detail", default_value_t = false)]
    detail: bool,

    /// Paths per item (default 1; 3 with --detail; at most 3; 0: none).
    #[arg(long = "max-paths")]
    max_paths: Option<usize>,

    /// Human output: the cited source line ±2 under each location (the text
    /// `aldump --dump-dependency-source` writes for a dependency).
    #[arg(long = "show-source", default_value_t = false)]
    show_source: bool,

    /// With `--routine`: answer "does it write table T?" for each card, by
    /// the evaluation `alsem prove <routine> writes-table:T` uses (the same
    /// answer, blockers and assumptions). May be repeated.
    #[arg(long = "writes-table", action = clap::ArgAction::Append)]
    writes_table: Vec<String>,
}

/// `FingerprintCli` — arguments for `alsem fingerprint <ws>`.
#[derive(Parser)]
struct FingerprintCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Output format: human | json | cbor | cbor.gz. Default resolves to human
    /// (or json when --shard). `None` = not passed (distinguishes the default).
    #[arg(long = "format")]
    format: Option<String>,

    /// Write output to a file (single-file modes) or a directory (--shard).
    #[arg(long = "out")]
    out: Option<String>,

    /// Emit sharded JSON output (one file per app). Value: primary-only | all.
    #[arg(long = "shard")]
    shard: Option<String>,

    /// Witness reconstruction limit: false | 0 | <1..256> | all. Default: 3.
    /// `None` = not passed; `Some("3")` (explicit default) does NOT trigger the
    /// query branch (mirrors index.ts:439 `!== "3"`).
    #[arg(long = "witness")]
    witness: Option<String>,

    /// Root-kind filter (comma-separated RootKind list; human/json only).
    #[arg(long = "roots")]
    roots: Option<String>,

    /// Routine selector (display name or StableRoutineId). May be repeated.
    #[arg(long = "routine", action = clap::ArgAction::Append)]
    routine: Vec<String>,

    /// Direct facts only (default is inherited). Mirrors --no-include-inherited.
    #[arg(long = "no-include-inherited", default_value_t = false)]
    no_include_inherited: bool,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// Skip loading roots.config.json overlay even if present.
    #[arg(long = "no-roots-config", default_value_t = false)]
    no_roots_config: bool,

    /// Exit non-zero on any analyzer error-severity diagnostic.
    #[arg(long = "strict", default_value_t = false)]
    strict: bool,

    /// Human output verbosity: compact | full. Defaults to compact.
    #[arg(long = "verbosity", default_value = "compact")]
    verbosity: String,

    /// Emit the lean routine-inventory projection (apps + identities + per-routine
    /// inventory + coverage + rootClassifications). Omits the heavy consumed-core
    /// keys. Implies --format=json; incompatible with --shard and binary formats.
    #[arg(long = "inventory-only", default_value_t = false)]
    inventory_only: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so effects on a dependency's tables resolve,
    /// at more memory and time. Applies to `--routine` and `--inventory-only`; the
    /// other forms analyse the workspace alone in this release.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,
}

/// `DigestCli` — arguments for `alsem digest <ws>`.
#[derive(Parser)]
struct DigestCli {
    /// Path to the AL workspace root.
    workspace: String,

    /// Source file(s) to treat as changed: a path relative to the workspace or
    /// to the git top level, or a `ws:` id naming one workspace file exactly.
    /// With --diff-root, a path is read relative to that folder only. May be
    /// repeated. Mutually exclusive with --order (rejected).
    #[arg(long = "file", action = clap::ArgAction::Append)]
    file: Vec<String>,

    /// Routine stable IDs or display names to treat as changed roots.
    #[arg(long = "routine", action = clap::ArgAction::Append)]
    routine: Vec<String>,

    /// Read a unified diff from a file (or `-` for stdin) to resolve changed roots.
    #[arg(long = "diff")]
    diff: Option<String>,

    /// Convenience alias — auto-detect: existing file path → --diff; comma-list with
    /// `.al` entries → read like --file paths; else → --changed-routines.
    #[arg(long = "changed")]
    changed: Option<String>,

    /// The folder the diff's and the file inputs' paths are relative to (a
    /// relative path resolves against the current directory; a `ws:` id still
    /// names its workspace file). The workspace must be inside it, as spelled
    /// or after resolving symlinks. Default: the nearest enclosing git top
    /// level (a `.git` folder or file at or above the workspace), where a path
    /// may also be workspace-relative; a path naming a workspace file both
    /// ways is reported `diff-file-ambiguous` (a diff path) or `file-ambiguous`
    /// (a file input) and gives no roots. With no enclosing repository, paths
    /// are workspace-relative. Folder names compare exactly, case included, on
    /// every platform.
    #[arg(long = "diff-root")]
    diff_root: Option<String>,

    /// Paths per effect in the JSON document and the human output (default 1
    /// on a decision effect; 3 with --detail; at most 3; 0: none, counts only).
    #[arg(long = "max-paths")]
    max_paths: Option<usize>,

    /// Unfolded 3.x document: every path kept (up to 3), every scoped
    /// guarantee, diagnostic, unresolved call site and per-resource group
    /// witness; the human output lists every name.
    #[arg(long = "detail", default_value_t = false)]
    detail: bool,

    /// JSON without indentation.
    #[arg(long = "compact", default_value_t = false)]
    compact: bool,

    /// Answer "does it write table T?" for each root, by the evaluation
    /// `alsem prove <routine> writes-table:T` uses (the same answer, blockers
    /// and assumptions): `entries[].tableAnswers`. May be repeated. Not with
    /// --schema 2.
    #[arg(long = "writes-table", action = clap::ArgAction::Append)]
    writes_table: Vec<String>,

    /// `2`: the 2.x JSON document (the old shape over the current analysis),
    /// kept for one release window after wave 2 (removed at the latest of one
    /// full release, bc-dev-mcp's pin moving past 2.x, and 90 days; the date
    /// goes into the CHANGELOG at release preparation). Not with --detail,
    /// --max-paths or --compact.
    #[arg(long = "schema")]
    schema: Option<u32>,

    /// Output format: json | human. Defaults to json.
    #[arg(long = "format", default_value = "json")]
    format: String,

    /// Write output to a file instead of stdout.
    #[arg(long = "out")]
    out: Option<String>,

    /// Pin timestamps / version for byte-stable output.
    #[arg(long, default_value_t = false)]
    deterministic: bool,

    /// NOT SUPPORTED: the digest command does not support ordering output.
    #[arg(long, default_value_t = false, hide = true)]
    order: bool,

    /// Analyse the workspace alone. The default loads the dependencies it requires
    /// (as `analyze` and `graph` do), so effects on a dependency's tables resolve,
    /// at more memory and time.
    #[arg(long = "single-app", default_value_t = false)]
    single_app: bool,

    /// Add a testCoverage block: which changed routines a [Test] procedure reaches.
    #[arg(long, default_value_t = false)]
    tests: bool,

    /// Add DB_READ effects: one per table read site (Get, Find*, IsEmpty,
    /// Count*, Next, CalcFields, CalcSums), with the same coverage, witness and
    /// dependency-boundary rules as writes, and `payload.includeReads: true`.
    /// Limits: per read SITE, not per table (physical and temporary reads of a
    /// table are separate effects); an unresolved table has no resourceId; a
    /// dependency-boundary group has no resourceId or tempState and names at
    /// most 10 tables; LockTable is not a read; CalcFields reads only the
    /// record's own table, not FlowField sources; DB_READ is NOT ordering-graded,
    /// so its guarantees are always empty ("not graded", not "no label applies").
    #[arg(long = "include-reads", default_value_t = false)]
    include_reads: bool,

    /// Whether `--changed` names an existing diff file, decided once by the
    /// caller before planning ([`pin_changed`]; a batch's parent, and its
    /// worker gets the decision). Planning never asks the file system.
    #[arg(skip)]
    changed_is_file: Option<bool>,
}

fn main() -> ExitCode {
    // FIRST statement: mimalloc reads an option lazily and then caches it, and
    // the first purge cannot happen before the process has allocated its way
    // through argument parsing. See `MI_OPTION_PURGE_DELAY`'s doc for why 0 is
    // load-bearing (without it this allocator REGRESSES peak RSS on a small
    // workspace) and how the index is pinned.
    //
    // SAFETY: an FFI call into mimalloc's own option table. It is
    // thread-safe by contract, and this runs before the process starts any
    // thread of its own.
    unsafe { libmimalloc_sys::mi_option_set(MI_OPTION_PURGE_DELAY, 0) };
    // The process span; its drop emits `peak.process_mb` / `peak.post_attach_mb`
    // (#160 §4.1.4). Declared before `code`, so it drops after the census report.
    let _run = al_sem::engine::perf_trace::run("alsem");
    // Warnings go to stderr (a dropped dependency was once only a `warn!` that
    // nothing printed); `RUST_LOG` overrides the level. stdout is unchanged.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::Batch(b) => run_batch_cmd(b),
        Commands::EngineHost(h) => engine::run_engine_host(h),
        Commands::EngineAsk(a) => engine::run_engine_ask(a),
        Commands::Mcp(m) => engine::run_mcp(m),
        Commands::Policy(PolicyCli {
            command: PolicyCommands::Explain(e),
        }) => run_policy_explain_cmd(e),
        Commands::Cache(CacheCli {
            command: CacheCommands::Prune(p),
        }) => run_cache_prune_cmd(p),
        Commands::Tests(t) if t.part.is_none() => run_tests_cmd(t),
        Commands::Impact(t) if t.part.is_none() => run_impact_cmd(t),
        job => {
            // The process's one request (context-server design r3 §4.3): the
            // argv decoded, the request's file-system questions decided here
            // (real stdin, the file system now), then planned.
            let cx = PlanCx::new(&PlanReads::process());
            let (job, cx) = decide(job, cx, None);
            run_planned(plan(job, false, &cx), &cx)
        }
    };
    // The process-cumulative cone census, once, after every cone the command
    // composed (#160 §4.1.7). One OnceLock read when off.
    al_sem::engine::l5::detector_context::report_cones_census();
    code
}

// ── output (#160 spec §4.2.5, #277 D1/D2.3) ──────────────────────────────────
//
// A command through the batch driver: standalone is a batch of one, and its
// writes replay in its standalone order (`al_sem::engine::batch::output`,
// which holds the one write-failure rule). The commands with no view
// (`policy explain`, `cache prune`) write through `write_actions`, the same
// replay.

/// A command parsed and validated, before its workspace is read (#277 D3):
/// its validation failure (a job with no view), or what it reads and how its
/// job is made off those reads.
enum Planned {
    Failed(Failed),
    Ready {
        /// Each workspace it reads, with what it reads of it.
        reads: Vec<(PathBuf, WorkspaceNeeds)>,
        /// Read the workspaces through the shared reader and make the job
        /// (it borrows what `make` holds in its slot).
        make: Box<MakeJob>,
    },
}

impl Planned {
    /// Each workspace it reads, with what it reads of it.
    fn reads(&self) -> Vec<(PathBuf, WorkspaceNeeds)> {
        match self {
            Planned::Ready { reads, .. } => reads.clone(),
            Planned::Failed(_) => vec![],
        }
    }
}

/// What a job borrows, owned by the batch, one slot per job: the jobs are
/// made in a loop, never one call deeper per job (#277 final r1 FR-12).
type Slot = std::cell::OnceCell<Box<dyn std::any::Any>>;

/// Makes a command's job off the shared reads, borrowing its slot.
type MakeJob = dyn for<'s> FnOnce(&'s WorkspaceReader, &'s Slot) -> Box<dyn Job + 's>;

/// `value` held in `slot` for as long as the batch's jobs live.
fn hold<T: 'static>(slot: &Slot, value: T) -> &T {
    assert!(slot.set(Box::new(value)).is_ok(), "one value per slot");
    slot.get()
        .and_then(|v| v.downcast_ref())
        .expect("the value just held")
}

/// Every planned job made, in order, each off its own slot; then the reader
/// gives its reads up to the jobs (#277 final r2 R2-5).
fn make_jobs<'s>(
    planned: Vec<Planned>,
    reader: &'s WorkspaceReader,
    slots: &'s [Slot],
) -> Vec<Box<dyn Job + 's>> {
    let jobs = planned
        .into_iter()
        .zip(slots)
        .map(|(p, slot)| match p {
            Planned::Failed(f) => Box::new(f) as Box<dyn Job>,
            Planned::Ready { make, .. } => make(reader, slot),
        })
        .collect();
    reader.made();
    jobs
}

/// A command that reads `workspace` with `needs` (stated once, #277 R7-5):
/// `make` gets that read, which its job hands to the driver (R7-1), so no
/// job keeps a workspace read alive past its views.
fn ready(
    workspace: &str,
    needs: WorkspaceNeeds,
    make: impl for<'s> FnOnce(&'s Slot, HandOff) -> Box<dyn Job + 's> + 'static,
) -> Planned {
    let path = PathBuf::from(workspace);
    Planned::Ready {
        reads: vec![(path.clone(), needs)],
        make: Box::new(move |reader, slot| make(slot, HandOff::new(reader.read(&path, needs)))),
    }
}

/// What digest, prove and fingerprint read: the snapshot's input rows.
const FINGERPRINT_ROWS: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

/// The reader a request's jobs read their workspaces through: the run's own
/// reads, holding the request's given workspace read ([`PlanCx::inputs`]) when
/// it has one, so its job reads no workspace itself.
fn reader_for(reads: Vec<(PathBuf, WorkspaceNeeds)>, cx: &PlanCx) -> WorkspaceReader {
    let reader = WorkspaceReader::new(reads);
    if let Some(inputs) = &cx.inputs {
        reader.give(Arc::clone(inputs));
    }
    reader
}

/// Run a command as a batch of one (#277 D1).
fn run_planned(planned: Planned, cx: &PlanCx) -> ExitCode {
    let reader = reader_for(planned.reads(), cx);
    let slots = [Slot::new()];
    let jobs = make_jobs(vec![planned], &reader, &slots);
    ExitCode::from(run_standalone(&*jobs[0]))
}

/// A job that failed its own validation: `line` on stderr, exit `exit`; a
/// failed stderr write exits `write_failure_exit` (analyze 2, else 1).
fn failed(line: impl Into<String>, exit: u8, write_failure_exit: u8) -> Planned {
    Planned::Failed(Failed {
        output: JobOutput::lines(&[line.into()], exit),
        write_failure_exit,
    })
}

/// A parsed command as a planned job. `batch`, `policy explain` and `cache
/// prune` are no jobs: `main` runs them, and a manifest refuses them first.
/// `in_batch`: a manifest job, whose `--format auto` is JSON (A12.3). `cx`:
/// the request's planning context, the only way planning reads stdin or a
/// file ([`PlanCx`]); its file-system questions were decided by the caller
/// ([`decide`]), so planning probes nothing (`make_job` of context-server
/// design r3 §4.3). A request given a workspace read ([`PlanCx::inputs`]) may
/// read only that workspace, with no more than the read holds: anything else
/// is refused (exit 3) before a job is made.
fn plan(command: Commands, in_batch: bool, cx: &PlanCx) -> Planned {
    let planned = plan_command(command, in_batch, cx);
    if let Some(inputs) = &cx.inputs {
        let given = cx.absolute(&inputs.workspace).ok();
        for (path, needs) in planned.reads() {
            if cx.absolute(&path).ok() != given || !inputs.holds(needs) {
                return failed(
                    format!(
                        "al-sem: workspace '{}' is not the workspace this request reads ('{}')",
                        path.display(),
                        inputs.workspace.display()
                    ),
                    exit::CONFIG_ERROR,
                    1,
                );
            }
        }
    }
    planned
}

/// A batch parent's decisions for its worker's job: `--changed` names a diff
/// file, and the diff root.
type Shipped = (Option<bool>, Option<Result<DiffRoot, String>>);

/// The decoded request's file-system questions, decided by its caller before
/// planning (context-server design r3 §4.3; review r2 #16): whether
/// `digest --changed` names a diff file ([`pin_changed`]) and the diff root
/// ([`decide_diff_root`]), each through `cx` (so its probes are this request's
/// touched reads), or as `shipped` when a batch's parent decided them for its
/// worker.
fn decide(mut command: Commands, mut cx: PlanCx, shipped: Option<Shipped>) -> (Commands, PlanCx) {
    match shipped {
        Some((changed, root)) => {
            pin_changed(&mut command, changed, &cx);
            cx.diff_root_decided = root;
        }
        None => {
            pin_changed(&mut command, None, &cx);
            cx.diff_root_decided = diff_root_of(&command, &cx);
        }
    }
    (command, cx)
}

fn plan_command(command: Commands, in_batch: bool, cx: &PlanCx) -> Planned {
    match command {
        Commands::Analyze(a) => plan_analyze(a, in_batch),
        Commands::Digest(d) => plan_digest(d, cx),
        Commands::Prove(p) => plan_prove(p),
        Commands::Explain(e) => plan_explain(e, cx),
        Commands::Fingerprint(f) => plan_fingerprint(f),
        Commands::Diff(d) => plan_diff(d),
        Commands::Events(e) => match e.command {
            EventsCommands::Fanout(f) => plan_events_fanout(f),
            EventsCommands::Chains(c) => plan_events_chains(c),
        },
        Commands::Policy(p) => match p.command {
            PolicyCommands::Check(c) => plan_policy_check(c, cx),
            PolicyCommands::Explain(_) => unreachable!("policy explain is no job"),
        },
        Commands::Query(q) => match q.command {
            QueryCommands::Touches(t) => plan_query_touches(t),
            QueryCommands::Effects(e) => plan_query_effects(e),
        },
        Commands::Graph(g) => plan_graph(g),
        Commands::Tests(t) => plan_tests(t, cx),
        Commands::Impact(t) => plan_impact(t, cx),
        Commands::Cache(_)
        | Commands::Batch(_)
        | Commands::EngineHost(_)
        | Commands::EngineAsk(_)
        | Commands::Mcp(_) => {
            unreachable!("no job")
        }
    }
}

// ── batch command (#277 D2) ──────────────────────────────────────────────────

/// The command a manifest job may not name, or `None` for a job command.
fn refused(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Batch(_) => Some("batch"),
        Commands::EngineHost(_) => Some("engine-host"),
        Commands::EngineAsk(_) => Some("engine-ask"),
        Commands::Mcp(_) => Some("mcp"),
        Commands::Cache(_) => Some("cache prune"),
        // `tests` runs several parts (one per universe); a part is a job.
        Commands::Tests(t) if t.part.is_none() => Some("tests"),
        Commands::Impact(t) if t.part.is_none() => Some("impact"),
        Commands::Policy(PolicyCli {
            command: PolicyCommands::Explain(_),
        }) => Some("policy explain"),
        _ => None,
    }
}

/// One manifest job's command, or the reason it cannot run: clap's first
/// line, or a refused command.
fn parse_job(argv: &[String]) -> Result<Commands, String> {
    let cli = Cli::try_parse_from(std::iter::once("alsem").chain(argv.iter().map(String::as_str)))
        .map_err(|e| {
            // clap's `error:` line; a help or version request has none.
            let text = e.to_string();
            let line = text.lines().find(|l| l.starts_with("error:"));
            match (argv.is_empty(), line) {
                (true, _) => "no command".to_string(),
                (false, Some(line)) => line.to_string(),
                (false, None) => format!("not a command: {}", argv.join(" ")),
            }
        })?;
    match refused(&cli.command) {
        Some(name) => Err(format!("'{name}' cannot run in a batch")),
        None => Ok(cli.command),
    }
}

/// What a job that failed its own validation writes (D2.4): its two streams,
/// nothing its command line names (#277 final r1 FR-2).
fn stream_paths(streams: &JobStreams) -> JobPaths {
    use al_sem::engine::batch::manifest::Target;
    JobPaths {
        writes: vec![
            Target::File(streams.stdout.clone()),
            Target::File(streams.stderr.clone()),
        ],
        ..JobPaths::default()
    }
}

/// What a manifest job writes and reads (#277 D2.4): its two streams, then
/// what its command line names.
fn job_paths(command: &Commands, streams: &JobStreams) -> JobPaths {
    use al_sem::engine::batch::manifest::Target;
    let mut p = stream_paths(streams);
    let file = |s: &String| Target::File(PathBuf::from(s));
    let (workspace, out): (Option<&String>, Option<&String>) = match command {
        Commands::Analyze(a) => {
            match (&a.baseline, a.update_baseline) {
                (Some(b), true) => p.writes.push(file(b)),
                (Some(b), false) => p.reads.push(PathBuf::from(b)),
                (None, _) => {}
            }
            (Some(&a.workspace), None)
        }
        Commands::Digest(d) => {
            let changed = d.changed.as_deref().map(|v| detect_changed(d, v));
            let changed_diff = match &changed {
                Some(ChangedAutoDetect::Diff(f)) => Some(f),
                _ => None,
            };
            for diff in d.diff.iter().chain(changed_diff).filter(|f| *f != "-") {
                p.reads.push(PathBuf::from(diff));
            }
            (Some(&d.workspace), d.out.as_ref())
        }
        Commands::Prove(c) => (Some(&c.workspace), c.out.as_ref()),
        Commands::Explain(e) => {
            p.reads
                .extend(e.diff.iter().filter(|f| *f != "-").map(PathBuf::from));
            (Some(&e.workspace), e.out.as_ref())
        }
        Commands::Fingerprint(f) if f.shard.is_some() => {
            p.writes
                .extend(f.out.as_deref().map(|o| Target::Dir(PathBuf::from(o))));
            (Some(&f.workspace), None)
        }
        Commands::Fingerprint(f) => (Some(&f.workspace), f.out.as_ref()),
        Commands::Diff(d) => {
            let workspaces = al_sem::engine::gate::diff::cli::diff_reads(&d.old, &d.new);
            for side in [&d.old, &d.new] {
                if !workspaces.iter().any(|(w, _)| w == Path::new(side)) {
                    p.reads.push(PathBuf::from(side));
                }
            }
            p.workspaces.extend(workspaces.into_iter().map(|(w, _)| w));
            p.reads.extend(d.renames.iter().map(PathBuf::from));
            (None, d.out.as_ref())
        }
        Commands::Events(e) => match &e.command {
            EventsCommands::Fanout(f) => (Some(&f.workspace), f.out.as_ref()),
            EventsCommands::Chains(c) => (Some(&c.workspace), c.out.as_ref()),
        },
        Commands::Policy(PolicyCli {
            command: PolicyCommands::Check(c),
        }) => {
            p.reads.extend(c.policy.iter().map(PathBuf::from));
            (Some(&c.workspace), c.out.as_ref())
        }
        Commands::Query(q) => match &q.command {
            QueryCommands::Touches(t) => (Some(&t.workspace), t.out.as_ref()),
            QueryCommands::Effects(e) => (Some(&e.workspace), e.out.as_ref()),
        },
        Commands::Graph(g) => (Some(&g.workspace), g.out.as_ref()),
        Commands::Tests(t) => {
            if let Some(d) = t.diff.as_deref().filter(|d| *d != "-") {
                p.reads.push(PathBuf::from(d));
            }
            (Some(&t.workspace), t.out.as_ref())
        }
        Commands::Impact(t) => {
            if let Some(d) = t.diff.as_deref().filter(|d| *d != "-") {
                p.reads.push(PathBuf::from(d));
            }
            (Some(&t.workspace), t.out.as_ref())
        }
        Commands::Policy(_)
        | Commands::Cache(_)
        | Commands::Batch(_)
        | Commands::EngineHost(_)
        | Commands::EngineAsk(_)
        | Commands::Mcp(_) => (None, None),
    };
    p.workspaces.extend(workspace.map(PathBuf::from));
    p.writes.extend(out.map(file));
    p
}

/// A usage error of `alsem batch`: `line`, exit 3, nothing runs.
fn usage(line: String) -> ExitCode {
    ExitCode::from(replay(&JobOutput::lines(&[line], exit::CONFIG_ERROR), 1))
}

/// `alsem batch <manifest>`: the manifest is read once and every stream
/// directory checked (D2.1); every job's argv is parsed (a usage error runs
/// nothing, exit 3) and planned once, here: its validation, its `--changed`
/// decision and its stdin read (D3). Then every write target is checked
/// (D2.4), one batch runs in this process, two or more in `W` workers (D2.5,
/// D12), and the run prints one line per job in manifest order. Exit 0 when
/// every job ran, whatever its own exit; 1 when a worker died without its
/// lines (D2.2). `--worker <b>`: run the batch the parent writes on stdin.
fn run_batch_cmd(b: BatchCli) -> ExitCode {
    use al_sem::engine::batch::manifest::{check_stream_dirs, load};
    if let Some(w) = b.worker {
        return run_worker(w);
    }
    let manifest_path = Path::new(&b.manifest);
    let out_dir = b.out_dir.as_deref().map(Path::new);
    let manifest = match load(manifest_path, out_dir) {
        Ok(m) => m,
        Err(line) => return usage(line),
    };
    if let Err(line) = check_stream_dirs(&manifest, out_dir) {
        return usage(line);
    }
    // The run's shared reads: one stdin read for every `--diff -` job, each
    // file read once (D3), handed to the workers below. Each job is its own
    // request ([`PlanCx`]) over them.
    let reads = PlanReads::process();
    let mut parsed = Vec::new();
    for (bi, batch) in manifest.batches.iter().enumerate() {
        let mut commands = Vec::new();
        for (i, job) in batch.jobs.iter().enumerate() {
            match parse_job(&job.argv) {
                Ok(mut c) => {
                    pin_changed(&mut c, None, &PlanCx::new(&reads));
                    commands.push(c);
                }
                Err(why) => return usage(format!("al-sem batch: batch {bi} job {i}: {why}")),
            }
        }
        parsed.push(commands);
    }
    // What a job writes and reads, and its kind, follow its own validation:
    // a failed job writes only its streams and builds nothing (FR-2, FR-14),
    // but keeps its reads, so no other job writes a file it read (R2-1): its
    // command line's and every file its planning read or probed (F1).
    let (mut paths, mut kinds, mut planned, mut changed) = (vec![], vec![], vec![], vec![]);
    for (bi, (batch, commands)) in manifest.batches.iter().zip(parsed).enumerate() {
        let (mut plans, mut decided) = (Vec::new(), Vec::new());
        for (i, (job, command)) in batch.jobs.iter().zip(commands).enumerate() {
            let (kind, mut written) = (job_kind(&command), job_paths(&command, &job.streams));
            // The job's own request: its diff-root probes (the `.git` checks)
            // and its plan's reads join this job's reads.
            let mut cx = PlanCx::new(&reads);
            cx.diff_root_decided = diff_root_of(&command, &cx);
            decided.push((changed_is_file(&command), cx.diff_root_decided.clone()));
            let p = plan(command, true, &cx);
            written.reads.append(&mut cx.take_touched());
            let written = match p {
                Planned::Failed(_) => JobPaths {
                    reads: written.reads,
                    ..stream_paths(&job.streams)
                },
                Planned::Ready { .. } => written,
            };
            paths.push((format!("batch {bi} job {i}"), written));
            kinds.extend(kind.filter(|_| !p.reads().is_empty()));
            plans.push(p);
        }
        planned.push(plans);
        changed.push(decided);
    }
    // The run reads its manifest: no job may write it (FR-3).
    paths.push((
        "the run".to_string(),
        JobPaths {
            reads: vec![manifest_path.to_path_buf()],
            ..JobPaths::default()
        },
    ));
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Err(line) = check_write_sets(&paths, &cwd) {
        return usage(line);
    }
    let w = admit(&kinds, manifest.batches.len());
    if let ([batch], Some(plans)) = (&manifest.batches[..], planned.pop()) {
        let streams: Vec<JobStreams> = batch.jobs.iter().map(|j| j.streams.clone()).collect();
        let lines = run_one_batch(0, &streams, plans);
        report_peak(0);
        return print_lines(&lines);
    }
    drop(planned);
    // The workers get the parent's one stdin read, if a job made it (D3).
    let (stdin, files) = (reads.stdin_read(), reads.files());
    let batches: Vec<WorkerBatch> = manifest
        .batches
        .iter()
        .zip(&changed)
        .map(|(batch, decided)| {
            let input = WorkerInput {
                jobs: batch
                    .jobs
                    .iter()
                    .zip(decided)
                    .map(|(j, (c, root))| WorkerJob {
                        argv: j.argv.clone(),
                        streams: j.streams.clone(),
                        changed_is_file: *c,
                        diff_root: root.clone(),
                    })
                    .collect(),
                stdin: stdin.clone(),
                files: files.clone(),
            };
            WorkerBatch {
                streams: batch.jobs.iter().map(|j| j.streams.clone()).collect(),
                input: serde_json::to_vec(&input).expect("a worker input serializes"),
            }
        })
        .collect();
    let run = Workers {
        manifest: manifest_path,
        w,
    };
    let (out, whole) = run_workers(&run, &batches);
    write_run_stdout(out, whole)
}

/// `alsem batch --worker <b>`: the batch its parent wrote on stdin (a
/// [`WorkerInput`]), in this process, then its lines. It reads no manifest
/// and decides nothing its parent decided (#277 final r1 FR-3).
fn run_worker(b: u32) -> ExitCode {
    use std::io::Read;
    let mut bytes = Vec::new();
    let input = std::io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())
        .and_then(|_| serde_json::from_slice::<WorkerInput>(&bytes).map_err(|e| e.to_string()));
    let input = match input {
        Ok(input) => input,
        Err(why) => return usage(format!("al-sem batch: worker {b}: {why}")),
    };
    // Seeded with the parent's stdin read and file reads: this process's
    // own stdin was its batch input.
    let reads = PlanReads::seeded(input.stdin, input.files);
    let mut planned = Vec::new();
    for (i, job) in input.jobs.iter().enumerate() {
        match parse_job(&job.argv) {
            Ok(c) => {
                // Decided by the parent: the worker probes nothing.
                let shipped = Some((job.changed_is_file, job.diff_root.clone()));
                let (c, cx) = decide(c, PlanCx::new(&reads), shipped);
                planned.push(plan(c, true, &cx));
            }
            Err(why) => return usage(format!("al-sem batch: batch {b} job {i}: {why}")),
        }
    }
    let streams: Vec<JobStreams> = input.jobs.into_iter().map(|j| j.streams).collect();
    let lines = run_one_batch(b, &streams, planned);
    #[cfg(debug_assertions)]
    al_sem::engine::batch::worker::test_seam_die(b);
    report_peak(b);
    print_lines(&lines)
}

/// The run's job lines on stdout (D2.2).
fn print_lines(lines: &[JobLine]) -> ExitCode {
    let mut out = Vec::new();
    for line in lines {
        out.extend(serde_json::to_vec(line).expect("a job line serializes"));
        out.push(b'\n');
    }
    write_run_stdout(out, true)
}

/// Write the run's stdout: exit 0, or 1 when the write fails or a worker
/// died without its lines (`whole` false).
fn write_run_stdout(out: Vec<u8>, whole: bool) -> ExitCode {
    let code = replay(
        &JobOutput {
            actions: vec![WriteAction::Stdout(out)],
            exit: if whole { exit::CLEAN } else { 1 },
        },
        1,
    );
    ExitCode::from(code)
}

/// A job's kind for the peak table (D12): its command and its view's
/// universe. `None`: `batch`, `policy explain`, `cache prune` (no job).
fn job_kind(command: &Commands) -> Option<JobKind> {
    let universe = |single_app: bool| match single_app {
        true => ViewUniverse::SingleApp,
        false => ViewUniverse::CrossApp,
    };
    Some(match command {
        Commands::Analyze(a) => ("analyze", universe(a.single_app)),
        // `--include-reads` has its own, larger admission row (#246).
        Commands::Digest(d) if d.include_reads => ("digest reads", universe(d.single_app)),
        Commands::Digest(d) => ("digest", universe(d.single_app)),
        Commands::Prove(p) => ("prove", universe(p.single_app)),
        // No measured row yet: `explain` takes the table's largest (admission).
        Commands::Explain(e) => ("explain", universe(e.single_app)),
        Commands::Fingerprint(f) => ("fingerprint", universe(fingerprint_single_app(f))),
        Commands::Diff(_) => ("diff", ViewUniverse::SingleApp),
        Commands::Events(e) => match &e.command {
            EventsCommands::Fanout(f) => ("events fanout", universe(f.single_app)),
            EventsCommands::Chains(c) => ("events chains", universe(c.single_app)),
        },
        Commands::Policy(PolicyCli {
            command: PolicyCommands::Check(c),
        }) => ("policy check", universe(c.single_app)),
        Commands::Query(q) => match &q.command {
            QueryCommands::Touches(t) => ("query touches", universe(t.single_app)),
            QueryCommands::Effects(e) => ("query effects", universe(e.single_app)),
        },
        Commands::Graph(_) => ("graph", ViewUniverse::CrossApp),
        Commands::Tests(t) if t.part.as_deref() == Some("roots") => {
            ("tests", universe(t.single_app))
        }
        Commands::Tests(t) if t.part.as_deref() == Some("select") => {
            ("tests-select", ViewUniverse::CrossApp)
        }
        Commands::Tests(_) => return None,
        // The main part runs digest's plan over the roots; the up part
        // builds the program and its sited graph, as `graph` does.
        Commands::Impact(t) if t.part.as_deref() == Some("main") => {
            ("digest", universe(t.single_app))
        }
        Commands::Impact(t) if t.part.as_deref() == Some("up") => ("graph", universe(t.single_app)),
        Commands::Impact(_) => return None,
        Commands::Policy(_)
        | Commands::Cache(_)
        | Commands::Batch(_)
        | Commands::EngineHost(_)
        | Commands::EngineAsk(_)
        | Commands::Mcp(_) => return None,
    })
}

/// Whether a fingerprint job's view is single-app. Read only for a job that
/// passed its validation (FR-14), so its format resolves.
fn fingerprint_single_app(f: &FingerprintCli) -> bool {
    default_format(f.format.as_deref(), f.shard.is_some(), f.inventory_only).is_ok_and(|format| {
        reads_single_app(
            &format,
            f.shard.is_some(),
            fingerprint_flags(f).is_query_requested(),
            !f.routine.is_empty(),
            f.inventory_only,
            f.single_app,
        )
    })
}

/// One batch: every workspace read once with the union of its jobs' needs
/// (D3), the jobs made off those reads in a loop (FR-12), then the driver
/// over all of them.
fn run_one_batch(b: u32, streams: &[JobStreams], planned: Vec<Planned>) -> Vec<JobLine> {
    let reader = WorkspaceReader::new(planned.iter().flat_map(Planned::reads).collect::<Vec<_>>());
    let slots: Vec<Slot> = planned.iter().map(|_| Slot::new()).collect();
    let jobs = make_jobs(planned, &reader, &slots);
    let jobs: Vec<(&dyn Job, JobStreams)> = jobs
        .iter()
        .map(|job| &**job as &dyn Job)
        .zip(streams.iter().cloned())
        .collect();
    run_batch(b, &jobs)
}

/// Write `actions` in order; a failed write ends them with `fail`.
fn write_actions(actions: Vec<WriteAction>, fail: u8) -> Result<(), ExitCode> {
    match replay(&JobOutput { actions, exit: 0 }, fail) {
        0 => Ok(()),
        code => Err(ExitCode::from(code)),
    }
}

/// Write `bytes` to stdout.
fn write_stdout(bytes: impl Into<Vec<u8>>, fail: u8) -> Result<(), ExitCode> {
    write_actions(vec![WriteAction::Stdout(bytes.into())], fail)
}

/// Write each line plus `\n` to stderr (the lines a command prints after its output).
fn write_stderr_lines(lines: &[String], fail: u8) -> Result<(), ExitCode> {
    write_actions(lines.iter().map(|l| WriteAction::line(l)).collect(), fail)
}

// ── graph command ────────────────────────────────────────────────────────────

fn plan_graph(g: GraphCli) -> Planned {
    ready(
        &g.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let (g, inputs) = hold(slot, (g, inputs));
            Box::new(al_sem::program::impact_graph::GraphJob {
                inputs,
                workspace: Path::new(&g.workspace),
                out: g.out.as_deref(),
                keep_local_sources: g.keep_local_sources,
            })
        },
    )
}

// ── tests (agent-tasks design §4) ────────────────────────────────────────────

/// A part of `alsem tests` as a job: `--part roots` over the production
/// model, `--part select` over a test universe's sited graph (its read takes
/// the universe's injected inputs).
fn plan_tests(t: TestsCli, cx: &PlanCx) -> Planned {
    use al_sem::engine::test_select::jobs::{RootsAsk, RootsJob, SelectJob};
    use al_sem::engine::test_select::scope::ScopeRequest;
    match t.part.as_deref() {
        Some("roots") => {
            let modes = [!t.routine.is_empty(), !t.file.is_empty(), t.diff.is_some()]
                .iter()
                .filter(|m| **m)
                .count();
            if modes != 1 {
                return failed(
                    "al-sem: tests: give one of --routine, --file, or --diff",
                    exit::CONFIG_ERROR,
                    1,
                );
            }
            let ask = if !t.routine.is_empty() {
                RootsAsk::Routines(t.routine.clone())
            } else if !t.file.is_empty() {
                RootsAsk::Files(t.file.clone())
            } else {
                let diff = t.diff.clone().unwrap_or_default();
                let text = match diff.as_str() {
                    "-" => match cx.stdin_text() {
                        Some(text) => text.to_string(),
                        None => {
                            return failed(
                                "al-sem: tests: failed to read diff from stdin",
                                exit::CONFIG_ERROR,
                                1,
                            );
                        }
                    },
                    path => match FileInput::read_with(path.into(), &|p| cx.read(p)).text() {
                        Ok(s) => s.to_string(),
                        Err(err) => {
                            return failed(
                                format!("al-sem: tests: could not read diff file '{path}': {err}"),
                                exit::CONFIG_ERROR,
                                1,
                            );
                        }
                    },
                };
                let root = match decided_root(cx, true) {
                    Ok(r) => r,
                    Err(line) => return failed(line, exit::CONFIG_ERROR, 1),
                };
                RootsAsk::Diff { text, root }
            };
            let scope = ScopeRequest {
                test_apps: (!t.test_app.is_empty()).then(|| {
                    t.test_app
                        .iter()
                        .map(|a| {
                            let p = PathBuf::from(a);
                            if p.is_absolute() {
                                p
                            } else {
                                std::path::absolute(&p).unwrap_or(p)
                            }
                        })
                        .collect()
                }),
            };
            let single_app = t.single_app;
            ready(
                &t.workspace.clone(),
                WorkspaceNeeds::default(),
                move |slot, inputs| {
                    let inputs = hold(slot, inputs);
                    Box::new(RootsJob {
                        inputs,
                        single_app,
                        ask,
                        scope,
                    })
                },
            )
        }
        Some("select") => {
            let universe = t.universe();
            let injected = universe.injected();
            let roots = t.root_id.clone();
            let probes = t.probe_id.clone();
            let path = PathBuf::from(&t.workspace);
            let needs = WorkspaceNeeds::default();
            Planned::Ready {
                reads: vec![(path.clone(), needs)],
                make: Box::new(move |reader, slot| {
                    let read = reader.read_scoped(&path, needs, &injected);
                    let inputs = hold(slot, HandOff::new(read));
                    Box::new(SelectJob::new(inputs, universe, roots).with_probes(probes))
                }),
            }
        }
        Some(other) => failed(
            format!("al-sem: tests: unknown --part '{other}'"),
            exit::CONFIG_ERROR,
            1,
        ),
        None => failed(
            "al-sem: tests runs several parts and is not a batch job",
            exit::CONFIG_ERROR,
            1,
        ),
    }
}

/// `alsem tests`: the roots part, then a select part per test universe, each
/// run in this process as its own command, then the answer
/// (`test_select::run::orchestrate`, the MCP tool's own path).
fn run_tests_cmd(t: TestsCli) -> ExitCode {
    use al_sem::engine::test_select::render::{RenderOpts, TierPick};
    if !["human", "json"].contains(&t.format.as_str()) {
        eprintln!(
            "al-sem tests: invalid --format '{}'. Expected: human | json",
            t.format
        );
        return ExitCode::from(1);
    }
    let tier = match TierPick::parse(&t.tier) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };
    let opts = RenderOpts {
        tier,
        max_tests: t.max_tests,
        json: t.format == "json",
    };
    let mut run = |_: al_sem::engine::test_select::run::Target<'_>,
                   argv: Vec<String>|
     -> Result<serde_json::Value, String> { run_tests_part(&argv) };
    match al_sem::engine::test_select::run::orchestrate(&t.request(), opts, &mut run) {
        Ok(r) => {
            let out = JobOutput::text(t.out.as_deref(), Ok((r.text, r.exit)));
            ExitCode::from(al_sem::engine::batch::output::replay(&out, 1))
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// One part of `alsem tests`, run in this process: its JSON, or its stderr.
fn run_tests_part(argv: &[String]) -> Result<serde_json::Value, String> {
    let command = parse_job(argv)?;
    let cx = PlanCx::new(&PlanReads::process());
    let (command, cx) = decide(command, cx, None);
    let planned = plan(command, false, &cx);
    let reader = reader_for(planned.reads(), &cx);
    let slots = [Slot::new()];
    let jobs = make_jobs(vec![planned], &reader, &slots);
    let out = al_sem::engine::batch::standalone_output(&*jobs[0]);
    part_json(&out)
}

/// A part's answer: its stdout's JSON, or (a failure) its message.
fn part_json(out: &JobOutput) -> Result<serde_json::Value, String> {
    use al_sem::engine::batch::output::WriteAction;
    let stdout: Vec<u8> = out
        .actions
        .iter()
        .filter_map(|a| match a {
            WriteAction::Stdout(b) => Some(b.clone()),
            _ => None,
        })
        .flatten()
        .collect();
    if out.exit != 0 || stdout.is_empty() {
        return Err(out.message().trim_end().to_string());
    }
    serde_json::from_slice(&stdout).map_err(|e| format!("tests: a part's answer is no JSON: {e}"))
}

// ── impact (agent-tasks design §8) ───────────────────────────────────────────

/// A part of `alsem impact` as a job: `--part main` over the production
/// model, `--part up` over its sited graph.
fn plan_impact(t: ImpactCli, cx: &PlanCx) -> Planned {
    use al_sem::engine::impact::jobs::{MainAsk, MainJob, UpAsk, UpJob};
    use al_sem::engine::test_select::scope::ScopeRequest;
    match t.part.as_deref() {
        Some("main") => {
            let modes = [!t.routine.is_empty(), !t.file.is_empty(), t.diff.is_some()]
                .iter()
                .filter(|m| **m)
                .count();
            if modes != 1 {
                return failed(
                    "al-sem: impact: give one of --routine, --file, or --diff",
                    exit::CONFIG_ERROR,
                    1,
                );
            }
            let ask = if !t.routine.is_empty() {
                MainAsk::Routines(t.routine.clone())
            } else if !t.file.is_empty() {
                MainAsk::Files(t.file.clone())
            } else {
                let diff = t.diff.clone().unwrap_or_default();
                let text = match diff.as_str() {
                    "-" => match cx.stdin_text() {
                        Some(text) => text.to_string(),
                        None => {
                            return failed(
                                "al-sem: impact: failed to read diff from stdin",
                                exit::CONFIG_ERROR,
                                1,
                            );
                        }
                    },
                    path => match FileInput::read_with(path.into(), &|p| cx.read(p)).text() {
                        Ok(s) => s.to_string(),
                        Err(err) => {
                            return failed(
                                format!("al-sem: impact: could not read diff file '{path}': {err}"),
                                exit::CONFIG_ERROR,
                                1,
                            );
                        }
                    },
                };
                let root = match decided_root(cx, true) {
                    Ok(r) => r,
                    Err(line) => return failed(line, exit::CONFIG_ERROR, 1),
                };
                MainAsk::Diff { text, root }
            };
            let scope = ScopeRequest {
                test_apps: (!t.test_app.is_empty()).then(|| {
                    t.test_app
                        .iter()
                        .map(|a| {
                            let p = PathBuf::from(a);
                            if p.is_absolute() {
                                p
                            } else {
                                std::path::absolute(&p).unwrap_or(p)
                            }
                        })
                        .collect()
                }),
            };
            let single_app = t.single_app;
            ready(
                &t.workspace.clone(),
                WorkspaceNeeds::default(),
                move |slot, inputs| {
                    let inputs = hold(slot, inputs);
                    Box::new(MainJob {
                        inputs,
                        single_app,
                        ask,
                        scope,
                    })
                },
            )
        }
        Some("up") => {
            let ask = UpAsk {
                roots: t.root_id.clone(),
                assume_no_dependents: t.no_dependents,
                detail: t.detail,
                single_app: t.single_app,
            };
            ready(
                &t.workspace.clone(),
                WorkspaceNeeds::default(),
                move |slot, inputs| {
                    let inputs = hold(slot, inputs);
                    Box::new(UpJob::new(inputs, ask))
                },
            )
        }
        Some(other) => failed(
            format!("al-sem: impact: unknown --part '{other}'"),
            exit::CONFIG_ERROR,
            1,
        ),
        None => failed(
            "al-sem: impact runs several parts and is not a batch job",
            exit::CONFIG_ERROR,
            1,
        ),
    }
}

/// `alsem impact`: the comparison tuple (git, read only), then the main
/// part, the up part and a select part per test universe, each run in this
/// process as its own command, then the answer (`impact::run::orchestrate`,
/// the MCP tool's own path).
fn run_impact_cmd(t: ImpactCli) -> ExitCode {
    use al_sem::engine::impact::compare;
    use al_sem::engine::impact::render::{DEFAULT_MAX_ROOTS, RenderOpts};
    if !["human", "json"].contains(&t.format.as_str()) {
        eprintln!(
            "al-sem impact: invalid --format '{}'. Expected: human | json",
            t.format
        );
        return ExitCode::from(1);
    }
    if let Err(e) = impact_modes(&t) {
        eprintln!("{e}");
        return ExitCode::from(exit::CONFIG_ERROR);
    }
    let ws = Path::new(&t.workspace);
    // The tuple, and the diff text the main part reads on stdin.
    let (cmp, diff_text) = if let Some(rev) = &t.base {
        match compare::for_base(ws, rev) {
            Ok((c, text)) => (c, Some(text)),
            Err(e) => {
                eprintln!("al-sem {e}");
                return ExitCode::from(exit::CONFIG_ERROR);
            }
        }
    } else if let Some(d) = &t.diff {
        let text = if d == "-" {
            let mut s = String::new();
            if std::io::Read::read_to_string(&mut std::io::stdin(), &mut s).is_err() {
                eprintln!("al-sem: impact: failed to read diff from stdin");
                return ExitCode::from(exit::CONFIG_ERROR);
            }
            s
        } else {
            match std::fs::read_to_string(d) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("al-sem: impact: could not read diff file '{d}': {e}");
                    return ExitCode::from(exit::CONFIG_ERROR);
                }
            }
        };
        (compare::for_input("diff", ws, Some(&text)), Some(text))
    } else if !t.file.is_empty() {
        (compare::for_input("files", ws, None), None)
    } else {
        (compare::for_input("routines", ws, None), None)
    };
    let opts = RenderOpts {
        json: t.format == "json",
        max_roots: t.max_roots.unwrap_or(DEFAULT_MAX_ROOTS),
        detail: t.detail,
    };
    let mut run = |_: al_sem::engine::test_select::run::Target<'_>,
                   argv: Vec<String>|
     -> Result<serde_json::Value, String> {
        let stdin = argv
            .iter()
            .any(|a| a == "--diff=-")
            .then(|| diff_text.clone())
            .flatten();
        run_part_with_stdin(&argv, stdin)
    };
    match al_sem::engine::impact::run::orchestrate(&t.request(), cmp, opts, &mut run) {
        Ok(r) => {
            let out = JobOutput::text(t.out.as_deref(), Ok((r.text, r.exit)));
            ExitCode::from(al_sem::engine::batch::output::replay(&out, 1))
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// One part, run in this process with `stdin` as its stdin text (a diff
/// the front already read): its JSON, or its stderr.
fn run_part_with_stdin(
    argv: &[String],
    stdin: Option<String>,
) -> Result<serde_json::Value, String> {
    let command = parse_job(argv)?;
    let reads = match stdin {
        Some(text) => PlanReads::seeded(
            Some(al_sem::engine::batch::worker::StdinRead::Text(text)),
            Default::default(),
        ),
        None => PlanReads::process(),
    };
    let cx = PlanCx::new(&reads);
    let (command, cx) = decide(command, cx, None);
    let planned = plan(command, false, &cx);
    let reader = reader_for(planned.reads(), &cx);
    let slots = [Slot::new()];
    let jobs = make_jobs(vec![planned], &reader, &slots);
    let out = al_sem::engine::batch::standalone_output(&*jobs[0]);
    part_json(&out)
}

// ── query commands ───────────────────────────────────────────────────────────

const VALID_QUERY_FORMATS: &[&str] = &["human", "json"];

fn plan_query_touches(t: QueryTouchesCli) -> Planned {
    if !VALID_QUERY_FORMATS.contains(&t.format.as_str()) {
        return failed(
            format!(
                "al-sem query touches: invalid --format '{}'. Expected: human | json",
                t.format
            ),
            1,
            1,
        );
    }
    let direction = match Direction::parse(&t.direction) {
        Ok(d) => d,
        Err(msg) => return failed(msg, 1, 1),
    };
    // `--direction down` without `--from` is rejected by `render_query_touches`
    // itself, not duplicated here: one rule in one place, and the golden family
    // covers it end to end (a second copy could drift and would test nothing).
    ready(
        &t.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (t, inputs, version) = hold(slot, (t, inputs, version));
            Box::new(
                QueryJob::touches(
                    inputs,
                    &t.table,
                    t.from.as_deref(),
                    direction,
                    t.all,
                    version,
                    t.deterministic,
                    t.single_app,
                )
                .list_callers(t.callers)
                .show_source(t.show_source)
                .written(t.format == "json", t.out.as_deref()),
            )
        },
    )
}

fn plan_query_effects(e: QueryEffectsCli) -> Planned {
    if !VALID_QUERY_FORMATS.contains(&e.format.as_str()) {
        return failed(
            format!(
                "al-sem query effects: invalid --format '{}'. Expected: human | json",
                e.format
            ),
            1,
            1,
        );
    }
    ready(
        &e.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (e, inputs, version) = hold(slot, (e, inputs, version));
            Box::new(
                QueryJob::effects(inputs, &e.routine, version, e.deterministic, e.single_app)
                    .all_rows(e.all)
                    .show_source(e.show_source)
                    .written(e.format == "json", e.out.as_deref()),
            )
        },
    )
}

// ── policy check command ─────────────────────────────────────────────────────

fn plan_policy_check(c: PolicyCheckCli, cx: &PlanCx) -> Planned {
    use al_sem::engine::gate::policy::pipeline::{
        PolicyCheck, PolicyCheckOptions, PolicyJob, policy_request,
    };

    // Validate --format (human | json | sarif). al-sem writes the message + exit 1.
    const VALID_FORMATS: &[&str] = &["human", "json", "sarif"];
    if !VALID_FORMATS.contains(&c.format.as_str()) {
        return failed(
            format!("al-sem policy check: invalid --format '{}'", c.format),
            1,
            1,
        );
    }

    // The policy is read and resolved here, before the workspace: a bad one
    // is a job with no view, which reads nothing and counts for no admission
    // row (#277 final r2 R2-4).
    let workspace = Path::new(&c.workspace);
    let request = RequestInputs::read_with(
        workspace,
        policy_request(c.policy.as_deref(), c.no_policy),
        None,
        &|p| cx.exists(p),
        &|p| cx.read(p),
    );
    let check = match PolicyCheck::resolve(workspace, &request) {
        Ok(check) => check,
        Err(early) => {
            return Planned::Failed(Failed {
                output: early.into_output(),
                write_failure_exit: 1,
            });
        }
    };
    ready(
        &c.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (c, inputs, request, version) = hold(slot, (c, inputs, request, version));
            let opts = PolicyCheckOptions {
                inputs,
                request,
                format: &c.format,
                out: c.out.as_deref(),
                deterministic: c.deterministic,
                strict: c.strict,
                driver_version: version,
                single_app: c.single_app,
            };
            Box::new(PolicyJob::resolved(opts, check))
        },
    )
}

// ── policy explain command ───────────────────────────────────────────────────

fn run_policy_explain_cmd(e: PolicyExplainCli) -> ExitCode {
    use al_sem::engine::gate::policy::pipeline::{PolicyExplainOptions, run_policy_explain};

    // al-sem validates --format against {human, json} but IGNORES it (always human).
    const VALID_FORMATS: &[&str] = &["human", "json"];
    if !VALID_FORMATS.contains(&e.format.as_str()) {
        eprintln!("al-sem policy explain: invalid --format '{}'", e.format);
        return ExitCode::from(1);
    }

    let workspace = std::path::Path::new(&e.workspace);
    let opts = PolicyExplainOptions {
        workspace,
        rule_id: &e.rule,
        policy_path: e.policy.as_deref(),
    };

    let outcome = run_policy_explain(&opts);

    if let Some(stdout) = &outcome.stdout
        && let Err(code) = write_stdout(stdout.as_str(), 1)
    {
        return code;
    }
    if let Err(code) = write_stderr_lines(&outcome.stderr_lines, 1) {
        return code;
    }
    ExitCode::from(outcome.exit_code)
}

// ── diff command ────────────────────────────────────────────────────────────

fn plan_diff(d: DiffCli) -> Planned {
    use al_sem::engine::gate::diff::cli::{DiffCliOptions, DiffJob, diff_reads};
    use al_sem::engine::gate::diff::{CoveragePolicy, Severity};

    // Validate --format (human | json | sarif). al-sem writes the message + exit 1.
    const VALID_FORMATS: &[&str] = &["human", "json", "sarif"];
    if !VALID_FORMATS.contains(&d.format.as_str()) {
        return failed(
            format!(
                "unknown --format '{}'; valid: {}",
                d.format,
                VALID_FORMATS.join(", ")
            ),
            1,
            1,
        );
    }

    // Validate --coverage-policy (loose | strict).
    let coverage_policy = match d.coverage_policy.as_str() {
        "loose" => CoveragePolicy::Loose,
        "strict" => CoveragePolicy::Strict,
        other => {
            return failed(
                format!("--coverage-policy must be loose|strict (got '{other}')"),
                1,
                1,
            );
        }
    };

    // Validate --fail-on.
    let fail_on = match d.fail_on.as_deref() {
        None => None,
        Some("critical") => Some(Severity::Critical),
        Some("high") => Some(Severity::High),
        Some("medium") => Some(Severity::Medium),
        Some("low") => Some(Severity::Low),
        Some("info") => Some(Severity::Info),
        Some(_) => {
            return failed(
                "--fail-on must be one of: critical|high|medium|low|info",
                1,
                1,
            );
        }
    };

    Planned::Ready {
        reads: diff_reads(&d.old, &d.new),
        make: Box::new(move |reader, slot| {
            let (d, version) = hold(slot, (d, driver_version()));
            let opts = DiffCliOptions {
                old_arg: &d.old,
                new_arg: &d.new,
                format: &d.format,
                out: d.out.as_deref(),
                coverage_policy,
                renames_path: d.renames.as_deref(),
                fail_on,
                strict: d.strict,
                deterministic: d.deterministic,
                driver_version: version,
            };

            // An input error is found before any build, and before either side
            // is read: a job with no view.
            match DiffJob::new(opts, reader) {
                Ok(job) => Box::new(job),
                Err(early) => Box::new(Failed {
                    output: early.into_output(None),
                    write_failure_exit: 1,
                }),
            }
        }),
    }
}

const GROUP_BY_VALUES: &[&str] = &["object", "routine", "table", "detector", "file"];

// ── digest command ──────────────────────────────────────────────────────────

/// The exact stderr message emitted when `--order` is used with `digest`.
const ORDER_REJECTION: &str = "al-sem: digest --order is not supported by the Rust engine";

// Stdin, the plans' file reads and the paths a plan touched are the
// request's planning context ([`PlanCx`]), never process state: a `OnceLock`
// for stdin gave every later request of one process the first request's diff
// (context-server design r2 §4.3; review r1 #3, r2 #16).

/// Decide once whether `digest --changed` names an existing diff file
/// (#277 final r1 FR-3): `decided` when a batch's parent decided it, else
/// the request's file system now.
fn pin_changed(command: &mut Commands, decided: Option<bool>, cx: &PlanCx) {
    if let Commands::Digest(d) = command
        && let Some(value) = &d.changed
    {
        d.changed_is_file = Some(decided.unwrap_or_else(|| cx.probe_exists(Path::new(value))));
    }
}

/// A digest's `--changed` decision, for its worker.
fn changed_is_file(command: &Commands) -> Option<bool> {
    match command {
        Commands::Digest(d) => d.changed_is_file,
        _ => None,
    }
}

/// What `--changed <value>` names: a diff file as decided by the caller
/// ([`pin_changed`]); planning never asks the file system.
fn detect_changed(d: &DigestCli, value: &str) -> ChangedAutoDetect {
    auto_detect_changed_with(value, |_| {
        d.changed_is_file
            .expect("the caller decides --changed before planning (pin_changed)")
    })
}

/// The request's diff root as its caller decided it ([`PlanCx::diff_root_decided`]).
/// A plan that reads no changed path needs none (the workspace); one that does
/// and finds none is a caller that skipped [`decide`]: planning never decides
/// it itself.
fn decided_root(cx: &PlanCx, reads_paths: bool) -> Result<DiffRoot, String> {
    match &cx.diff_root_decided {
        Some(root) => root.clone(),
        None if !reads_paths => Ok(DiffRoot::Workspace),
        None => panic!("the caller decides the diff root before planning (decide)"),
    }
}

/// A digest's or explain's diff root (#77), decided once through `cx` (its
/// probes are the request's touched reads); `None` for every other command.
/// `--changed` must be pinned first ([`pin_changed`]).
fn diff_root_of(command: &Commands, cx: &PlanCx) -> Option<Result<DiffRoot, String>> {
    match command {
        Commands::Digest(d) => Some(decide_diff_root(
            "digest",
            &d.workspace,
            d.diff_root.as_deref(),
            DigestInputs::of(d).reads_paths(),
            cx,
        )),
        // `explain --diff` reads its diff paths as digest does (one decision).
        Commands::Explain(e) => Some(decide_diff_root(
            "explain",
            &e.workspace,
            e.diff_root.as_deref(),
            e.diff.is_some(),
            cx,
        )),
        // `tests --diff` reads its diff paths as digest does (the roots part).
        Commands::Tests(t) if t.part.as_deref() != Some("select") => Some(decide_diff_root(
            "tests",
            &t.workspace,
            t.diff_root.as_deref(),
            t.diff.is_some(),
            cx,
        )),
        Commands::Impact(t) if t.part.as_deref() != Some("up") => Some(decide_diff_root(
            "impact",
            &t.workspace,
            t.diff_root.as_deref(),
            t.diff.is_some(),
            cx,
        )),
        _ => None,
    }
}

/// A digest's inputs with the `--changed` alias resolved (auto-detected into
/// diff / files / routines, mirroring cli/index.ts) and merged with the
/// explicit flags.
struct DigestInputs {
    files: Vec<String>,
    routines: Vec<String>,
    diff: Option<String>,
}

impl DigestInputs {
    fn of(d: &DigestCli) -> DigestInputs {
        let mut inputs = DigestInputs {
            files: d.file.clone(),
            routines: d.routine.clone(),
            diff: d.diff.clone(),
        };
        if let Some(changed) = d.changed.as_deref()
            && !changed.trim().is_empty()
        {
            match detect_changed(d, changed) {
                ChangedAutoDetect::Diff(p) => inputs.diff = Some(p),
                ChangedAutoDetect::Files(fs) => inputs.files.extend(fs),
                ChangedAutoDetect::Routines(rs) => inputs.routines.extend(rs),
            }
        }
        inputs
    }

    /// Whether a diff is read: a non-blank `--diff`, or `--changed` naming one.
    fn has_diff(&self) -> bool {
        self.diff.as_deref().is_some_and(|s| !s.trim().is_empty())
    }

    /// Whether changed paths are read: a diff, or file inputs (#339).
    fn reads_paths(&self) -> bool {
        self.has_diff() || !self.files.is_empty()
    }
}

/// What a digest's changed paths (a diff's and the file inputs, #77, #339) are
/// relative to, decided once per run ([`pin_diff_root`]). `--diff-root`: it
/// must be an existing folder; the workspace's path below it, first as spelled
/// (a symlinked workspace keeps its link name), else canonically; not inside
/// it is an error; a workspace that cannot be canonicalized is left to the
/// load's error. Else the nearest folder at or above the canonical workspace
/// holding `.git` (a folder, or a worktree's or submodule's file); none, or a
/// workspace that cannot be canonicalized: the workspace itself (auto mode
/// never fails). No changed path read (`reads_paths`, decided once by the
/// caller): the workspace, probing nothing. Every probe is recorded
/// ([`PlanCx::touch`]), so a batch job's reads hold `--diff-root` and the
/// workspace even when the probe fails. Every probe (`canonicalize`,
/// `metadata`, the `.git` checks, the current directory) is asked of the
/// request's [`PlanCx`]: a captured input, never the process directly
/// (review r2 #16).
/// `command` names the command in an error line (`digest`, `explain`).
fn decide_diff_root(
    command: &str,
    workspace: &str,
    diff_root: Option<&str>,
    reads_paths: bool,
    cx: &PlanCx,
) -> Result<DiffRoot, String> {
    if !reads_paths {
        return Ok(DiffRoot::Workspace);
    }
    cx.touch(Path::new(workspace));
    let canonical_ws = cx.canonicalize(Path::new(workspace));
    let Some(root) = diff_root else {
        let Ok(ws) = canonical_ws else {
            return Ok(DiffRoot::Workspace);
        };
        return Ok(ws
            .ancestors()
            .find(|dir| cx.exists(&dir.join(".git")))
            .and_then(|top| ws.strip_prefix(top).ok())
            .map_or(DiffRoot::Workspace, |rel| DiffRoot::below(rel, false)));
    };
    // Checked first, so a lexical containment cannot pass a missing root.
    cx.touch(Path::new(root));
    match cx.is_dir(Path::new(root)) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "al-sem: {command}: --diff-root '{root}' is not a folder"
            ));
        }
        Err(e) => return Err(format!("al-sem: {command}: --diff-root '{root}': {e}")),
    }
    // A workspace that does not resolve is left to the load, which reports it
    // (exit 1) however it is spelled, as in auto mode.
    let Ok(ws) = canonical_ws else {
        return Ok(DiffRoot::Workspace);
    };
    let lexical = cx
        .absolute(Path::new(workspace))
        .ok()
        .zip(cx.absolute(Path::new(root)).ok())
        .and_then(|(ws, r)| ws.strip_prefix(&r).ok().map(Path::to_path_buf))
        .filter(|rel| {
            !rel.components()
                .any(|c| c == std::path::Component::ParentDir)
        });
    let rel = match lexical {
        Some(rel) => rel,
        None => {
            let r = cx
                .canonicalize(Path::new(root))
                .map_err(|e| format!("al-sem: {command}: --diff-root '{root}': {e}"))?;
            ws.strip_prefix(&r).map(Path::to_path_buf).map_err(|_| {
                format!(
                    "al-sem: {command}: workspace '{workspace}' is not inside --diff-root '{root}'"
                )
            })?
        }
    };
    Ok(DiffRoot::below(&rel, true))
}

fn plan_digest(d: DigestCli, cx: &PlanCx) -> Planned {
    // Reject --order (CONFIG_ERROR — a not-ported flag rejected cleanly rather than silently ignored)
    if d.order {
        return failed(ORDER_REJECTION, exit::CONFIG_ERROR, 1);
    }

    // Validate --format. al-sem (cli/index.ts:578) writes this exact message and
    // exits 1 (FINDINGS), NOT CONFIG_ERROR (3) — the digest format check is at the
    // CLI layer, distinct from analyze's enum-flag CONFIG_ERROR exits (#4).
    const VALID_DIGEST_FORMATS: &[&str] = &["json", "human"];
    if !VALID_DIGEST_FORMATS.contains(&d.format.as_str()) {
        return failed(
            format!(
                "al-sem digest: invalid --format '{}'. Expected: json | human",
                d.format
            ),
            1,
            1,
        );
    }

    // W2-2 (design §5): `--schema 2` is the 2.x document, which has no folds
    // or caps to set; any other schema is unknown. Usage errors: exit 2.
    let render = match d.schema {
        None | Some(3) => al_sem::engine::l5::digest_v3::DigestRender {
            schema2: false,
            detail: d.detail,
            compact: d.compact,
            max_paths: d.max_paths,
        },
        Some(2) if d.detail || d.compact || d.max_paths.is_some() || !d.writes_table.is_empty() => {
            return failed(
                "al-sem digest: --schema 2 takes no --detail, --max-paths or --compact \
                 (the 2.x document has no folds or caps)",
                2,
                1,
            );
        }
        Some(2) => al_sem::engine::l5::digest_v3::DigestRender::schema2(),
        Some(n) => {
            return failed(
                format!("al-sem digest: unknown --schema {n}. Expected: 2 or 3"),
                2,
                1,
            );
        }
    };

    // Resolve the --changed alias FIRST, once: the diff decision below and the
    // diff-root decision both use this one answer (#77).
    let inputs = DigestInputs::of(&d);
    let (has_diff, reads_paths) = (inputs.has_diff(), inputs.reads_paths());
    let DigestInputs {
        files: file_inputs,
        routines: routine_inputs,
        diff: diff_arg,
    } = inputs;

    // No-input check (cli/digest.ts:138): emit the exact CLI message + exit 1.
    // Done at the CLI layer BEFORE the pipeline so the user sees the --changed-files
    // wording (the pipeline's own message is the internal run-digest-pipeline one).
    let has_files = !file_inputs.is_empty();
    let has_routines = !routine_inputs.is_empty();
    // Before the no-input check: a `--diff-root` with no path to read is a flag
    // misuse (exit 3).
    if d.diff_root.is_some() && !reads_paths {
        return failed(
            "al-sem: digest: --diff-root needs a diff or file inputs (--diff, --file or --changed)",
            exit::CONFIG_ERROR,
            1,
        );
    }
    if !has_files && !has_routines && !has_diff {
        return failed(
            "digest: at least one of --changed-files, --changed-routines, --diff, or --changed is required",
            1,
            1,
        );
    }

    // Read diff input
    let diff_text: Option<String> = match diff_arg.as_deref() {
        None => None,
        Some("-") => match cx.stdin_text() {
            Some(text) => Some(text.to_string()),
            None => {
                return failed(
                    "al-sem: digest: failed to read diff from stdin",
                    exit::CONFIG_ERROR,
                    1,
                );
            }
        },
        Some(path) => match FileInput::read_with(path.into(), &|p| cx.read(p)).text() {
            Ok(s) => Some(s.to_string()),
            Err(e) => {
                return failed(
                    format!("al-sem: digest: could not read diff file '{path}': {e}"),
                    exit::CONFIG_ERROR,
                    1,
                );
            }
        },
    };

    // Decided by the caller ([`decide`]; a batch worker gets its parent's).
    let diff_root = match decided_root(cx, reads_paths) {
        Ok(r) => r,
        Err(line) => return failed(line, exit::CONFIG_ERROR, 1),
    };

    let changed_files = if file_inputs.is_empty() {
        None
    } else {
        Some(file_inputs)
    };
    let changed_routines = if routine_inputs.is_empty() {
        None
    } else {
        Some(routine_inputs)
    };

    ready(
        &d.workspace.clone(),
        FINGERPRINT_ROWS,
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (d, inputs, version) = hold(slot, (d, inputs, version));
            let req = DigestRequest {
                inputs,
                changed_files,
                changed_routines,
                diff_text,
                alsem_ver: version,
                deterministic: d.deterministic,
                single_app: d.single_app,
                tests: d.tests,
                include_reads: d.include_reads,
                diff_root,
            };
            Box::new(
                DigestJob::new(req, d.format == "human", d.out.as_deref())
                    .with_render(render)
                    .with_writes_tables(d.writes_table.clone()),
            )
        },
    )
}

// ── prove command ───────────────────────────────────────────────────────────

fn plan_prove(p: ProveCli) -> Planned {
    // Validate --format
    const VALID_PROVE_FORMATS: &[&str] = &["json", "human"];
    if !VALID_PROVE_FORMATS.contains(&p.format.as_str()) {
        return failed(
            format!(
                "al-sem prove: invalid --format '{}'. Expected: json | human",
                p.format
            ),
            1,
            1,
        );
    }

    // Validate question early (mirrors prove.ts exit 1 for unknown question)
    if parse_question(&p.question).is_none() {
        let valid: Vec<&str> = question_ids().to_vec();
        return failed(
            format!(
                "prove: unknown question '{}'\nValid questions:\n{}",
                p.question,
                valid
                    .iter()
                    .map(|q| format!("  {q}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            1,
            1,
        );
    }

    ready(
        &p.workspace.clone(),
        FINGERPRINT_ROWS,
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (p, inputs, version) = hold(slot, (p, inputs, version));
            Box::new(ProveJob {
                detectors: al_sem::engine::l5::detectors::default_detectors(),
                inputs,
                routine_selector: &p.routine,
                question_text: &p.question,
                alsem_ver: version,
                deterministic: p.deterministic,
                single_app: p.single_app,
                closed_world: p.closed_world,
                human: p.format == "human",
                detail: p.detail,
                out: p.out.as_deref(),
            })
        },
    )
}

// ── explain command ─────────────────────────────────────────────────────────

/// `alsem explain` (intent-evidence spec §4.4). Usage errors, before the
/// workspace is read: a bad `--format` exits 1 (as digest's and prove's do);
/// no mode exits 1 (as digest with no input); two modes, `--table` without
/// `--field` (or the reverse), and `--diff-root` without `--diff` are flag
/// misuse, exit 3; an unreadable diff exits 3 (digest's). The job then exits
/// 0 with an answer and 2 when nothing resolved.
fn plan_explain(e: ExplainCli, cx: &PlanCx) -> Planned {
    use al_sem::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    const VALID_EXPLAIN_FORMATS: &[&str] = &["human", "json"];
    if !VALID_EXPLAIN_FORMATS.contains(&e.format.as_str()) {
        return failed(
            format!(
                "al-sem explain: invalid --format '{}'. Expected: human | json",
                e.format
            ),
            1,
            1,
        );
    }
    if e.table.is_some() != e.field.is_some() {
        return failed(
            "al-sem: explain: --table and --field go together",
            exit::CONFIG_ERROR,
            1,
        );
    }
    let modes = [!e.routine.is_empty(), e.diff.is_some(), e.table.is_some()]
        .iter()
        .filter(|m| **m)
        .count();
    if modes > 1 {
        return failed(
            "al-sem: explain: give one of --routine, --diff, or --table with --field",
            exit::CONFIG_ERROR,
            1,
        );
    }
    if e.diff_root.is_some() && e.diff.is_none() {
        return failed(
            "al-sem: explain: --diff-root needs --diff",
            exit::CONFIG_ERROR,
            1,
        );
    }
    if e.emit_policy {
        return plan_emit_policy(e);
    }
    let mode = if !e.routine.is_empty() {
        ExplainMode::Routines(e.routine.clone())
    } else if let Some(diff) = &e.diff {
        let text = match diff.as_str() {
            "-" => match cx.stdin_text() {
                Some(text) => text.to_string(),
                None => {
                    return failed(
                        "al-sem: explain: failed to read diff from stdin",
                        exit::CONFIG_ERROR,
                        1,
                    );
                }
            },
            path => match FileInput::read_with(path.into(), &|p| cx.read(p)).text() {
                Ok(s) => s.to_string(),
                Err(err) => {
                    return failed(
                        format!("al-sem: explain: could not read diff file '{path}': {err}"),
                        exit::CONFIG_ERROR,
                        1,
                    );
                }
            },
        };
        // Decided by the caller ([`decide`]; a batch worker gets its parent's).
        let root = match decided_root(cx, true) {
            Ok(r) => r,
            Err(line) => return failed(line, exit::CONFIG_ERROR, 1),
        };
        ExplainMode::Diff { text, root }
    } else if let (Some(table), Some(field)) = (&e.table, &e.field) {
        ExplainMode::Field {
            table: table.clone(),
            field: field.clone(),
        }
    } else {
        return failed(
            "explain: one of --routine, --diff, or --table with --field is required",
            1,
            1,
        );
    };
    ready(
        &e.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let (e, inputs) = hold(slot, (e, inputs));
            let req = ExplainRequest {
                inputs,
                mode,
                single_app: e.single_app,
            };
            Box::new(
                ExplainJob::new(req, e.format == "human", e.out.as_deref())
                    .with_render(e.detail, e.max_paths)
                    .with_show_source(e.show_source)
                    .with_writes_tables(e.writes_table.clone()),
            )
        },
    )
}

/// `alsem explain --emit-policy` (rules-to-policy spec §2.5): with
/// `--table/--field` that field, alone every field. `--routine` / `--diff`
/// with it, and a `--format` other than the default (the output is always
/// YAML), are flag misuse (exit 3). The job exits 0 with a document and 2
/// for an unknown table or field.
fn plan_emit_policy(e: ExplainCli) -> Planned {
    use al_sem::engine::l5::emit_policy::{EmitPolicyJob, EmitPolicyRequest, EmitScope};
    if !e.routine.is_empty() || e.diff.is_some() {
        return failed(
            "al-sem: explain: --emit-policy goes with --table/--field or alone, not with \
             --routine or --diff",
            exit::CONFIG_ERROR,
            1,
        );
    }
    if e.format != "human" {
        return failed(
            "al-sem: explain: --emit-policy writes YAML; --format does not apply",
            exit::CONFIG_ERROR,
            1,
        );
    }
    let scope = match (&e.table, &e.field) {
        (Some(table), Some(field)) => EmitScope::Field {
            table: table.clone(),
            field: field.clone(),
        },
        _ => EmitScope::Workspace,
    };
    ready(
        &e.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let (e, inputs) = hold(slot, (e, inputs));
            let req = EmitPolicyRequest {
                inputs,
                scope,
                single_app: e.single_app,
            };
            Box::new(EmitPolicyJob::new(req, e.out.as_deref()))
        },
    )
}

// ── fingerprint command ─────────────────────────────────────────────────────

/// _specifiedFlags (index.ts:434-439).
fn fingerprint_flags(f: &FingerprintCli) -> SpecifiedFlags {
    SpecifiedFlags {
        roots: f.roots.is_some(),
        routine_selectors: !f.routine.is_empty(),
        include_inherited: f.no_include_inherited,
        witness: f.witness.is_some() && f.witness.as_deref() != Some("3"),
    }
}

fn plan_fingerprint(f: FingerprintCli) -> Planned {
    // Parse --shard mode (primary-only | all). Invalid value → exit 1.
    let shard_mode: Option<ShardMode> = match f.shard.as_deref() {
        None => None,
        Some("primary-only") => Some(ShardMode::PrimaryOnly),
        Some("all") => Some(ShardMode::All),
        Some(other) => {
            return failed(
                format!("unknown --shard mode '{other}'; valid: primary-only, all"),
                1,
                1,
            );
        }
    };

    let specified = fingerprint_flags(&f);

    // defaultFormat (fingerprint.ts:140) + rejectIllegalCombos (fingerprint.ts:110).
    let format = match default_format(f.format.as_deref(), shard_mode.is_some(), f.inventory_only) {
        Ok(fmt) => fmt,
        Err(msg) => return failed(msg, 1, 1),
    };
    if let Err(msg) = reject_illegal_combos(specified, &format, shard_mode.is_some()) {
        return failed(msg, 1, 1);
    }

    // Validate --verbosity (compact | full).
    if !matches!(f.verbosity.as_str(), "compact" | "full") {
        return failed(
            format!(
                "unknown --verbosity '{}'; valid: compact, full",
                f.verbosity
            ),
            1,
            1,
        );
    }

    // normalizeWitness (fingerprint.ts:82): None→3, false/all, 0..256, >256 → exit 1.
    let witness_limit = match normalize_witness(f.witness.as_deref()) {
        Ok(wl) => Some(wl),
        Err(msg) => return failed(msg, 1, 1),
    };

    // validateRoots (fingerprint.ts:67): split on commas, each must be a RootKind.
    let roots: Option<std::collections::BTreeSet<String>> = match &f.roots {
        None => None,
        Some(raw) => {
            let values: Vec<String> = raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            match validate_roots(&values) {
                Ok(vs) => {
                    if vs.is_empty() {
                        None
                    } else {
                        Some(vs.into_iter().collect())
                    }
                }
                Err(msg) => return failed(msg, 1, 1),
            }
        }
    };

    let is_query_requested = specified.is_query_requested();
    // A usage error is found before the workspace is read (final panel r1).
    if let Err(msg) = reject_inventory_only_combos(
        f.inventory_only,
        shard_mode.is_some(),
        &format,
        is_query_requested,
    ) {
        return failed(msg, 1, 1);
    }
    // Before the workspace is read too: a job with no view, which holds no
    // read and names no shard directory (#277 final r1 FR-2, FR-11).
    if let Err(output) = reject_shard_without_out(shard_mode.is_some(), f.out.as_deref()) {
        return Planned::Failed(Failed {
            output,
            write_failure_exit: 1,
        });
    }
    ready(
        &f.workspace.clone(),
        FINGERPRINT_ROWS,
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (f, inputs, version) = hold(slot, (f, inputs, version));
            let opts = FingerprintOptions {
                inputs,
                driver_version: version,
                format,
                out: f.out.as_deref(),
                shard: shard_mode,
                witness_limit,
                roots,
                routine_selectors: f.routine.clone(),
                // includeInherited default true; --no-include-inherited → direct-only.
                include_inherited: !f.no_include_inherited,
                is_query_requested,
                deterministic: f.deterministic,
                strict: f.strict,
                verbosity: &f.verbosity,
                inventory_only: f.inventory_only,
                no_roots_config: f.no_roots_config,
                single_app: f.single_app,
            };

            Box::new(FingerprintJob::new(opts).expect("--shard without --out failed validation"))
        },
    )
}

// ── events fanout command ───────────────────────────────────────────────────

fn plan_events_fanout(f: EventsFanoutCli) -> Planned {
    const VALID_FORMATS: &[&str] = &["human", "json"];
    if !VALID_FORMATS.contains(&f.format.as_str()) {
        return failed(
            format!("al-sem events fanout: invalid --format '{}'", f.format),
            1,
            1,
        );
    }

    const VALID_COVERAGE: &[&str] = &["warn", "strict", "ignore"];
    if !VALID_COVERAGE.contains(&f.coverage_policy.as_str()) {
        return failed(
            format!(
                "al-sem events fanout: invalid --coverage-policy '{}'",
                f.coverage_policy
            ),
            1,
            1,
        );
    }

    let scope = match f.scope.as_str() {
        "primary" => EventScope::Primary,
        "all" => EventScope::All,
        other => {
            return failed(
                format!("al-sem events fanout: invalid --scope '{other}'. Expected: primary | all"),
                1,
                1,
            );
        }
    };

    ready(
        &f.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (f, inputs, version) = hold(slot, (f, inputs, version));
            let opts = EventsFanoutOptions {
                inputs,
                format: &f.format,
                scope,
                coverage_policy: &f.coverage_policy,
                driver_version: version,
                deterministic: f.deterministic,
                strict: f.strict,
                single_app: f.single_app,
            };
            Box::new(EventsJob::fanout(opts, f.out.as_deref()))
        },
    )
}

// ── events chains command ───────────────────────────────────────────────────

fn plan_events_chains(c: EventsChainsCli) -> Planned {
    const VALID_FORMATS: &[&str] = &["human", "json"];
    if !VALID_FORMATS.contains(&c.format.as_str()) {
        return failed(
            format!("al-sem events chains: invalid --format '{}'", c.format),
            1,
            1,
        );
    }

    const VALID_COVERAGE: &[&str] = &["warn", "strict", "ignore"];
    if !VALID_COVERAGE.contains(&c.coverage_policy.as_str()) {
        return failed(
            format!(
                "al-sem events chains: invalid --coverage-policy '{}'",
                c.coverage_policy
            ),
            1,
            1,
        );
    }

    let scope = match c.scope.as_str() {
        "primary" => EventScope::Primary,
        "all" => EventScope::All,
        other => {
            return failed(
                format!("al-sem events chains: invalid --scope '{other}'. Expected: primary | all"),
                1,
                1,
            );
        }
    };

    if let Some(md) = c.max_depth
        && md > 256
    {
        return failed("al-sem events chains: --max-depth must be in 0..256", 1, 1);
    }

    ready(
        &c.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let version = inputs.get().driver_version.clone();
            let (c, inputs, version) = hold(slot, (c, inputs, version));
            let opts = EventsChainsOptions {
                inputs,
                format: &c.format,
                scope,
                coverage_policy: &c.coverage_policy,
                max_depth: c.max_depth,
                max_nodes: c.max_nodes,
                driver_version: version,
                deterministic: c.deterministic,
                strict: c.strict,
                single_app: c.single_app,
            };
            Box::new(EventsJob::chains(opts, c.out.as_deref()))
        },
    )
}

// ── cache prune command ─────────────────────────────────────────────────────

fn run_cache_prune_cmd(c: CachePruneCli) -> ExitCode {
    let result = prune_cache(c.dep_cache_dir.as_deref(), c.dry_run);
    let report = format_prune_report(&result, c.dry_run);
    match write_stdout(report, 1) {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Resolve `--format auto` (or omitted) to a concrete `OutputFormat`.
/// Non-TTY stdout → `Json`; TTY stdout → `Terminal`; a batch job → `Json`
/// whatever the process's stdout is (A12.3; #277 final r1 FR-1).
/// This is the testable contract (corpus differentials always pipe → non-TTY → json).
fn resolve_auto_format(format_str: &str, in_batch: bool) -> OutputFormat {
    match format_str {
        "auto" if !in_batch && stdout_is_terminal() => OutputFormat::Terminal,
        "auto" => OutputFormat::Json,
        "sarif" => OutputFormat::Sarif,
        "pr-summary" => OutputFormat::PrSummary,
        "terminal" => OutputFormat::Terminal,
        "json" => OutputFormat::Json,
        "html" => OutputFormat::Html,
        // Unknown values are caught before this call; return Json as a safe fallback.
        _ => OutputFormat::Json,
    }
}

/// Whether stdout is a terminal. Test seam, debug builds only:
/// `ALSEM_TEST_STDOUT_IS_TERMINAL` makes it one without a pty (#277 final
/// r1 FR-1, `batch_auto_format_is_json_on_a_terminal`).
fn stdout_is_terminal() -> bool {
    #[cfg(debug_assertions)]
    if std::env::var_os("ALSEM_TEST_STDOUT_IS_TERMINAL").is_some() {
        return true;
    }
    std::io::stdout().is_terminal()
}

fn plan_analyze(a: AnalyzeCli, in_batch: bool) -> Planned {
    // --- enum-flag validation (mirrors the al-sem CLI's CONFIG_ERROR exits) ---
    if let Some(sev) = &a.min_severity
        && !SEVERITY_VALUES.contains(&sev.as_str())
    {
        return failed(
            format!(
                "al-sem: invalid --min-severity '{sev}'. Expected one of: {}",
                SEVERITY_VALUES.join(", ")
            ),
            exit::CONFIG_ERROR,
            exit::ANALYSIS_FAILURE,
        );
    }

    let scope = match a.scope.as_str() {
        "primary" => Scope::Primary,
        "all" => Scope::All,
        other => {
            return failed(
                format!("al-sem: invalid --scope '{other}'. Expected one of: primary, all"),
                exit::CONFIG_ERROR,
                exit::ANALYSIS_FAILURE,
            );
        }
    };

    // --- validate --format (accept auto + all concrete values) ---
    const VALID_FORMATS: &[&str] = &["auto", "sarif", "pr-summary", "terminal", "json", "html"];
    if !VALID_FORMATS.contains(&a.format.as_str()) {
        return failed(
            format!(
                "al-sem: unsupported --format '{}'. Supported: {} (known presets: {})",
                a.format,
                VALID_FORMATS.join(", "),
                preset_names().join(", ")
            ),
            exit::CONFIG_ERROR,
            exit::ANALYSIS_FAILURE,
        );
    }
    let format = resolve_auto_format(&a.format, in_batch);

    // --- validate --group-by ---
    let group_by = if let Some(ref g) = a.group_by {
        if !GROUP_BY_VALUES.contains(&g.as_str()) {
            return failed(
                format!(
                    "al-sem: invalid --group-by '{g}'. Expected one of: {}",
                    GROUP_BY_VALUES.join(", ")
                ),
                exit::CONFIG_ERROR,
                exit::ANALYSIS_FAILURE,
            );
        }
        Some(g.clone())
    } else {
        None
    };

    // --- validate --fail-on (CONFIG_ERROR on a bad value, mirroring al-sem parseFailOn). ---
    let fail_on = match &a.fail_on {
        Some(s) => match parse_fail_on(s) {
            Ok(sev) => Some(sev),
            Err(msg) => {
                return failed(
                    format!("al-sem: {msg}"),
                    exit::CONFIG_ERROR,
                    exit::ANALYSIS_FAILURE,
                );
            }
        },
        None => None,
    };

    let args = AnalyzeArgs {
        workspace: a.workspace,
        min_severity: a.min_severity,
        detector: a.detector,
        preset: a.preset,
        scope,
        limit: a.limit,
        format,
        sarif_version_override: a.sarif_version_override,
        fail_on,
        require_dependencies: a.require_dependencies,
        baseline: a.baseline,
        update_baseline: a.update_baseline,
        disable_inline_suppression: false,
        group_by,
        deterministic: a.deterministic,
        with_evidence: a.with_evidence,
        single_app: a.single_app,
    };

    // `default_version` is the engine's default (unpinned) SARIF `driver.version`.
    // The differential always pins via `--sarif-version-override gate-sarif-v1`;
    // this is only the fallback for a real, unpinned invocation.
    // A bad `--preset` / `--detector` is a usage error found before the
    // workspace is read (`run_analyze_with_exit` resolves the set again).
    if let Err(msg) = resolve_analyze_detectors(args.preset.as_deref(), args.detector.as_deref()) {
        return failed(
            format!("al-sem: {msg}"),
            exit::CONFIG_ERROR,
            exit::ANALYSIS_FAILURE,
        );
    }
    ready(
        &args.workspace.clone(),
        WorkspaceNeeds::default(),
        move |slot, inputs| {
            let request = args.read_request();
            let version = inputs.get().driver_version.clone();
            let (args, inputs, request, version) = hold(slot, (args, inputs, request, version));
            // `new` fails only on the detector set, resolved above.
            Box::new(
                AnalyzeJob::new(inputs, request, args, version).expect("the detector set resolved"),
            )
        },
    )
}

// ── the engine child and its stage-2 front end (context-server stage 2) ─────

/// `alsem engine-host` (the child), `alsem engine-ask` (a front end of one
/// ask) and `alsem mcp`. The child keeps a resident session
/// (`al_sem::engine::resident`) and answers each `session/ask` in the
/// design's order (§4.3): **decode** the argv (clap), check the front end's
/// mandatory decisions, **validate** through the CLI's own planning with no
/// workspace bound (a planning failure answers its CLI text and exit, 3
/// included, `revision: null`, and reads nothing), enforce the session's
/// workspace and universe, **pin** a revision by the ask's freshness mode,
/// plan again **bound** to the pinned read, and answer off the revision
/// (explain, prove, digest, query) or through the one-shot job path over
/// its read (every other command). A semantic build failure is the job's
/// own CLI answer (`view_failed`); infrastructure failures are
/// `kind: "failed"`.
mod engine {
    use super::*;
    use al_sem::engine::batch::plan_cx::ProcessFs;
    use al_sem::engine::host::child::{AskCx, Engine, protocol_stdout, serve_with};
    use al_sem::engine::host::front::{EngineFront, FrontConfig};
    use al_sem::engine::host::{AskParams, ChildStatus, Decisions, OpenParams, Universe};
    use al_sem::engine::resident::budget::{Budget, Case, Estimates, ProcMeter};
    use al_sem::engine::resident::revision::{SERVED_COMMANDS, answer_failed_build};
    use al_sem::engine::resident::session::{Built, PinError, ResidentSession};
    use al_sem::engine::resident::store::{Control, PrepareError, Stop};
    use al_sem::wire::{
        Answer, FailedOutcome, Outcome, Stderr, Stdout, Timing, WireError, WorkDone, clamp_inline,
        cli_outcome,
    };
    use std::io::Write as _;
    use std::time::Instant;

    /// `alsem engine-host`: serve the engine protocol on stdin/stdout until
    /// the front end shuts the session down or closes stdin.
    pub(super) fn run_engine_host(h: EngineHostCli) -> ExitCode {
        // The resident child frees each revision's model and artifacts on the
        // next refresh: hand freed large blocks back to the OS, so the RSS
        // does not keep the freed memory inside pages a live block holds;
        // under `--low-mem` also turn transparent huge pages off, so the
        // kernel does not fill them back in (slower, less memory:
        // `al_sem::release_alloc`). `ALSEM_RELEASE_MIN` (bytes, 0 = both off)
        // overrides the threshold, for measurement.
        al_sem::release_alloc::set_resident(
            std::env::var("ALSEM_RELEASE_MIN")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(al_sem::release_alloc::RELEASE_MIN),
            h.low_mem,
        );
        let memory_budget = match h
            .memory_budget
            .as_deref()
            .map(al_sem::engine::resident::budget::parse_bytes)
            .transpose()
        {
            Ok(b) => b,
            Err(e) => return usage(format!("al-sem: engine-host: --memory-budget: {e}")),
        };
        let out = match protocol_stdout() {
            Ok(out) => out,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "al-sem: engine-host: {e}");
                return ExitCode::from(1);
            }
        };
        let idle_timeout =
            (h.idle_timeout > 0).then(|| std::time::Duration::from_secs(h.idle_timeout));
        let engine = HostEngine {
            memory_budget,
            low_mem: h.low_mem,
            graph: h.graph,
            idle_timeout_secs: idle_timeout.map(|d| d.as_secs()),
            estimate_per_al_byte: h.estimate_per_al_byte,
            estimate_floor: h.estimate_floor,
            ..HostEngine::default()
        };
        let mut config = al_sem::engine::host::child::ServeConfig {
            idle_timeout,
            ..Default::default()
        };
        if let Some(ms) = h.idle_grace_ms {
            config.idle_grace = std::time::Duration::from_millis(ms);
        }
        let stdin = std::io::BufReader::new(std::io::stdin());
        match serve_with(&engine, stdin, std::io::BufWriter::new(out), config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "al-sem: engine-host: {e}");
                ExitCode::from(1)
            }
        }
    }

    /// The open session: the workspace as `session/open` named it, its
    /// absolute spelling (the CLI's view-key rule), the universe, and the
    /// resident session.
    #[derive(Clone)]
    struct Open {
        workspace: PathBuf,
        absolute: PathBuf,
        universe: Universe,
        session: Arc<ResidentSession>,
        budget: Arc<Budget>,
    }

    /// The child's engine: the open session, its counters and its memory
    /// configuration (`--memory-budget`, the estimate's test overrides).
    #[derive(Default)]
    struct HostEngine {
        open: std::sync::Mutex<Option<Open>>,
        asks: std::sync::atomic::AtomicU64,
        memory_budget: Option<u64>,
        low_mem: bool,
        /// `--graph`: every revision builds the sited graph.
        graph: bool,
        idle_timeout_secs: Option<u64>,
        estimate_per_al_byte: Option<u64>,
        estimate_floor: Option<u64>,
    }

    impl HostEngine {
        fn session(&self) -> Option<Open> {
            self.open.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    impl Engine for HostEngine {
        fn open(&self, params: &OpenParams, instance: &str) -> Result<(), String> {
            if params.workspace.as_os_str().is_empty() {
                return Err("session/open names no workspace".into());
            }
            if !params.workspace.is_dir() {
                return Err(format!(
                    "session/open: workspace '{}' is not a folder",
                    params.workspace.display()
                ));
            }
            let absolute =
                std::path::absolute(&params.workspace).unwrap_or_else(|_| params.workspace.clone());
            let single = params.universe == Universe::SingleApp;
            let mut estimates = Estimates::new(single);
            if let Some(n) = self.estimate_per_al_byte {
                estimates.per_al_byte = n;
            }
            if let Some(n) = self.estimate_floor {
                estimates.floor = n;
            }
            let bytes = self.memory_budget.unwrap_or_else(|| {
                al_sem::engine::resident::budget::default_budget(single, self.low_mem)
            });
            let budget = Arc::new(Budget::new(bytes, Arc::new(ProcMeter), estimates));
            let session = if params.universe == Universe::Tests {
                // A test universe (agent-tasks §4.3): graph-only, every read
                // and observation with the descriptor's injected inputs.
                let Some(u) = &params.test_universe else {
                    return Err("session/open: universe tests names no test universe".into());
                };
                ResidentSession::graph_only(params.workspace.clone(), instance.to_string())
                    .with_injected(u.injected())
                    .with_universe_label(u.key())
            } else if params.universe == Universe::GraphOnly {
                ResidentSession::graph_only(params.workspace.clone(), instance.to_string())
            } else {
                ResidentSession::new(params.workspace.clone(), single, instance.to_string())
            };
            let session = session
                .with_low_mem(self.low_mem)
                .with_sited(self.graph)
                .with_budget(Arc::clone(&budget));
            *self.open.lock().unwrap_or_else(|p| p.into_inner()) = Some(Open {
                workspace: params.workspace.clone(),
                absolute,
                universe: params.universe,
                session: Arc::new(session),
                budget,
            });
            Ok(())
        }

        fn ask(&self, id: &serde_json::Value, params: AskParams, cx: &AskCx<'_>) -> Answer {
            self.asks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(open) = self.session() else {
                return Answer::terminal(
                    id.clone(),
                    None,
                    FailedOutcome::Protocol,
                    "session/open first",
                    None,
                );
            };
            let mut answer = answer(id, params, &open, cx);
            answer.meta.universe = Some(open.universe.as_str().to_string());
            answer
        }

        fn descriptor(&self) -> serde_json::Value {
            self.session()
                .and_then(|o| o.session.descriptor())
                .and_then(|d| serde_json::to_value(d).ok())
                .unwrap_or(serde_json::Value::Null)
        }

        fn status(&self) -> ChildStatus {
            let open = self.session();
            let mut status = ChildStatus {
                pid: std::process::id(),
                asks: self.asks.load(std::sync::atomic::Ordering::Relaxed),
                idle_timeout_secs: self.idle_timeout_secs,
                low_mem: self.low_mem,
                ..ChildStatus::default()
            };
            if let Some(o) = open {
                status.memory = o
                    .session
                    .memory_status()
                    .and_then(|m| serde_json::to_value(m).ok());
                // The allocator's release of freed large blocks (see
                // `run_engine_host`): its threshold (0 = off) and what it
                // handed back so far.
                if let Some(serde_json::Value::Object(m)) = status.memory.as_mut() {
                    let (blocks, bytes) = al_sem::release_alloc::released();
                    m.insert(
                        "releaseFromBytes".into(),
                        al_sem::release_alloc::release_on_free().into(),
                    );
                    m.insert("releasedBlocks".into(), blocks.into());
                    m.insert("releasedBytes".into(), bytes.into());
                    m.insert(
                        "thpDisabled".into(),
                        al_sem::release_alloc::thp_disabled().into(),
                    );
                }
                let stats = o.session.stats();
                status.workspace = Some(o.workspace.clone());
                status.universe = Some(o.universe);
                status.workspace_reads = stats.builds + stats.discarded_builds;
                let published = o.session.published();
                status.revision = published.as_ref().and_then(Built::revision_id);
                if let Some(Built::Revision(r)) = &published {
                    let store = r.stats();
                    let (hit, miss, batches) = r.witness_counts();
                    status.fills = Some(serde_json::json!({
                        "byKind": r.fill_counts(),
                        "witness": { "hit": hit, "miss": miss, "batches": batches },
                        "answered": r.answered(),
                        "fills": store.fills,
                        "joins": store.joins,
                        "discards": store.discards,
                        "evictions": store.evictions,
                        "failures": store.failures,
                    }));
                }
                let mut s = serde_json::to_value(&stats).unwrap_or_default();
                if let Some(r) = o.session.last_refresh() {
                    s["lastRefresh"] = serde_json::json!({
                        "changed": r.changed.len(),
                        "parsed": r.parsed,
                        "resolved": r.resolved,
                        "dependencyBodiesResolved": r.dependency_bodies_resolved,
                        "dependencyFilesResolved": r.dependency_files_resolved,
                        "wallMs": r.wall_ms,
                    });
                }
                status.session = Some(s);
            }
            status
        }

        fn notify_changed(&self, paths: &[PathBuf]) {
            if let Some(o) = self.session() {
                o.session.notify_changed(paths.iter().cloned());
            }
        }

        fn refresh(&self, cx: &AskCx<'_>) -> serde_json::Value {
            let Some(o) = self.session() else {
                return serde_json::json!({"revision": null, "error": "session/open first"});
            };
            let start = Instant::now();
            let builds_before = o.session.stats().builds;
            match o
                .session
                .pin(al_sem::engine::resident::session::Freshness::Fresh, cx.stop)
            {
                Ok(p) => {
                    // Whether this refresh built (agent-tasks design §2.3:
                    // `rebuilt` or `already-fresh`), its class and the
                    // workspace files it found changed.
                    let stats = o.session.stats();
                    let built = stats.builds > builds_before;
                    let mut v = serde_json::json!({
                        "revision": p.built.revision_id(),
                        "waitedForBuildMs": p.waited_for_build_ms,
                        "wallMs": start.elapsed().as_millis() as u64,
                        "built": built,
                    });
                    if built {
                        v["class"] = serde_json::json!(stats.last_build_class);
                        v["changedFiles"] =
                            serde_json::json!(o.session.last_refresh().map(|r| r.changed.len()));
                    }
                    v
                }
                Err(e) => serde_json::json!({
                    "revision": null,
                    "error": match e {
                        PinError::Unstable(m) => m,
                        PinError::Stopped(_) => "stopped".to_string(),
                        PinError::Budget(r) => r.message(),
                    },
                }),
            }
        }
    }

    /// The argv decoded as the CLI decodes it, or the CLI's own answer to a
    /// line it does not run: clap's error (or help) text and exit, or a
    /// command the engine does not serve (exit 3, as a batch refuses it).
    fn decode(argv: &[String]) -> Result<Commands, JobOutput> {
        let cli =
            Cli::try_parse_from(std::iter::once("alsem").chain(argv.iter().map(String::as_str)))
                .map_err(|e| {
                    let text = e.to_string().into_bytes();
                    let exit = u8::try_from(e.exit_code()).unwrap_or(2);
                    let action = if e.use_stderr() {
                        WriteAction::Stderr(text)
                    } else {
                        WriteAction::Stdout(text)
                    };
                    JobOutput {
                        actions: vec![action],
                        exit,
                    }
                })?;
        match refused(&cli.command) {
            Some(name) => Err(JobOutput::lines(
                &[format!("al-sem: '{name}' cannot run in the engine")],
                exit::CONFIG_ERROR,
            )),
            None => Ok(cli.command),
        }
    }

    /// The decoded command with the front end's decisions applied, through
    /// the request's shipped reads (never the file system). `Err`: the ask
    /// reads paths but carries no decision for them (design §3: the child
    /// never decides).
    fn decided(
        command: Commands,
        ask: &AskParams,
        reads: &Arc<PlanReads>,
    ) -> Result<(Commands, PlanCx), String> {
        let mut command = command;
        let decisions = ask.decisions.clone().unwrap_or_default();
        let cx = PlanCx::new(reads);
        if let Commands::Digest(d) = &command
            && d.changed.is_some()
            && decisions.changed_is_file.is_none()
        {
            return Err("session/ask: `digest --changed` needs the front end's \
                        decisions.changedIsFile: the engine never probes the file system"
                .into());
        }
        pin_changed(&mut command, decisions.changed_is_file, &cx);
        let reads_paths = match &command {
            Commands::Digest(d) => DigestInputs::of(d).reads_paths(),
            Commands::Explain(e) => e.diff.is_some(),
            _ => false,
        };
        if reads_paths && decisions.diff_root.is_none() {
            return Err(
                "session/ask: the command reads changed paths; the front end must \
                        decide their diff root (decisions.diffRoot, the design's \
                        diffRootDecided): the engine never probes the file system"
                    .into(),
            );
        }
        let mut cx = cx;
        cx.diff_root_decided = decisions.diff_root;
        Ok((command, cx))
    }

    /// One ask's answer (see the module doc for the order).
    fn answer(id: &serde_json::Value, ask: AskParams, open: &Open, cx: &AskCx<'_>) -> Answer {
        let start = Instant::now();
        let inline = clamp_inline(ask.max_inline_bytes);
        let mut timing = Timing {
            queued_ms: cx.queued_ms,
            ..Timing::default()
        };
        let cli = |out: &JobOutput, revision, timing: Timing| {
            let outcome = match cli_outcome(out, inline, cx.spill) {
                Ok(o) => o,
                Err(WireError::FileWrite(p)) => Outcome::failed(
                    FailedOutcome::RefusedPath,
                    format!("the engine writes no file a command names (--out {p})"),
                ),
                Err(e) => Outcome::failed(FailedOutcome::Protocol, e.to_string()),
            };
            let mut a = Answer::new(id.clone(), outcome, revision);
            a.meta.timing = Some(timing);
            a
        };
        let failed = |outcome, message: &str| {
            Answer::new(id.clone(), Outcome::failed(outcome, message), None)
        };
        let stopped = |stop: Stop, done: &[&str]| {
            let all = ["pin", "prepare", "compute"];
            let (outcome, why) = match stop {
                Stop::Cancelled => (FailedOutcome::Cancelled, "cancelled"),
                Stop::Deadline => (FailedOutcome::Deadline, "the deadline passed"),
            };
            let mut a = Answer::new(id.clone(), Outcome::failed(outcome, why), None);
            a.meta.work = Some(WorkDone {
                work_done_ms: start.elapsed().as_millis() as u64,
                skipped_phases: all
                    .iter()
                    .filter(|p| !done.contains(p))
                    .map(|p| (*p).to_string())
                    .collect(),
            });
            a
        };
        // Decode and validate: planning reads only the request's shipped
        // inputs (its diff text, its request files), no workspace.
        let command = match decode(&ask.argv) {
            Ok(c) => c,
            Err(out) => return cli(&out, None, timing),
        };
        let reads = PlanReads::shipped(
            ask.diff_text.clone(),
            ask.request_files.clone().unwrap_or_default(),
        );
        let (command, plan_cx) = match decided(command, &ask, &reads) {
            Ok(d) => d,
            Err(why) => return failed(FailedOutcome::Protocol, &why),
        };
        let universe = job_kind(&command).map(|(_, u)| u);
        let planned = plan(command, true, &plan_cx);
        let wanted = match &planned {
            Planned::Failed(f) => return cli(&f.output, None, timing),
            Planned::Ready { reads, .. } => reads.clone(),
        };
        drop(planned);
        // The session's workspace and universe.
        if let Some((other, _)) = wanted
            .iter()
            .find(|(p, _)| std::path::absolute(p).unwrap_or_else(|_| p.clone()) != open.absolute)
        {
            return failed(
                FailedOutcome::NotThisWorkspace,
                &format!(
                    "workspace '{}' is not this session's ('{}')",
                    other.display(),
                    open.workspace.display()
                ),
            );
        }
        let single = open.universe == Universe::SingleApp;
        if universe.is_some_and(|u| (u == ViewUniverse::SingleApp) != single) {
            return failed(
                FailedOutcome::NotThisWorkspace,
                &format!(
                    "this session serves the {} universe; ask a session of the other",
                    open.universe.as_str()
                ),
            );
        }
        // Admission (§4.5): this request holds memory from here on; a
        // reclaim in progress holds it at the gate (bounded: the reclaim
        // makes one attempt and reopens).
        let is_stopped = || (cx.stop)().is_some();
        let Some(_admitted) = open.budget.admit(&is_stopped) else {
            return stopped((cx.stop)().unwrap_or(Stop::Cancelled), &[]);
        };
        let over_budget = |r: &al_sem::engine::resident::budget::Refusal| {
            let mut a = failed(FailedOutcome::Budget, &r.message());
            a.meta.budget = Some(r.figures());
            a
        };
        // Freshness and pin: one state for the whole ask.
        (cx.progress)("pin");
        // A tests select part on a model engine (the workspace's own tests)
        // reads the sited graph: the revision must have it (M0's pin_sited,
        // which turns the graph on once).
        // So does an impact up part (agent-tasks B1).
        let part_is = |p: &str| {
            ask.argv.iter().any(|a| *a == format!("--part={p}"))
                || ask.argv.windows(2).any(|w| w[0] == "--part" && w[1] == p)
        };
        let reads_sited = ((ask.argv.first().is_some_and(|c| c == "tests") && part_is("select"))
            || (ask.argv.first().is_some_and(|c| c == "impact") && part_is("up")))
            && !open.session.is_graph_only();
        let pin = if reads_sited {
            open.session.pin_sited(ask.freshness, cx.stop)
        } else {
            open.session.pin(ask.freshness, cx.stop)
        };
        let pinned = match pin {
            Ok(p) => p,
            Err(PinError::Stopped(s)) => return stopped(s, &[]),
            Err(PinError::Unstable(m)) => return failed(FailedOutcome::InputsUnstable, &m),
            Err(PinError::Budget(r)) => return over_budget(&r),
        };
        timing.waited_for_build_ms = pinned.waited_for_build_ms;
        if let Some(s) = (cx.stop)() {
            return stopped(s, &["pin"]);
        }
        // Bind: plan again over the pinned read (it may read nothing else).
        let command = match decode(&ask.argv) {
            Ok(c) => c,
            Err(out) => return cli(&out, None, timing),
        };
        let (command, mut bound) = match decided(command, &ask, &reads) {
            Ok(d) => d,
            Err(why) => return failed(FailedOutcome::Protocol, &why),
        };
        bound.inputs = Some(Arc::clone(pinned.built.inputs()));
        let planned = plan(command, true, &bound);
        let reader = reader_for(planned.reads(), &bound);
        let slots = [Slot::new()];
        let jobs = make_jobs(vec![planned], &reader, &slots);
        let job = &*jobs[0];
        (cx.progress)("prepare");
        let served = SERVED_COMMANDS.contains(&job.name());
        let compute = Instant::now();
        let (out, revision) = match &pinned.built {
            Built::Revision(rev) if served => {
                // Every fill and the trees reserve first (fill workers are
                // counted while they hold theirs).
                let refused = std::sync::Mutex::new(None);
                let reserve = |kind: &'static str,
                               _: usize|
                 -> Result<al_sem::engine::resident::store::Guard<'_>, String> {
                    let case = if kind == "trees" {
                        Case::Trees
                    } else {
                        Case::Fill(kind)
                    };
                    let estimate = open.budget.estimate(case, Default::default());
                    match open.session.reserve_or_reclaim(
                        &open.budget,
                        case,
                        estimate,
                        1,
                        &is_stopped,
                    ) {
                        Ok(r) => Ok(Box::new(r)),
                        Err(f) => {
                            let m = f.message();
                            *refused.lock().unwrap_or_else(|p| p.into_inner()) = Some(f);
                            Err(m)
                        }
                    }
                };
                let ctl = Control {
                    stop: cx.stop,
                    reserve: &reserve,
                };
                let answered = rev.answer_timed(job, &ctl);
                open.session.update_floor();
                match answered {
                    Ok((out, (fill_ms, compute_ms))) => {
                        timing.shared_fill_ms = fill_ms;
                        timing.compute_ms = compute_ms;
                        (out, Some(rev.id.clone()))
                    }
                    Err(PrepareError::Cancelled) => {
                        return stopped(Stop::Cancelled, &["pin"]);
                    }
                    Err(PrepareError::Deadline) => return stopped(Stop::Deadline, &["pin"]),
                    Err(PrepareError::Budget(m)) => {
                        return match refused.lock().unwrap_or_else(|p| p.into_inner()).take() {
                            Some(r) => over_budget(&r),
                            None => failed(FailedOutcome::Budget, &m),
                        };
                    }
                    Err(PrepareError::Engine(m)) => {
                        return failed(FailedOutcome::EngineError, &m);
                    }
                }
            }
            // A graph-only engine (agent-tasks M0-2) answers a sited-graph
            // job off its graph and refuses every other with the reason.
            Built::Graph(g) => match g.answer(job) {
                Ok(out) => (out, Some(g.id.clone())),
                Err(PrepareError::Engine(m)) => return failed(FailedOutcome::EngineError, &m),
                Err(PrepareError::Cancelled) => return stopped(Stop::Cancelled, &["pin"]),
                Err(PrepareError::Deadline) => return stopped(Stop::Deadline, &["pin"]),
                Err(PrepareError::Budget(m)) => return failed(FailedOutcome::Budget, &m),
            },
            Built::Failed(f) if served => (answer_failed_build(job, &f.failure), None),
            // The one-shot job path over the pinned read: the same inputs,
            // its own model (equal to the revision's by the stage-4 contract).
            Built::Revision(rev) => (
                al_sem::engine::batch::standalone_output(job),
                Some(rev.id.clone()),
            ),
            Built::Failed(_) => (al_sem::engine::batch::standalone_output(job), None),
        };
        if timing.compute_ms == 0 {
            timing.compute_ms = compute.elapsed().as_millis() as u64;
        }
        (cx.progress)("compute");
        let mut a = cli(&out, revision, timing);
        a.meta.stale = pinned.stale;
        if let Outcome::Cli { changed_paths, .. } = &mut a.outcome {
            *changed_paths = pinned.changed_paths;
        }
        a
    }

    /// Whether the command reads its diff from stdin (`--diff -`).
    fn reads_stdin(command: &Commands) -> bool {
        match command {
            Commands::Digest(d) => d.diff.as_deref() == Some("-"),
            Commands::Explain(e) => e.diff.as_deref() == Some("-"),
            Commands::Tests(t) => t.diff.as_deref() == Some("-"),
            Commands::Impact(t) => t.diff.as_deref() == Some("-"),
            _ => false,
        }
    }

    /// `alsem engine-ask -- <command line>`: decode the command line, decide
    /// its file-system questions here (the front end's job, r3 §4.3), read
    /// its stdin if it asks for it, ask an engine child, and deliver the
    /// answer: its exact stdout and stderr and its exit code. A `kind:
    /// "failed"` answer prints its outcome and message, exit 1.
    pub(super) fn run_engine_ask(a: EngineAskCli) -> ExitCode {
        let parsed =
            Cli::try_parse_from(std::iter::once("alsem").chain(a.argv.iter().map(String::as_str)));
        let command = match parsed {
            Ok(cli) => cli.command,
            Err(e) => e.exit(),
        };
        let streams = JobStreams {
            stdout: PathBuf::new(),
            stderr: PathBuf::new(),
        };
        let Some(workspace) = job_paths(&command, &streams).workspaces.into_iter().next() else {
            return usage("al-sem: engine-ask: the command names no workspace".into());
        };
        let single_app = job_kind(&command).is_some_and(|(_, u)| u == ViewUniverse::SingleApp);
        let Ok(freshness) = serde_json::from_value(serde_json::json!(a.freshness)) else {
            return usage(format!(
                "al-sem: engine-ask: --freshness must be fresh, latest-built or verify, not '{}'",
                a.freshness
            ));
        };
        let mut params = ask_params(command, a.argv, a.max_inline);
        params.freshness = freshness;
        if a.params {
            let _ = writeln!(
                std::io::stdout(),
                "{}",
                serde_json::to_string(&params).unwrap_or_default()
            );
            return ExitCode::SUCCESS;
        }
        let program = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => return usage(format!("al-sem: engine-ask: {e}")),
        };
        let mut cfg = FrontConfig::new(program, workspace);
        if single_app {
            cfg.universe = Universe::SingleApp;
        }
        let mut front = match EngineFront::new(cfg) {
            Ok(f) => f,
            Err(e) => return usage(format!("al-sem: engine-ask: {e}")),
        };
        let answer = front.ask(params);
        let code = deliver(answer.outcome);
        front.shutdown();
        code
    }

    /// The `session/ask` params a front end sends for a decoded command
    /// (design §3, §4.3): its file-system questions decided here, through
    /// this process's file system (the diff root, `--changed`'s
    /// file-or-list); its stdin read if it reads `--diff -`; and the bytes of
    /// every file its planning reads (`requestFiles`), read here by planning
    /// it once with no workspace bound. The child then probes and reads
    /// nothing for the request.
    pub(super) fn ask_params(
        command: Commands,
        argv: Vec<String>,
        max_inline: Option<u64>,
    ) -> AskParams {
        let plan_reads = PlanReads::process();
        let (command, cx) = decide(command, PlanCx::new(&plan_reads), None);
        let decisions = Decisions {
            changed_is_file: changed_is_file(&command),
            diff_root: cx.diff_root_decided.clone(),
        };
        let diff_text = reads_stdin(&command)
            .then(|| cx.stdin_text().map(str::to_string))
            .flatten();
        // Planning reads the request's files through `cx` (recorded once).
        drop(plan(command, true, &cx));
        let files = plan_reads.files();
        let request_files = (!files.reads.is_empty() || !files.exists.is_empty()).then_some(files);
        AskParams {
            argv,
            diff_text,
            request_files,
            decisions: Some(decisions),
            max_inline_bytes: max_inline,
            ..AskParams::default()
        }
    }

    /// The MCP front end's decisions (context-server design r3 §4.3): the
    /// argv decoded with this binary's clap and decided by the CLI's own
    /// [`decide`], through the file system now and the call's diff text, as
    /// `engine-ask` decides. A command line that does not decode has none
    /// (the child answers clap's error).
    struct CliDecide;

    impl al_sem::mcp::Decide for CliDecide {
        fn decide(&self, argv: &[String], diff_text: Option<&str>) -> Option<Decisions> {
            let cli = Cli::try_parse_from(
                std::iter::once("alsem").chain(argv.iter().map(String::as_str)),
            )
            .ok()?;
            let reads = PlanReads::request(diff_text.map(str::to_string), Box::new(ProcessFs));
            let (command, cx) = decide(cli.command, PlanCx::new(&reads), None);
            Some(Decisions {
                changed_is_file: changed_is_file(&command),
                diff_root: cx.diff_root_decided.clone(),
            })
        }
    }

    /// `alsem mcp`: serve MCP on stdin/stdout (`al_sem::mcp`). stdout
    /// carries only protocol: fd 1 is moved to the protocol stream and
    /// pointed at stderr, as the engine child does.
    pub(super) fn run_mcp(m: McpCli) -> ExitCode {
        let Some(freshness) = al_sem::mcp::tools::Freshness::parse(&m.freshness) else {
            return usage(format!(
                "al-sem: mcp: --freshness must be fresh, latest-built or verify, not '{}'",
                m.freshness
            ));
        };
        let program = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => return usage(format!("al-sem: mcp: {e}")),
        };
        let out = match protocol_stdout() {
            Ok(out) => out,
            Err(e) => return usage(format!("al-sem: mcp: {e}")),
        };
        let size = |flag: &str, v: Option<&str>| {
            v.map(al_sem::engine::resident::budget::parse_bytes)
                .transpose()
                .map_err(|e| format!("al-sem: mcp: {flag}: {e}"))
        };
        let memory_budget = match size("--memory-budget", m.memory_budget.as_deref()) {
            Ok(b) => b,
            Err(e) => return usage(e),
        };
        let total_budget = match size("--total-budget", m.total_budget.as_deref()) {
            Ok(b) => b,
            Err(e) => return usage(e),
        };
        if m.max_children == 0 {
            return usage("al-sem: mcp: --max-children must be at least 1".into());
        }
        let mut cfg = al_sem::mcp::McpConfig::new(
            program,
            al_sem::mcp::tools::Defaults {
                workspace: m.workspace,
                universe: if m.single_app {
                    al_sem::mcp::tools::Universe::SingleApp
                } else {
                    al_sem::mcp::tools::Universe::CrossApp
                },
                freshness,
            },
            m.output_root
                .map_or_else(al_sem::mcp::default_output_root, PathBuf::from),
        );
        cfg.memory_budget = memory_budget;
        cfg.low_mem = m.low_mem;
        cfg.total_budget = total_budget;
        cfg.idle_timeout_secs = m.idle_timeout;
        cfg.max_children = m.max_children;
        let stdin = std::io::BufReader::new(std::io::stdin().lock());
        match al_sem::mcp::serve(cfg, Arc::new(CliDecide), stdin, Box::new(out)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "al-sem: mcp: {e}");
                ExitCode::from(1)
            }
        }
    }

    /// Bytes a spill file holds, checked against its length and hash.
    fn spilled(f: &al_sem::wire::SpillFile) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(&f.path).map_err(|e| format!("{}: {e}", f.path))?;
        if bytes.len() as u64 != f.bytes
            || al_sem::engine::ids::sha256_bytes_hex(&bytes) != f.sha256
        {
            return Err(format!(
                "{}: length or hash differs from the answer",
                f.path
            ));
        }
        let _ = std::fs::remove_file(&f.path);
        Ok(bytes)
    }

    /// Write an answer's streams and return its exit.
    fn deliver(outcome: Outcome) -> ExitCode {
        let failed = |line: String| ExitCode::from(replay(&JobOutput::lines(&[line], 1), 1));
        match outcome {
            Outcome::Cli {
                exit_code,
                stdout,
                stderr,
                ..
            } => {
                let stdout = match stdout {
                    Stdout::Text(t) => t.into_bytes(),
                    Stdout::File(f) => match spilled(&f) {
                        Ok(b) => b,
                        Err(e) => return failed(format!("al-sem: engine-ask: {e}")),
                    },
                };
                let stderr = match stderr {
                    Stderr::Lines(l) => l.bytes().to_vec(),
                    Stderr::File(f) => match spilled(&f) {
                        Ok(b) => b,
                        Err(e) => return failed(format!("al-sem: engine-ask: {e}")),
                    },
                };
                let out = JobOutput {
                    actions: vec![WriteAction::Stderr(stderr), WriteAction::Stdout(stdout)],
                    exit: exit_code,
                };
                ExitCode::from(replay(&out, 1))
            }
            Outcome::Failed {
                outcome,
                message,
                diagnostic_file,
            } => {
                let more = diagnostic_file
                    .map_or_else(String::new, |f| format!(" (full text: {})", f.path));
                failed(format!(
                    "al-sem: engine: {}: {message}{more}",
                    outcome.as_str()
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #246 G-2: a digest's admission kind follows `--include-reads`, so only
    /// a flag-on job takes the raised row.
    #[test]
    fn digest_job_kind_follows_include_reads() {
        let kind = |args: &[&str]| {
            let cli = Cli::try_parse_from([&["alsem", "digest", "ws"][..], args].concat()).unwrap();
            job_kind(&cli.command)
        };
        assert_eq!(kind(&[]), Some(("digest", ViewUniverse::CrossApp)));
        assert_eq!(
            kind(&["--include-reads"]),
            Some(("digest reads", ViewUniverse::CrossApp))
        );
        assert_eq!(
            kind(&["--include-reads", "--single-app"]),
            Some(("digest reads", ViewUniverse::SingleApp))
        );
    }

    /// Non-TTY stdout (piped) resolves `auto` → `Json`. This is the corpus-differential
    /// contract: differentials always pipe, so they always get JSON under `--format auto`.
    /// The test runner's stdout is non-TTY, so `resolve_auto_format("auto")` exercises the
    /// real `is_terminal() == false` branch here.
    #[test]
    fn auto_format_non_tty_resolves_to_json() {
        // The load-bearing assertion: `auto` under non-TTY stdout (the test harness) → Json.
        assert_eq!(resolve_auto_format("auto", false), OutputFormat::Json);
        // The explicit-string passthroughs.
        assert_eq!(resolve_auto_format("json", false), OutputFormat::Json);
        assert_eq!(
            resolve_auto_format("terminal", false),
            OutputFormat::Terminal
        );
        assert_eq!(resolve_auto_format("sarif", false), OutputFormat::Sarif);
        assert_eq!(
            resolve_auto_format("pr-summary", false),
            OutputFormat::PrSummary
        );
        assert_eq!(resolve_auto_format("html", false), OutputFormat::Html);
    }

    /// All production formats (Terminal/Html) return `Ok` from the pipeline. We drive a
    /// REAL fixture workspace so the primary format-match arm is exercised (NOT the
    /// fail-closed empty_output path).
    /// Json is implemented (Stage A2), Terminal (Stage A3), Html (Stage A4).
    #[test]
    fn stub_formats_return_err() {
        use al_sem::engine::gate::filter::Scope;
        use al_sem::engine::gate::run::{AnalyzeArgs, run_analyze_with_exit};
        use std::path::PathBuf;

        // A real, resolvable workspace fixture (the SARIF/pr-summary differentials use it).
        let ws = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("r0-corpus")
            .join("ws-d8-commit-in-tx");
        assert!(
            ws.is_dir(),
            "fixture ws-d8-commit-in-tx missing at {} (offline corpus incomplete)",
            ws.display()
        );

        // Terminal is implemented (Stage A3) — must succeed.
        {
            let args = AnalyzeArgs {
                workspace: ws.to_string_lossy().to_string(),
                min_severity: None,
                detector: None,
                preset: Some("transaction-integrity".to_string()),
                scope: Scope::Primary,
                limit: None,
                format: OutputFormat::Terminal,
                sarif_version_override: None,
                fail_on: None,
                require_dependencies: false,
                baseline: None,
                update_baseline: false,
                disable_inline_suppression: false,
                group_by: None,
                deterministic: false,
                with_evidence: false,
                single_app: false,
            };
            let inputs = args.read_inputs();
            let result = run_analyze_with_exit(&inputs, &args.read_request(), &args, "test");
            assert!(
                result.is_ok(),
                "Terminal format must succeed (Stage A3 implemented); got: {result:?}"
            );
        }
        // Html is implemented (Stage A4) — must succeed.
        {
            let args = AnalyzeArgs {
                workspace: ws.to_string_lossy().to_string(),
                min_severity: None,
                detector: None,
                preset: Some("transaction-integrity".to_string()),
                scope: Scope::Primary,
                limit: None,
                format: OutputFormat::Html,
                sarif_version_override: None,
                fail_on: None,
                require_dependencies: false,
                baseline: None,
                update_baseline: false,
                disable_inline_suppression: false,
                group_by: None,
                deterministic: false,
                with_evidence: false,
                single_app: false,
            };
            let inputs = args.read_inputs();
            let result = run_analyze_with_exit(&inputs, &args.read_request(), &args, "test");
            assert!(
                result.is_ok(),
                "Html format must succeed (Stage A4 implemented); got: {result:?}"
            );
        }
    }

    /// #277 final r2 R2-3 (FR-12): the jobs are made in a loop. 100 000 jobs
    /// on a 256 KiB stack: one frame per job would overflow it.
    #[test]
    fn make_jobs_runs_in_constant_stack() {
        const JOBS: usize = 100_000;
        let made = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let planned: Vec<Planned> = (0..JOBS).map(|_| failed("x", 1, 1)).collect();
                let reader = WorkspaceReader::new(vec![]);
                let slots: Vec<Slot> = (0..JOBS).map(|_| Slot::new()).collect();
                make_jobs(planned, &reader, &slots).len()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(made, JOBS);
    }

    /// #277 final r2 R2-5 (D3): while a batch's jobs are made, a job that gives
    /// its read up early does not make the next job on that workspace read it
    /// again; once every job is made, the reader holds no read.
    #[test]
    fn make_jobs_reads_a_workspace_once() {
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::sync::{Arc, Weak};
        let ws =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d1-multi-caller");
        let needs = WorkspaceNeeds::default();
        let first: Rc<RefCell<Weak<_>>> = Rc::default();
        let failed_job = || -> Box<dyn Job> {
            Box::new(Failed {
                output: JobOutput::lines(&["x".to_string()], 1),
                write_failure_exit: 1,
            })
        };
        let plan = |make: Box<MakeJob>| Planned::Ready {
            reads: vec![(ws.clone(), needs)],
            make,
        };
        let (path, seen) = (ws.clone(), Rc::clone(&first));
        let early = plan(Box::new(move |reader, _| {
            // Read, then give it up at once, as a job failing before its view.
            *seen.borrow_mut() = Arc::downgrade(&reader.read(&path, needs));
            failed_job()
        }));
        let (path, seen) = (ws.clone(), Rc::clone(&first));
        let next = plan(Box::new(move |reader, _| {
            let read = reader.read(&path, needs);
            let shared = seen.borrow().upgrade();
            assert!(
                shared.is_some_and(|s| Arc::ptr_eq(&s, &read)),
                "the workspace was read twice"
            );
            failed_job()
        }));
        let planned = vec![early, next];
        let reader =
            WorkspaceReader::new(planned.iter().flat_map(Planned::reads).collect::<Vec<_>>());
        let slots = [Slot::new(), Slot::new()];
        let jobs = make_jobs(planned, &reader, &slots);
        assert_eq!(jobs.len(), 2);
        assert!(
            first.borrow().upgrade().is_none(),
            "the reader kept the read past the jobs' making"
        );
    }

    // ── request-scoped planning (context-server design r3 §4.3, T-plan) ──

    use al_sem::engine::batch::plan_cx::{PROCESS_STDIN_READS, PlanFs, ProcessFs};
    use al_sem::program::inputs::WorkspaceInputs;
    use std::sync::atomic::Ordering;

    /// A request over the process's file system whose stdin is `stdin`.
    fn request(stdin: Option<&str>) -> PlanCx {
        PlanCx::new(&PlanReads::request(
            stdin.map(str::to_string),
            Box::new(ProcessFs),
        ))
    }

    /// The CLI's request: real stdin, the file system now.
    fn cli_request() -> PlanCx {
        PlanCx::new(&PlanReads::process())
    }

    /// `ws-explain`, absolute.
    fn explain_ws() -> String {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-explain")
            .to_string_lossy()
            .into_owned()
    }

    /// A diff of `ws-explain` changing `PostOrder` only.
    const DIFF_POST: &str = "--- a/src/post.al\n+++ b/src/post.al\n@@ -14,5 +14,5 @@\n         if IsHandled then\n             exit;\n-        Order.Posted := false;\n+        Order.Posted := true;\n         Order.Validate(Status, Order.Status::Posted);\n         Order.Modify(true);\n";

    /// A diff of `ws-explain` changing `Archive` only.
    const DIFF_ARCHIVE: &str = "--- a/src/post.al\n+++ b/src/post.al\n@@ -24,5 +24,5 @@\n     begin\n         Order.Status := Order.Status::Archived;\n-        Order.Modify(true);\n+        Order.Modify();\n         Commit();\n     end;\n";

    /// `alsem <args>` decided and planned through `cx` and run through the
    /// driver as a standalone run does, its writes held: (stdout bytes,
    /// stderr bytes, exit).
    fn run_through(args: &[&str], cx: PlanCx) -> (Vec<u8>, Vec<u8>, u8) {
        let cli = Cli::try_parse_from(std::iter::once("alsem").chain(args.iter().copied()))
            .expect("the argv parses");
        let (command, cx) = decide(cli.command, cx, None);
        let planned = plan(command, false, &cx);
        let reader = reader_for(planned.reads(), &cx);
        let slots = [Slot::new()];
        let jobs = make_jobs(vec![planned], &reader, &slots);
        let out = al_sem::engine::batch::standalone_output(&*jobs[0]);
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        for a in out.actions {
            match a {
                WriteAction::Stdout(b) => stdout.extend(b),
                WriteAction::Stderr(b) => stderr.extend(b),
                // A front end's summary (MCP output plan 1b/1e): no stream.
                WriteAction::Summary(_) => {}
                other => panic!("no file write expected: {other:?}"),
            }
        }
        (stdout, stderr, out.exit)
    }

    /// T-plan: two `--diff -` requests made one after the other in ONE
    /// process, each with its own diff text, answer their own bytes, and each
    /// equals the same diff read from a file (the CLI's route). The process's
    /// stdin is never read. Before, `--diff -` read a process `OnceLock`, so
    /// the second request answered the first diff's cards.
    #[test]
    fn sequential_diff_requests_get_their_own_bytes() {
        let ws = explain_ws();
        let stdin_reads = PROCESS_STDIN_READS.load(Ordering::SeqCst);
        let ask = |text: &str| {
            run_through(
                &["explain", &ws, "--diff", "-", "--format", "json"],
                request(Some(text)),
            )
        };
        let post = ask(DIFF_POST);
        let archive = ask(DIFF_ARCHIVE);
        assert_eq!((post.2, archive.2), (0, 0), "both resolve a routine");
        assert_ne!(post.0, archive.0, "two diffs, two answers");
        let names = |out: &[u8]| {
            let doc: serde_json::Value = serde_json::from_slice(out).expect("JSON");
            doc["cards"]
                .as_array()
                .expect("cards")
                .iter()
                .map(|c| c["routine"]["name"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&post.0), ["PostOrder"]);
        assert_eq!(names(&archive.0), ["Archive"]);
        // The oracle: the same diffs as files, through the CLI's context.
        let dir = tempfile::tempdir().unwrap();
        for (text, got) in [(DIFF_POST, &post), (DIFF_ARCHIVE, &archive)] {
            let file = dir.path().join("d.diff");
            std::fs::write(&file, text).unwrap();
            let file = file.to_string_lossy().into_owned();
            let cli = run_through(
                &["explain", &ws, "--diff", &file, "--format", "json"],
                cli_request(),
            );
            assert_eq!(&cli, got, "a request's bytes equal the CLI's");
        }
        assert_eq!(
            PROCESS_STDIN_READS.load(Ordering::SeqCst),
            stdin_reads,
            "a request context never reads the process's stdin"
        );
    }

    /// T-plan: a request that carries no diff text fails its `--diff -` as an
    /// unreadable stdin does (exit 3), and never reads the process's stdin
    /// (in a resident server that stream is the protocol).
    #[test]
    fn diff_stdin_without_text_never_reads_process_stdin() {
        let ws = explain_ws();
        let stdin_reads = PROCESS_STDIN_READS.load(Ordering::SeqCst);
        let (stdout, stderr, exit) = run_through(&["explain", &ws, "--diff", "-"], request(None));
        assert_eq!(exit, 3);
        assert!(stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&stderr),
            "al-sem: explain: failed to read diff from stdin\n"
        );
        assert_eq!(PROCESS_STDIN_READS.load(Ordering::SeqCst), stdin_reads);
    }

    /// T-plan: requests of ONE run planned at once on two threads, each
    /// through its own context over the run's shared reads, record only their
    /// own touched paths (a batch job's reads, D2.4). Before, one process list
    /// took both plans' paths.
    #[test]
    fn concurrent_plans_keep_their_touched_paths() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
        let dir = tempfile::tempdir().unwrap();
        let side = |name: &str, ws: &str, text: &str| {
            let diff = dir.path().join(format!("{name}.diff"));
            std::fs::write(&diff, text).unwrap();
            (root.join(ws), diff)
        };
        let a = side("a", "ws-explain", DIFF_POST);
        let b = side("b", "ws-rules", DIFF_ARCHIVE);
        let barrier = std::sync::Barrier::new(2);
        let reads = PlanReads::process();
        let plan_many = |(ws, diff): &(PathBuf, PathBuf)| {
            barrier.wait();
            let mut touched = Vec::new();
            for _ in 0..100 {
                let cli = Cli::try_parse_from([
                    "alsem",
                    "digest",
                    &ws.to_string_lossy(),
                    "--diff",
                    &diff.to_string_lossy(),
                ])
                .unwrap();
                let (command, cx) = decide(cli.command, PlanCx::new(&reads), None);
                drop(plan(command, true, &cx));
                touched.push(cx.take_touched());
            }
            touched
        };
        let (ta, tb) = std::thread::scope(|s| {
            let ta = s.spawn(|| plan_many(&a));
            let tb = s.spawn(|| plan_many(&b));
            (ta.join().unwrap(), tb.join().unwrap())
        });
        for ((ws, diff), other, runs) in [(&a, &b, &ta), (&b, &a, &tb)] {
            for touched in runs {
                assert!(touched.contains(diff), "its diff: {touched:?}");
                assert!(touched.contains(ws), "its workspace: {touched:?}");
                assert!(
                    !touched.contains(&other.1) && !touched.contains(&other.0),
                    "another plan's path: {touched:?}"
                );
            }
        }
    }

    /// Review r2 #16: the diff root's probes (`canonicalize`, the `.git`
    /// checks, `metadata`, the current directory) are asked of the request's
    /// context, never the process. A file system that exists only in the
    /// context decides the root.
    #[test]
    fn diff_root_probes_are_the_requests_inputs() {
        struct Virtual;
        impl PlanFs for Virtual {
            fn read(&self, _: &Path) -> std::io::Result<Vec<u8>> {
                Err(std::io::Error::other("no file"))
            }
            fn exists(&self, path: &Path) -> bool {
                path == Path::new("/virtual/repo/.git")
            }
            fn canonicalize(&self, path: &Path) -> std::io::Result<PathBuf> {
                Ok(Path::new("/virtual/repo/apps").join(path))
            }
            fn is_dir(&self, path: &Path) -> std::io::Result<bool> {
                Ok(path == Path::new("vroot"))
            }
            fn absolute(&self, path: &Path) -> std::io::Result<PathBuf> {
                Ok(Path::new("/virtual/repo").join(path))
            }
            fn read_stdin(&self) -> Option<String> {
                None
            }
        }
        let cx = PlanCx::new(&PlanReads::request(None, Box::new(Virtual)));
        // Auto mode: the nearest `.git` above the canonical workspace.
        assert_eq!(
            decide_diff_root("digest", "ws", None, true, &cx),
            Ok(DiffRoot::GitTop("apps/ws".into()))
        );
        // `--diff-root`: a folder only the context has, then a lexical
        // containment through the context's current directory.
        assert_eq!(
            decide_diff_root("digest", "vroot/ws", Some("vroot"), true, &cx),
            Ok(DiffRoot::Explicit("ws".into()))
        );
        assert_eq!(
            cx.take_touched(),
            [
                PathBuf::from("ws"),
                PathBuf::from("/virtual/repo/apps/ws/.git"),
                PathBuf::from("/virtual/repo/apps/.git"),
                PathBuf::from("/virtual/repo/.git"),
                PathBuf::from("vroot/ws"),
                PathBuf::from("vroot"),
            ]
        );
    }

    /// r3 §4.3: `make_job` (planning) probes no file system. A request whose
    /// caller decided the diff root and `--changed` plans `digest --diff -`
    /// and `explain --diff -` over a file system that panics on every
    /// question.
    #[test]
    fn planning_probes_no_file_system() {
        struct Refuses;
        impl PlanFs for Refuses {
            fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
                panic!("planning read {}", p.display())
            }
            fn exists(&self, p: &Path) -> bool {
                panic!("planning probed {}", p.display())
            }
            fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf> {
                panic!("planning canonicalized {}", p.display())
            }
            fn is_dir(&self, p: &Path) -> std::io::Result<bool> {
                panic!("planning asked metadata of {}", p.display())
            }
            fn absolute(&self, p: &Path) -> std::io::Result<PathBuf> {
                panic!("planning made {} absolute", p.display())
            }
            fn read_stdin(&self) -> Option<String> {
                panic!("planning read stdin")
            }
        }
        for argv in [
            ["alsem", "digest", "ws", "--diff", "-"],
            ["alsem", "explain", "ws", "--diff", "-"],
        ] {
            let decoded = Cli::try_parse_from(argv).unwrap().command;
            let reads = PlanReads::request(Some(DIFF_POST.into()), Box::new(Refuses));
            let shipped = Some((None, Some(Ok(DiffRoot::GitTop("apps/ws".into())))));
            let (decoded, cx) = decide(decoded, PlanCx::new(&reads), shipped);
            let planned = plan(decoded, false, &cx);
            assert!(matches!(planned, Planned::Ready { .. }), "{argv:?}");
            assert!(cx.take_touched().is_empty(), "{argv:?}: no probe recorded");
        }
    }

    /// r3 §4.3: a request given a workspace read (a resident revision's) makes
    /// its job off that read, never reading the workspace again: the files
    /// are changed on disk after the read and the answer still describes the
    /// read. A request naming another workspace, or needing more than the read
    /// holds, is refused (exit 3) before any job is made.
    #[test]
    fn a_given_workspace_read_answers_and_others_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        let src = PathBuf::from(explain_ws());
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::copy(src.join("app.json"), ws.join("app.json")).unwrap();
        for f in ["card.al", "order.al", "post.al", "tests.al"] {
            std::fs::copy(src.join("src").join(f), ws.join("src").join(f)).unwrap();
        }
        let ws_s = ws.to_string_lossy().into_owned();
        let inputs = WorkspaceInputs::read(&ws, &WorkspaceNeeds::default());
        // After the read: `Archive` is renamed on disk.
        let post = ws.join("src/post.al");
        let text = std::fs::read_to_string(&post).unwrap();
        assert_eq!(text.matches("procedure Archive(").count(), 1);
        std::fs::write(
            &post,
            text.replace("procedure Archive(", "procedure Stored("),
        )
        .unwrap();
        let argv = ["explain", &ws_s, "--routine", "Archive", "--format", "json"];
        let given = |inputs: &Arc<WorkspaceInputs>| {
            let mut cx = cli_request();
            cx.inputs = Some(Arc::clone(inputs));
            cx
        };
        let (out, err, exit) = run_through(&argv, given(&inputs));
        assert_eq!(
            exit,
            0,
            "the read still has Archive: {}",
            String::from_utf8_lossy(&err)
        );
        assert!(String::from_utf8_lossy(&out).contains("\"name\": \"Archive\""));
        // The CLI reads the disk now: no such routine.
        assert_eq!(run_through(&argv, cli_request()).2, 2);
        // Another workspace: refused.
        let other = explain_ws();
        let (out, err, exit) =
            run_through(&["explain", &other, "--routine", "Archive"], given(&inputs));
        assert_eq!((exit, out.is_empty()), (3, true));
        assert!(String::from_utf8_lossy(&err).contains("is not the workspace this request reads"));
        // More than the read holds (prove reads the fingerprint rows): refused.
        let (_, _, exit) = run_through(&["prove", &ws_s, "Archive", "may-commit"], given(&inputs));
        assert_eq!(exit, 3);
    }
}
