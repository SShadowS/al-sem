//! Code quality analysis - cyclomatic complexity and metrics

use serde::Serialize;

/// Metrics for a single procedure/trigger. The fields after `quality_score`
/// were added by #159 (appended, so existing keys keep their order).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProcedureMetrics {
    pub object_type: String,
    pub object_name: String,
    pub procedure_name: String,
    /// The file's base name (kept for existing consumers; see `path`).
    pub file: String,
    pub line: u32,
    pub complexity: u32,
    pub line_count: u32,
    pub parameter_count: u32,
    pub quality_score: f32,
    /// Workspace-relative POSIX path of the file.
    pub path: String,
    /// The object number; `None` when the object declares none.
    pub object_id: Option<i64>,
    /// `local` | `internal` | `protected` as declared, else `public`.
    pub access: String,
    pub is_event_subscriber: bool,
    pub is_trigger: bool,
    /// alsem's `stableRoutineId`; `None` without an app id, for a file in a
    /// nested app, for Interface / ControlAddIn members, object kinds alsem
    /// gives no id, and nameless routines. Independent of the call graph.
    pub routine_id: Option<String>,
    /// Distinct callers in the resolved call graph; `None` = no graph for
    /// this row (no snapshot, or no matching declaration), never `0`, and
    /// for Interface / ControlAddIn members (never a call target).
    pub fan_in: Option<usize>,
    /// Distinct callees (routine and dependency-symbol targets); `None` as
    /// for `fan_in`.
    pub fan_out: Option<usize>,
}

/// A finding/issue detected during analysis
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub category: String,
    pub severity: String,
    pub location: String,
    pub procedure: String,
    pub description: String,
    /// Workspace-relative POSIX path (#159). Set on `unused_procedure`
    /// findings only and omitted otherwise, so existing findings keep their
    /// exact shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Summary statistics for the analysis
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisSummary {
    pub total_procedures: usize,
    pub avg_complexity: f32,
    pub avg_quality_score: f32,
    pub critical_findings: usize,
    pub warning_findings: usize,
}

/// Complete analysis result
#[derive(Debug, Clone, Serialize)]
pub struct AnalysisResult {
    pub metrics: Vec<ProcedureMetrics>,
    pub findings: Vec<Finding>,
    pub summary: AnalysisSummary,
    /// #253: per-`[Test]` assert counts. Last, so existing keys keep their order.
    pub test_quality: TestQuality,
}

use crate::config::DiagnosticConfig;
use crate::test_quality::{TestProcedure, TestQuality};

/// Calculate quality score on a 0-10 scale
/// Based on tree-sitter-mcp's quality score formula
pub fn calculate_quality_score(complexity: u32, line_count: u32, params: u32) -> f32 {
    let mut score = 10.0;

    // Complexity penalties
    if complexity > 4 {
        score -= 1.6 + (complexity - 4) as f32 * 1.2;
    } else if complexity > 2 {
        score -= (complexity - 2) as f32 * 0.8;
    }

    // Method length penalties
    if line_count > 15 {
        score -= 1.5 + (line_count - 15) as f32 * 0.15;
    } else if line_count > 10 {
        score -= (line_count - 10) as f32 * 0.3;
    }

    // Parameter penalties
    if params > 4 {
        score -= 1.0 + (params - 4) as f32 * 0.8;
    } else if params > 2 {
        score -= (params - 2) as f32 * 0.5;
    }

    score.clamp(0.0, 10.0)
}

/// Generate findings based on metrics
pub fn generate_findings(metrics: &ProcedureMetrics, config: &DiagnosticConfig) -> Vec<Finding> {
    let mut findings = Vec::new();
    let location = format!("{}:{}", metrics.file, metrics.line);
    let procedure = format!("{}.{}", metrics.object_name, metrics.procedure_name);

    // Complexity findings
    if config.complexity_enabled && metrics.complexity >= config.complexity_critical {
        findings.push(Finding {
            path: None,
            category: "high_complexity".to_string(),
            severity: "critical".to_string(),
            location: location.clone(),
            procedure: procedure.clone(),
            description: format!(
                "Cyclomatic complexity {} exceeds critical threshold of {}",
                metrics.complexity, config.complexity_critical
            ),
        });
    } else if config.complexity_enabled && metrics.complexity >= config.complexity_warning {
        findings.push(Finding {
            path: None,
            category: "high_complexity".to_string(),
            severity: "warning".to_string(),
            location: location.clone(),
            procedure: procedure.clone(),
            description: format!(
                "Cyclomatic complexity {} exceeds warning threshold of {}",
                metrics.complexity, config.complexity_warning
            ),
        });
    }

    // Length findings
    if config.length_enabled && metrics.line_count >= config.length_critical {
        findings.push(Finding {
            path: None,
            category: "long_method".to_string(),
            severity: "critical".to_string(),
            location: location.clone(),
            procedure: procedure.clone(),
            description: format!(
                "Method length {} lines exceeds critical threshold of {}",
                metrics.line_count, config.length_critical
            ),
        });
    } else if config.length_enabled && metrics.line_count >= config.length_warning {
        findings.push(Finding {
            path: None,
            category: "long_method".to_string(),
            severity: "warning".to_string(),
            location: location.clone(),
            procedure: procedure.clone(),
            description: format!(
                "Method length {} lines exceeds warning threshold of {}",
                metrics.line_count, config.length_warning
            ),
        });
    }

    // Parameter findings
    if config.params_enabled && metrics.parameter_count >= config.params_critical {
        findings.push(Finding {
            path: None,
            category: "too_many_parameters".to_string(),
            severity: "critical".to_string(),
            location: location.clone(),
            procedure: procedure.clone(),
            description: format!(
                "Parameter count {} exceeds critical threshold of {}",
                metrics.parameter_count, config.params_critical
            ),
        });
    } else if config.params_enabled && metrics.parameter_count >= config.params_warning {
        findings.push(Finding {
            path: None,
            category: "too_many_parameters".to_string(),
            severity: "warning".to_string(),
            location,
            procedure,
            description: format!(
                "Parameter count {} exceeds warning threshold of {}",
                metrics.parameter_count, config.params_warning
            ),
        });
    }

    findings
}

