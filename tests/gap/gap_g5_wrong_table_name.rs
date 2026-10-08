//! Gap G-5 (docs/engine-gaps.md): wrong table name in rootCause.
//!
//! Symptom (CDO batch 2/3): in a routine with SEVERAL SEQUENTIAL `repeat` blocks,
//! each over a DIFFERENT local record var of a DIFFERENT table, an op in a LATER
//! sub-loop was reported against an EARLIER/unrelated record var's table name.
//! The finding itself is real — only the table/record NAME in the rootCause text
//! is wrong, which erodes trust in every finding.
//!
//! These tests drive the REAL d1 detector over inline AL workspaces (mirrors
//! `tests/gap_g6_virtual_tables.rs`) and assert that EVERY finding's rootCause
//! names the table of the record var the terminal op ACTUALLY operates on — never
//! a different sub-loop's var/table.

use al_sem::engine::l5::detectors::registered_detectors;
use al_sem::engine::l5::finding::Finding;
use al_sem::engine::l5::registry::run_detectors;
use al_sem::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP_GUID: &str = "11111111-0000-0000-0000-0000000g5abc";

/// Run d1 over an inline workspace and return its findings.
fn run_d1(files: &[(String, String)]) -> Vec<Finding> {
    let resolved = assemble_and_resolve_inline_program_default(files, APP_GUID);
    let detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| d.name == "d1-db-op-in-loop")
        .collect();
    assert_eq!(detectors.len(), 1, "d1 must be registered exactly once");
    run_detectors(&resolved, &detectors).findings
}

fn al(name: &str, body: &str) -> (String, String) {
    (format!("src/{name}.al"), body.to_string())
}

const ALPHA_TABLE_SRC: &str = r#"
table 50170 "G5 Alpha"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Name; Text[100]) { }
    }
    keys { key(PK; "No.") { } }
}
"#;

const BETA_TABLE_SRC: &str = r#"
table 50171 "G5 Beta"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
        field(2; Description; Text[100]) { }
    }
    keys { key(PK; "Entry No.") { } }
}
"#;

const GAMMA_TABLE_SRC: &str = r#"
table 50172 "G5 Gamma"
{
    fields
    {
        field(1; "Code"; Code[10]) { }
    }
    keys { key(PK; "Code") { } }
}
"#;

/// Assert exactly one d1 finding mentions `expected_table` in its rootCause, and
/// that NO finding pairs `op_word` with a table other than `expected_table`.
fn root_cause_of<'a>(findings: &'a [Finding], op_and_table: &str) -> Option<&'a Finding> {
    findings
        .iter()
        .find(|f| f.root_cause.contains(op_and_table))
}

// --- (1) minimal repro: two sequential repeat sub-loops, different vars/tables ---

/// One routine, TWO sequential `repeat` blocks: the first over `AlphaRec`
/// (table "G5 Alpha"), the second over `BetaRec` (table "G5 Beta"), each doing a
/// `Modify()` in-loop (d1 fires on both). Each finding's rootCause must name the
/// table the op is actually on — the SECOND finding must say "G5 Beta", never
/// the first sub-loop's "G5 Alpha".
#[test]
fn d1_names_correct_table_in_two_sequential_subloops() {
    let cu_src = r#"
codeunit 50170 "G5 Two Subloops"
{
    procedure ProcessBoth()
    var
        AlphaRec: Record "G5 Alpha";
        BetaRec: Record "G5 Beta";
    begin
        if AlphaRec.FindSet() then
            repeat
                AlphaRec.Modify();
            until AlphaRec.Next() = 0;

        if BetaRec.FindSet() then
            repeat
                BetaRec.Modify();
            until BetaRec.Next() = 0;
    end;
}
"#;
    let findings = run_d1(&[
        al("G5Alpha", ALPHA_TABLE_SRC),
        al("G5Beta", BETA_TABLE_SRC),
        al("G5TwoSubloops", cu_src),
    ]);
    let modify: Vec<_> = findings
        .iter()
        .filter(|f| f.root_cause.contains("Modify on "))
        .collect();
    assert_eq!(
        modify.len(),
        2,
        "exactly two in-loop Modify findings expected (one per sub-loop). findings: {findings:#?}"
    );
    assert!(
        root_cause_of(&findings, "Modify on G5 Alpha").is_some(),
        "first sub-loop's Modify must be reported on table 'G5 Alpha'. findings: {findings:#?}"
    );
    assert!(
        root_cause_of(&findings, "Modify on G5 Beta").is_some(),
        "second sub-loop's Modify must be reported on table 'G5 Beta' — NOT the \
         first sub-loop's 'G5 Alpha' (the G-5 mislabel). findings: {findings:#?}"
    );
}

// --- (2) escalated repro: three sub-loops, unloaded types, Insert cross-writes ---

