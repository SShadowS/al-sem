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

use al_sem::engine::gate::cache_prune::{format_prune_report, prune_cache};
use al_sem::engine::gate::events::{
    EventsChainsOptions, EventsFanoutOptions, run_events_chains, run_events_fanout,
};
use al_sem::engine::gate::exit_code::{exit, parse_fail_on};
use al_sem::engine::gate::filter::Scope;
use al_sem::engine::gate::presets::PRESET_NAMES_LIST;
use al_sem::engine::gate::run::{AnalyzeArgs, OutputFormat, run_analyze_with_exit};
use al_sem::engine::gate::version::driver_version;
use al_sem::engine::l4::effect_query_cli::{
    Direction, QueryRunResult, run_query_effects_pipeline, run_query_touches_pipeline,
};
use al_sem::engine::l5::digest_cli::{ChangedAutoDetect, auto_detect_changed, run_digest_pipeline};
use al_sem::engine::l5::event_flow::Scope as EventScope;
use al_sem::engine::l5::fingerprint_cli::{
    FingerprintOptions, FingerprintOutput, ShardMode, SpecifiedFlags, default_format,
    normalize_witness, reject_illegal_combos, run_fingerprint_pipeline, validate_roots,
    write_fingerprint_output,
};
use al_sem::engine::l5::prove::{parse_question, question_ids, run_prove_pipeline};
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

    /// Table selector: a table NAME, an internal table id, or the literal
    /// `unknown` (the "effect is real, target table unresolved" bucket — not a
    /// table).
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
    // Warnings go to stderr (a dropped dependency was once only a `warn!` that
    // nothing printed); `RUST_LOG` overrides the level. stdout is unchanged.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Analyze(a) => run_analyze_cmd(a),
        Commands::Digest(d) => run_digest_cmd(d),
        Commands::Prove(p) => run_prove_cmd(p),
        Commands::Fingerprint(f) => run_fingerprint_cmd(f),
        Commands::Diff(d) => run_diff_cmd(d),
        Commands::Events(e) => match e.command {
            EventsCommands::Fanout(f) => run_events_fanout_cmd(f),
            EventsCommands::Chains(c) => run_events_chains_cmd(c),
        },
        Commands::Policy(p) => match p.command {
            PolicyCommands::Check(c) => run_policy_check_cmd(c),
            PolicyCommands::Explain(e) => run_policy_explain_cmd(e),
        },
        Commands::Cache(c) => match c.command {
            CacheCommands::Prune(p) => run_cache_prune_cmd(p),
        },
        Commands::Query(q) => match q.command {
            QueryCommands::Touches(t) => run_query_touches_cmd(t),
            QueryCommands::Effects(e) => run_query_effects_cmd(e),
        },
    }
}

// ── query commands ───────────────────────────────────────────────────────────

/// Write a query result to `--out` or stdout, returning its exit code.
fn emit_query(label: &str, result: QueryRunResult, format: &str, out: Option<&str>) -> ExitCode {
    let output = if format == "json" {
        result.json_text
    } else {
        result.human_text
    };
    let write_result = if let Some(out_path) = out {
        std::fs::write(out_path, &output).map_err(|e| format!("{e}"))
    } else {
        use std::io::Write;
        std::io::stdout()
            .write_all(output.as_bytes())
            .map_err(|e| format!("{e}"))
    };
    if let Err(e) = write_result {
        eprintln!("al-sem: {label}: write error: {e}");
        return ExitCode::from(1);
    }
    ExitCode::from(result.exit_code)
}

const VALID_QUERY_FORMATS: &[&str] = &["human", "json"];

fn run_query_touches_cmd(t: QueryTouchesCli) -> ExitCode {
    if !VALID_QUERY_FORMATS.contains(&t.format.as_str()) {
        eprintln!(
            "al-sem query touches: invalid --format '{}'. Expected: human | json",
            t.format
        );
        return ExitCode::from(1);
    }
    let direction = match Direction::parse(&t.direction) {
        Ok(d) => d,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    // `--direction down` without `--from` is rejected by `render_query_touches`
    // itself, not duplicated here: one rule in one place, and the golden family
    // covers it end to end (a second copy could drift and would test nothing).
    let workspace = std::path::Path::new(&t.workspace);
    let version = driver_version();
    match run_query_touches_pipeline(
        workspace,
        &t.table,
        t.from.as_deref(),
        direction,
        &version,
        t.deterministic,
    ) {
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(1)
        }
        Ok(result) => emit_query("query touches", result, &t.format, t.out.as_deref()),
    }
}

