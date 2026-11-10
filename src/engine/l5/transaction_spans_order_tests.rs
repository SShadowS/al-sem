//! Statement order in transaction spans: a `Commit()` ends the transaction it
//! closes and the next one starts after it. Each test builds a small inline AL
//! fixture through the real pipeline (parse, resolve, detector context) and
//! states its preconditions before the rule it pins.
//!
//! The rules (see `transaction_spans.rs`'s module doc, "Statement order"):
//! - a span's coverage reads only what may run before the Commit (a callee
//!   called after it is not in the transaction);
//! - a routine whose call toward the Commit follows a `Commit()` of its own
//!   closes the span there: its callers are not in it;
//! - a routine that commits only after (or beside) its call toward the Commit
//!   does not close the span: its callers' earlier writes are pending.

use std::collections::BTreeSet;

use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::detectors::d9::detect_d9;
use crate::engine::l5::registry::substrate;
use crate::engine::l5::transaction_spans::{SeedKind, TransactionSpan};
use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;
use crate::program::model::workspace::{Model, ModelRoutine};

const APP: &str = "55555555-0000-0000-0000-0000000d9001";

/// Five physical tables, `TO A` .. `TO E` (50100 .. 50104), and the
/// ambiguous-overload pair that makes a routine's coverage partial (as in
/// `detector_context_step5_tests.rs`'s fixture A).
const COMMON: &str = r#"table 50100 "TO A"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}
table 50101 "TO B"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}
table 50102 "TO C"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}
table 50103 "TO D"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}
table 50104 "TO E"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { Clustered = true; } }
}

interface "TO ILog"
{
    procedure Touch();
}

codeunit 50100 "TO Target"
{
    procedure O(X: Interface "TO ILog"): Integer
    begin
        exit(0);
    end;

    procedure O(X: Text): Integer
    begin
        exit(1);
    end;
}
"#;

fn build(work: &str) -> Model {
    let src = format!("{COMMON}\n{work}");
    assemble_and_resolve_inline_program_default(&[("src/TO.al".to_string(), src)], APP)
}

fn id_in(routines: &[ModelRoutine], name: &str) -> String {
    let ids: BTreeSet<&str> = routines
        .iter()
        .filter(|r| r.name == name)
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "precondition: one routine {name}, got {ids:?}"
    );
    ids.into_iter().next().unwrap().to_string()
}

/// The explicit-Commit spans of routine `rid`, in source order of their Commit.
fn spans_of<'s>(spans: &'s [TransactionSpan], m: &Model, rid: &str) -> Vec<&'s TransactionSpan> {
    let r = m
        .workspace
        .routines
        .iter()
        .find(|r| r.id == rid)
        .expect("routine");
    let line_of = |op: &str| {
        r.operation_sites
            .iter()
            .find(|o| o.id == op)
            .map(|o| (o.source_anchor.start_line, o.source_anchor.start_column))
            .expect("commit op")
    };
    let mut out: Vec<&TransactionSpan> = spans
        .iter()
        .filter(|s| s.seed_kind == SeedKind::ExplicitCommit && s.commit_routine_id == rid)
        .collect();
    out.sort_by_key(|s| line_of(&s.commit_operation_id));
    out
}

/// Table display names (`TO A` ..) of a span's table ids, sorted.
fn table_names(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = ids
        .iter()
        .map(|id| {
            let n: u32 = id
                .rsplit(|c: char| !c.is_ascii_digit())
                .find(|s| !s.is_empty())
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("a table number in {id}"));
            assert!((50100..=50104).contains(&n), "a fixture table: {id}");
            format!("TO {}", (b'A' + (n - 50100) as u8) as char)
        })
        .collect();
    out.sort();
    out
}

fn sorted(ids: &[&String]) -> Vec<String> {
    let mut v: Vec<String> = ids.iter().map(|s| (*s).clone()).collect();
    v.sort();
    v
}