/// Build analysis summary from metrics and findings
pub fn build_summary(metrics: &[ProcedureMetrics], findings: &[Finding]) -> AnalysisSummary {
    let total = metrics.len();
    let avg_complexity = if total > 0 {
        metrics.iter().map(|m| m.complexity as f32).sum::<f32>() / total as f32
    } else {
        0.0
    };
    let avg_quality = if total > 0 {
        metrics.iter().map(|m| m.quality_score).sum::<f32>() / total as f32
    } else {
        0.0
    };

    let critical = findings.iter().filter(|f| f.severity == "critical").count();
    let warnings = findings.iter().filter(|f| f.severity == "warning").count();

    AnalysisSummary {
        total_procedures: total,
        avg_complexity,
        avg_quality_score: avg_quality,
        critical_findings: critical,
        warning_findings: warnings,
    }
}

// ---------------------------------------------------------------------------
// `--analyze`: the whole analysis of one project (#159 moved it here from the
// binary so it can share the LSP's crate-private rules).
// ---------------------------------------------------------------------------

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::lsp::snapshot::{DeclEntry, LspSnapshot, LspTarget};
use crate::program::RoutineNodeId;
use crate::snapshot::DependencySource;

/// The call graph one analysis reads: the LSP snapshot plus every routine's
/// distinct outgoing targets (computed once, not per routine).
struct Graph<'s> {
    snap: &'s LspSnapshot,
    fan_out: HashMap<&'s RoutineNodeId, HashSet<&'s LspTarget>>,
}

impl<'s> Graph<'s> {
    fn new(snap: &'s LspSnapshot) -> Self {
        Graph {
            snap,
            fan_out: crate::lsp::handlers::outgoing_targets_by_caller(snap),
        }
    }
}

/// Analyze every `.al` file under `project`: per-routine metrics, then the
/// threshold findings, then (with a graph) the `unused_procedure` findings.
///
/// `graph: Some(source)` builds the LSP's call graph with that dependency
/// source and fills `fan_in`/`fan_out` and the unused findings; `None` skips
/// it (text/CSV print none of that). When the graph cannot be built, one
/// warning with the reason goes to stderr and those stay empty.
pub fn analyze_project(
    project: &Path,
    graph: Option<DependencySource>,
    config: &DiagnosticConfig,
) -> anyhow::Result<AnalysisResult> {
    use rayon::prelude::*;

    let files =
        crate::source_text::discover_al_files(project, crate::source_text::NestedApps::Walk)?;
    let snap = graph.and_then(|source| {
        let snap = LspSnapshot::build_full_with(project, source);
        if snap.is_none() {
            // The builder reports no reason; the one we can tell apart is a
            // missing root app.json.
            let reason = if project.join("app.json").is_file() {
                "the program snapshot could not be built"
            } else {
                "no app.json at the project root"
            };
            eprintln!(
                "warning: call graph unavailable for {} ({reason}): \
                 fan_in and fan_out are null and unused procedures are not reported",
                project.display()
            );
        }
        snap
    });
    let graph = snap.as_ref().map(Graph::new);
    let app_guid = crate::source_text::read_root_app_guid(project);
    let roots: HashMap<String, String> = files
        .iter()
        .map(|f| (f.rel_posix.clone(), app_root(project, &f.rel_posix)))
        .collect();

    // Big-stack pool: the complexity walk recurses (see `big_stack`'s doc).
    let per_file: Vec<FileRows> = crate::big_stack::big_stack_pool().install(|| {
        files
            .par_iter()
            .map(|f| {
                // A nested app's file belongs to another app: the root's
                // guid would mint a wrong routine_id, so it gets none.
                let guid = app_guid
                    .as_deref()
                    .filter(|_| roots[&f.rel_posix].is_empty());
                file_metrics(
                    &f.rel_posix,
                    &f.abs_path,
                    graph.as_ref(),
                    guid,
                    config.unused_procedures,
                )
            })
            .collect()
    });

    let mut metrics = Vec::new();
    let mut unused = Vec::new();
    let mut tests = Vec::new();
    let mut own_at: HashMap<&str, HashMap<usize, usize>> = HashMap::new();
    for (f, (m, u, t, own)) in files.iter().zip(per_file) {
        metrics.extend(m);
        unused.extend(u);
        tests.extend(t);
        own_at.insert(&f.rel_posix, own.into_iter().collect());
    }
    if let Some(g) = &graph {
        let calls = crate::test_quality::CallMap::new(g.snap, &roots, &own_at);
        for t in &mut tests {
            if let (Some(id), Some(root)) = (&t.node, roots.get(&t.path)) {
                let n = calls.reachable(root, id, t.assert_count);
                t.assert_count_reachable = Some(n);
                t.zero_assert = Some(n == 0);
            }
        }
    }
    let mut findings: Vec<Finding> = metrics
        .iter()
        .flat_map(|m| generate_findings(m, config))
        .collect();
    // After every existing finding, so those keep their positions.
    findings.extend(unused);
    let summary = build_summary(&metrics, &findings);
    Ok(AnalysisResult {
        metrics,
        findings,
        summary,
        test_quality: TestQuality::new(tests),
    })
}

