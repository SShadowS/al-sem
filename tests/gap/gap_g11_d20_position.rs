//! Gap G-11 (docs/engine-gaps.md): d20-unreachable-after-exit must fire ONLY when
//! the exit (or other terminator) is UNCONDITIONAL and an actual subsequent
//! STATEMENT follows it in the same block — never on:
//!   (a) a trailing inline comment on the `exit(...)` line (`exit(0); // note`),
//!   (b) a single-line function body that is just `exit(expr)` (nothing follows),
//!   (c) the fall-through `exit(0)` after a CONDITIONAL `if … then exit(x)`.
//!
//! Suppression-direction safety (control): an UNCONDITIONAL `exit(x);` followed
//! by a REAL statement (`Foo := 2;`) MUST still fire on the dead statement.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g11abc";

/// Run d20 in isolation over an inline workspace and return its emitted findings.
fn run_d20(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let d20: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d20-unreachable-after-exit")
        .collect();
    assert_eq!(d20.len(), 1, "d20 detector must be registered exactly once");
    run_detectors(&resolved, &d20).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

// --- (a) trailing inline comment on the exit line must NOT fire --------------

/// `exit(0); // a comment` as the LAST statement of the proc: the comment is not
/// a statement, nothing executable follows the exit → NO d20.
#[test]
fn trailing_inline_comment_after_exit_does_not_fire() {
    let src = r#"
codeunit 50911 "G11 Trailing Comment"
{
    procedure GetZero(): Integer
    begin
        exit(0); // a comment
    end;
}
"#;
    let findings = run_d20(&[al("G11TrailingComment", src)]);
    assert!(
        findings.is_empty(),
        "a trailing inline comment on the exit line is not a statement — d20 must \
         not fire. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

/// Same shape, but the comment sits on its OWN line after the exit: still not a
/// statement → NO d20.
#[test]
fn own_line_comment_after_exit_does_not_fire() {
    let src = r#"
codeunit 50912 "G11 Own Line Comment"
{
    procedure GetOne(): Integer
    begin
        exit(1);
        // explanatory note about the return value
    end;
}
"#;
    let findings = run_d20(&[al("G11OwnLineComment", src)]);
    assert!(
        findings.is_empty(),
        "a comment on its own line after the exit is not a statement — d20 must \
         not fire. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

// --- (b) single-line body that is just exit(expr) must NOT fire --------------

/// A one-line proc body `exit(true);` — nothing follows the exit at all → NO d20.
#[test]
fn single_line_exit_only_body_does_not_fire() {
    let src = r#"
codeunit 50913 "G11 Single Line"
{
    procedure IsEnabled(): Boolean
    begin exit(true); end;

    procedure GetName(): Text
    begin
        exit('G11');
    end;
}
"#;
    let findings = run_d20(&[al("G11SingleLine", src)]);
    assert!(
        findings.is_empty(),
        "a body that is just `exit(expr)` has no following statement — d20 must \
         not fire. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

// --- (c) conditional exit then fall-through must NOT fire --------------------

/// `if Cond then exit(1); exit(0);` — the first exit is CONDITIONAL, so the
/// fall-through `exit(0)` is reachable → NO d20.
#[test]
fn conditional_exit_then_fallthrough_does_not_fire() {
    let src = r#"
codeunit 50914 "G11 Conditional"
{
    procedure Pick(Cond: Boolean): Integer
    begin
        if Cond then
            exit(1);
        exit(0);
    end;
}
"#;
    let findings = run_d20(&[al("G11Conditional", src)]);
    assert!(
        findings.is_empty(),
        "a CONDITIONAL exit does not make the fall-through dead — d20 must not \
         fire. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

// --- CONTROL: unconditional exit + real following statement MUST still fire --

/// `exit(1); Foo := 2;` — the assignment after the UNCONDITIONAL exit is genuinely
/// dead. d20 MUST still fire (suppression-direction guard).
#[test]
fn unconditional_exit_with_real_following_statement_still_fires() {
    let src = r#"
codeunit 50915 "G11 Control"
{
    procedure Dead(): Integer
    var
        Foo: Integer;
    begin
        exit(1);
        Foo := 2;
    end;
}
"#;
    let findings = run_d20(&[al("G11Control", src)]);
    assert_eq!(
        findings.len(),
        1,
        "the dead `Foo := 2;` after an unconditional exit MUST still fire. \
         findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
    let f = &findings[0];
    assert_eq!(f.detector, "d20-unreachable-after-exit");
    assert!(
        f.root_cause.contains("unreachable"),
        "rootCause must describe the unreachable statement. rootCause: {}",
        f.root_cause
    );
}

/// A comment BETWEEN the exit and a real dead statement must not mask the dead
/// statement: `exit(1); // note` then `Foo := 2;` still fires.
#[test]
fn comment_between_exit_and_dead_statement_still_fires() {
    let src = r#"
codeunit 50917 "G11 Comment Between"
{
    procedure Dead(): Integer
    var
        Foo: Integer;
    begin
        exit(1); // explanatory note
        Foo := 2;
    end;
}
"#;
    let findings = run_d20(&[al("G11CommentBetween", src)]);
    assert_eq!(
        findings.len(),
        1,
        "a comment between the exit and the dead statement must not suppress the \
         finding. findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}

/// Same control through the other exit kinds: an unconditional `Error(...)` with a
/// real statement after it still fires.
#[test]
fn unconditional_error_with_real_following_statement_still_fires() {
    let src = r#"
codeunit 50916 "G11 Error Control"
{
    procedure Fail()
    var
        Foo: Integer;
    begin
        Error('boom');
        Foo := 3;
    end;
}
"#;
    let findings = run_d20(&[al("G11ErrorControl", src)]);
    assert_eq!(
        findings.len(),
        1,
        "the dead statement after an unconditional Error MUST still fire. \
         findings: {:#?}",
        findings
            .iter()
            .map(|f| (&f.id, &f.root_cause))
            .collect::<Vec<_>>()
    );
}