/// `Root`: write A; Commit; `Helper()` (writes B, partial coverage). The
/// span ending at the Commit holds A only, does not hold `Helper`, and is
/// coverage-complete: `Helper`'s gap runs after the Commit. d9 therefore says
/// nothing (2 routines, 1 table, complete).
#[test]
fn a_callee_after_the_commit_is_not_in_the_span_it_closes() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Helper()
    var
        L: Codeunit "TO Target";
        T: Codeunit "TO Target";
        R: Record "TO B";
    begin
        T.O(L);
        R.Modify();
    end;

    procedure Root()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
        Helper();
    end;

    procedure Entry()
    begin
        Root();
    end;
}
"#,
    );
    let [helper, root, entry] =
        ["Helper", "Root", "Entry"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    // Preconditions: Helper's coverage is partial, so Root's whole cone is.
    let status = |id: &str| ctx.summaries[id].inherited_status().to_string();
    assert_eq!(
        status(&helper),
        "partial",
        "precondition: Helper is partial"
    );
    assert_eq!(
        status(&root),
        "partial",
        "precondition: Root's cone is partial"
    );
    assert_eq!(
        status(&entry),
        "partial",
        "precondition: Entry's cone is partial"
    );

    let spans = spans_of(&ctx.transaction_spans, &m, &root);
    assert_eq!(spans.len(), 1, "precondition: one Commit in Root");
    let s = spans[0];
    assert_eq!(s.routines_in_span, sorted(&[&entry, &root]));
    assert!(
        !s.routines_in_span.contains(&helper),
        "Helper runs after the Commit"
    );
    assert_eq!(table_names(&s.pending_physical_tables), ["TO A"]);
    assert!(
        s.coverage_complete,
        "only Helper is partial, and it runs after the Commit"
    );
    let d9 = detect_d9(&m, &ctx).expect("d9 runs");
    assert!(
        d9.findings.is_empty(),
        "1 pending table and complete coverage: no d9 finding: {:?}",
        d9.findings
            .iter()
            .map(|f| &f.root_cause)
            .collect::<Vec<_>>()
    );
}

/// A gap BEFORE the Commit still makes the span incomplete (the guard above
/// must not drop it): `Root`: `Helper()`; write A; Commit.
#[test]
fn a_callee_before_the_commit_keeps_its_coverage_gap() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Helper()
    var
        L: Codeunit "TO Target";
        T: Codeunit "TO Target";
    begin
        T.O(L);
    end;

    procedure Root()
    var
        R: Record "TO A";
    begin
        Helper();
        R.Modify();
        Commit();
    end;

    procedure Entry()
    begin
        Root();
    end;
}
"#,
    );
    let root = id_in(&m.workspace.routines, "Root");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &root);
    assert_eq!(spans.len(), 1);
    assert!(
        !spans[0].coverage_complete,
        "Helper's gap runs before the Commit"
    );
}

/// `Twice`: write A; Commit (c1); `X()` (writes B); Commit (c2). `Caller`:
/// write C; `Twice()`. The span ending at c1 holds `Caller` and `Twice` with
/// A and C pending. The span ending at c2 starts after c1: it holds `Twice`
/// alone, with only B pending.
#[test]
fn a_second_commit_starts_a_new_span_after_the_first() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure X()
    var
        R: Record "TO B";
    begin
        R.Modify();
    end;

    procedure Twice()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
        X();
        Commit();
    end;

    procedure Caller()
    var
        R: Record "TO C";
    begin
        R.Modify();
        Twice();
    end;
}
"#,
    );
    let [x, twice, caller] = ["X", "Twice", "Caller"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &twice);
    assert_eq!(spans.len(), 2, "precondition: two Commits in Twice");
    let (c1, c2) = (spans[0], spans[1]);
    assert_eq!(c1.routines_in_span, sorted(&[&caller, &twice]));
    assert_eq!(table_names(&c1.pending_physical_tables), ["TO A", "TO C"]);
    assert!(!c1.routines_in_span.contains(&x));
    assert_eq!(
        c2.routines_in_span,
        vec![twice.clone()],
        "Caller's call reaches c2 only through c1: it is not in c2's span"
    );
    assert_eq!(table_names(&c2.pending_physical_tables), ["TO B"]);
    assert_eq!(
        c2.span_roots,
        vec![twice.clone()],
        "the span starts in Twice, after c1"
    );
}

/// `Inner`: write A; Commit. `Mid`: `Inner()`; Commit. `Top`: write T;
/// `Mid()`. `Mid` commits only AFTER its call to `Inner`, so `Top`'s write is
/// pending at `Inner`'s Commit: `Top` is in that span.
#[test]
fn a_caller_that_commits_after_its_call_does_not_close_the_span() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Inner()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;

    procedure Mid()
    begin
        Inner();
        Commit();
    end;

    procedure Top()
    var
        R: Record "TO D";
    begin
        R.Modify();
        Mid();
    end;
}
"#,
    );
    let [inner, mid, top] = ["Inner", "Mid", "Top"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &inner);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].routines_in_span, sorted(&[&inner, &mid, &top]));
    assert_eq!(
        table_names(&spans[0].pending_physical_tables),
        ["TO A", "TO D"]
    );
    assert_eq!(spans[0].span_roots, vec![top.clone()]);
}

