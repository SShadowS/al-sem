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
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
use al_sem::engine::batch::worker::{
    PlanFiles, StdinRead, WorkerBatch, WorkerInput, WorkerJob, Workers, report_peak, run_workers,
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
    /// on its `primaryLocation`, and bump the envelope `schemaVersion` to `1.1.0`.
    /// OFF by default — the default analyze output is byte-identical to today.
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

    /// Maximum number of via-paths per effect (reserved; wired but not yet honoured).
    #[arg(long = "max-paths")]
    max_paths: Option<usize>,

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

    /// A batch's decision whether `--changed` names an existing diff file,
    /// made once by the parent ([`pin_changed`]); `None`: ask the file system.
    #[arg(skip)]
    changed_is_file: Option<bool>,

    /// The run's diff-root decision ([`decide_diff_root`]), made once by the
    /// parent and carried to its worker ([`pin_diff_root`], #77); `None`:
    /// decide in `plan_digest`.
    #[arg(skip)]
    diff_root_decided: Option<Result<DiffRoot, String>>,
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
        Commands::Policy(PolicyCli {
            command: PolicyCommands::Explain(e),
        }) => run_policy_explain_cmd(e),
        Commands::Cache(CacheCli {
            command: CacheCommands::Prune(p),
        }) => run_cache_prune_cmd(p),
        job => run_planned(plan(job, false)),
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

/// Run a command as a batch of one (#277 D1).
fn run_planned(planned: Planned) -> ExitCode {
    let reader = WorkspaceReader::new(planned.reads());
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
/// `in_batch`: a manifest job, whose `--format auto` is JSON (A12.3).
fn plan(command: Commands, in_batch: bool) -> Planned {
    match command {
        Commands::Analyze(a) => plan_analyze(a, in_batch),
        Commands::Digest(d) => plan_digest(d),
        Commands::Prove(p) => plan_prove(p),
        Commands::Fingerprint(f) => plan_fingerprint(f),
        Commands::Diff(d) => plan_diff(d),
        Commands::Events(e) => match e.command {
            EventsCommands::Fanout(f) => plan_events_fanout(f),
            EventsCommands::Chains(c) => plan_events_chains(c),
        },
        Commands::Policy(p) => match p.command {
            PolicyCommands::Check(c) => plan_policy_check(c),
            PolicyCommands::Explain(_) => unreachable!("policy explain is no job"),
        },
        Commands::Query(q) => match q.command {
            QueryCommands::Touches(t) => plan_query_touches(t),
            QueryCommands::Effects(e) => plan_query_effects(e),
        },
        Commands::Graph(g) => plan_graph(g),
        Commands::Cache(_) | Commands::Batch(_) => unreachable!("no job"),
    }
}

// ── batch command (#277 D2) ──────────────────────────────────────────────────

/// The command a manifest job may not name, or `None` for a job command.
fn refused(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::Batch(_) => Some("batch"),
        Commands::Cache(_) => Some("cache prune"),
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
        Commands::Policy(_) | Commands::Cache(_) | Commands::Batch(_) => (None, None),
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
    let mut parsed = Vec::new();
    for (bi, batch) in manifest.batches.iter().enumerate() {
        let mut commands = Vec::new();
        for (i, job) in batch.jobs.iter().enumerate() {
            match parse_job(&job.argv) {
                Ok(mut c) => {
                    pin_changed(&mut c, None);
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
        for (i, (job, mut command)) in batch.jobs.iter().zip(commands).enumerate() {
            let (kind, mut written) = (job_kind(&command), job_paths(&command, &job.streams));
            PLAN_TOUCHED.lock().expect("plan touches").clear();
            // After the clear: the `.git` probes join this job's reads.
            let root = pin_diff_root(&mut command, None);
            decided.push((changed_is_file(&command), root));
            let p = plan(command, true);
            written
                .reads
                .append(&mut PLAN_TOUCHED.lock().expect("plan touches"));
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
    let stdin = STDIN.get().map(|read| match read {
        Some(text) => StdinRead::Text(text.clone()),
        None => StdinRead::Failed,
    });
    let files = PLAN_FILES.lock().expect("plan files").clone();
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
    if let Some(read) = input.stdin {
        let _ = STDIN.set(match read {
            StdinRead::Text(text) => Some(text),
            StdinRead::Failed => None,
        });
    }
    *PLAN_FILES.lock().expect("plan files") = input.files;
    let mut planned = Vec::new();
    for (i, job) in input.jobs.iter().enumerate() {
        match parse_job(&job.argv) {
            Ok(mut c) => {
                pin_changed(&mut c, job.changed_is_file);
                pin_diff_root(&mut c, job.diff_root.clone());
                planned.push(plan(c, true));
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
        Commands::Policy(_) | Commands::Cache(_) | Commands::Batch(_) => return None,
    })
}

/// Whether a fingerprint job's view is single-app. Read only for a job that
/// passed its validation (FR-14), so its format resolves.
fn fingerprint_single_app(f: &FingerprintCli) -> bool {
    default_format(f.format.as_deref(), f.shard.is_some()).is_ok_and(|format| {
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
            })
        },
    )
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
                    version,
                    t.deterministic,
                    t.single_app,
                )
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
                    .written(e.format == "json", e.out.as_deref()),
            )
        },
    )
}

// ── policy check command ─────────────────────────────────────────────────────

fn plan_policy_check(c: PolicyCheckCli) -> Planned {
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
        &plan_exists,
        &plan_read,
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

/// Stdin, read whole on the first ask; every `--diff -` job of the run gets
/// the same text (#277 D3: one reader). `None`: the read failed. A batch's
/// parent hands its read to every worker, which is seeded with it (FR-8).
static STDIN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// [`STDIN`]'s text, read on the first ask.
fn stdin_text() -> Option<&'static str> {
    STDIN
        .get_or_init(|| {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin().read_to_string(&mut buf).ok().map(|_| buf)
        })
        .as_deref()
}

/// What this run's plans read of the file system, once each (#277 final r2
/// R2-1). A batch's parent hands it to every worker, which is seeded with it.
static PLAN_FILES: std::sync::LazyLock<std::sync::Mutex<PlanFiles>> =
    std::sync::LazyLock::new(Default::default);

/// Every path the plan being made read or probed, in order: a batch's parent
/// adds them to that job's reads (D2.4; #277 final r3 F1).
static PLAN_TOUCHED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Record that the plan being made probed `path` ([`PLAN_TOUCHED`]), whatever
/// the probe's outcome.
fn plan_touch(path: &Path) {
    PLAN_TOUCHED
        .lock()
        .expect("plan touches")
        .push(path.to_path_buf());
}

/// `path`'s bytes as a plan reads them: read once per run ([`PLAN_FILES`]).
/// The error keeps its text, not its kind.
fn plan_read(path: &Path) -> std::io::Result<Vec<u8>> {
    plan_touch(path);
    let mut files = PLAN_FILES.lock().expect("plan files");
    let read = files
        .reads
        .entry(path.to_path_buf())
        .or_insert_with(|| std::fs::read(path).map_err(|e| e.to_string()));
    read.clone().map_err(std::io::Error::other)
}

/// Whether `path` exists, as a plan asks it: answered once per run.
fn plan_exists(path: &Path) -> bool {
    plan_touch(path);
    let mut files = PLAN_FILES.lock().expect("plan files");
    *files
        .exists
        .entry(path.to_path_buf())
        .or_insert_with(|| path.exists())
}

/// Decide once whether `digest --changed` names an existing diff file
/// (#277 final r1 FR-3): `decided` when a batch's parent decided it, else
/// the file system now.
fn pin_changed(command: &mut Commands, decided: Option<bool>) {
    if let Commands::Digest(d) = command
        && let Some(value) = &d.changed
    {
        d.changed_is_file = Some(decided.unwrap_or_else(|| Path::new(value).exists()));
    }
}

/// A digest's `--changed` decision, for its worker.
fn changed_is_file(command: &Commands) -> Option<bool> {
    match command {
        Commands::Digest(d) => d.changed_is_file,
        _ => None,
    }
}

/// What `--changed <value>` names: a diff file as decided ([`pin_changed`]),
/// else as the file system says now.
fn detect_changed(d: &DigestCli, value: &str) -> ChangedAutoDetect {
    auto_detect_changed_with(value, |p| {
        d.changed_is_file.unwrap_or_else(|| Path::new(p).exists())
    })
}

/// Decide a digest's diff root once (#77): `decided` when a batch's parent
/// decided it (a worker never decides it again), else the file system now.
/// Returns the decision, for the worker.
fn pin_diff_root(
    command: &mut Commands,
    decided: Option<Result<DiffRoot, String>>,
) -> Option<Result<DiffRoot, String>> {
    let Commands::Digest(d) = command else {
        return None;
    };
    // The batch parse pinned `--changed` ([`pin_changed`]), so this asks the
    // file system nothing more about it.
    let root = decided.unwrap_or_else(|| decide_diff_root(d, DigestInputs::of(d).reads_paths()));
    d.diff_root_decided = Some(root.clone());
    Some(root)
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
/// ([`plan_touch`]), so a batch job's reads hold `--diff-root` and the
/// workspace even when the probe fails.
fn decide_diff_root(d: &DigestCli, reads_paths: bool) -> Result<DiffRoot, String> {
    if !reads_paths {
        return Ok(DiffRoot::Workspace);
    }
    plan_touch(Path::new(&d.workspace));
    let canonical_ws = std::fs::canonicalize(&d.workspace);
    let Some(root) = &d.diff_root else {
        let Ok(ws) = canonical_ws else {
            return Ok(DiffRoot::Workspace);
        };
        return Ok(ws
            .ancestors()
            .find(|dir| plan_exists(&dir.join(".git")))
            .and_then(|top| ws.strip_prefix(top).ok())
            .map_or(DiffRoot::Workspace, |rel| DiffRoot::below(rel, false)));
    };
    // Checked first, so a lexical containment cannot pass a missing root.
    plan_touch(Path::new(root));
    match std::fs::metadata(root) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "al-sem: digest: --diff-root '{root}' is not a folder"
            ));
        }
        Err(e) => return Err(format!("al-sem: digest: --diff-root '{root}': {e}")),
    }
    // A workspace that does not resolve is left to the load, which reports it
    // (exit 1) however it is spelled, as in auto mode.
    let Ok(ws) = canonical_ws else {
        return Ok(DiffRoot::Workspace);
    };
    let lexical = std::path::absolute(&d.workspace)
        .ok()
        .zip(std::path::absolute(root).ok())
        .and_then(|(ws, r)| ws.strip_prefix(&r).ok().map(Path::to_path_buf))
        .filter(|rel| {
            !rel.components()
                .any(|c| c == std::path::Component::ParentDir)
        });
    let rel = match lexical {
        Some(rel) => rel,
        None => {
            let r = std::fs::canonicalize(root)
                .map_err(|e| format!("al-sem: digest: --diff-root '{root}': {e}"))?;
            ws.strip_prefix(&r).map(Path::to_path_buf).map_err(|_| {
                format!(
                    "al-sem: digest: workspace '{}' is not inside --diff-root '{root}'",
                    d.workspace
                )
            })?
        }
    };
    Ok(DiffRoot::below(&rel, true))
}

fn plan_digest(d: DigestCli) -> Planned {
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
        Some("-") => match stdin_text() {
            Some(text) => Some(text.to_string()),
            None => {
                return failed(
                    "al-sem: digest: failed to read diff from stdin",
                    exit::CONFIG_ERROR,
                    1,
                );
            }
        },
        Some(path) => match FileInput::read_with(path.into(), &plan_read).text() {
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

    // A batch's parent decided it (its workers get the decision); else now.
    let diff_root = match d
        .diff_root_decided
        .clone()
        .unwrap_or_else(|| decide_diff_root(&d, reads_paths))
    {
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
            Box::new(DigestJob::new(req, d.format == "human", d.out.as_deref()))
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
                human: p.format == "human",
                out: p.out.as_deref(),
            })
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
    let format = match default_format(f.format.as_deref(), shard_mode.is_some()) {
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
}
