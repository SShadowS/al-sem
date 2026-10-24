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

use al_sem::engine::batch::job::{Failed, Job, WorkspaceReader};
use al_sem::engine::batch::manifest::BatchSpec;
use al_sem::engine::batch::output::{JobOutput, WriteAction, replay};
use al_sem::engine::batch::{JobLine, run_batch, run_standalone};
use al_sem::engine::gate::cache_prune::{format_prune_report, prune_cache};
use al_sem::engine::gate::events::{EventsChainsOptions, EventsFanoutOptions, EventsJob};
use al_sem::engine::gate::exit_code::{exit, parse_fail_on};
use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::presets::{preset_names, resolve_analyze_detectors};
use al_sem::engine::gate::run::{AnalyzeArgs, AnalyzeJob, OutputFormat};
use al_sem::engine::gate::version::driver_version;
use al_sem::engine::l4::effect_query_cli::{Direction, QueryJob};
use al_sem::engine::l5::digest_cli::{
    ChangedAutoDetect, DigestJob, DigestRequest, auto_detect_changed,
};
use al_sem::engine::l5::event_flow::Scope as EventScope;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintJob, FingerprintOptions, ShardMode, SpecifiedFlags, default_format,
    normalize_witness, reject_illegal_combos, reject_inventory_only_combos, validate_roots,
};
use al_sem::engine::l5::prove::{ProveJob, parse_question, question_ids};
use al_sem::program::inputs::{RequestInputs, WorkspaceNeeds};
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
    /// Run the commands of a JSON manifest in one process: jobs on one
    /// workspace share one build. Prints one JSON line per job.
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
    ///   | publishes-event:<name> | reaches-ui | throws-error.
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

    /// Workspace-relative source file(s) to treat as changed.
    /// May be repeated. Mutually exclusive with --order (rejected).
    #[arg(long = "file", action = clap::ArgAction::Append)]
    file: Vec<String>,

    /// Routine stable IDs or display names to treat as changed roots.
    #[arg(long = "routine", action = clap::ArgAction::Append)]
    routine: Vec<String>,

    /// Read a unified diff from a file (or `-` for stdin) to resolve changed roots.
    #[arg(long = "diff")]
    diff: Option<String>,

    /// Convenience alias — auto-detect: existing file path → --diff; comma-list with
    /// `.al` entries → --changed-files; else → --changed-routines.
    #[arg(long = "changed")]
    changed: Option<String>,

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
        job => run_planned(plan(job)),
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
        /// Read the workspaces through the shared reader, make the job and hand
        /// it on (the job borrows what this owns).
        make: Box<MakeJob>,
    },
}

/// Makes a command's job off the shared reads and hands it to the continuation.
type MakeJob = dyn FnOnce(&WorkspaceReader, &mut dyn FnMut(&dyn Job));

/// A command that reads `workspace` with `needs`, its job made by `make`.
fn ready(
    workspace: &str,
    needs: WorkspaceNeeds,
    make: impl FnOnce(&WorkspaceReader, &mut dyn FnMut(&dyn Job)) + 'static,
) -> Planned {
    Planned::Ready {
        reads: vec![(PathBuf::from(workspace), needs)],
        make: Box::new(make),
    }
}

/// What digest, prove and fingerprint read: the snapshot's input rows.
const FINGERPRINT_ROWS: WorkspaceNeeds = WorkspaceNeeds {
    build_only: false,
    fingerprint_rows: true,
};

/// Run a command as a batch of one (#277 D1).
fn run_planned(planned: Planned) -> ExitCode {
    match planned {
        Planned::Failed(f) => standalone(&f),
        Planned::Ready { reads, make } => {
            let reader = WorkspaceReader::new(reads);
            let mut code = None;
            make(&reader, &mut |job| code = Some(standalone(job)));
            code.expect("the command made its job")
        }
    }
}