fn run_query_effects_cmd(e: QueryEffectsCli) -> ExitCode {
    if !VALID_QUERY_FORMATS.contains(&e.format.as_str()) {
        eprintln!(
            "al-sem query effects: invalid --format '{}'. Expected: human | json",
            e.format
        );
        return ExitCode::from(1);
    }
    let workspace = std::path::Path::new(&e.workspace);
    let version = driver_version();
    match run_query_effects_pipeline(workspace, &e.routine, &version, e.deterministic) {
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(1)
        }
        Ok(result) => emit_query("query effects", result, &e.format, e.out.as_deref()),
    }
}

// ── policy check command ─────────────────────────────────────────────────────

fn run_policy_check_cmd(c: PolicyCheckCli) -> ExitCode {
    use al_sem::engine::gate::policy::pipeline::{PolicyCheckOptions, run_policy_check};

    // Validate --format (human | json | sarif). al-sem writes the message + exit 1.
    const VALID_FORMATS: &[&str] = &["human", "json", "sarif"];
    if !VALID_FORMATS.contains(&c.format.as_str()) {
        eprintln!("al-sem policy check: invalid --format '{}'", c.format);
        return ExitCode::from(1);
    }

    let workspace = std::path::Path::new(&c.workspace);
    let version = driver_version();
    let opts = PolicyCheckOptions {
        workspace,
        policy_path: c.policy.as_deref(),
        no_policy: c.no_policy,
        format: &c.format,
        out: c.out.as_deref(),
        deterministic: c.deterministic,
        strict: c.strict,
        driver_version: &version,
    };

    let outcome = run_policy_check(&opts);

    // A non-zero exit with no text → stderr-only error (load error / strict gate /
    // modelInstanceId failure). Print stderr lines, no stdout.
    if outcome.text.is_none() {
        for line in &outcome.stderr_lines {
            eprintln!("{line}");
        }
        return ExitCode::from(outcome.exit_code);
    }

    if let Some(text) = &outcome.text {
        let write_result = if let Some(ref out_path) = outcome.out_path {
            std::fs::write(out_path, text).map_err(|e| format!("{e}"))
        } else {
            use std::io::Write;
            std::io::stdout()
                .write_all(text.as_bytes())
                .map_err(|e| format!("{e}"))
        };
        if let Err(e) = write_result {
            eprintln!("failed to write: {e}");
            return ExitCode::from(1);
        }
    }

    // Analyzer diagnostics go to stderr AFTER the output (al-sem's
    // `for (const d of diagnostics) process.stderr.write(...)` at the tail).
    for line in &outcome.stderr_lines {
        eprintln!("{line}");
    }

    ExitCode::from(outcome.exit_code)
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

    if let Some(stdout) = &outcome.stdout {
        use std::io::Write;
        if std::io::stdout().write_all(stdout.as_bytes()).is_err() {
            eprintln!("failed to write");
            return ExitCode::from(1);
        }
    }
    for line in &outcome.stderr_lines {
        eprintln!("{line}");
    }
    ExitCode::from(outcome.exit_code)
}

// ── diff command ────────────────────────────────────────────────────────────

