//! Engine limits B2 (per-arm ids): a routine declared in two `#if` arms is
//! two rows of one routine id, and each arm numbers its own sites (arm A's
//! `{id}/cs0` is in arm A's row only). These tests pin the detector readers
//! that take a row by routine id and then look a SITE up in it: each must read
//! the row (the arm) that holds the site, so it neither drops the other arm's
//! facts nor cites the wrong arm's line.
//!
//! Every fixture is built through the production pipeline, and each test
//! states its precondition (two rows, one id, each arm its own sites) first.

use super::*;
use crate::engine::l5::finding::{EvidenceStep, Finding};
use crate::engine::l5::registry::substrate;
use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

const APP: &str = "22222222-0000-0000-0000-0000000b2a00";

fn build(src: &str) -> Model {
    assemble_and_resolve_inline_program_default(&[("src/T.al".to_string(), src.to_string())], APP)
}

/// The rows of `name`: asserts the B2 precondition (two rows, one id, no
/// shared site id) and returns them.
fn two_arms<'m>(m: &'m Model, name: &str) -> Vec<&'m ModelRoutine> {
    let rows: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rows.len(), 2, "precondition: {name} is two rows");
    assert_eq!(rows[0].id, rows[1].id, "precondition: one id");
    let ids = |r: &ModelRoutine| -> Vec<String> {
        r.call_sites
            .iter()
            .map(|c| c.id.clone())
            .chain(r.operation_sites.iter().map(|o| o.id.clone()))
            .collect()
    };
    let a = ids(rows[0]);
    assert!(
        ids(rows[1]).iter().all(|s| !a.contains(s)),
        "precondition: each arm has its own site ids"
    );
    rows
}

/// The 0-based line of site `site` in the row that holds it.
fn site_line(m: &Model, site: &str) -> Option<u32> {
    m.workspace.routines.iter().find_map(|r| {
        r.call_sites
            .iter()
            .find(|c| c.id == site)
            .map(|c| c.source_anchor.start_line)
            .or_else(|| {
                r.operation_sites
                    .iter()
                    .find(|o| o.id == site)
                    .map(|o| o.source_anchor.start_line)
            })
            .or_else(|| {
                r.record_operations
                    .iter()
                    .find(|o| o.id == site)
                    .map(|o| o.source_anchor.start_line)
            })
    })
}

/// Every evidence step of `findings` that names a site of a routine with
/// several rows cites that site's own line.
fn assert_steps_cite_their_sites(m: &Model, findings: &[Finding], what: &str) {
    let multi = |id: &str| m.workspace.routines.iter().filter(|r| r.id == id).count() > 1;
    // Every path: the evidence path and the other paths, or (d1) each
    // cohort's witness.
    let steps: Vec<EvidenceStep> = findings
        .iter()
        .flat_map(|f| {
            let mut v: Vec<EvidenceStep> = f.evidence_path.clone();
            v.extend(f.additional_paths.iter().flatten().flatten().cloned());
            for c in f.cohort_contexts.iter().flatten() {
                v.extend(crate::engine::l5::d1_witness::flatten_witness(&c.witness));
            }
            v
        })
        .collect();
    let mut checked = 0;
    for s in &steps {
        let EvidenceStep {
            routine_id,
            operation_id,
            callsite_id,
            source_anchor,
            ..
        } = s;
        let Some(site) = callsite_id.as_deref().or(operation_id.as_deref()) else {
            continue;
        };
        if !multi(routine_id) {
            continue;
        }
        let Some(want) = site_line(m, site) else {
            continue;
        };
        assert_eq!(
            source_anchor.start_line, want,
            "{what}: the step at {site} cites its own line"
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "precondition: {what} has a step in a two-arm routine"
    );
}

const TABLES: &str = r#"
table 50340 "AR A"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } }
    keys { key(PK; "No.") { } }
}

table 50341 "AR B"
{
    fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } }
    keys { key(PK; "No.") { } }
}

table 50342 "AR C"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}
"#;