/// `Inner2`: write A; Commit. `Mid2`: write E; Commit; `Inner2()`. `Top2`:
/// write D; `Mid2()`. `Mid2`'s Commit runs before its call to `Inner2`, so
/// the span ending at `Inner2`'s Commit stops at `Mid2`: `Top2` and its write
/// are not in it, and neither is `Mid2`'s write E.
#[test]
fn a_caller_that_commits_before_its_call_closes_the_span() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Inner2()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;

    procedure Mid2()
    var
        R: Record "TO E";
    begin
        R.Modify();
        Commit();
        Inner2();
    end;

    procedure Top2()
    var
        R: Record "TO D";
    begin
        R.Modify();
        Mid2();
    end;
}
"#,
    );
    let [inner, mid, top] = ["Inner2", "Mid2", "Top2"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &inner);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].routines_in_span, sorted(&[&inner, &mid]));
    assert!(!spans[0].routines_in_span.contains(&top));
    assert_eq!(table_names(&spans[0].pending_physical_tables), ["TO A"]);
}

/// A Commit inside a branch does not close the span: `Mid3`: `if B then
/// Commit;` `Inner3()`. `Top3`'s write may be pending at `Inner3`'s Commit
/// (when `B` is false), so `Top3` is in the span.
#[test]
fn a_conditional_commit_before_the_call_does_not_close_the_span() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Inner3()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;

    procedure Mid3(B: Boolean)
    begin
        if B then
            Commit();
        Inner3();
    end;

    procedure Top3(B: Boolean)
    var
        R: Record "TO D";
    begin
        R.Modify();
        Mid3(B);
    end;
}
"#,
    );
    let [inner, mid, top] = ["Inner3", "Mid3", "Top3"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &inner);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].routines_in_span, sorted(&[&inner, &mid, &top]));
    assert_eq!(
        table_names(&spans[0].pending_physical_tables),
        ["TO A", "TO D"]
    );
}

/// A caller reached FIRST through a call a Commit of its own precedes, and
/// only later (deeper in the walk) through one no Commit precedes, still has
/// its callers walked. `C`: `M1()`; Commit; `Inner4()`. The direct call to
/// `Inner4` (depth 1) follows `C`'s Commit; the call to `M1` (depth 3, through
/// `M1` -> `M2` -> `Inner4`) does not. `Top4`: write D; `C()`: D is pending at
/// `Inner4`'s Commit.
#[test]
fn a_caller_opened_late_in_the_walk_still_has_its_callers_walked() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure Inner4()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;

    procedure M2()
    begin
        Inner4();
    end;

    procedure M1()
    begin
        M2();
    end;

    procedure C()
    begin
        M1();
        Commit();
        Inner4();
    end;

    procedure Top4()
    var
        R: Record "TO D";
    begin
        R.Modify();
        C();
    end;
}
"#,
    );
    let [inner, m2, m1, c, top] =
        ["Inner4", "M2", "M1", "C", "Top4"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &inner);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        spans[0].routines_in_span,
        sorted(&[&inner, &m2, &m1, &c, &top])
    );
    assert_eq!(
        table_names(&spans[0].pending_physical_tables),
        ["TO A", "TO D"]
    );
}

// ---------------------------------------------------------------------------
// Engine limits B2 (per-arm ids): a span member declared in two `#if` arms is
// two rows of one id, each holding its own call sites. Every walk reads the
// row (the arm) that holds the site.
// ---------------------------------------------------------------------------

/// The rows of `name`: asserts they are two rows of one id, each with its
/// own first call site.
fn two_arms<'m>(m: &'m Model, name: &str) -> (String, Vec<&'m ModelRoutine>) {
    let rows: Vec<&ModelRoutine> = m
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == name)
        .collect();
    assert_eq!(rows.len(), 2, "precondition: {name} is two rows");
    assert_eq!(rows[0].id, rows[1].id, "precondition: one id");
    assert_ne!(
        rows[0].call_sites[0].id, rows[1].call_sites[0].id,
        "precondition: each arm has its own call site"
    );
    (rows[0].id.clone(), rows)
}

/// `Work`'s arms each write a table, then call `DoCommit`: arm A writes
/// `TO A`, arm B `TO B`. Whichever arm a build compiles, its write is pending
/// at the Commit, so the span holds both. Break: read only the id's last row
/// (`rows_before` over `[last]`) -> arm A's call is not in it, the walk falls
/// back to arm B's whole body, and `TO A` is lost.
#[test]
fn a_two_arm_member_has_every_arms_writes_pending() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure DoCommit()
    begin
        Commit();
    end;

#if A
    procedure Work()
    var
        R: Record "TO A";
    begin
        R.Modify();
        DoCommit();
    end;
#else
    procedure Work()
    var
        R: Record "TO B";
    begin
        R.Modify();
        DoCommit();
    end;