/// The app root of the file at `rel_posix`: the deepest folder below `project`
/// that holds its own `app.json` (a nested app; `discover_al_files` walks into
/// those), as a `/`-separated relative path, else `""` (the project itself).
fn app_root(project: &Path, rel_posix: &str) -> String {
    let mut dir = project.to_path_buf();
    let mut folders: Vec<&str> = rel_posix.split('/').collect();
    folders.pop(); // the file name
    let mut root_len = 0;
    for (i, folder) in folders.iter().enumerate() {
        dir.push(folder);
        if dir.join("app.json").is_file() {
            root_len = i + 1;
        }
    }
    folders[..root_len].join("/")
}

/// One file's metrics rows, `unused_procedure` findings, `[Test]` rows, and
/// the own assert count of each routine with a body (by start byte).
type FileRows = (
    Vec<ProcedureMetrics>,
    Vec<Finding>,
    Vec<TestProcedure>,
    Vec<(usize, usize)>,
);

/// One file's rows and `unused_procedure` findings. With a graph that holds
/// the file, the rows come from the snapshot's OWN parse and join its
/// declarations on the routine's byte offset, so the two cannot describe
/// different versions of the file. Otherwise the file is read and parsed
/// here and the graph fields stay `None`.
fn file_metrics(
    rel_posix: &str,
    abs_path: &Path,
    graph: Option<&Graph<'_>>,
    app_guid: Option<&str>,
    report_unused: bool,
) -> FileRows {
    use crate::engine::ids::encode_object_id;
    use crate::program::body::ir_walk::{ir_object_key, ir_object_type, ir_routine_kind};
    use crate::program::body::l2_workspace::{stable_object_id_ir, stable_routine_id_ir};
    use al_syntax::ir::{ObjectKind, RoutineKind};

    // The file's declarations by start byte: one lookup per routine.
    let in_snapshot = graph.and_then(|g| {
        let entry = g.snap.parsed.get(rel_posix)?;
        let decl_at: HashMap<usize, &DeclEntry> = g
            .snap
            .decls_by_file
            .get(rel_posix)
            .map(|decls| decls.iter().map(|d| (d.origin.byte.start, d)).collect())
            .unwrap_or_default();
        Some((g, entry.file.clone(), decl_at))
    });
    let file = match &in_snapshot {
        Some((_, file, _)) => file.clone(),
        None => match crate::source_text::read_al_source(abs_path) {
            Ok(source) => std::sync::Arc::new(al_syntax::parse(&source)),
            Err(_) => return Default::default(),
        },
    };
    let file_name = abs_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| abs_path.display().to_string());

    let mut metrics = Vec::new();
    let mut unused = Vec::new();
    let mut tests = Vec::new();
    let mut own_at = Vec::new();
    for obj in &file.objects {
        let object_type = object_kind_label(obj.kind);
        let object_name = obj.name.trim_matches('"').to_string();
        // Interface and ControlAddIn members are signatures: never a call
        // target, so no routine_id and no fan_in/fan_out (null, not 0).
        let signature_only = matches!(obj.kind, ObjectKind::Interface | ObjectKind::ControlAddIn);
        let stable_object_id = app_guid
            .filter(|_| !signature_only)
            .and_then(|guid| stable_object_id_ir(guid, obj));
        // The internal form of the same id (`<guid>/<Type>/<key>`), same nulls.
        let internal_object_id = app_guid.filter(|_| !signature_only).and_then(|guid| {
            Some(encode_object_id(
                guid,
                ir_object_type(&obj.kind)?,
                &ir_object_key(obj),
            ))
        });
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
            let complexity = routine_complexity_ir(&file.ir, r);
            let line_count = r.origin.end.row.saturating_sub(r.origin.start.row) + 1;
            let parameter_count = r.params.len() as u32;
            let quality_score = calculate_quality_score(complexity, line_count, parameter_count);
            let line = r.origin.start.row + 1;

            let decl = in_snapshot
                .as_ref()
                .and_then(|(g, _, decl_at)| Some((*g, *decl_at.get(&r.origin.byte.start)?)));
            let counted = decl.filter(|_| !signature_only);
            if report_unused
                && let Some((g, d)) = decl
                && crate::lsp::diagnostics::is_unused_procedure(
                    d,
                    r,
                    crate::lsp::lens::effective_incoming_count(g.snap, &d.id),
                )
            {
                unused.push(Finding {
                    category: "unused_procedure".to_string(),
                    severity: "info".to_string(),
                    location: format!("{file_name}:{line}"),
                    procedure: format!("{object_name}.{procedure_name}"),
                    description: format!(
                        "Procedure '{object_name}.{procedure_name}' is never called"
                    ),
                    path: Some(rel_posix.to_string()),
                });
            }

            let routine_id = stable_object_id
                .as_deref()
                .filter(|_| !r.name.is_empty())
                .map(|o| stable_routine_id_ir(o, r));
            let body = crate::test_quality::body_counts(&file.ir, obj, r);
            if let Some(b) = &body {
                own_at.push((r.origin.byte.start, b.own()));
            }
            if crate::test_quality::is_test(r) {
                let b = body.unwrap_or_default();
                tests.push(TestProcedure {
                    path: rel_posix.to_string(),
                    line,
                    object_type: object_type.clone(),
                    object_name: object_name.clone(),
                    object_id: obj.id,
                    object_stable_id: internal_object_id.clone(),
                    procedure_name: procedure_name.clone(),
                    routine_id: routine_id.clone(),
                    assert_count: b.own(),
                    assert_count_reachable: None,
                    zero_assert: None,
                    has_asserterror: b.asserterrors > 0,
                    are_equal: b.are_equal,
                    object_line: obj.origin.start.row + 1,
                    node: decl.map(|(_, d)| d.id.clone()),
                });
            }

            metrics.push(ProcedureMetrics {
                object_type: object_type.clone(),
                object_name: object_name.clone(),
                procedure_name,
                file: file_name.clone(),
                line,
                complexity,
                line_count,
                parameter_count,
                quality_score,
                path: rel_posix.to_string(),
                object_id: obj.id,
                access: r
                    .access_modifier
                    .clone()
                    .unwrap_or_else(|| "public".to_string()),
                is_event_subscriber: ir_routine_kind(r) == "event-subscriber",
                is_trigger: r.kind == RoutineKind::Trigger,
                routine_id,
                fan_in: counted
                    .map(|(g, d)| crate::lsp::handlers::incoming_callers(g.snap, &d.id).len()),
                fan_out: counted.map(|(g, d)| g.fan_out.get(&d.id).map_or(0, HashSet::len)),
            });
        }
    }
    (metrics, unused, tests, own_at)
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

