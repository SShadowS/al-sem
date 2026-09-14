//! Issue 25 — d19-unused-parameter must not flag an EVENT PUBLISHER's parameters.
//!
//! A publisher body is empty by language definition (the compiler generates the
//! dispatch), so d19's body-local "is this parameter referenced?" predicate is
//! 100% false-positive on that population by construction. The parameters exist
//! for SUBSCRIBERS to read and write, which d19 cannot see. Measured on the r0
//! corpus before the fix: `ws-d59` 6, `ws-d38` 3, `ws-d12-dead-event` 1 — all
//! false positives — against `ws-d19`'s 2 genuine ones.
//!
//! Every test here drives REAL workspace assembly (`assemble_and_resolve_default`)
//! and the REGISTERED detector (`registered_detectors` + `run_detectors`), never a
//! helper function — and asserts the assembly actually produced the routines, with
//! the kind and parameters the case depends on, so a case can never pass by having
//! analysed nothing.

use std::collections::BTreeMap;

use al_sem::engine::l3::l3_workspace::{L3Resolved, assemble_and_resolve_default};
use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;

const APP_GUID: &str = "11111111-0000-0000-0000-00000d19abcd";
const DETECTOR: &str = "d19-unused-parameter";

/// What one d19 run over an assembled single-file workspace produced.
struct D19Run {
    findings: Vec<Finding>,
    /// d19's own `DetectorStats.skipped` map (present-iff-nonzero, see
    /// `registry::DetectorStats::add_skip`).
    skipped: BTreeMap<String, u64>,
    resolved: L3Resolved,
}

impl D19Run {
    fn skip(&self, key: &str) -> u64 {
        self.skipped.get(key).copied().unwrap_or(0)
    }

    /// Assert the assembly actually produced a routine with this name, and that it
    /// carries the kind / body / parameter count the case under test depends on.
    ///
    /// This is the anti-degenerate guard: without it a workspace that failed to
    /// parse would analyse zero routines and every "expects 0 findings" case would
    /// pass while proving nothing.
    fn assert_assembled(&self, name: &str, kind: &str, params: usize) {
        let names: Vec<&str> = self
            .resolved
            .workspace
            .routines
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        let r = self
            .resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| {
                panic!("assembly produced no routine named {name:?}; assembled routines: {names:?}")
            });
        assert_eq!(
            r.kind, kind,
            "routine {name:?} classified {:?}, expected {kind:?} — the case under test \
             depends on this classification",
            r.kind
        );
        assert_eq!(
            r.parameters.len(),
            params,
            "routine {name:?} assembled with {} parameter(s), expected {params} — \
             the case under test depends on the parameters being present",
            r.parameters.len()
        );
        assert!(
            r.body_available,
            "routine {name:?} assembled without a body; d19 skips !body_available \
             BEFORE the kind checks, which would make this case vacuous"
        );
        assert!(
            !r.parse_incomplete,
            "routine {name:?} assembled parse_incomplete; d19 skips that BEFORE the \
             kind checks, which would make this case vacuous"
        );
    }
}

/// Assemble the given AL source into a real one-app workspace and run the
/// REGISTERED d19 detector over it.
fn run_d19(src: &str) -> D19Run {
    let files = vec![("src/Issue25.al".to_string(), src.to_string())];
    let resolved = assemble_and_resolve_default(&files, APP_GUID);
    let d19: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(d19.len(), 1, "{DETECTOR} must be registered exactly once");
    let out = run_detectors(&resolved, &d19);
    let stats = out
        .detector_stats
        .iter()
        .find(|s| s.detector == DETECTOR)
        .unwrap_or_else(|| panic!("{DETECTOR} produced no DetectorStats"));
    D19Run {
        skipped: stats.skipped.clone(),
        findings: out.findings.clone(),
        resolved,
    }
}