fn run_diff_cmd(d: DiffCli) -> ExitCode {
    use al_sem::engine::gate::diff::cli::{DiffCliOptions, run_diff};
    use al_sem::engine::gate::diff::{CoveragePolicy, Severity};

    // Validate --format (human | json | sarif). al-sem writes the message + exit 1.
    const VALID_FORMATS: &[&str] = &["human", "json", "sarif"];
    if !VALID_FORMATS.contains(&d.format.as_str()) {
        eprintln!(
            "unknown --format '{}'; valid: {}",
            d.format,
            VALID_FORMATS.join(", ")
        );
        return ExitCode::from(1);
    }

    // Validate --coverage-policy (loose | strict).
    let coverage_policy = match d.coverage_policy.as_str() {
        "loose" => CoveragePolicy::Loose,
        "strict" => CoveragePolicy::Strict,
        other => {
            eprintln!("--coverage-policy must be loose|strict (got '{other}')");
            return ExitCode::from(1);
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
            eprintln!("--fail-on must be one of: critical|high|medium|low|info");
            return ExitCode::from(1);
        }
    };

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

    let outcome = run_diff(&opts);

    if let Some(msg) = outcome.error_message {
        eprintln!("{msg}");
        return ExitCode::from(outcome.exit_code);
    }

    if let Some(text) = outcome.output {
        let write_result = if let Some(ref out_path) = d.out {
            std::fs::write(out_path, &text).map_err(|e| format!("{e}"))
        } else {
            use std::io::Write;
            std::io::stdout()
                .write_all(text.as_bytes())
                .map_err(|e| format!("{e}"))
        };
        if let Err(e) = write_result {
            eprintln!("failed to write: {e}");
            return ExitCode::from(1);
        }
    }

    for line in &outcome.stderr_lines {
        eprintln!("{line}");
    }

    ExitCode::from(outcome.exit_code)
}

const GROUP_BY_VALUES: &[&str] = &["object", "routine", "table", "detector", "file"];

// ── digest command ──────────────────────────────────────────────────────────

/// The exact stderr message emitted when `--order` is used with `digest`.
const ORDER_REJECTION: &str = "al-sem: digest --order is not supported by the Rust engine";

