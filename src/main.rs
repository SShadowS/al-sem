use anyhow::Result;
use clap::{Parser, ValueEnum};
use log::info;
use std::path::{Path, PathBuf};

mod server;
mod watcher;

// `config`, `telemetry`, `app_package`, `dependencies`, `protocol` live in
// `lib.rs` so library consumers (benches, tests) can use them. `analysis`
// joined them at T3 Task 12's fix-wave (see `lib.rs`'s doc on that module) —
// no more binary-only `mod analysis;` here. The legacy `graph`/`handlers`/
// `indexer`/`parser` modules that used to live here too were deleted at T3
// Task 17 (the LSP surface now runs entirely on `lsp::*`, see `lib.rs`'s doc
// on that module). Re-export here so binary modules (server, watcher, etc.)
// can keep referring to `crate::lsp::*` / ... without churn.
pub use al_sem::{
    analysis, app_package, big_stack, config, dependencies, lsp, protocol, snapshot, source_text,
    telemetry,
};

use lsp::snapshot::LspSnapshot;
use server::run_server;
use snapshot::DependencySource;

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
    Csv,
}

#[derive(Parser, Debug)]
#[command(name = "al-call-hierarchy")]
#[command(about = "Blazing-fast call hierarchy server for AL (Business Central)")]
struct Args {
    /// Path to the AL project root (CLI mode - index and report stats)
    #[arg(short, long)]
    project: Option<PathBuf>,

    /// Run in LSP server mode (stdio). This is the default if --project is not specified.
    #[arg(long)]
    lsp: bool,

    /// Run code quality analysis (requires --project)
    #[arg(short, long)]
    analyze: bool,

    /// Output format for analysis results
    #[arg(short, long, value_enum, default_value = "text")]
    format: OutputFormat,

    /// Disable the file system watcher (use LSP notifications for changes instead)
    #[arg(long)]
    no_watcher: bool,

    /// Disable anonymous failure-diagnostics telemetry for this run.
    /// (Telemetry is also off by default in dev/CI builds.)
    #[arg(long)]
    no_telemetry: bool,

    /// Skip code-quality diagnostics entirely. For clients that discard
    /// them anyway — computing them means a full analysis of every root
    /// that is opened, so a client which filters them client-side pays a
    /// large cost for output it throws away.
    #[arg(long)]
    no_diagnostics: bool,

    /// Where dependency apps' code comes from. `embedded` indexes the source
    /// a dependency ships, so calls can be followed into it. `symbols` reads
    /// only its symbols: calls into it still resolve, nothing inside it is
    /// indexed, and memory drops sharply (Base Application's source alone
    /// costs ~1.4 GB). Also read from AL_SEM_DEPENDENCY_SOURCE. With
    /// --analyze, used only by --format json (the call-graph fields).
    #[arg(
        long,
        value_enum,
        env = "AL_SEM_DEPENDENCY_SOURCE",
        default_value = "embedded"
    )]
    dependency_source: DependencySource,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging - suppress for JSON output
    let log_level = if matches!(args.format, OutputFormat::Json) && args.analyze {
        log::LevelFilter::Off
    } else if args.verbose {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    env_logger::Builder::new().filter_level(log_level).init();

    // `--analyze` (documented "requires --project") silently fell through to the
    // default LSP-server branch when `--project` was omitted, blocking forever on
    // stdin with no explanation. Hard-error up front instead, before any mode
    // dispatch — this must fire even when `--lsp` is also (contradictorily) set.
    if args.analyze && args.project.is_none() {
        anyhow::bail!("--analyze requires --project <path>");
    }

    if args.lsp {
        // `--lsp` was parsed but never consulted below — passing it alongside
        // `--project` silently ran CLI/analyze mode instead of the LSP server it
        // asked for. Give it real, unconditional effect (highest precedence): it
        // always starts the LSP server, regardless of --project/--analyze.
        info!("Starting AL Call Hierarchy LSP server (--lsp)");
        run_server(
            args.no_watcher,
            args.no_telemetry,
            args.no_diagnostics,
            args.dependency_source,
        )?;
    } else if let Some(project) = args.project {
        if args.analyze {
            // Analysis mode
            run_analysis(&project, &args.format, args.dependency_source)?;
        } else {
            // CLI mode for testing/indexing (T3 Task 15: re-pointed at the
            // program-engine snapshot — see this block's own doc below).
            info!("Indexing project: {}", project.display());
            report_index_stats(&project, args.dependency_source)?;
        }
    } else {
        // LSP server mode (default)
        info!("Starting AL Call Hierarchy LSP server");
        run_server(
            args.no_watcher,
            args.no_telemetry,
            args.no_diagnostics,
            args.dependency_source,
        )?;
    }

    Ok(())
}