/// Case 1 — an `[IntegrationEvent]` publisher's parameters are never d19 findings.
/// Both parameters are unreferenced (the body is empty, as the language requires),
/// so pre-fix this fixture produced 2 findings.
#[test]
fn integration_event_publisher_parameters_are_not_flagged() {
    let src = r#"
codeunit 50250 "D19 Integration Publisher"
{
    [IntegrationEvent(false, false)]
    procedure OnBeforePost(DocumentNo: Code[20]; var IsHandled: Boolean)
    begin
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("OnBeforePost", "event-publisher", 2);
    assert!(
        run.findings.is_empty(),
        "an IntegrationEvent publisher's parameters exist for SUBSCRIBERS to use; \
         d19 cannot see that and must not flag them. findings: {:#?}",
        run.findings
    );
    assert_eq!(
        run.skip("eventPublisher"),
        1,
        "the publisher must be booked as an eventPublisher skip. skipped: {:?}",
        run.skipped
    );
}

/// Case 2 — `[BusinessEvent]` is the other kind `ir_routine_kind` maps to
/// `event-publisher`. The issue's own IntegrationEvent-only proposal would have
/// missed this; skipping by KIND covers both for free.
#[test]
fn business_event_publisher_parameters_are_not_flagged() {
    let src = r#"
codeunit 50251 "D19 Business Publisher"
{
    [BusinessEvent(false)]
    procedure OnCustomerBlocked(CustomerNo: Code[20]; Reason: Text)
    begin
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("OnCustomerBlocked", "event-publisher", 2);
    assert!(
        run.findings.is_empty(),
        "a BusinessEvent publisher's parameters must not be flagged either. \
         findings: {:#?}",
        run.findings
    );
    assert_eq!(
        run.skip("eventPublisher"),
        1,
        "the publisher must be booked as an eventPublisher skip. skipped: {:?}",
        run.skipped
    );
}

/// Case 3 — attribute names are lowercased at `ir/decl.rs` before
/// `ir_routine_kind` compares them, so mixed ASCII casing classifies identically.
#[test]
fn mixed_case_integration_event_attribute_is_still_a_publisher() {
    let src = r#"
codeunit 50252 "D19 Mixed Case Publisher"
{
    [integrationEVENT(false, false)]
    procedure OnAfterRelease(var IsHandled: Boolean)
    begin
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("OnAfterRelease", "event-publisher", 1);
    assert!(
        run.findings.is_empty(),
        "[integrationEVENT] is the same attribute as [IntegrationEvent]; the skip \
         must not be casing-sensitive. findings: {:#?}",
        run.findings
    );
    assert_eq!(run.skip("eventPublisher"), 1, "skipped: {:?}", run.skipped);
}

/// Case 4 — THE POSITIVE CONTROL. An ordinary procedure with a genuinely unused
/// parameter must STILL fire; without this, "skip everything" would pass cases 1-3.
#[test]
fn ordinary_procedure_with_unused_parameter_still_fires() {
    let src = r#"
codeunit 50253 "D19 Ordinary Procedure"
{
    procedure Compute(Used: Integer; Unused: Integer): Integer
    begin
        exit(Used * 2);
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("Compute", "procedure", 2);
    assert_eq!(
        run.findings.len(),
        1,
        "an ordinary procedure's genuinely unused parameter is still a d19 finding. \
         findings: {:#?}",
        run.findings
    );
    assert!(
        run.findings[0].root_cause.contains("'Unused'"),
        "the finding must name the UNUSED parameter, not the used one. rootCause: {}",
        run.findings[0].root_cause
    );
    assert_eq!(
        run.skip("eventPublisher"),
        0,
        "an ordinary procedure must not be booked as a publisher skip. skipped: {:?}",
        run.skipped
    );
}

/// Case 5 — the new publisher skip must not steal the PRE-EXISTING subscriber
/// accounting: a plain `[EventSubscriber]` still books `eventSubscriber`, and books
/// nothing against `eventPublisher`.
#[test]
fn event_subscriber_books_a_subscriber_skip_not_a_publisher_skip() {
    let src = r#"
codeunit 50254 "D19 Subscriber"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"D19 Subscriber", 'OnSomething', '', false, false)]
    procedure HandleSomething(DocumentNo: Code[20]; var IsHandled: Boolean)
    begin
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("HandleSomething", "event-subscriber", 2);
    assert!(run.findings.is_empty(), "findings: {:#?}", run.findings);
    assert_eq!(
        run.skip("eventSubscriber"),
        1,
        "a subscriber books the PRE-EXISTING eventSubscriber counter. skipped: {:?}",
        run.skipped
    );
    assert_eq!(
        run.skip("eventPublisher"),
        0,
        "a subscriber must NOT be booked against the publisher counter. skipped: {:?}",
        run.skipped
    );
}

/// Case 5b — the PRECEDENCE pin. A routine carrying a subscriber attribute AND a
/// publisher attribute classifies `event-subscriber`, so it lands on the subscriber
/// counter.
///
/// Where that precedence actually lives, measured (issue 25): NOT in `detect_d19`.
/// The spec and plan both said d19's check ORDER was load-bearing — "move the
/// publisher check ahead of the subscriber check and a dual-attribute routine is
/// booked against the wrong counter". That is FALSE, and swapping the two blocks
/// proved it: all 7 tests stayed green. d19's three checks are mutually-exclusive
/// equality tests against ONE scalar (`routine.kind`), which by then already holds a
/// single decided value, so their order cannot be observed.
///
/// The real guard is `ir_walk::ir_routine_kind`'s branch order, and THAT is what
/// this test pins: inverting its `eventsubscriber` / publisher branches fails the
/// `assert_assembled` line below with `left: "event-publisher"`.
#[test]
fn dual_attribute_routine_books_the_subscriber_counter() {
    let src = r#"
codeunit 50255 "D19 Dual Attribute"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"D19 Dual Attribute", 'OnSomething', '', false, false)]
    [IntegrationEvent(false, false)]
    procedure HandleAndPublish(DocumentNo: Code[20])
    begin
    end;
}
"#;
    let run = run_d19(src);
    // THIS is the discriminating line — it fails when `ir_routine_kind`'s branch
    // order is inverted. Everything below it follows from the classification.
    run.assert_assembled("HandleAndPublish", "event-subscriber", 1);
    assert_eq!(
        run.skip("eventSubscriber"),
        1,
        "subscriber precedence means the SUBSCRIBER counter takes it. skipped: {:?}",
        run.skipped
    );
    assert_eq!(
        run.skip("eventPublisher"),
        0,
        "a subscriber-classified routine must never reach the publisher counter. \
         skipped: {:?}",
        run.skipped
    );
}

/// Case 6 — the skip statistic counts ROUTINES, not parameters. Two publishers
/// carrying 3 and 2 parameters must book 2, not 5.
#[test]
fn publisher_skip_statistic_counts_routines_not_parameters() {
    let src = r#"
codeunit 50256 "D19 Publisher Counting"
{
    [IntegrationEvent(false, false)]
    procedure OnThree(A: Integer; B: Integer; C: Integer)
    begin
    end;

    [BusinessEvent(false)]
    procedure OnTwo(D: Integer; E: Integer)
    begin
    end;
}
"#;
    let run = run_d19(src);
    run.assert_assembled("OnThree", "event-publisher", 3);
    run.assert_assembled("OnTwo", "event-publisher", 2);
    assert!(run.findings.is_empty(), "findings: {:#?}", run.findings);
    assert_eq!(
        run.skip("eventPublisher"),
        2,
        "5 unreferenced parameters across 2 publisher routines must book 2 skips, \
         not 5. skipped: {:?}",
        run.skipped
    );
}