/// The real-world G-5 shape (CDO CreateMergeTables): several sequential
/// `FindSet`/`repeat`/`Next` sub-loops, each reading one var and WRITING a
/// different var whose type is NOT loaded in the workspace (dependency table).
/// The unloaded write targets exercise describe_table tier 2
/// (`<declared type> (type not loaded)`) — each op must surface ITS OWN var's
/// declared type, not an earlier sub-loop's.
#[test]
fn d1_names_correct_table_across_three_subloops_with_unloaded_types() {
    let cu_src = r#"
codeunit 50171 "G5 Merge Tables"
{
    procedure CreateMergeTables()
    var
        AlphaRec: Record "G5 Alpha";
        BetaRec: Record "G5 Beta";
        GammaRec: Record "G5 Gamma";
        MergeTableTopBottom: Record "G5 Merge Top Bottom";
        HtmlTableStyle: Record "G5 Html Table Style";
        HtmlTableStyleLine: Record "G5 Html Table Style Line";
    begin
        if AlphaRec.FindSet() then
            repeat
                MergeTableTopBottom.Init();
                MergeTableTopBottom.Insert();
            until AlphaRec.Next() = 0;

        if BetaRec.FindSet() then
            repeat
                HtmlTableStyle.Init();
                HtmlTableStyle.Insert();
            until BetaRec.Next() = 0;

        if GammaRec.FindSet() then
            repeat
                HtmlTableStyleLine.Init();
                HtmlTableStyleLine.Insert();
            until GammaRec.Next() = 0;
    end;
}
"#;
    let findings = run_d1(&[
        al("G5Alpha", ALPHA_TABLE_SRC),
        al("G5Beta", BETA_TABLE_SRC),
        al("G5Gamma", GAMMA_TABLE_SRC),
        al("G5MergeTables", cu_src),
    ]);
    let inserts: Vec<_> = findings
        .iter()
        .filter(|f| f.root_cause.contains("Insert on "))
        .collect();
    assert_eq!(
        inserts.len(),
        3,
        "exactly three in-loop Insert findings expected (one per sub-loop). findings: {findings:#?}"
    );
    for (op_and_table, var) in [
        (
            "Insert on G5 Merge Top Bottom (type not loaded)",
            "MergeTableTopBottom",
        ),
        (
            "Insert on G5 Html Table Style (type not loaded)",
            "HtmlTableStyle",
        ),
        (
            "Insert on G5 Html Table Style Line (type not loaded)",
            "HtmlTableStyleLine",
        ),
    ] {
        assert!(
            root_cause_of(&findings, op_and_table).is_some(),
            "the Insert on `{var}` must be reported as '{op_and_table}' — never an \
             earlier sub-loop's table (the G-5 mislabel). findings: {findings:#?}"
        );
    }
}

// --- (3) THE G-5 root cause: tableextension object-number collision --------------

/// The ACTUAL G-5 shape (CDO batch 2/3): a `tableextension` whose OWN object
/// number equals a REAL table's number in the same app. Both were indexed as
/// `L3Table` under the SAME internal id `{appGuid}/table/{number}`, so the
/// last-wins `table_by_id` maps rendered ops on the REAL table with the
/// EXTENSION's name (e.g. real op on "CDO Merge Table Top/Bottom" reported as
/// "CDOReturnShipmentHeader"). Each sub-loop's finding must name the REAL table.
#[test]
fn d1_names_real_table_not_colliding_tableextension() {
    // Real table number 50180 — its name is what findings must report.
    let real_table_src = r#"
table 50180 "G5 Merge Top Bottom"
{
    fields
    {
        field(1; "Entry No."; Integer) { }
    }
    keys { key(PK; "Entry No.") { } }
}
"#;
    // A tableextension with the SAME object number 50180 (extends an unrelated
    // table). Its name must NEVER appear as a finding's table name.
    let ext_src = r#"
tableextension 50180 "G5ReturnShipmentExt" extends "G5 Alpha"
{
    fields
    {
        field(50100; "G5 Extra"; Code[10]) { }
    }
}
"#;
    let cu_src = r#"
codeunit 50180 "G5 Collision"
{
    procedure ProcessBoth()
    var
        AlphaRec: Record "G5 Alpha";
        MergeTopBottom: Record "G5 Merge Top Bottom";
    begin
        if AlphaRec.FindSet() then
            repeat
                AlphaRec.Modify();
            until AlphaRec.Next() = 0;

        if MergeTopBottom.FindSet() then
            repeat
                MergeTopBottom.Insert();
            until MergeTopBottom.Next() = 0;
    end;
}
"#;
    // The extension file sorts/assembles AFTER the table file, so the stub
    // clobbers the real table in a last-wins id map (the G-5 mislabel).
    let findings = run_d1(&[
        al("G5Alpha", ALPHA_TABLE_SRC),
        al("G5MergeTopBottom", real_table_src),
        al("G5ReturnShipmentExt", ext_src),
        al("G5Collision", cu_src),
    ]);
    assert!(
        root_cause_of(&findings, "Insert on G5 Merge Top Bottom").is_some(),
        "the Insert on `MergeTopBottom` must be reported on the REAL table \
         'G5 Merge Top Bottom' — never the number-colliding tableextension's \
         name 'G5ReturnShipmentExt' (the G-5 mislabel). findings: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|f| !f.root_cause.contains("G5ReturnShipmentExt")),
        "no finding may name the tableextension 'G5ReturnShipmentExt' as a \
         table. findings: {findings:#?}"
    );
    // The mislabel must not change finding PRESENCE: both sub-loops still fire.
    assert!(
        root_cause_of(&findings, "Modify on G5 Alpha").is_some(),
        "first sub-loop's Modify on 'G5 Alpha' must still fire. findings: {findings:#?}"
    );
}