/// CLI index-and-report mode: build the program-engine snapshot for
/// `project` and log summary counts (T3 Task 15 cutover — replaces the
/// legacy `Indexer::index_directory`/`into_graph` path).
///
/// **Count-definition change (CHANGELOG-documented):** `definitions` is now
/// the workspace [`LspSnapshot::decls_by_file`] entry count (every routine
/// declaration the snapshot indexed) and `call sites` is the sum of every
/// [`LspSnapshot::edges_by_file`] bucket's length (workspace `Call`/`Run`/
/// `ImplicitTrigger` edges — NOT including `event_edges`, mirroring the
/// brief's literal "Σ bucket lens"). Neither number is directly comparable to
/// the legacy `CallGraph::definition_count`/`call_site_count` this replaces:
/// the program engine's identity/dedup rules differ from the legacy
/// `QualifiedName`-keyed graph. The legacy "external definitions" line is
/// replaced by a count of dependency routines with EMBEDDED source
/// (`dep_meta` — real per-routine identities, unlike a `.app`'s
/// symbol-only ABI catalog, which has no equivalent "definition" to count).
fn report_index_stats(project: &Path, dependency_source: DependencySource) -> Result<()> {
    let Some(snap) = LspSnapshot::build_full_with(project, dependency_source) else {
        anyhow::bail!(
            "Failed to build the program snapshot for {} — is this a valid AL app \
             workspace (a readable app.json at its root)?",
            project.display()
        );
    };

    let definitions: usize = snap.decls_by_file.values().map(|v| v.len()).sum();
    let call_sites: usize = snap.edges_by_file.values().map(|v| v.len()).sum();
    let dep_definitions = snap.dep_meta.len();

    info!("Indexed {} definitions", definitions);
    info!(
        "Indexed {} dependency definitions (embedded source)",
        dep_definitions
    );
    info!("Found {} call sites", call_sites);
    Ok(())
}

/// Run code quality analysis on a project
fn run_analysis(
    project: &Path,
    format: &OutputFormat,
    dependency_source: DependencySource,
) -> Result<()> {
    use std::time::Instant;

    let start = Instant::now();
    info!("Analyzing project: {}", project.display());

    // Config from the project root. Only JSON prints the call-graph fields,
    // so only JSON pays for building the graph.
    let config = config::DiagnosticConfig::load(project);
    let graph = matches!(format, OutputFormat::Json).then_some(dependency_source);
    let result = analysis::analyze_project(project, graph, &config)?;

    info!(
        "Analyzed {} procedures in {:.1}ms",
        result.summary.total_procedures,
        start.elapsed().as_secs_f64() * 1000.0
    );

    // Output results
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        OutputFormat::Csv => {
            print_csv(&result);
        }
        OutputFormat::Text => {
            print_text(&result, project, &config);
        }
    }

    Ok(())
}

/// Print results in CSV format
fn print_csv(result: &analysis::AnalysisResult) {
    println!(
        "object_type,object_name,procedure_name,file,line,complexity,line_count,parameter_count,quality_score"
    );
    for m in &result.metrics {
        println!(
            "{},{},{},{},{},{},{},{},{:.1}",
            m.object_type,
            m.object_name,
            m.procedure_name,
            m.file,
            m.line,
            m.complexity,
            m.line_count,
            m.parameter_count,
            m.quality_score
        );
    }
}

/// Print results in human-readable text format
fn print_text(
    result: &analysis::AnalysisResult,
    project: &std::path::Path,
    config: &config::DiagnosticConfig,
) {
    println!("\nCode Quality Analysis: {}\n", project.display());
    println!("═══════════════════════════════════════════════════════════════════════════════\n");

    // Sort by complexity (descending)
    let mut sorted_metrics = result.metrics.clone();
    sorted_metrics.sort_by_key(|m| std::cmp::Reverse(m.complexity));

    println!("PROCEDURES (sorted by complexity):\n");
    println!(
        "{:<40} {:>4} {:>6} {:>6} {:>8}",
        "Procedure", "CC", "Lines", "Params", "Score"
    );
    println!("{}", "-".repeat(70));

    for m in sorted_metrics.iter().take(20) {
        let name = format!("{}.{}", m.object_name, m.procedure_name);
        let name_truncated = if name.len() > 38 {
            format!("{}...", &name[..35])
        } else {
            name
        };

        let severity = if m.complexity >= config.complexity_critical {
            " [CRITICAL]"
        } else if m.complexity >= config.complexity_warning {
            " [WARNING]"
        } else {
            ""
        };

        println!(
            "{:<40} {:>4} {:>6} {:>6} {:>7.1}{}",
            name_truncated,
            m.complexity,
            m.line_count,
            m.parameter_count,
            m.quality_score,
            severity
        );
    }

    if sorted_metrics.len() > 20 {
        println!("  ... and {} more procedures", sorted_metrics.len() - 20);
    }

    // Findings. Text output is unchanged by #159: `run_analysis` builds no
    // call graph for text, so the JSON-only `unused_procedure` never exists here.
    debug_assert!(
        result
            .findings
            .iter()
            .all(|f| f.category != "unused_procedure"),
        "text output must not get unused_procedure findings"
    );
    if !result.findings.is_empty() {
        println!("\nFINDINGS:\n");
        for f in &result.findings {
            let severity_str = match f.severity.as_str() {
                "critical" => "[CRITICAL]",
                "warning" => "[WARNING]",
                _ => "[INFO]",
            };
            println!("  {} {} - {}", severity_str, f.location, f.description);
        }
    }

    // Summary
    println!("\nSUMMARY:\n");
    println!(
        "  Total procedures:     {}",
        result.summary.total_procedures
    );
    println!(
        "  Average complexity:   {:.1}",
        result.summary.avg_complexity
    );
    println!(
        "  Average quality score: {:.1}",
        result.summary.avg_quality_score
    );
    println!(
        "  Critical findings:    {}",
        result.summary.critical_findings
    );
    println!(
        "  Warning findings:     {}",
        result.summary.warning_findings
    );
    println!();
}