/// d1: `Loop` (two arms) loops over `Write` (two arms: arm A inserts `AR A`,
/// arm B modifies `AR B`). Each arm of `Write` holds its own operation, so
/// both are terminals of the loop's chain, and every hop and terminal step
/// cites its own site's line. Breaks: d1's graph takes the terminals of the
/// id's last row only (`d1_graph`) -> arm A's insert is no finding;
/// `hop_step`/`terminal_step` read the id's last row -> arm A's call step
/// cites arm B's declaration.
#[test]
fn d1_reads_every_arm_and_cites_each_site() {
    let src = format!(
        "{TABLES}
codeunit 50340 \"AR Work\"
{{
    local procedure Touch()
    begin
    end;

#if A
    procedure Write()
    var
        RA: Record \"AR A\";
    begin
        RA.Insert();
    end;
#else
    procedure Write()
    var
        RB: Record \"AR B\";
    begin
        Touch();
        RB.Modify();
    end;
#endif

#if A
    procedure Mid()
    begin
        Write();
    end;
#else
    procedure Mid()
    begin
        Touch();
        Write();
    end;
#endif

#if A
    procedure Loop()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            Mid();
    end;
#else
    procedure Loop()
    var
        I: Integer;
    begin
        Touch();
        for I := 1 to 3 do
            Mid();
    end;
#endif
}}
"
    );
    let m = build(&src);
    let write = two_arms(&m, "Write");
    two_arms(&m, "Mid");
    two_arms(&m, "Loop");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d1 = crate::engine::l5::detectors::d1::detect_d1(&m, &ctx).expect("d1 runs");
    // A d1 finding's id ends with its terminal operation.
    let ids: Vec<&str> = d1.findings.iter().map(|f| f.id.as_str()).collect();
    for r in &write {
        let op = &r.record_operations[0].id;
        assert!(
            ids.iter().any(|id| id.ends_with(op.as_str())),
            "each arm's write ({op}) is a terminal: {ids:?}"
        );
    }
    assert_steps_cite_their_sites(&m, &d1.findings, "d1");
}

/// d8: `PostIt` (a transaction-managing name) calls `DoCommit`, declared in
/// two arms that each commit. Each span's commit step cites its own arm's
/// Commit. Break: d8 reads the id's last row -> arm A's Commit step cites the
/// routine's declaration.
#[test]
fn d8_cites_each_arms_commit() {
    let src = format!(
        "{TABLES}
codeunit 50341 \"AR Post\"
{{
    local procedure Touch()
    begin
    end;

#if A
    procedure DoCommit()
    begin
        Commit();
    end;
#else
    procedure DoCommit()
    begin
        Touch();
        Commit();
    end;
#endif

    procedure PostIt()
    var
        A: Record \"AR A\";
        B: Record \"AR B\";
        C: Record \"AR C\";
    begin
        A.Insert();
        B.Insert();
        C.Insert();
        DoCommit();
    end;
}}
"
    );
    let m = build(&src);
    two_arms(&m, "DoCommit");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d8 = crate::engine::l5::detectors::d8::detect_d8(&m, &ctx).expect("d8 runs");
    assert_eq!(d8.findings.len(), 2, "one finding per Commit");
    assert_steps_cite_their_sites(&m, &d8.findings, "d8");
}

/// d32: `P(Flag)` is local, and its only caller (two arms) passes `true` in
/// both. Each arm's call is read in its own row. Break: d32 looks the call up
/// in the id's last row -> arm A's call is not found, d32 bails, no finding.
#[test]
fn d32_reads_each_arms_call() {
    let src = r#"
codeunit 50342 "AR Flag"
{
    local procedure Touch()
    begin
    end;

    local procedure P(Flag: Boolean)
    begin
        if Flag then
            Touch();
    end;

#if A
    procedure Caller()
    begin
        P(true);
    end;
#else
    procedure Caller()
    begin
        Touch();
        P(true);
    end;
#endif
}
"#;
    let m = build(src);
    two_arms(&m, "Caller");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d32 = crate::engine::l5::detectors::d32::detect_d32(&m, &ctx).expect("d32 runs");
    assert_eq!(
        d32.findings.len(),
        1,
        "every call passes true: {:?}",
        d32.findings
            .iter()
            .map(|f| &f.root_cause)
            .collect::<Vec<_>>()
    );
}