/// Reversed assembly order: the REAL table file sorts AFTER the extension file.
/// Whichever side last-wins, the real table's name must be reported.
#[test]
fn d1_names_real_table_not_colliding_tableextension_reversed_order() {
    let real_table_src = r#"
table 50181 "G5 Html Table Style"
{
    fields
    {
        field(1; "Code"; Code[10]) { }
    }
    keys { key(PK; "Code") { } }
}
"#;
    let ext_src = r#"
tableextension 50181 "G5AJobExt" extends "G5 Alpha"
{
    fields
    {
        field(50100; "G5 Extra"; Code[10]) { }
    }
}
"#;
    let cu_src = r#"
codeunit 50181 "G5 Collision Rev"
{
    procedure ProcessStyles()
    var
        StyleRec: Record "G5 Html Table Style";
    begin
        if StyleRec.FindSet() then
            repeat
                StyleRec.Modify();
            until StyleRec.Next() = 0;
    end;
}
"#;
    // Extension file name sorts BEFORE the table file ("G5AJobExt" < "G5Html...").
    let findings = run_d1(&[
        al("G5Alpha", ALPHA_TABLE_SRC),
        al("G5AJobExt", ext_src),
        al("G5HtmlTableStyle", real_table_src),
        al("G5CollisionRev", cu_src),
    ]);
    assert!(
        root_cause_of(&findings, "Modify on G5 Html Table Style").is_some(),
        "the Modify on `StyleRec` must be reported on the REAL table 'G5 Html \
         Table Style' regardless of assembly order. findings: {findings:#?}"
    );
    assert!(
        findings.iter().all(|f| !f.root_cause.contains("G5AJobExt")),
        "no finding may name the tableextension 'G5AJobExt' as a table. \
         findings: {findings:#?}"
    );
}

// --- (4) transitive shape: in-loop calls to helpers, terminal op naming ---------

/// Sub-loop → helper-call shape: each sequential sub-loop calls a DIFFERENT local
/// helper whose only db op writes a DIFFERENT table. The interprocedural walk
/// recovers the terminal op from the callee — each finding must name the CALLEE
/// op's table, never another sub-loop's callee table.
#[test]
fn d1_names_correct_table_for_transitive_subloop_callees() {
    let cu_src = r#"
codeunit 50172 "G5 Transitive"
{
    procedure ProcessAll()
    var
        AlphaRec: Record "G5 Alpha";
        BetaRec: Record "G5 Beta";
    begin
        if AlphaRec.FindSet() then
            repeat
                WriteBeta();
            until AlphaRec.Next() = 0;

        if BetaRec.FindSet() then
            repeat
                WriteGamma();
            until BetaRec.Next() = 0;
    end;

    local procedure WriteBeta()
    var
        BetaOut: Record "G5 Beta";
    begin
        BetaOut.Insert();
    end;

    local procedure WriteGamma()
    var
        GammaOut: Record "G5 Gamma";
    begin
        GammaOut.Insert();
    end;
}
"#;
    let findings = run_d1(&[
        al("G5Alpha", ALPHA_TABLE_SRC),
        al("G5Beta", BETA_TABLE_SRC),
        al("G5Gamma", GAMMA_TABLE_SRC),
        al("G5Transitive", cu_src),
    ]);
    assert!(
        root_cause_of(&findings, "Insert on G5 Beta").is_some(),
        "first sub-loop's transitive Insert must be reported on 'G5 Beta'. findings: {findings:#?}"
    );
    assert!(
        root_cause_of(&findings, "Insert on G5 Gamma").is_some(),
        "second sub-loop's transitive Insert must be reported on 'G5 Gamma' — NOT \
         'G5 Beta' (the G-5 mislabel). findings: {findings:#?}"
    );
}
