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
    analysis, app_package, big_stack, config, dependencies, lsp, protocol, telemetry,
};

use lsp::snapshot::LspSnapshot;
use server::run_server;

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
        run_server(args.no_watcher, args.no_telemetry, args.no_diagnostics)?;
    } else if let Some(project) = args.project {
        if args.analyze {
            // Analysis mode
            run_analysis(&project, &args.format)?;
        } else {
            // CLI mode for testing/indexing (T3 Task 15: re-pointed at the
            // program-engine snapshot — see this block's own doc below).
            info!("Indexing project: {}", project.display());
            report_index_stats(&project)?;
        }
    } else {
        // LSP server mode (default)
        info!("Starting AL Call Hierarchy LSP server");
        run_server(args.no_watcher, args.no_telemetry, args.no_diagnostics)?;
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
fn report_index_stats(project: &Path) -> Result<()> {
    let Some(snap) = LspSnapshot::build_full(project) else {
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
fn run_analysis(project: &PathBuf, format: &OutputFormat) -> Result<()> {
    use analysis::{AnalysisResult, ProcedureMetrics, build_summary, generate_findings};
    use rayon::prelude::*;
    use std::fs;
    use std::time::Instant;
    use walkdir::WalkDir;

    let start = Instant::now();
    info!("Analyzing project: {}", project.display());

    // Collect all .al files
    let al_files: Vec<PathBuf> = WalkDir::new(project)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("al"))
                .unwrap_or(false)
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    info!("Found {} AL files", al_files.len());

    // Parse + collect per-procedure metrics in parallel, from the owned IR, on
    // a big-stack pool (T2.1: the CLI main thread's default pool has no
    // guaranteed-generous stack; see `big_stack`'s doc).
    let pool = big_stack::big_stack_pool();
    let all_metrics: Vec<ProcedureMetrics> = pool.install(|| {
        al_files
            .par_iter()
            .flat_map(|path| match fs::read_to_string(path) {
                Ok(source) => extract_metrics_ir(&source, path),
                Err(_) => vec![],
            })
            .collect()
    });

    // Generate findings using config from project root
    let config = config::DiagnosticConfig::load(project);
    let mut all_findings = Vec::new();
    for metrics in &all_metrics {
        all_findings.extend(generate_findings(metrics, &config));
    }

    // Build summary
    let summary = build_summary(&all_metrics, &all_findings);

    let result = AnalysisResult {
        metrics: all_metrics,
        findings: all_findings,
        summary,
    };

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

/// Extract per-procedure quality metrics for one file from the owned IR. Each
/// routine is attributed to its enclosing object (object type/name). Replaces the
/// former tree-sitter walk; complexity comes from the canonical IR walker.
fn extract_metrics_ir(source: &str, path: &Path) -> Vec<analysis::ProcedureMetrics> {
    use al_syntax::ir::RoutineKind;
    use analysis::{calculate_quality_score, routine_complexity_ir};

    let f = al_syntax::parse(source);
    let file_str = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());

    let mut metrics = Vec::new();
    for obj in &f.objects {
        let object_type = object_kind_label(obj.kind);
        let object_name = obj.name.trim_matches('"').to_string();
        for r in &obj.routines {
            let procedure_name = if r.name.is_empty() {
                match r.kind {
                    RoutineKind::Procedure => "procedure",
                    RoutineKind::Trigger => "trigger_declaration",
                }
                .to_string()
            } else {
                r.name.trim_matches('"').to_string()
            };
            let complexity = routine_complexity_ir(&f.ir, r);
            let line_count = r.origin.end.row.saturating_sub(r.origin.start.row) + 1;
            let parameter_count = r.params.len() as u32;
            let quality_score = calculate_quality_score(complexity, line_count, parameter_count);

            metrics.push(analysis::ProcedureMetrics {
                object_type: object_type.clone(),
                object_name: object_name.clone(),
                procedure_name,
                file: file_str.clone(),
                line: r.origin.start.row + 1,
                complexity,
                line_count,
                parameter_count,
                quality_score,
            });
        }
    }
    metrics
}

/// Human-readable object-type label (e.g. `Codeunit`, `Pageextension`), matching
/// the former kind-string capitalization used in the CLI metrics output.
fn object_kind_label(k: al_syntax::ir::ObjectKind) -> String {
    use al_syntax::ir::ObjectKind as K;
    match k {
        K::Codeunit => "Codeunit",
        K::Table => "Table",
        K::TableExtension => "Tableextension",
        K::Page => "Page",
        K::PageExtension => "Pageextension",
        K::Report => "Report",
        K::ReportExtension => "Reportextension",
        K::Query => "Query",
        K::XmlPort => "Xmlport",
        K::Enum => "Enum",
        K::EnumExtension => "Enumextension",
        K::Interface => "Interface",
        K::ControlAddIn => "Controladdin",
        K::Entitlement => "Entitlement",
        K::PermissionSet => "Permissionset",
        K::PermissionSetExtension => "Permissionsetextension",
        K::Profile => "Profile",
        K::Other => "",
    }
    .to_string()
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

    // Findings
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