// ---------------------------------------------------------------------------
// IR-direct routine metrics (T3 Task 12 fix-wave: relocated from `parser.rs`,
// a Task-17 deletion target, so the permanent `src/lsp/lens.rs`/
// `diagnostics.rs` library modules can depend on them without depending on a
// module scheduled for deletion — `parser.rs` re-exports these two so its own
// existing call sites, and `main.rs`'s legacy `--analyze` path, keep working
// unchanged.
// ---------------------------------------------------------------------------

use al_syntax::ir::{self, BinaryOp, BlockId, BlockItem, ExprId, ExprKind, RoutineDecl, StmtKind};

/// Cyclomatic complexity over the IR body. Base 1; +1 per if (+1 more if it has an
/// else), +1 per loop, +1 per case branch, +1 per `and`/`or`. The canonical
/// complexity metric (the tree-sitter `analysis::calculate_complexity` is retired).
pub fn routine_complexity_ir(ir: &ir::Ir, r: &RoutineDecl) -> u32 {
    let mut c = 1u32;
    if let Some(body) = r.body {
        complexity_block(ir, body, &mut c);
    }
    c
}

fn complexity_block(ir: &ir::Ir, bid: BlockId, c: &mut u32) {
    for item in &ir.block(bid).items {
        match item {
            BlockItem::Stmt(sid) => complexity_stmt(ir, *sid, c),
            BlockItem::Preproc(g) => {
                for b in &g.branches {
                    complexity_block(ir, *b, c);
                }
            }
        }
    }
}