fn run_digest_cmd(d: DigestCli) -> ExitCode {
    // Reject --order (CONFIG_ERROR — a not-ported flag rejected cleanly rather than silently ignored)
    if d.order {
        eprintln!("{ORDER_REJECTION}");
        return ExitCode::from(exit::CONFIG_ERROR);
    }

    // Validate --format. al-sem (cli/index.ts:578) writes this exact message and
    // exits 1 (FINDINGS), NOT CONFIG_ERROR (3) — the digest format check is at the
    // CLI layer, distinct from analyze's enum-flag CONFIG_ERROR exits (#4).
    const VALID_DIGEST_FORMATS: &[&str] = &["json", "human"];
    if !VALID_DIGEST_FORMATS.contains(&d.format.as_str()) {
        eprintln!(
            "al-sem digest: invalid --format '{}'. Expected: json | human",
            d.format
        );
        return ExitCode::from(1);
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
        eprintln!(
            "digest: at least one of --changed-files, --changed-routines, --diff, or --changed is required"
        );
        return ExitCode::from(1);
    }

    // Read diff input
    let diff_text: Option<String> = match diff_arg.as_deref() {
        None => None,
        Some("-") => {
            // stdin: buffer all of it
            use std::io::Read;
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("al-sem: digest: failed to read diff from stdin");
                return ExitCode::from(exit::CONFIG_ERROR);
            }
            Some(buf)
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("al-sem: digest: could not read diff file '{path}': {e}");
                return ExitCode::from(exit::CONFIG_ERROR);
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

    let workspace = std::path::Path::new(&d.workspace);
    let version = driver_version();

    match run_digest_pipeline(
        workspace,
        changed_files,
        changed_routines,
        diff_text,
        &version,
        d.deterministic,
        d.max_paths,
    ) {
        Err(msg) => {
            // A DigestPipelineError (input/analyze phase) → write the message verbatim
            // and exit 1, mirroring cli/digest.ts's catch. The message already starts
            // with "digest:" so no prefix is added (#3).
            eprintln!("{msg}");
            ExitCode::from(1)
        }
        Ok(result) => {
            let output = if d.format == "human" {
                result.human_text
            } else {
                result.json_text
            };

            // Write output
            let write_result = if let Some(ref out_path) = d.out {
                std::fs::write(out_path, &output).map_err(|e| format!("{e}"))
            } else {
                use std::io::Write;
                std::io::stdout()
                    .write_all(output.as_bytes())
                    .map_err(|e| format!("{e}"))
            };

            if let Err(e) = write_result {
                eprintln!("al-sem: digest: write error: {e}");
                return ExitCode::from(1);
            }

            ExitCode::from(result.exit_code)
        }
    }
}

// ── prove command ───────────────────────────────────────────────────────────

fn run_prove_cmd(p: ProveCli) -> ExitCode {
    // Validate --format
    const VALID_PROVE_FORMATS: &[&str] = &["json", "human"];
    if !VALID_PROVE_FORMATS.contains(&p.format.as_str()) {
        eprintln!(
            "al-sem prove: invalid --format '{}'. Expected: json | human",
            p.format
        );
        return ExitCode::from(1);
    }

    // Validate question early (mirrors prove.ts exit 1 for unknown question)
    if parse_question(&p.question).is_none() {
        let valid: Vec<&str> = question_ids().to_vec();
        eprintln!(
            "prove: unknown question '{}'\nValid questions:\n{}",
            p.question,
            valid
                .iter()
                .map(|q| format!("  {q}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        return ExitCode::from(1);
    }

    let workspace = std::path::Path::new(&p.workspace);
    let version = driver_version();

    match run_prove_pipeline(
        workspace,
        &p.routine,
        &p.question,
        &version,
        p.deterministic,
    ) {
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(1)
        }
        Ok(result) => {
            let output = if p.format == "human" {
                result.human_text
            } else {
                result.json_text
            };

            let write_result = if let Some(ref out_path) = p.out {
                std::fs::write(out_path, &output).map_err(|e| format!("{e}"))
            } else {
                use std::io::Write;
                std::io::stdout()
                    .write_all(output.as_bytes())
                    .map_err(|e| format!("{e}"))
            };

            if let Err(e) = write_result {
                eprintln!("al-sem: prove: write error: {e}");
                return ExitCode::from(1);
            }

            ExitCode::from(result.exit_code)
        }
    }
}

// ── fingerprint command ─────────────────────────────────────────────────────

fn run_fingerprint_cmd(f: FingerprintCli) -> ExitCode {
    // Parse --shard mode (primary-only | all). Invalid value → exit 1.
    let shard_mode: Option<ShardMode> = match f.shard.as_deref() {
        None => None,
        Some("primary-only") => Some(ShardMode::PrimaryOnly),
        Some("all") => Some(ShardMode::All),
        Some(other) => {
            eprintln!("unknown --shard mode '{other}'; valid: primary-only, all");
            return ExitCode::from(1);
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
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
    };
    if let Err(msg) = reject_illegal_combos(specified, &format, shard_mode.is_some()) {
        eprintln!("{msg}");
        return ExitCode::from(1);
    }

    // Validate --verbosity (compact | full).
    if !matches!(f.verbosity.as_str(), "compact" | "full") {
        eprintln!(
            "unknown --verbosity '{}'; valid: compact, full",
            f.verbosity
        );
        return ExitCode::from(1);
    }

    // normalizeWitness (fingerprint.ts:82): None→3, false/all, 0..256, >256 → exit 1.
    let witness_limit = match normalize_witness(f.witness.as_deref()) {
        Ok(wl) => Some(wl),
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
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
                Err(msg) => {
                    eprintln!("{msg}");
                    return ExitCode::from(1);
                }
            }
        }
    };

    let is_query_requested = specified.is_query_requested();
    let workspace = std::path::Path::new(&f.workspace);
    let version = driver_version();

    let opts = FingerprintOptions {
        workspace,
        driver_version: &version,
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
    };

    match run_fingerprint_pipeline(&opts) {
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::from(1)
        }
        Ok(result) => {
            // A stderr-only error (selector error, --shard-no-out, strict gate):
            // print message, no stdout, exit with the result's code.
            if let Some(ref err_msg) = result.selector_error_message {
                eprintln!("{err_msg}");
                return ExitCode::from(result.exit_code);
            }
            // strict gate emits the diagnostics block + exit 1, no output.
            if result.exit_code == 1 && !result.stderr_diagnostics.is_empty() {
                for line in &result.stderr_diagnostics {
                    eprintln!("{line}");
                }
                return ExitCode::from(1);
            }

            // Write output (skip empty text).
            let should_write = match &result.output {
                FingerprintOutput::Text(t) => !t.is_empty(),
                _ => true,
            };
            if should_write
                && let Err(e) = write_fingerprint_output(&result.output, f.out.as_deref())
            {
                eprintln!("{e}");
                return ExitCode::from(1);
            }

            // Per-mode stderr diagnostics AFTER writing output (human/base-json/cbor).
            for line in &result.stderr_diagnostics {
                eprintln!("{line}");
            }

            ExitCode::from(result.exit_code)
        }
    }
}

// ── events fanout command ───────────────────────────────────────────────────

fn run_events_fanout_cmd(f: EventsFanoutCli) -> ExitCode {
    const VALID_FORMATS: &[&str] = &["human", "json"];
    if !VALID_FORMATS.contains(&f.format.as_str()) {
        eprintln!("al-sem events fanout: invalid --format '{}'", f.format);
        return ExitCode::from(1);
    }

    const VALID_COVERAGE: &[&str] = &["warn", "strict", "ignore"];
    if !VALID_COVERAGE.contains(&f.coverage_policy.as_str()) {
        eprintln!(
            "al-sem events fanout: invalid --coverage-policy '{}'",
            f.coverage_policy
        );
        return ExitCode::from(1);
    }

    let scope = match f.scope.as_str() {
        "primary" => EventScope::Primary,
        "all" => EventScope::All,
        other => {
            eprintln!("al-sem events fanout: invalid --scope '{other}'. Expected: primary | all");
            return ExitCode::from(1);
        }
    };

    let workspace = std::path::Path::new(&f.workspace);
    let version = driver_version();
    let opts = EventsFanoutOptions {
        workspace,
        format: &f.format,
        scope,
        coverage_policy: &f.coverage_policy,
        driver_version: &version,
        deterministic: f.deterministic,
        strict: f.strict,
    };

    let result = run_events_fanout(&opts);

    if !result.text.is_empty() {
        let write_result = if let Some(ref out_path) = f.out {
            std::fs::write(out_path, &result.text).map_err(|e| format!("{e}"))
        } else {
            use std::io::Write;
            std::io::stdout()
                .write_all(result.text.as_bytes())
                .map_err(|e| format!("{e}"))
        };
        if let Err(e) = write_result {
            eprintln!("failed to write: {e}");
            return ExitCode::from(1);
        }
    }

    for line in &result.stderr_lines {
        eprintln!("{line}");
    }

    ExitCode::from(result.exit_code)
}

// ── events chains command ───────────────────────────────────────────────────

fn run_events_chains_cmd(c: EventsChainsCli) -> ExitCode {
    const VALID_FORMATS: &[&str] = &["human", "json"];
    if !VALID_FORMATS.contains(&c.format.as_str()) {
        eprintln!("al-sem events chains: invalid --format '{}'", c.format);
        return ExitCode::from(1);
    }

    const VALID_COVERAGE: &[&str] = &["warn", "strict", "ignore"];
    if !VALID_COVERAGE.contains(&c.coverage_policy.as_str()) {
        eprintln!(
            "al-sem events chains: invalid --coverage-policy '{}'",
            c.coverage_policy
        );
        return ExitCode::from(1);
    }

    let scope = match c.scope.as_str() {
        "primary" => EventScope::Primary,
        "all" => EventScope::All,
        other => {
            eprintln!("al-sem events chains: invalid --scope '{other}'. Expected: primary | all");
            return ExitCode::from(1);
        }
    };

    if let Some(md) = c.max_depth
        && md > 256
    {
        eprintln!("al-sem events chains: --max-depth must be in 0..256");
        return ExitCode::from(1);
    }

    let workspace = std::path::Path::new(&c.workspace);
    let version = driver_version();
    let opts = EventsChainsOptions {
        workspace,
        format: &c.format,
        scope,
        coverage_policy: &c.coverage_policy,
        max_depth: c.max_depth,
        max_nodes: c.max_nodes,
        driver_version: &version,
        deterministic: c.deterministic,
        strict: c.strict,
    };

    let result = run_events_chains(&opts);

    if !result.text.is_empty() {
        let write_result = if let Some(ref out_path) = c.out {
            std::fs::write(out_path, &result.text).map_err(|e| format!("{e}"))
        } else {
            use std::io::Write;
            std::io::stdout()
                .write_all(result.text.as_bytes())
                .map_err(|e| format!("{e}"))
        };
        if let Err(e) = write_result {
            eprintln!("failed to write: {e}");
            return ExitCode::from(1);
        }
    }

    for line in &result.stderr_lines {
        eprintln!("{line}");
    }

    ExitCode::from(result.exit_code)
}

// ── cache prune command ─────────────────────────────────────────────────────

fn run_cache_prune_cmd(c: CachePruneCli) -> ExitCode {
    let result = prune_cache(c.dep_cache_dir.as_deref(), c.dry_run);
    let report = format_prune_report(&result, c.dry_run);
    use std::io::Write;
    if std::io::stdout().write_all(report.as_bytes()).is_err() {
        eprintln!("al-sem: cache prune: write error");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
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

fn run_analyze_cmd(a: AnalyzeCli) -> ExitCode {
    // --- enum-flag validation (mirrors the al-sem CLI's CONFIG_ERROR exits) ---
    if let Some(sev) = &a.min_severity
        && !SEVERITY_VALUES.contains(&sev.as_str())
    {
        eprintln!(
            "al-sem: invalid --min-severity '{sev}'. Expected one of: {}",
            SEVERITY_VALUES.join(", ")
        );
        return ExitCode::from(3);
    }

    let scope = match a.scope.as_str() {
        "primary" => Scope::Primary,
        "all" => Scope::All,
        other => {
            eprintln!("al-sem: invalid --scope '{other}'. Expected one of: primary, all");
            return ExitCode::from(3);
        }
    };

    // --- validate --format (accept auto + all concrete values) ---
    const VALID_FORMATS: &[&str] = &["auto", "sarif", "pr-summary", "terminal", "json", "html"];
    if !VALID_FORMATS.contains(&a.format.as_str()) {
        eprintln!(
            "al-sem: unsupported --format '{}'. Supported: {} (known presets: {})",
            a.format,
            VALID_FORMATS.join(", "),
            PRESET_NAMES_LIST.join(", ")
        );
        return ExitCode::from(3);
    }
    let format = resolve_auto_format(&a.format);

    // --- validate --group-by ---
    let group_by = if let Some(ref g) = a.group_by {
        if !GROUP_BY_VALUES.contains(&g.as_str()) {
            eprintln!(
                "al-sem: invalid --group-by '{g}'. Expected one of: {}",
                GROUP_BY_VALUES.join(", ")
            );
            return ExitCode::from(3);
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
                eprintln!("al-sem: {msg}");
                return ExitCode::from(3);
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
    match run_analyze_with_exit(&args, &driver_version()) {
        Ok((out, exit_code, stderr_warning)) => {
            // F2: emit the preflight degraded warning to stderr (the "no silent clean"
            // contract). Matches al-sem index.ts:263-264:
            //   `if (pf.degraded) process.stderr.write(`al-sem: warning: ${pf.message}\n`)`.
            if let Some(msg) = stderr_warning {
                eprintln!("al-sem: warning: {msg}");
            }
            // al-sem appends a trailing newline to the formatted output.
            println!("{out}");
            ExitCode::from(exit_code)
        }
        Err(msg) => {
            // "analysis failure — …" is tagged by run.rs for errors that al-sem maps to
            // EXIT.ANALYSIS_FAILURE (2) — e.g. a malformed baseline file that would throw
            // inside al-sem's loadBaseline and be caught by the analyze-action catch block.
            // All other Err strings are config/usage errors → EXIT.CONFIG_ERROR (3).
            if msg.starts_with("analysis failure — ") {
                eprintln!("al-sem: {msg}");
                ExitCode::from(2)
            } else {
                eprintln!("al-sem: {msg}");
                ExitCode::from(3)
            }
        }
    }
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
            let result = run_analyze_with_exit(&args, "test");
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
            let result = run_analyze_with_exit(&args, "test");
            assert!(
                result.is_ok(),
                "Html format must succeed (Stage A4 implemented); got: {result:?}"
            );
        }
    }
}
