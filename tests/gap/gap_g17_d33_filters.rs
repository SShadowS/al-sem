//! Gap G-17 (docs/engine-gaps.md): d33 still misses some one-hop filters +
//! the page row-selection filter.
//!
//! (a) The real-world G-3 miss: the filter helper is a procedure defined ON
//! THE RECEIVER'S TABLE, called as `LineReport.SetLineFilter(Rec)` — the
//! by-value argument is only the FILTER-VALUE SOURCE; the helper filters its
//! own implicit self record (bare `SetRange(...)` in the table method body),
//! which aliases the caller's receiver. G-3's by-`var`-argument summary can
//! never match this shape, so `LineReport.DeleteAll()` after it falsely fired.
//!
//! (b) `CurrPage.SetSelectionFilter(Rec)` — the platform builtin that copies
//! the page's row selection onto the argument record as filters — was not
//! recognized as a filter-setter.
//!
//! Suppression-direction guardrails (controls below): a bulk write with NO
//! prior filter still fires; a receiver method that does NOT filter its self
//! record still fires; a receiver method whose net effect is unfiltered
//! (filter then Reset) still fires; `SetSelectionFilter` on a DIFFERENT
//! record still fires.
//!
//! Drives the REAL detector over inline AL workspaces (mirrors
//! `tests/gap_g3_interproc_filter.rs`).

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-000000g17abc";

/// Run d33 over an inline workspace and return all emitted findings.
fn run_g17_detector(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d33-unfiltered-bulk-write")
        .collect();
    assert_eq!(detectors.len(), 1, "d33 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

/// The "template line" table — the filter-VALUE source record.
const TEMPLATE_LINE_SRC: &str = r#"
table 50170 "G17 Template Line"
{
    fields
    {
        field(1; "Template Code"; Code[20]) { }
        field(2; "Language Code"; Code[10]) { }
        field(3; "Line No."; Integer) { }
    }
    keys { key(PK; "Template Code", "Language Code", "Line No.") { } }
}
"#;

/// The "line report" table with the in-source filter helper defined ON the
/// table itself: a by-VALUE argument supplies the filter values; the bare
/// `SetRange(...)` calls filter the implicit self record (the mirror of CDO's
/// `SetEMailTemplateLineFilter` / `SetTemplateFilter` / `SetMergeFieldFilter`).
const LINE_REPORT_SRC: &str = r#"
table 50171 "G17 Line Report"
{
    fields
    {
        field(1; "Template Code"; Code[20]) { }
        field(2; "Language Code"; Code[10]) { }
        field(3; Sequence; Integer) { }
    }
    keys { key(PK; "Template Code", "Language Code", Sequence) { } }

    procedure SetLineFilter(TemplateLine: Record "G17 Template Line")
    begin
        SetRange("Template Code", TemplateLine."Template Code");
        SetRange("Language Code", TemplateLine."Language Code");
    end;

    procedure InspectLines(TemplateLine: Record "G17 Template Line")
    begin
        if FindFirst() then;
    end;

    procedure FilterThenReset(TemplateLine: Record "G17 Template Line")
    begin
        SetRange("Template Code", TemplateLine."Template Code");
        Reset();
    end;
}
"#;

// --- Suppression (a): receiver-method filter helper before the bulk write -------

/// The real-world G-3 miss (CDO `EMailTemplLineReport.SetEMailTemplateLineFilter(Rec);
/// EMailTemplLineReport.DeleteAll();`): the helper is defined ON the receiver's
/// table and filters its implicit self record. The receiver IS filtered — NO d33.
#[test]
fn receiver_table_method_filter_suppresses_d33() {
    let caller_src = r#"
table 50172 "G17 Caller Line"
{
    fields
    {
        field(1; "Template Code"; Code[20]) { }
        field(2; "Language Code"; Code[10]) { }
        field(3; "Line No."; Integer) { }
    }
    keys { key(PK; "Template Code", "Language Code", "Line No.") { } }

    trigger OnDelete()
    var
        TemplateLine: Record "G17 Template Line";
        LineReport: Record "G17 Line Report";
    begin
        LineReport.SetLineFilter(TemplateLine);
        LineReport.DeleteAll();
    end;
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17LineReport", LINE_REPORT_SRC),
        al("G17CallerLine", caller_src),
    ]);
    assert!(
        !findings.iter().any(|f| f.root_cause.contains("LineReport")),
        "a receiver-table filter method (`LineReport.SetLineFilter(..)`) filters the \
         receiver's implicit self record before the DeleteAll — d33 must not fire on \
         LineReport. findings: {findings:#?}"
    );
}

// --- Suppression (b): CurrPage.SetSelectionFilter before the bulk write ---------

/// `CurrPage.SetSelectionFilter(Hdr)` copies the page's row selection onto
/// `Hdr` as filters — the subsequent `Hdr.ModifyAll(..)` is scoped to the
/// selected rows. NO d33.
#[test]
fn currpage_setselectionfilter_suppresses_d33() {
    let page_src = r#"
page 50173 "G17 Template Lines"
{
    PageType = List;
    SourceTable = "G17 Template Line";

    actions
    {
        area(Processing)
        {
            action(ClearSelected)
            {
                trigger OnAction()
                var
                    TemplateLine: Record "G17 Template Line";
                begin
                    CurrPage.SetSelectionFilter(TemplateLine);
                    TemplateLine.ModifyAll("Line No.", 0);
                end;
            }
        }
    }
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17TemplateLines", page_src),
    ]);
    assert!(
        findings.is_empty(),
        "CurrPage.SetSelectionFilter(TemplateLine) scopes the ModifyAll to the user's \
         selected rows — d33 must not fire. findings: {findings:#?}"
    );
}