fn complexity_stmt(ir: &ir::Ir, sid: ir::StmtId, c: &mut u32) {
    match &ir.stmt(sid).kind {
        StmtKind::If {
            cond,
            then_block,
            else_block,
        } => {
            *c += 1;
            if else_block.is_some() {
                *c += 1;
            }
            complexity_expr(ir, *cond, c);
            complexity_block(ir, *then_block, c);
            if let Some(b) = else_block {
                complexity_block(ir, *b, c);
            }
        }
        StmtKind::While { cond, body } => {
            *c += 1;
            complexity_expr(ir, *cond, c);
            complexity_block(ir, *body, c);
        }
        StmtKind::Repeat { body, until } => {
            *c += 1;
            complexity_block(ir, *body, c);
            complexity_expr(ir, *until, c);
        }
        StmtKind::For {
            var,
            from,
            to,
            body,
            ..
        } => {
            *c += 1;
            complexity_expr(ir, *var, c);
            complexity_expr(ir, *from, c);
            complexity_expr(ir, *to, c);
            complexity_block(ir, *body, c);
        }
        StmtKind::Foreach {
            var,
            iterable,
            body,
        } => {
            *c += 1;
            complexity_expr(ir, *var, c);
            complexity_expr(ir, *iterable, c);
            complexity_block(ir, *body, c);
        }
        StmtKind::Case {
            scrutinee,
            branches,
            else_block,
        } => {
            complexity_expr(ir, *scrutinee, c);
            for br in branches {
                *c += 1;
                for p in &br.patterns {
                    complexity_expr(ir, *p, c);
                }
                complexity_block(ir, br.body, c);
            }
            if let Some(b) = else_block {
                complexity_block(ir, *b, c);
            }
        }
        StmtKind::Assignment { target, value, .. } => {
            complexity_expr(ir, *target, c);
            complexity_expr(ir, *value, c);
        }
        StmtKind::Call(e) => complexity_expr(ir, *e, c),
        StmtKind::With { receiver, body } => {
            complexity_expr(ir, *receiver, c);
            complexity_block(ir, *body, c);
        }
        StmtKind::Try { body, catch_block } => {
            complexity_block(ir, *body, c);
            if let Some(b) = catch_block {
                complexity_block(ir, *b, c);
            }
        }
        StmtKind::AssertError(b) => complexity_block(ir, *b, c),
        StmtKind::Exit(Some(e)) => complexity_expr(ir, *e, c),
        StmtKind::Block(b) => complexity_block(ir, *b, c),
        _ => {}
    }
}

fn complexity_expr(ir: &ir::Ir, eid: ExprId, c: &mut u32) {
    let e = ir.expr(eid);
    // `and`/`or` and the ternary each add a decision point.
    if matches!(
        &e.kind,
        ExprKind::Binary {
            op: BinaryOp::And | BinaryOp::Or,
            ..
        } | ExprKind::Ternary { .. }
    ) {
        *c += 1;
    }
    for_each_subexpr(ir, eid, &mut |sub| complexity_expr(ir, sub, c));
}

/// Visit the direct sub-expressions of an expression (one level). The caller
/// recurses; this just enumerates children so the two walkers (`parser.rs`'s
/// call-site walker, and this module's complexity walker) share one
/// definition of the expression shape. `pub(crate)` — `parser.rs`'s
/// `calls_in_expr` imports this directly (dying module depends on the
/// surviving one, never the reverse).
pub(crate) fn for_each_subexpr(ir: &ir::Ir, eid: ExprId, f: &mut dyn FnMut(ExprId)) {
    for c in ir.expr(eid).kind.children() {
        f(c);
    }
}

/// True for AL attributes whose procedure is invoked by a framework (the test
/// runner or test framework) rather than by an explicit call, so the procedure
/// must not be reported as unused. AL attribute names are case-insensitive.
/// Event publishers/subscribers are handled separately and are not listed here.
pub(crate) fn is_framework_invocation_attribute(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "test"
            | "confirmhandler"
            | "messagehandler"
            | "pagehandler"
            | "modalpagehandler"
            | "reporthandler"
            | "requestpagehandler"
            | "sendnotificationhandler"
            | "recallnotificationhandler"
            | "sessionsettingshandler"
            | "strmenuhandler"
            | "filterpagehandler"
            | "hyperlinkhandler"
    )
}

/// Render a procedure/trigger header as raw source text: everything from
/// `r.origin`'s start up to (but not including) the body's `var` section or
/// `begin` keyword, whitespace-collapsed to single spaces. Relocated here
/// from `parser.rs` (T3 Task 13 review fix-wave — mirrors this module's own
/// earlier relocation of [`is_framework_invocation_attribute`]/
/// [`routine_complexity_ir`] above): `src/lsp/custom.rs`'s
/// `event_publishers_in_file` needs this SAME signature-rendering logic (to
/// stay byte-identical to what `parser.rs`'s own
/// `ParsedEventPublisher::signature` produces for the same routine), but
/// `parser.rs` is a documented Task-17 deletion target — sharing one
/// definition here means neither module drifts, and Task 17 can delete
/// `parser.rs` without orphaning anything `custom.rs` depends on.
pub(crate) fn signature_ir(source: &str, r: &RoutineDecl) -> String {
    let raw = &source[r.origin.byte.clone()];
    let end = find_body_start(raw).unwrap_or(raw.len());
    normalize_signature_ws(&raw[..end])
}

