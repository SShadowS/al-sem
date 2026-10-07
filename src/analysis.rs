//! Code quality analysis - cyclomatic complexity and metrics

use serde::Serialize;

/// Metrics for a single procedure/trigger
#[derive(Debug, Clone, Serialize)]
pub struct ProcedureMetrics {
    pub object_type: String,
    pub object_name: String,
    pub procedure_name: String,
    pub file: String,
    pub line: u32,
    pub complexity: u32,
    pub line_count: u32,
    pub parameter_count: u32,
    pub quality_score: f32,
}

/// A finding/issue detected during analysis
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub category: String,
    pub severity: String,
    pub location: String,
    pub procedure: String,
    pub description: String,
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
}

use crate::config::DiagnosticConfig;

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
            },
        ];
        let findings = vec![
            Finding {
                category: "high_complexity".to_string(),
                severity: "critical".to_string(),
                location: "test.al:30".to_string(),
                procedure: "Test.Proc2".to_string(),
                description: "test".to_string(),
            },
            Finding {
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
}