/// Run `job` as a batch of one (#277 D1).
fn standalone(job: &dyn Job) -> ExitCode {
    ExitCode::from(run_standalone(job))
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
fn plan(command: Commands) -> Planned {
    match command {
        Commands::Analyze(a) => plan_analyze(a),
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

/// `alsem batch <manifest>`: every job's argv is parsed first (a usage error
/// runs nothing, exit 3); then each batch runs in this process, in manifest
/// order, and the run prints one line per job. Exit 0 when every job ran,
/// whatever its own exit (D2.2).
fn run_batch_cmd(b: BatchCli) -> ExitCode {
    use al_sem::engine::batch::manifest::load;
    let usage =
        |line: String| ExitCode::from(replay(&JobOutput::lines(&[line], exit::CONFIG_ERROR), 1));
    let manifest = match load(Path::new(&b.manifest), b.out_dir.as_deref().map(Path::new)) {
        Ok(m) => m,
        Err(line) => return usage(line),
    };
    let mut parsed = Vec::new();
    for (bi, batch) in manifest.batches.iter().enumerate() {
        let mut commands = Vec::new();
        for (i, job) in batch.jobs.iter().enumerate() {
            match parse_job(&job.argv) {
                Ok(c) => commands.push(c),
                Err(why) => return usage(format!("al-sem batch: batch {bi} job {i}: {why}")),
            }
        }
        parsed.push(commands);
    }
    let mut lines = Vec::new();
    for (bi, (batch, commands)) in manifest.batches.iter().zip(parsed).enumerate() {
        let b = u32::try_from(bi).expect("at most u32::MAX batches");
        lines.extend(run_one_batch(
            b,
            batch,
            commands.into_iter().map(plan).collect(),
        ));
    }
    let mut out = String::new();
    for line in &lines {
        out.push_str(&serde_json::to_string(line).expect("a job line serializes"));
        out.push('\n');
    }
    ExitCode::from(replay(
        &JobOutput {
            actions: vec![WriteAction::Stdout(out.into_bytes())],
            exit: exit::CLEAN,
        },
        1,
    ))
}

/// One batch: every workspace read once with the union of its jobs' needs
/// (D3), the jobs made off those reads, then the driver over all of them.
fn run_one_batch(b: u32, batch: &BatchSpec, planned: Vec<Planned>) -> Vec<JobLine> {
    let reads = planned.iter().flat_map(|p| match p {
        Planned::Ready { reads, .. } => reads.clone(),
        Planned::Failed(_) => vec![],
    });
    let reader = WorkspaceReader::new(reads.collect::<Vec<_>>());
    let mut lines = None;
    with_jobs(&mut planned.into_iter(), &reader, Vec::new(), &mut |jobs| {
        let jobs: Vec<_> = jobs
            .iter()
            .zip(&batch.jobs)
            .map(|(job, spec)| (*job, spec.streams.clone()))
            .collect();
        lines = Some(run_batch(b, &jobs));
    });
    lines.expect("the batch ran")
}

/// Make every planned job (each borrows what its plan owns) and hand them all
/// to `run`, in order.
fn with_jobs(
    rest: &mut std::vec::IntoIter<Planned>,
    reader: &WorkspaceReader,
    made: Vec<&dyn Job>,
    run: &mut dyn FnMut(&[&dyn Job]),
) {
    match rest.next() {
        None => run(&made),
        Some(Planned::Failed(f)) => {
            let mut made = made;
            made.push(&f);
            with_jobs(rest, reader, made, run);
        }
        Some(Planned::Ready { make, .. }) => make(reader, &mut |job| {
            let mut made: Vec<&dyn Job> = made.clone();
            made.push(job);
            with_jobs(rest, reader, made, run);
        }),
    }
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
        move |reader, k| {
            let inputs = reader.read(Path::new(&g.workspace), WorkspaceNeeds::default());
            k(&al_sem::program::impact_graph::GraphJob {
                inputs: &inputs,
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
        move |reader, k| {
            let inputs = reader.read(Path::new(&t.workspace), WorkspaceNeeds::default());
            k(&QueryJob::touches(
                &inputs,
                &t.table,
                t.from.as_deref(),
                direction,
                &inputs.driver_version,
                t.deterministic,
                t.single_app,
            )
            .written(t.format == "json", t.out.as_deref()))
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
        move |reader, k| {
            let inputs = reader.read(Path::new(&e.workspace), WorkspaceNeeds::default());
            k(&QueryJob::effects(
                &inputs,
                &e.routine,
                &inputs.driver_version,
                e.deterministic,
                e.single_app,
            )
            .written(e.format == "json", e.out.as_deref()))
        },
    )
}

// ── policy check command ─────────────────────────────────────────────────────

fn plan_policy_check(c: PolicyCheckCli) -> Planned {
    use al_sem::engine::gate::policy::pipeline::{PolicyCheckOptions, PolicyJob, policy_request};

    // Validate --format (human | json | sarif). al-sem writes the message + exit 1.
    const VALID_FORMATS: &[&str] = &["human", "json", "sarif"];
    if !VALID_FORMATS.contains(&c.format.as_str()) {
        return failed(
            format!("al-sem policy check: invalid --format '{}'", c.format),
            1,
            1,
        );
    }

    ready(
        &c.workspace.clone(),
        WorkspaceNeeds::default(),
        move |reader, k| {
            let workspace = Path::new(&c.workspace);
            let inputs = reader.read(workspace, WorkspaceNeeds::default());
            let request = RequestInputs::read(
                workspace,
                policy_request(c.policy.as_deref(), c.no_policy),
                None,
            );
            let opts = PolicyCheckOptions {
                inputs: &inputs,
                request: &request,
                format: &c.format,
                out: c.out.as_deref(),
                deterministic: c.deterministic,
                strict: c.strict,
                driver_version: &inputs.driver_version,
                single_app: c.single_app,
            };

            // A bad policy is found before any build: a job with no view (#277 D3).
            match PolicyJob::new(&opts) {
                Ok(job) => k(&job),
                Err(early) => k(&Failed {
                    output: early.into_output(),
                    write_failure_exit: 1,
                }),
            }
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
    use al_sem::engine::gate::diff::cli::{DIFF_NEEDS, DiffCliOptions, DiffJob};
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

    // Each side that is a directory is a workspace side (`DiffJob::new` decides).
    let reads = [&d.old, &d.new]
        .into_iter()
        .filter(|arg| Path::new(arg).is_dir())
        .map(|arg| (PathBuf::from(arg), DIFF_NEEDS))
        .collect();
    Planned::Ready {
        reads,
        make: Box::new(move |reader, k| {
            let version = driver_version();
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
                driver_version: &version,
            };

            // An input error is found before any build: a job with no view.
            match DiffJob::new(&opts, reader) {
                Ok(job) => k(&job),
                Err(early) => k(&Failed {
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

    // Resolve the --changed alias FIRST (auto-detect into diff / files / routines),
    // mirroring cli/index.ts. The detected values are merged with explicit flags.
    let mut file_inputs: Vec<String> = d.file.clone();
    let mut routine_inputs: Vec<String> = d.routine.clone();
    let mut diff_arg: Option<String> = d.diff.clone();
    if let Some(changed) = d.changed.as_deref()
        && !changed.trim().is_empty()
    {
        match auto_detect_changed(changed) {
            ChangedAutoDetect::Diff(p) => diff_arg = Some(p),
            ChangedAutoDetect::Files(fs) => file_inputs.extend(fs),
            ChangedAutoDetect::Routines(rs) => routine_inputs.extend(rs),
        }
    }

    // No-input check (cli/digest.ts:138): emit the exact CLI message + exit 1.
    // Done at the CLI layer BEFORE the pipeline so the user sees the --changed-files
    // wording (the pipeline's own message is the internal run-digest-pipeline one).
    let has_files = !file_inputs.is_empty();
    let has_routines = !routine_inputs.is_empty();
    let has_diff = diff_arg
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
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
        Some("-") => {
            // stdin: buffer all of it
            use std::io::Read;
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                return failed(
                    "al-sem: digest: failed to read diff from stdin",
                    exit::CONFIG_ERROR,
                    1,
                );
            }
            Some(buf)
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(s) => Some(s),
            Err(e) => {
                return failed(
                    format!("al-sem: digest: could not read diff file '{path}': {e}"),
                    exit::CONFIG_ERROR,
                    1,
                );
            }
        },
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

    ready(&d.workspace.clone(), FINGERPRINT_ROWS, move |reader, k| {
        let inputs = reader.read(Path::new(&d.workspace), FINGERPRINT_ROWS);
        let req = DigestRequest {
            inputs: &inputs,
            changed_files,
            changed_routines,
            diff_text,
            alsem_ver: &inputs.driver_version,
            deterministic: d.deterministic,
            single_app: d.single_app,
            tests: d.tests,
        };
        k(&DigestJob::new(req, d.format == "human", d.out.as_deref()))
    })
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

    ready(&p.workspace.clone(), FINGERPRINT_ROWS, move |reader, k| {
        let inputs = reader.read(Path::new(&p.workspace), FINGERPRINT_ROWS);
        k(&ProveJob {
            detectors: al_sem::engine::l5::detectors::default_detectors(),
            inputs: &inputs,
            routine_selector: &p.routine,
            question_text: &p.question,
            alsem_ver: &inputs.driver_version,
            deterministic: p.deterministic,
            single_app: p.single_app,
            human: p.format == "human",
            out: p.out.as_deref(),
        })
    })
}

// ── fingerprint command ─────────────────────────────────────────────────────

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

    // _specifiedFlags (index.ts:434-439).
    let specified = SpecifiedFlags {
        roots: f.roots.is_some(),
        routine_selectors: !f.routine.is_empty(),
        include_inherited: f.no_include_inherited,
        witness: f.witness.is_some() && f.witness.as_deref() != Some("3"),
    };

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
    ready(&f.workspace.clone(), FINGERPRINT_ROWS, move |reader, k| {
        let inputs = reader.read(Path::new(&f.workspace), FINGERPRINT_ROWS);
        let opts = FingerprintOptions {
            inputs: &inputs,
            driver_version: &inputs.driver_version,
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

        // `--shard` without `--out` is found before any build: a job with no view.
        match FingerprintJob::new(&opts) {
            Ok(job) => k(&job),
            Err(fail) => k(&Failed {
                output: fail,
                write_failure_exit: 1,
            }),
        }
    })
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
        move |reader, k| {
            let inputs = reader.read(Path::new(&f.workspace), WorkspaceNeeds::default());
            let opts = EventsFanoutOptions {
                inputs: &inputs,
                format: &f.format,
                scope,
                coverage_policy: &f.coverage_policy,
                driver_version: &inputs.driver_version,
                deterministic: f.deterministic,
                strict: f.strict,
                single_app: f.single_app,
            };
            k(&EventsJob::fanout(&opts, f.out.as_deref()))
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
        move |reader, k| {
            let inputs = reader.read(Path::new(&c.workspace), WorkspaceNeeds::default());
            let opts = EventsChainsOptions {
                inputs: &inputs,
                format: &c.format,
                scope,
                coverage_policy: &c.coverage_policy,
                max_depth: c.max_depth,
                max_nodes: c.max_nodes,
                driver_version: &inputs.driver_version,
                deterministic: c.deterministic,
                strict: c.strict,
                single_app: c.single_app,
            };
            k(&EventsJob::chains(&opts, c.out.as_deref()))
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
/// Non-TTY stdout → `Json`; TTY stdout → `Terminal`.
/// This is the testable contract (corpus differentials always pipe → non-TTY → json).
fn resolve_auto_format(format_str: &str) -> OutputFormat {
    match format_str {
        "auto" => {
            if std::io::stdout().is_terminal() {
                OutputFormat::Terminal
            } else {
                OutputFormat::Json
            }
        }
        "sarif" => OutputFormat::Sarif,
        "pr-summary" => OutputFormat::PrSummary,
        "terminal" => OutputFormat::Terminal,
        "json" => OutputFormat::Json,
        "html" => OutputFormat::Html,
        // Unknown values are caught before this call; return Json as a safe fallback.
        _ => OutputFormat::Json,
    }
}

fn plan_analyze(a: AnalyzeCli) -> Planned {
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
    let format = resolve_auto_format(&a.format);

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
        move |reader, k| {
            let inputs = reader.read(Path::new(&args.workspace), WorkspaceNeeds::default());
            let request = args.read_request();
            // `new` fails only on the detector set, resolved above.
            let job = AnalyzeJob::new(&inputs, &request, &args, &inputs.driver_version)
                .expect("the detector set resolved");
            k(&job)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Non-TTY stdout (piped) resolves `auto` → `Json`. This is the corpus-differential
    /// contract: differentials always pipe, so they always get JSON under `--format auto`.
    /// The test runner's stdout is non-TTY, so `resolve_auto_format("auto")` exercises the
    /// real `is_terminal() == false` branch here.
    #[test]
    fn auto_format_non_tty_resolves_to_json() {
        // The load-bearing assertion: `auto` under non-TTY stdout (the test harness) → Json.
        assert_eq!(resolve_auto_format("auto"), OutputFormat::Json);
        // The explicit-string passthroughs.
        assert_eq!(resolve_auto_format("json"), OutputFormat::Json);
        assert_eq!(resolve_auto_format("terminal"), OutputFormat::Terminal);
        assert_eq!(resolve_auto_format("sarif"), OutputFormat::Sarif);
        assert_eq!(resolve_auto_format("pr-summary"), OutputFormat::PrSummary);
        assert_eq!(resolve_auto_format("html"), OutputFormat::Html);
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
}