#endif

    procedure Entry()
    begin
        Work();
    end;
}
"#,
    );
    two_arms(&m, "Work");
    let commit = id_in(&m.workspace.routines, "DoCommit");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &commit);
    assert_eq!(spans.len(), 1, "precondition: one Commit");
    assert_eq!(
        table_names(&spans[0].pending_physical_tables),
        ["TO A", "TO B"],
        "both arms' writes are pending at the Commit"
    );
}

/// `Work`'s arm B commits before its call to `DoCommit`, arm A does not. In a
/// build of arm A, `Entry`'s write is pending at `DoCommit`'s Commit, so
/// `Entry` is in the span and `TO C` pending. Break: `closes_span` reads the
/// id's last row (arm B) for arm A's call -> the call is not in it, the order
/// is unknown, and the span closes at `Work`.
#[test]
fn a_two_arm_caller_closes_the_span_only_in_the_arm_that_commits() {
    let m = build(
        r#"codeunit 50101 "TO Work"
{
    procedure DoCommit()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;

#if A
    procedure Work()
    begin
        DoCommit();
    end;
#else
    procedure Work()
    begin
        Commit();
        DoCommit();
    end;
#endif

    procedure Entry()
    var
        R: Record "TO C";
    begin
        R.Modify();
        Work();
    end;
}
"#,
    );
    let (work, _) = two_arms(&m, "Work");
    let [commit, entry] = ["DoCommit", "Entry"].map(|n| id_in(&m.workspace.routines, n));
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &commit);
    assert_eq!(spans.len(), 1, "precondition: one Commit in DoCommit");
    assert_eq!(
        spans[0].routines_in_span,
        sorted(&[&commit, &entry, &work]),
        "arm A keeps the transaction open up to Entry"
    );
    assert_eq!(
        table_names(&spans[0].pending_physical_tables),
        ["TO A", "TO C"]
    );
}

/// d9 places a span at the row (the arm) that holds its Commit: `Work`'s
/// Commit is in arm A, so the finding's location is arm A's declaration, not
/// arm B's (the id's last row). Break: d9 reads `routine_by_id` -> arm B's
/// line.
#[test]
fn d9_locates_a_two_arm_commit_in_its_own_arm() {
    let src = r#"codeunit 50101 "TO Work"
{
    local procedure Touch()
    begin
    end;

#if A
    procedure Work()
    var
        R: Record "TO A";
        S: Record "TO B";
    begin
        R.Modify();
        S.Modify();
        Touch();
        Commit();
    end;
#else
    procedure Work()
    var
        R: Record "TO A";
    begin
        Touch();
        R.Modify();
    end;
#endif

    procedure Entry()
    begin
        Work();
    end;
}
"#;
    let m = build(src);
    let (work, rows) = two_arms(&m, "Work");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let d9 = detect_d9(&m, &ctx).expect("d9 runs");
    let mine: Vec<u32> = d9
        .findings
        .iter()
        .filter(|f| f.evidence_path[0].routine_id == work)
        .map(|f| f.primary_location.start_line)
        .collect();
    assert_eq!(
        mine,
        vec![rows[0].source_anchor.start_line],
        "the finding sits at arm A's declaration (arm B: line {})",
        rows[1].source_anchor.start_line
    );
}

/// `Work`'s arms each reach the committing `OnRun` only through a CHECKED
/// `Codeunit.Run` (its own transaction), so `Work` is not in that span. The
/// backward walk reads each call in the arm that holds it. Break: it looks
/// the call up in the id's last row -> arm A's call is not found, is not
/// seen as a checked run, and `Work` joins the span.
#[test]
fn a_two_arm_checked_run_is_read_in_its_own_arm() {
    let m = build(
        r#"codeunit 50102 "TO Runner"
{
    trigger OnRun()
    var
        R: Record "TO A";
    begin
        R.Modify();
        Commit();
    end;
}

codeunit 50101 "TO Work"
{
    local procedure Touch()
    begin
    end;

#if A
    procedure Work()
    begin
        if Codeunit.Run(Codeunit::"TO Runner") then;
    end;
#else
    procedure Work()
    begin
        Touch();
        if Codeunit.Run(Codeunit::"TO Runner") then;
    end;
#endif
}
"#,
    );
    let (work, _) = two_arms(&m, "Work");
    let run = id_in(&m.workspace.routines, "OnRun");
    let owned = build_detector_context(&m, substrate::ALL);
    let ctx = owned.ctx();
    let spans = spans_of(&ctx.transaction_spans, &m, &run);
    assert_eq!(spans.len(), 1, "precondition: one Commit in OnRun");
    assert!(
        !spans[0].routines_in_span.contains(&work),
        "a checked run is its own transaction: {:?}",
        spans[0].routines_in_span
    );
}
