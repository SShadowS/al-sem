//! d9 gates on the span's PHYSICAL written-table count (issue-23 rule: GATES
//! read the physical count, WITNESS sets the temp-inclusive one).
//!
//! It used to gate on `span.writes_tables.len()`, the temp-inclusive union, so a
//! span whose only writes went to `temporary` records still counted as
//! "interesting" and its text claimed "writes N known table(s)". On CDO that kept
//! a d9 at `CDOeSealServiceMgt.Codeunit.al:18` alive after the cone fix removed
//! the matching d8. Drives the REGISTERED d9 over inline workspaces.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-00000000d9a1";
const DETECTOR: &str = "d9-transaction-span-summary";

const SRC: &str = r#"
table 50131 "D9 One" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50132 "D9 Two" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }
table 50133 "D9 Three" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { } } }

codeunit 50130 "D9 Spans"
{
    procedure TempDriver()
    var
        T1: Record "D9 One" temporary;
        T2: Record "D9 Two" temporary;
    begin
        T1.Insert();
        T2.Insert();
        CommitAfterTemp();
    end;

    local procedure CommitAfterTemp()
    begin
        Commit();
    end;

    procedure PhysDriver()
    var
        P1: Record "D9 One";
        P2: Record "D9 Two";
    begin
        P1.Insert();
        P2.Insert();
        CommitAfterPhys();
    end;

    local procedure CommitAfterPhys()
    begin
        Commit();
    end;

    procedure MixedDriver()
    var
        T1: Record "D9 One" temporary;
        T2: Record "D9 Two" temporary;
        P3: Record "D9 Three";
    begin
        T1.Insert();
        T2.Insert();
        P3.Insert();
        CommitAfterMixed();
    end;

    local procedure CommitAfterMixed()
    begin
        Commit();
    end;
}
"#;

fn d9_findings() -> Vec<Finding> {
    let files = vec![("src/D9.al".to_string(), SRC.to_string())];
    let resolved = assemble_and_resolve_inline_program_default(&files, APP_GUID);
    assert!(
        resolved
            .workspace
            .routines
            .iter()
            .any(|r| r.name == "CommitAfterTemp" && r.body_available),
        "fixture did not assemble"
    );
    let selected: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == DETECTOR)
        .collect();
    assert_eq!(selected.len(), 1);
    run_detectors(&resolved, &selected).findings
}

fn finding_for<'f>(findings: &'f [Finding], commit_routine: &str) -> Option<&'f Finding> {
    findings
        .iter()
        .find(|f| f.root_cause.contains(&format!("{commit_routine}'s Commit")))
}

#[test]
fn physical_two_table_span_fires_with_the_physical_count() {
    let f = d9_findings();
    let hit = finding_for(&f, "CommitAfterPhys").unwrap_or_else(|| {
        panic!(
            "no d9 for the physical span; got {:?}",
            f.iter().map(|x| &x.root_cause).collect::<Vec<_>>()
        )
    });
    assert!(
        hit.root_cause.contains("writes 2 known table(s)"),
        "{}",
        hit.root_cause
    );
}

#[test]
fn temp_only_span_does_not_fire() {
    let f = d9_findings();
    // Non-vacuity: the detector ran and fired somewhere in this workspace.
    assert!(finding_for(&f, "CommitAfterPhys").is_some());
    assert!(
        finding_for(&f, "CommitAfterTemp").is_none(),
        "a span writing only temporary records is not an interesting transaction"
    );
}

#[test]
fn two_temp_plus_one_physical_is_below_the_gate() {
    let f = d9_findings();
    assert!(finding_for(&f, "CommitAfterPhys").is_some());
    assert!(
        finding_for(&f, "CommitAfterMixed").is_none(),
        "1 physical table is below MIN_INTERESTING_TABLES = 2"
    );
}