/// d39: `Caller` (two arms) loads a record and hands it to a helper that
/// validates and exits dirty; neither arm persists it after. Each arm's call
/// is its own finding. Break: d39 looks the call up in the id's last row ->
/// arm A's call is dropped.
#[test]
fn d39_reads_each_arms_call() {
    let src = format!(
        "{TABLES}
codeunit 50343 \"AR Dirty\"
{{
    local procedure Touch()
    begin
    end;

#if A
    procedure Caller()
    var
        R: Record \"AR A\";
    begin
        R.Get('C0001');
        ValidatesAndExits(R);
    end;
#else
    procedure Caller()
    var
        R: Record \"AR A\";
    begin
        Touch();
        R.Get('C0001');
        ValidatesAndExits(R);
    end;
#endif

    local procedure ValidatesAndExits(var Rec: Record \"AR A\")
    begin
        Rec.Validate(Name, 'X');
        exit;
    end;
}}
"
    );
    let m = build(&src);
    let rows = two_arms(&m, "Caller");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d39 = crate::engine::l5::detectors::d39::detect_d39(&m, &ctx).expect("d39 runs");
    let sites: std::collections::BTreeSet<&str> = d39
        .findings
        .iter()
        .flat_map(|f| f.evidence_path.iter())
        .filter_map(|s| s.callsite_id.as_deref())
        .collect();
    for r in &rows {
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text.contains("ValidatesAndExits"))
            .expect("precondition: the arm's call");
        assert!(
            sites.contains(cs.id.as_str()),
            "arm call {} is a finding: {sites:?}",
            cs.id
        );
    }
}

/// d43: `DoPost` (two arms) raises `OnBeforePost(Rec, IsHandled)` and guards
/// its insert on `IsHandled`; a subscriber sets it without writing. Each
/// arm's dispatch site is read in its own row. Break: d43 looks the call up in
/// the id's last row -> arm A's site is dropped.
#[test]
fn d43_reads_each_arms_dispatch_site() {
    let src = r#"
table 50344 "AR Entry"
{
    fields { field(1; "Entry No."; Integer) { } }
    keys { key(PK; "Entry No.") { Clustered = true; } }
}

codeunit 50344 "AR PostMgr"
{
    local procedure Touch()
    begin
    end;

#if A
    procedure DoPost(var Rec: Record "AR Entry")
    var
        IsHandled: Boolean;
    begin
        IsHandled := false;
        OnBeforePost(Rec, IsHandled);
        if not IsHandled then
            Rec.Insert(true);
    end;
#else
    procedure DoPost(var Rec: Record "AR Entry")
    var
        IsHandled: Boolean;
    begin
        Touch();
        IsHandled := false;
        OnBeforePost(Rec, IsHandled);
        if not IsHandled then
            Rec.Insert(true);
    end;
#endif

    [IntegrationEvent(false, false)]
    procedure OnBeforePost(var Rec: Record "AR Entry"; var IsHandled: Boolean)
    begin
    end;
}

codeunit 50345 "AR Handler"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"AR PostMgr", 'OnBeforePost', '', false, false)]
    local procedure OnBeforePostHandler(var Rec: Record "AR Entry"; var IsHandled: Boolean)
    begin
        IsHandled := true;
    end;
}
"#;
    let m = build(src);
    let rows = two_arms(&m, "DoPost");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d43 = crate::engine::l5::detectors::d43::detect_d43(&m, &ctx).expect("d43 runs");
    let text = format!("{:?}", d43.findings);
    for r in &rows {
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text.contains("OnBeforePost"))
            .expect("precondition: the arm's raise");
        assert!(
            text.contains(cs.id.as_str()),
            "arm site {} is in a finding: {text}",
            cs.id
        );
    }
}