// --- CONTROL: no filter of any kind still fires ----------------------------------

/// `DeleteAll` with NO prior filter — d33 must STILL fire
/// (suppression-direction guard).
#[test]
fn control_no_filter_still_fires() {
    let cu_src = r#"
codeunit 50174 "G17 NoFilter"
{
    procedure DeleteBlind()
    var
        LineReport: Record "G17 Line Report";
    begin
        LineReport.DeleteAll();
    end;
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17LineReport", LINE_REPORT_SRC),
        al("G17NoFilter", cu_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteBlind")),
        "d33 must still fire on DeleteAll with no prior filter. findings: {findings:#?}"
    );
}

// --- CONTROL: receiver method that does NOT filter its self record still fires ---

/// The receiver method only READS the record (`FindFirst`) — it leaves no
/// filter on the receiver. d33 must STILL fire.
#[test]
fn control_non_filtering_receiver_method_still_fires() {
    let cu_src = r#"
codeunit 50175 "G17 NonFiltering"
{
    procedure DeleteAfterInspect()
    var
        TemplateLine: Record "G17 Template Line";
        LineReport: Record "G17 Line Report";
    begin
        LineReport.InspectLines(TemplateLine);
        LineReport.DeleteAll();
    end;
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17LineReport", LINE_REPORT_SRC),
        al("G17NonFiltering", cu_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterInspect")),
        "d33 must still fire when the receiver method never filters its self record. \
         findings: {findings:#?}"
    );
}

// --- CONTROL: receiver method whose NET effect is unfiltered still fires ---------

/// The receiver method filters its self record but then `Reset`s it — its net
/// effect leaves the receiver unfiltered. d33 must STILL fire.
#[test]
fn control_receiver_method_filter_then_reset_still_fires() {
    let cu_src = r#"
codeunit 50176 "G17 SelfClearing"
{
    procedure DeleteAfterSelfClearing()
    var
        TemplateLine: Record "G17 Template Line";
        LineReport: Record "G17 Line Report";
    begin
        LineReport.FilterThenReset(TemplateLine);
        LineReport.DeleteAll();
    end;
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17LineReport", LINE_REPORT_SRC),
        al("G17SelfClearing", cu_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("DeleteAfterSelfClearing")),
        "d33 must still fire when the receiver method's net effect is unfiltered \
         (filter then Reset). findings: {findings:#?}"
    );
}

// --- CONTROL: SetSelectionFilter on a DIFFERENT record still fires ---------------

/// `CurrPage.SetSelectionFilter` targets ANOTHER record — the bulk-op record
/// stays unfiltered. d33 must STILL fire.
#[test]
fn control_setselectionfilter_other_record_still_fires() {
    let page_src = r#"
page 50177 "G17 Other Selection"
{
    PageType = List;
    SourceTable = "G17 Template Line";

    actions
    {
        area(Processing)
        {
            action(ClearOthers)
            {
                trigger OnAction()
                var
                    TemplateLine: Record "G17 Template Line";
                    LineReport: Record "G17 Line Report";
                begin
                    CurrPage.SetSelectionFilter(TemplateLine);
                    LineReport.DeleteAll();
                end;
            }
        }
    }
}
"#;
    let findings = run_g17_detector(&[
        al("G17TemplateLine", TEMPLATE_LINE_SRC),
        al("G17LineReport", LINE_REPORT_SRC),
        al("G17OtherSelection", page_src),
    ]);
    assert!(
        findings
            .iter()
            .any(|f| f.detector == "d33-unfiltered-bulk-write"
                && f.root_cause.contains("LineReport")),
        "d33 must still fire when SetSelectionFilter targets a different record than \
         the bulk op. findings: {findings:#?}"
    );
}