/// Find the byte offset (relative to the start of `text`) where a procedure
/// body begins (the `begin` keyword or `var` section). Returns `None` when no
/// body marker is present in this slice.
///
/// Requires the keyword to be alone at the start of a line (preceded only by
/// whitespace) so a `var` parameter modifier is never mistaken for the `var`
/// section, and skips scanning inside string literals so a quoted
/// identifier/comment containing "begin"/"var" text can't false-positive.
fn find_body_start(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut in_string = false;
    let mut string_quote = 0u8;
    while i < len {
        let b = bytes[i];
        if in_string {
            if b == string_quote {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if b == b'\'' || b == b'"' {
            in_string = true;
            string_quote = b;
            i += 1;
            continue;
        }
        // Look at line starts only (`\n` followed by optional whitespace).
        if b == b'\n' {
            let mut j = i + 1;
            while j < len && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if matches_keyword(bytes, j, b"begin") || matches_keyword(bytes, j, b"var") {
                return Some(j);
            }
        }
        i += 1;
    }
    None
}

fn matches_keyword(bytes: &[u8], at: usize, kw: &[u8]) -> bool {
    if at + kw.len() > bytes.len() {
        return false;
    }
    if &bytes[at..at + kw.len()] != kw {
        return false;
    }
    let next = bytes.get(at + kw.len()).copied().unwrap_or(b' ');
    !next.is_ascii_alphanumeric() && next != b'_'
}

/// Collapse runs of whitespace (including newlines) to single spaces, trimmed.
fn normalize_signature_ws(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cyclomatic complexity of a named routine, via the owned IR (the canonical
    /// complexity walker — the tree-sitter `calculate_complexity` is retired).
    fn complexity_of(al_code: &str, proc_name: &str) -> u32 {
        let f = al_syntax::parse(al_code);
        for obj in &f.objects {
            for r in &obj.routines {
                if r.name == proc_name {
                    return routine_complexity_ir(&f.ir, r);
                }
            }
        }
        panic!("procedure {proc_name} not found");
    }

    #[test]
    fn test_complexity_calculation_with_actual_al_parsing() {
        // Test AL code with various control flow constructs
        // Expected complexity calculation:
        // Base: 1
        // if i > 0: +1 (no else)
        // nested if i > 10: +1, else: +1 (has else branch)
        // while not done: +1
        // if (i > 0) and (i < 100): +1 for if, +1 for and
        // case: 2 case_branch items (+2, case_else_branch is not counted)
        // Total expected: 1 + 1 + 2 + 1 + 2 + 2 = 9
        //
        // Note: case_else_branch is correctly NOT counted per standard cyclomatic complexity
        // (the else/default path is not a decision point)
        let al_code = r#"codeunit 50100 "Test"
{
    procedure ComplexProcedure()
    var
        i: Integer;
        done: Boolean;
    begin
        if i > 0 then begin
            if i > 10 then
                i := 10
            else
                i := 5;
        end;

        while not done do begin
            if (i > 0) and (i < 100) then
                i += 1;
            done := i >= 100;
        end;

        case i of
            1: i := 2;
            2: i := 3;
            else i := 0;
        end;
    end;
}"#;

        let complexity = complexity_of(al_code, "ComplexProcedure");

        // Expected 9 (1 + if + nested if/else + while + if + `and` + 2 case branches).
        // The tree-sitter-al grammar fix (owned-IR consumer: named in/is/as expressions,
        // removing the spurious left/operator/right field bleed) corrected a previous
        // undercount to 8 — the clean grammar now yields the theoretically correct 9.
        assert_eq!(complexity, 9, "Expected complexity 9, got {}", complexity);
    }

    #[test]
    fn test_complexity_if_else() {
        // Test if-else counting: if adds +1, else adds +1
        let al_code = r#"codeunit 50100 "Test"
{
    procedure IfElseProcedure()
    var
        i: Integer;
    begin
        if i > 0 then
            i := 1
        else
            i := 0;
    end;
}"#;

        let complexity = complexity_of(al_code, "IfElseProcedure");
        // Base: 1, if: +1, else: +1 = 3
        assert_eq!(
            complexity, 3,
            "If-else procedure should have complexity 3, got {}",
            complexity
        );
    }

    #[test]
    fn test_complexity_simple_procedure() {
        // A simple procedure with no control flow should have complexity 1
        let al_code = r#"codeunit 50100 "Test"
{
    procedure SimpleProcedure()
    begin
        Message('Hello');
    end;
}"#;

        let complexity = complexity_of(al_code, "SimpleProcedure");
        assert_eq!(complexity, 1, "Simple procedure should have complexity 1");
    }

    #[test]
    fn test_complexity_loops() {
        // Test all loop types: for, foreach, repeat, while
        let al_code = r#"codeunit 50100 "Test"
{
    procedure LoopProcedure()
    var
        i: Integer;
        items: List of [Integer];
    begin
        for i := 1 to 10 do
            i := i;

        foreach i in items do
            i := i;

        repeat
            i += 1;
        until i > 10;

        while i < 20 do
            i += 1;
    end;
}"#;

        let complexity = complexity_of(al_code, "LoopProcedure");
        // Base: 1, for: +1, foreach: +1, repeat: +1, while: +1 = 5
        assert_eq!(
            complexity, 5,
            "Loop procedure should have complexity 5, got {}",
            complexity
        );
    }

    #[test]
    fn test_complexity_logical_operators() {
        // Test logical operators: and, or (xor is not counted per standard CC)
        let al_code = r#"codeunit 50100 "Test"
{
    procedure LogicalProcedure()
    var
        a: Boolean;
        b: Boolean;
        c: Boolean;
    begin
        if a and b then
            c := true;

        if a or b then
            c := false;

        if a and b and c then
            c := true;
    end;
}"#;

        let complexity = complexity_of(al_code, "LogicalProcedure");
        // Base: 1
        // if a and b: +1 (if) +1 (and)
        // if a or b: +1 (if) +1 (or)
        // if a and b and c: +1 (if) +2 (two and operators)
        // Total: 1 + 2 + 2 + 3 = 8
        assert_eq!(
            complexity, 8,
            "Logical procedure should have complexity 8, got {}",
            complexity
        );
    }

    #[test]
    fn test_quality_score_perfect() {
        let score = calculate_quality_score(1, 5, 1);
        assert!((score - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_quality_score_high_complexity() {
        let score = calculate_quality_score(10, 5, 1);
        assert!(score < 5.0);
    }

    #[test]
    fn test_quality_score_long_method() {
        let score = calculate_quality_score(1, 50, 1);
        assert!(score < 5.0);
    }

    #[test]
    fn test_quality_score_many_params() {
        let score = calculate_quality_score(1, 5, 8);
        assert!(score < 7.0);
    }

    #[test]
    fn test_findings_generated_for_high_complexity() {
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 12,
            line_count: 10,
            parameter_count: 2,
            quality_score: 5.0,
            ..Default::default()
        };
        let config = DiagnosticConfig::default();
        let findings = generate_findings(&metrics, &config);
        assert!(
            findings
                .iter()
                .any(|f| f.category == "high_complexity" && f.severity == "critical")
        );
    }

    #[test]
    fn test_quality_score_moderate_complexity() {
        // complexity 3 hits the else-if branch (> 2 but <= 4)
        let score = calculate_quality_score(3, 5, 1);
        // penalty = (3-2) * 0.8 = 0.8, so score = 9.2
        assert!((score - 9.2).abs() < 0.01);
    }

    #[test]
    fn test_quality_score_moderate_length() {
        // line_count 12 hits the else-if branch (> 10 but <= 15)
        let score = calculate_quality_score(1, 12, 1);
        // penalty = (12-10) * 0.3 = 0.6, so score = 9.4
        assert!((score - 9.4).abs() < 0.01);
    }

    #[test]
    fn test_quality_score_moderate_params() {
        // params 3 hits the else-if branch (> 2 but <= 4)
        let score = calculate_quality_score(1, 5, 3);
        // penalty = (3-2) * 0.5 = 0.5, so score = 9.5
        assert!((score - 9.5).abs() < 0.01);
    }

    #[test]
    fn test_quality_score_clamped_to_zero() {
        // Very bad metrics should clamp to 0
        let score = calculate_quality_score(50, 200, 20);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_findings_complexity_warning() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: config.complexity_warning, // at warning threshold
            line_count: 5,
            parameter_count: 1,
            quality_score: 8.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "high_complexity");
        assert_eq!(findings[0].severity, "warning");
    }

    #[test]
    fn test_findings_length_critical() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 1,
            line_count: config.length_critical, // at critical threshold
            parameter_count: 1,
            quality_score: 5.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert!(
            findings
                .iter()
                .any(|f| f.category == "long_method" && f.severity == "critical")
        );
    }

    #[test]
    fn test_findings_length_warning() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 1,
            line_count: config.length_warning, // at warning threshold
            parameter_count: 1,
            quality_score: 7.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "long_method");
        assert_eq!(findings[0].severity, "warning");
    }

    #[test]
    fn test_findings_params_critical() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 1,
            line_count: 5,
            parameter_count: config.params_critical, // at critical threshold
            quality_score: 5.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert!(
            findings
                .iter()
                .any(|f| f.category == "too_many_parameters" && f.severity == "critical")
        );
    }

    #[test]
    fn test_findings_params_warning() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 1,
            line_count: 5,
            parameter_count: config.params_warning, // at warning threshold
            quality_score: 7.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].category, "too_many_parameters");
        assert_eq!(findings[0].severity, "warning");
    }

    #[test]
    fn test_findings_no_issues() {
        let config = DiagnosticConfig::default();
        let metrics = ProcedureMetrics {
            object_type: "Codeunit".to_string(),
            object_name: "Test".to_string(),
            procedure_name: "TestProc".to_string(),
            file: "test.al".to_string(),
            line: 10,
            complexity: 1,
            line_count: 5,
            parameter_count: 1,
            quality_score: 10.0,
            ..Default::default()
        };
        let findings = generate_findings(&metrics, &config);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_build_summary_with_metrics() {
        let metrics = vec![
            ProcedureMetrics {
                object_type: "Codeunit".to_string(),
                object_name: "Test".to_string(),
                procedure_name: "Proc1".to_string(),
                file: "test.al".to_string(),
                line: 10,
                complexity: 4,
                line_count: 20,
                parameter_count: 2,
                quality_score: 8.0,
                ..Default::default()
            },
            ProcedureMetrics {
                object_type: "Codeunit".to_string(),
                object_name: "Test".to_string(),
                procedure_name: "Proc2".to_string(),
                file: "test.al".to_string(),
                line: 30,
                complexity: 6,
                line_count: 30,
                parameter_count: 3,
                quality_score: 6.0,
                ..Default::default()
            },
        ];
        let findings = vec![
            Finding {
                category: "high_complexity".to_string(),
                severity: "critical".to_string(),
                location: "test.al:30".to_string(),
                procedure: "Test.Proc2".to_string(),
                description: "test".to_string(),
                path: None,
            },
            Finding {
                path: None,
                category: "long_method".to_string(),
                severity: "warning".to_string(),
                location: "test.al:30".to_string(),
                procedure: "Test.Proc2".to_string(),
                description: "test".to_string(),
            },
        ];
        let summary = build_summary(&metrics, &findings);
        assert_eq!(summary.total_procedures, 2);
        assert!((summary.avg_complexity - 5.0).abs() < 0.01);
        assert!((summary.avg_quality_score - 7.0).abs() < 0.01);
        assert_eq!(summary.critical_findings, 1);
        assert_eq!(summary.warning_findings, 1);
    }

    #[test]
    fn test_build_summary_empty() {
        let summary = build_summary(&[], &[]);
        assert_eq!(summary.total_procedures, 0);
        assert_eq!(summary.avg_complexity, 0.0);
        assert_eq!(summary.avg_quality_score, 0.0);
        assert_eq!(summary.critical_findings, 0);
        assert_eq!(summary.warning_findings, 0);
    }

    /// #159: a file the snapshot does not hold (a join miss, stated by hand: it
    /// is written AFTER the snapshot is built) gets `None` graph fields and no
    /// `unused_procedure` finding, never `Some(0)` / "unused".
    #[test]
    fn join_miss_gives_null_not_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("app.json"),
            r#"{"id":"15900000-0000-4000-8000-000000000008","name":"JoinMiss","publisher":"t","version":"1.0.0.0","idRanges":[{"from":50100,"to":50199}],"runtime":"14.0"}"#,
        )
        .expect("app.json");
        let held_src =
            "codeunit 50100 \"Held\"\n{\n    procedure Uncalled()\n    begin\n    end;\n}\n";
        let held = dir.path().join("Held.Codeunit.al");
        std::fs::write(&held, held_src).expect("held");
        let snap = LspSnapshot::build_full(dir.path()).expect("snapshot");
        let missing = dir.path().join("Missing.Codeunit.al");
        std::fs::write(
            &missing,
            held_src.replace("50100 \"Held\"", "50101 \"Missing\""),
        )
        .expect("missing");
        let graph = Graph::new(&snap);

        // Precondition: the held file's uncalled procedure IS graph-backed and unused.
        let (rows, unused, ..) =
            file_metrics("Held.Codeunit.al", &held, Some(&graph), Some("g"), true);
        assert_eq!(
            (rows.len(), rows[0].fan_in, rows[0].fan_out),
            (1, Some(0), Some(0))
        );
        assert_eq!(unused.len(), 1, "precondition: Held.Uncalled is unused");

        let (rows, unused, ..) = file_metrics(
            "Missing.Codeunit.al",
            &missing,
            Some(&graph),
            Some("g"),
            true,
        );
        assert_eq!(rows.len(), 1, "the missing file is still analyzed");
        assert_eq!(
            rows[0].fan_in, None,
            "join miss: fan_in must be None, not 0"
        );
        assert_eq!(
            rows[0].fan_out, None,
            "join miss: fan_out must be None, not 0"
        );
        assert!(
            unused.is_empty(),
            "join miss: no unused finding: {unused:?}"
        );
    }
}
