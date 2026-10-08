//! Detector-audit d20 FN (docs/detector-audit.md): `break` terminates the
//! enclosing loop, so a statement that DIRECTLY follows it in the same block is
//! unreachable — d20 should flag it (the wording says "the enclosing loop", not
//! "the routine"). The scan is block-scoped (next-sibling only), so a `break`
//! that is the LAST statement of its block, or a conditional `if c then break`,
//! does not fire.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000d20abcd";

fn run_d20(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d20: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d20-unreachable-after-exit")
        .collect();
    assert_eq!(d20.len(), 1);
    run_detectors(&resolved, &d20).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// A statement directly after an unconditional `break` inside a loop body is
/// unreachable → d20 fires, worded "control leaves the enclosing loop".
#[test]
fn statement_after_unconditional_break_fires() {
    let src = r#"
codeunit 50200 "D20 Break"
{
    procedure LoopBreak()
    var
        i: Integer;
    begin
        for i := 1 to 10 do begin
            break;
            Message('never runs');
        end;
    end;
}
"#;
    let findings = run_d20(&[al("D20Break", src)]);
    assert_eq!(
        findings.len(),
        1,
        "the statement after an unconditional break is unreachable. findings: {findings:#?}"
    );
    assert!(
        findings[0].root_cause.contains("the enclosing loop"),
        "break leaves the LOOP, not the routine. rootCause: {}",
        findings[0].root_cause
    );
}

/// Control: `break` as the LAST statement of its block has no following sibling →
/// nothing unreachable → no d20.
#[test]
fn break_as_last_statement_does_not_fire() {
    let src = r#"
codeunit 50201 "D20 Break Last"
{
    procedure LoopBreakLast()
    var
        i: Integer;
    begin
        for i := 1 to 10 do begin
            Message('ok');
            break;
        end;
    end;
}
"#;
    let findings = run_d20(&[al("D20BreakLast", src)]);
    assert!(
        findings.is_empty(),
        "break with nothing after it in its block is not unreachable. findings: {findings:#?}"
    );
}

/// Control: a CONDITIONAL `if c then break` is an if_statement node — never an
/// unconditional terminator — so the following statement stays reachable.
#[test]
fn conditional_break_does_not_fire() {
    let src = r#"
codeunit 50202 "D20 Cond Break"
{
    procedure LoopCondBreak(Stop: Boolean)
    var
        i: Integer;
    begin
        for i := 1 to 10 do begin
            if Stop then
                break;
            Message('still reachable');
        end;
    end;
}
"#;
    let findings = run_d20(&[al("D20CondBreak", src)]);
    assert!(
        findings.is_empty(),
        "a conditional break does not make the next statement unreachable. findings: {findings:#?}"
    );
}