/// The operation ids the findings' evidence ends at, every path.
fn terminal_ops(findings: &[Finding]) -> std::collections::BTreeSet<String> {
    findings
        .iter()
        .flat_map(|f| {
            std::iter::once(&f.evidence_path)
                .chain(f.additional_paths.iter().flatten())
                .filter_map(|p| p.last()?.operation_id.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// d2: a loop raises an event whose subscriber calls `Mid` (two arms), which
/// calls `Write` (two arms: arm A inserts `AR A`, arm B modifies `AR B`).
/// Arm A's operation is a terminal, and each step cites its own site. Breaks:
/// `terminals_at` reads the id's last row -> arm A's insert is no terminal;
/// `build_hop_step` reads the last row -> `Mid`'s arm A call cites arm B's
/// declaration; `build_terminal_step` reads the last row -> arm A's terminal
/// cites line 0.
#[test]
fn d2_reads_every_subscriber_arm() {
    let src = format!(
        "{TABLES}
codeunit 50346 \"AR Pub\"
{{
    procedure Loop()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            OnEvt();
    end;

    [IntegrationEvent(false, false)]
    procedure OnEvt()
    begin
    end;
}}

codeunit 50347 \"AR Sub\"
{{
    local procedure Touch()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"AR Pub\", 'OnEvt', '', false, false)]
    local procedure Handle()
    begin
        Mid();
    end;

#if A
    local procedure Mid()
    begin
        Write();
    end;
#else
    local procedure Mid()
    begin
        Touch();
        Write();
    end;
#endif

#if A
    local procedure Write()
    var
        RA: Record \"AR A\";
    begin
        RA.Insert();
    end;
#else
    local procedure Write()
    var
        RB: Record \"AR B\";
    begin
        Touch();
        RB.Modify();
    end;
#endif
}}
"
    );
    let m = build(&src);
    two_arms(&m, "Mid");
    let rows = two_arms(&m, "Write");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d2 = crate::engine::l5::detectors::d2::detect_d2(&m, &ctx).expect("d2 runs");
    // d2 names one witness per subscriber: the first terminal its walk
    // finds, arm A's insert (the first row's).
    let ops = terminal_ops(&d2.findings);
    let op = &rows[0].record_operations[0].id;
    assert!(ops.contains(op), "arm A's op {op} is a terminal: {ops:?}");
    assert_steps_cite_their_sites(&m, &d2.findings, "d2");
}

/// d46: an Install trigger calls `Save`, declared in two arms that each
/// commit. Both arms' Commits are terminals, each citing its own line. Break:
/// the commit sites of an id are its last row's -> arm A's Commit is no
/// terminal.
#[test]
fn d46_reads_every_arms_commit() {
    let src = r#"
codeunit 50348 "AR Install"
{
    Subtype = Install;

    trigger OnInstallAppPerCompany()
    begin
        Mid();
    end;

    local procedure Touch()
    begin
    end;

#if A
    local procedure Mid()
    begin
        Save();
    end;
#else
    local procedure Mid()
    begin
        Touch();
        Save();
    end;
#endif

#if A
    procedure Save()
    begin
        Commit();
    end;
#else
    procedure Save()
    begin
        Touch();
        Commit();
    end;
#endif
}
"#;
    let m = build(src);
    two_arms(&m, "Mid");
    let rows = two_arms(&m, "Save");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d46 = crate::engine::l5::detectors::d46::detect_d46(&m, &ctx).expect("d46 runs");
    let ops = terminal_ops(&d46.findings);
    for r in &rows {
        let op = &r.operation_sites[0].id;
        assert!(ops.contains(op), "arm Commit {op} is a terminal: {ops:?}");
    }
    assert_steps_cite_their_sites(&m, &d46.findings, "d46");
}

/// d48: a loop calls `Send`, declared in two arms that each make an HTTP
/// call. Both arms' calls are terminals, each anchored on its own line. Break:
/// a collided row's terminals take every fact of the id (the other arm's IO
/// fact falls back to the routine's declaration) and the last row wins.
#[test]
fn d48_reads_every_arms_io() {
    let src = r#"
codeunit 50349 "AR Io Loop"
{
    local procedure Touch()
    begin
    end;

#if A
    procedure Send()
    var
        Client: HttpClient;
        Req: HttpRequestMessage;
        Resp: HttpResponseMessage;
    begin
        Client.Send(Req, Resp);
    end;
#else
    procedure Send()
    var
        Client: HttpClient;
        Req: HttpRequestMessage;
        Resp: HttpResponseMessage;
    begin
        Touch();
        Client.Send(Req, Resp);
    end;
#endif

#if A
    local procedure Mid()
    begin
        Send();
    end;
#else
    local procedure Mid()
    begin
        Touch();
        Send();
    end;
#endif

    procedure Loop()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            Mid();
    end;
}
"#;
    let m = build(src);
    two_arms(&m, "Mid");
    let rows = two_arms(&m, "Send");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d48 = crate::engine::l5::detectors::d48::detect_d48(&m, &ctx).expect("d48 runs");
    let text = format!("{:?}", d48.findings);
    for r in &rows {
        let cs = r
            .call_sites
            .iter()
            .find(|c| c.callee_text.contains("Send"))
            .expect("precondition: the arm's HTTP call");
        let line = cs.source_anchor.start_line;
        assert!(
            d48.findings
                .iter()
                .any(|f| f.primary_location.start_line == line
                    || f.evidence_path
                        .iter()
                        .any(|s| s.source_anchor.start_line == line)),
            "arm call {} (line {line}) is a terminal: {text}",
            cs.id
        );
    }
    assert_steps_cite_their_sites(&m, &d48.findings, "d48");
}

/// d1's temp verdict along a path: `Mid`'s arm A passes a TEMPORARY record to
/// `Write(var R)`, which inserts it (arm B, the id's last row, calls nothing);
/// a loop calls `Mid`. Arm A's call binds its temporary record, so the path's
/// insert is temporary.
/// Breaks: the forward reach (`d1_reach`) or the parameter liveness
/// (`d1_liveness`) reads the call in the id's last row -> arm A's call has no
/// binding there, and its path's verdict is not temporary.
#[test]
fn d1_temp_reads_each_arms_binding() {
    let src = format!(
        "{TABLES}
codeunit 50350 \"AR Temp\"
{{
    local procedure Touch()
    begin
    end;

    local procedure Write(var R: Record \"AR A\")
    begin
        R.Insert();
    end;

#if A
    local procedure Mid()
    var
        TA: Record \"AR A\" temporary;
    begin
        Write(TA);
    end;
#else
    local procedure Mid()
    begin
        Touch();
    end;
#endif

    procedure Loop()
    var
        I: Integer;
    begin
        for I := 1 to 3 do
            Mid();
    end;
}}
"
    );
    let m = build(&src);
    two_arms(&m, "Mid");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d1 = crate::engine::l5::detectors::d1::detect_d1(&m, &ctx).expect("d1 runs");
    assert_all_temporary(&d1.findings);
}

/// Impact's effect evidence (`explain::effect_evidence`) joins an operation to
/// the fields its owning routine assigns: for an operation of `Work`'s arm B
/// (the SECOND row), the row is arm B's, whose `Name` assignment is the one
/// listed. Break: `row_holding` takes the routine's first row -> arm A, which
/// assigns no field on that table. (A helper-level pin: the impact command's
/// own answer needs a git base; the call site passes `row_holding` through
/// unchanged.)
#[test]
fn effect_evidence_reads_the_arm_that_holds_the_op() {
    let src = format!(
        "{TABLES}
codeunit 50352 \"AR Assign\"
{{
    local procedure Touch()
    begin
    end;

#if A
    procedure Work()
    var
        A: Record \"AR A\";
    begin
        Touch();
        A.Insert();
    end;
#else
    procedure Work()
    var
        B: Record \"AR B\";
    begin
        B.Name := 'y';
        B.Insert();
    end;
#endif
}}
"
    );
    let m = build(&src);
    let rows = two_arms(&m, "Work");
    let index = crate::engine::l5::explain::EffectEvidenceIndex::new(&m);
    let b_op = &rows[1].record_operations[0];
    let row = index
        .row_holding(&rows[1].stable_routine_id, &b_op.id)
        .expect("a row");
    assert!(
        std::ptr::eq(&m.workspace.routines[row], rows[1]),
        "arm B's op is read in arm B's row"
    );
    let table = b_op.table_id.as_deref().expect("precondition: a table");
    assert!(
        index
            .assigned(row, table)
            .is_some_and(|f| f.iter().any(|x| x.eq_ignore_ascii_case("name"))),
        "arm B assigns Name"
    );
}

fn assert_all_temporary(findings: &[Finding]) {
    assert!(!findings.is_empty(), "precondition: a d1 finding");
    for f in findings {
        assert!(
            f.root_cause.contains("temporary record") && f.severity == "info",
            "every path's insert is temporary: {} ({})",
            f.root_cause,
            f.severity
        );
    }
}

/// As [`d1_temp_reads_each_arms_binding`], with `Mid`'s arm A FORWARDING its
/// own `var` parameter to `Write`, and the loop passing a temporary record to
/// `Mid`: the liveness fixpoint must see arm A's forward to carry the record's
/// state. Break: the fixpoint reads the call in the id's last row -> arm A's
/// forward is not seen, the parameter is not carried, and the path's verdict
/// is not temporary (or the transfer table misses its slot).
#[test]
fn d1_temp_follows_an_arms_forwarded_parameter() {
    let src = format!(
        "{TABLES}
codeunit 50351 \"AR Fwd\"
{{
    local procedure Touch()
    begin
    end;

    local procedure Write(var R: Record \"AR A\")
    begin
        R.Insert();
    end;

#if A
    local procedure Mid(var P: Record \"AR A\")
    begin
        Write(P);
    end;
#else
    local procedure Mid(var P: Record \"AR A\")
    begin
        Touch();
    end;
#endif

    procedure Loop()
    var
        I: Integer;
        T: Record \"AR A\" temporary;
    begin
        for I := 1 to 3 do
            Mid(T);
    end;
}}
"
    );
    let m = build(&src);
    two_arms(&m, "Mid");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d1 = crate::engine::l5::detectors::d1::detect_d1(&m, &ctx).expect("d1 runs");
    assert_all_temporary(&d1.findings);
}
