//! Stage 5's cone store (design §6.10, §11): hand-stated counterexamples,
//! each a resident session over a temporary single-app workspace edited step
//! by step. After every step every served answer equals a cold standalone run
//! over a fresh read of the edited disk, and the revision's fold (coverage,
//! derived rows, raw facts) equals a cold composition over the same model.
//! Each test also states, as a precondition, that the step's fold re-folded
//! only part of the program (so it exercises the store, not a full fold).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use super::incr_tests::{assert_answers_equal_cold, edit, pin};
use super::*;
use crate::engine::l4::capability_cone::ConeReport;
use crate::engine::resident::revision::{ArtifactKey, ArtifactValue};
use crate::engine::session::relation_of;

/// The fold's products as text: every coverage record, every derived row
/// read through the store's accessors, every raw subject materialized.
fn fold_text(
    caps: &crate::engine::l5::detector_context::CapabilityAnalysis,
    raw: &crate::engine::l4::inherited_facts::InheritedFacts,
) -> String {
    let mut out = String::new();
    let cov: BTreeMap<&String, _> = caps.coverage.iter().collect();
    writeln!(out, "coverage {cov:?}").unwrap();
    let d = &caps.derived;
    for id in d.row_ids() {
        writeln!(
            out,
            "derived {id} flags={} writes={:?} phys={:?} ops={:?} reads={:?} events={:?}",
            d.flags_of(id),
            d.writes_tables_of(id),
            d.writes_physical_tables_of(id),
            d.physical_table_write_ops_of(id),
            d.physical_table_reads_of(id),
            d.publishes_events_of(id),
        )
        .unwrap();
    }
    let mut keys: Vec<&str> = raw.keys().collect();
    keys.sort_unstable();
    for k in keys {
        let facts = raw.get(k).expect("a listed subject").materialize();
        writeln!(out, "raw {k} {facts:?}").unwrap();
    }
    out
}

/// The revision's fold as text, a cold composition's over the same model,
/// and the fold's cone report.
fn fold_and_cold(rev: &Revision) -> (String, String, ConeReport) {
    let snap = rev.snapshot();
    let Some(ArtifactValue::Fold(f)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
        panic!("the fold is published");
    };
    let warm = fold_text(&f.caps, &f.raw);
    let target = rev.target();
    let model = target.resolved();
    let scope = crate::engine::l4::cone_derived::ConeScope {
        derived: true,
        raw: crate::engine::session::WitnessRequest::primary().scope(model),
    };
    let (caps, raw) = crate::engine::session::compose_cone(model, &relation_of(target), scope);
    (warm, fold_text(&caps, &raw), f.cone_report)
}

/// A session over a single-app workspace of `files`.
fn session(files: &[(&str, &str)]) -> (tempfile::TempDir, Arc<ResidentSession>) {
    let dir = tempfile::tempdir().unwrap();
    crate::engine::l5::test_support::write_single_app(dir.path(), files);
    let s = Arc::new(ResidentSession::new(
        dir.path().to_path_buf(),
        false,
        "test".into(),
    ));
    (dir, s)
}

/// One step: pin, every answer on `roots` equal to cold, the fold equal to
/// cold. Returns the revision, its fold text and its cone report.
fn step(
    what: &str,
    s: &Arc<ResidentSession>,
    ws: &Path,
    roots: &[&str],
) -> (Arc<Revision>, String, ConeReport) {
    let rev = pin(s);
    assert_answers_equal_cold(what, &rev, ws, roots);
    let (warm, cold, report) = fold_and_cold(&rev);
    assert_eq!(warm, cold, "{what}: the fold equals a cold composition");
    (rev, warm, report)
}

/// Precondition of a reusing step: the fold re-folded some SCCs, not all,
/// and copied some members' rows.
fn assert_partial(what: &str, r: &ConeReport) {
    assert!(
        r.refolded < r.sccs && r.copied_rows > 0,
        "precondition: {what}: the fold reused the store ({r:?})"
    );
}

const TABLES: &str = r#"table 50100 "T Log"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}

table 50101 "T Other"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}
"#;

/// A routine no edit reaches: its SCC is copied by every reusing step.
const ASIDE: &str = r#"codeunit 50108 "Aside"
{
    procedure Alone()
    var
        O: Record "T Other";
    begin
        O.Modify();
    end;
}
"#;

const PUB: &str = r#"codeunit 50103 "Pub"
{
    procedure Raise()
    begin
        OnThing(false);
    end;

    [IntegrationEvent(false, false)]
    procedure OnThing(Flag: Boolean)
    begin
    end;
}
"#;

const SUB: &str = r#"codeunit 50104 "Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnThing', '', false, false)]
    local procedure HandleThing(Flag: Boolean)
    var
        R: Record "T Log";
    begin
        if Flag then
            R.Insert();
    end;
}
"#;

/// Design §6.10, the edge-transfer digest (T-cone-edge): `Raise` raises
/// `OnThing(false)`; the subscriber inserts only `if Flag`, and its guard
/// re-anchors on the SAME-NAMED publisher parameter, which `false`
/// contradicts, so `Raise` writes nothing. Renaming the subscriber's
/// parameter breaks the by-name mapping: the guard is dropped and `Raise`
/// writes `T Log`. The subscriber's products are unchanged (its guard is by
/// index), so its fingerprint cuts off; only the publisher's transfer digest
/// over its `event-dispatch` edge (the callee's parameters) re-folds it.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): dropping
/// the callee's parameters from `member_digests`' transfer digest fails
/// "rename: Explain on Raise equals cold" (the stale guarded entry keeps
/// `Raise` write-free); reverted, green. Dropping the successors'
/// fingerprints from `scc_input_digest` fails it too.
#[test]
fn renamed_subscriber_parameter_refolds_the_publisher() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Aside.al", ASIDE),
        ("src/Pub.al", PUB),
        ("src/Sub.al", SUB),
    ]);
    let ws = dir.path();
    let (_, before, _) = step("cold", &s, ws, &["Raise"]);
    edit(
        ws,
        "src/Sub.al",
        "    local procedure HandleThing(Flag: Boolean)\n    var\n        R: Record \"T Log\";\n    begin\n        if Flag then",
        "    local procedure HandleThing(Other: Boolean)\n    var\n        R: Record \"T Log\";\n    begin\n        if Other then",
    );
    let (_, after, r) = step("rename", &s, ws, &["Raise"]);
    assert_partial("rename", &r);
    assert_ne!(before, after, "precondition: the rename moves the fold");
    assert!(
        r.cut_off > 0,
        "precondition: the subscriber re-folded to equal products ({r:?})"
    );
}

const LEAF: &str = r#"codeunit 50100 "Leaf"
{
    procedure WriteLog()
    var
        R: Record "T Log";
    begin
        R.Insert();
    end;
}
"#;

const MID: &str = r#"codeunit 50101 "Mid"
{
    procedure CallLeaf()
    var
        L: Codeunit "Leaf";
    begin
        L.WriteLog();
    end;
}
"#;

const TOP: &str = r#"codeunit 50102 "Top"
{
    procedure Go()
    var
        M: Codeunit "Mid";
        L: Codeunit "Leaf";
    begin
        M.CallLeaf();
        L.WriteLog();
    end;
}
"#;

/// Design §6.10, the product fingerprint covers each representative's full
/// content (T-cone): renaming `WriteLog`'s record variable keeps its fact's
/// dedup key (`insert|table|T Log|..`) but changes the representative (its
/// `recordVariableId`). `Leaf`'s cone has the same keys, distances and
/// guards, so a fingerprint over those alone would cut off and leave
/// `Mid`'s and `Go`'s inherited facts naming the old variable.
///
/// DISCRIMINATION (2026-11-03): a fingerprint over distances and guards only
/// (`Fingerprints::of` without the representative's content hash) fails "rename
/// variable: the fold equals a cold composition" (and the moved-subscriber
/// test, whose representative's subject moves); reverted, green.
#[test]
fn equal_dedup_key_with_a_new_representative_is_not_cut_off() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Aside.al", ASIDE),
    ]);
    let ws = dir.path();
    let (_, before, _) = step("cold", &s, ws, &["Go", "CallLeaf"]);
    edit(
        ws,
        "src/Leaf.al",
        "        R: Record \"T Log\";\n    begin\n        R.Insert();",
        "        S: Record \"T Log\";\n    begin\n        S.Insert();",
    );
    let (_, after, r) = step("rename variable", &s, ws, &["Go", "CallLeaf"]);
    assert_partial("rename variable", &r);
    assert_ne!(before, after, "precondition: the representative moved");
}

/// Design §6.10, cut-off (T-cone): a comment in `WriteLog` changes its rows
/// (positions) but none of its products: its SCC re-folds to an equal
/// fingerprint and every predecessor is copied.
///
/// DISCRIMINATION (2026-11-03): publishing a re-folded SCC's input digest in
/// place of its fingerprint (no cut-off) fails "only Leaf re-folds" (and the
/// object-only and producer tests' reuse preconditions); reverted, green.
#[test]
fn an_equal_refold_cuts_off_its_predecessors() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Aside.al", ASIDE),
    ]);
    let ws = dir.path();
    step("cold", &s, ws, &["Go"]);
    edit(
        ws,
        "src/Leaf.al",
        "    begin\n        R.Insert();",
        "    begin\n        // a comment\n        R.Insert();",
    );
    let (_, _, r) = step("comment", &s, ws, &["Go"]);
    assert_eq!(s.stats().last_build_class.as_deref(), Some("B"));
    assert_eq!(
        (r.refolded, r.cut_off),
        (1, 1),
        "only Leaf re-folds, to equal products ({r:?})"
    );
}

/// Design §11, a removed edge and the deletion of an alternate path: `Go`
/// reaches `WriteLog` directly (distance 1) and through `Mid` (distance 2).
/// Removing the direct call keeps the fact at distance 2 with another first
/// hop; removing the other call too leaves `Go` with nothing.
///
/// LIMIT (2026-11-03): the removed edge changes `Go`'s own rows, out-edges
/// and successor list, three independent inputs of its SCC's digest;
/// dropping the successors' fingerprints alone left this test green. It
/// pins equality, not one rule.
#[test]
fn removed_edge_and_alternate_path_deletion_equal_cold() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Aside.al", ASIDE),
    ]);
    let ws = dir.path();
    let (_, cold, _) = step("cold", &s, ws, &["Go"]);
    edit(
        ws,
        "src/Top.al",
        "        M.CallLeaf();\n        L.WriteLog();",
        "        M.CallLeaf();",
    );
    let (_, one, r) = step("direct call removed", &s, ws, &["Go"]);
    assert_partial("direct call removed", &r);
    assert_ne!(cold, one, "precondition: the first hop moved");
    edit(
        ws,
        "src/Top.al",
        "        M.CallLeaf();\n    end;",
        "    end;",
    );
    let (_, two, r) = step("both calls removed", &s, ws, &["Go"]);
    assert_partial("both calls removed", &r);
    assert_ne!(one, two, "precondition: Go lost the fact");
}

const REC: &str = r#"codeunit 50105 "Rec"
{
    procedure A(N: Integer)
    begin
        if N > 0 then
            B(N - 1);
    end;

    procedure B(N: Integer)
    var
        R: Record "T Log";
    begin
        R.Insert();
        A(N);
    end;

    procedure Root()
    begin
        A(3);
    end;
}
"#;

/// Design §11, an SCC split and merge: `A` and `B` call each other (one
/// recursive SCC). Removing `B`'s call splits it into two singletons (new
/// SCC keys); restoring it merges them again. Each step's fold and answers
/// equal cold. LIMIT: a split or merge mints new SCC keys, so no record is
/// found; like the removed-edge test it pins equality, not one rule.
#[test]
fn scc_split_and_merge_equal_cold() {
    let (dir, s) = session(&[("src/Tables.al", TABLES), ("src/Rec.al", REC)]);
    let ws = dir.path();
    step("cold", &s, ws, &["Root", "A", "B"]);
    edit(
        ws,
        "src/Rec.al",
        "        R.Insert();\n        A(N);",
        "        R.Insert();",
    );
    let (_, _, r) = step("split", &s, ws, &["Root", "A", "B"]);
    assert!(r.refolded > 0, "precondition: the split re-folds ({r:?})");
    edit(
        ws,
        "src/Rec.al",
        "        R.Insert();\n    end;",
        "        R.Insert();\n        A(N);\n    end;",
    );
    step("merge", &s, ws, &["Root", "A", "B"]);
}

const SUB2: &str = r#"codeunit 50106 "Sub Two"
{
}
"#;

/// Design §11, a moved subscriber: `HandleThing` moves from `Sub` to `Sub
/// Two` (a new routine id). The publisher's text is unchanged, but its
/// `event-dispatch` edge now ends at another routine.
#[test]
fn a_moved_subscriber_changes_the_unchanged_publishers_edge() {
    let sub = SUB.replace(
        "        if Flag then\n            R.Insert();",
        "        R.Insert();",
    );
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Pub.al", PUB),
        ("src/Sub.al", &sub),
        ("src/Sub2.al", SUB2),
    ]);
    let ws = dir.path();
    step("cold", &s, ws, &["Raise"]);
    let handler = "    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Pub\", 'OnThing', '', false, false)]\n    local procedure HandleThing(Flag: Boolean)\n    var\n        R: Record \"T Log\";\n    begin\n        R.Insert();\n    end;\n";
    edit(ws, "src/Sub.al", handler, "");
    edit(ws, "src/Sub2.al", "{\n}", &format!("{{\n{handler}}}"));
    let (_, _, r) = step("moved", &s, ws, &["Raise"]);
    assert!(r.refolded > 0, "precondition: the move re-folds ({r:?})");
}

const CHAIN: &str = r#"codeunit 50107 "Chain"
{
    procedure A()
    var
        T: Record "T Log" temporary;
    begin
        C(T);
    end;

    procedure C(var Rec: Record "T Log")
    begin
        P(Rec);
    end;

    procedure P(var Rec: Record "T Log")
    begin
        Rec.Insert();
    end;
}
"#;

/// Design §6.10 Δ_pd_output (T-pd-out): `P` inserts into its parameter (a
/// producer), `C` passes its own parameter on, `A` passes a temporary. `P`'s
/// insert becomes a modify: the producer's fact changes, so it is re-solved
/// and its states at `C` and `A` change. No cone holds a producer, so `P`'s
/// SCC re-folds to an equal fingerprint and `C` and `A` are cut off; only
/// Δ_pd_output re-folds `A`'s (and `C`'s) rows.
///
/// DISCRIMINATION (2026-11-03): ignoring Δ_pd_output inside a reused SCC
/// (`refold` reading only `reused`) fails "producer edit: Explain on A equals
/// cold"; reverted, green.
#[test]
fn a_changed_producer_refolds_the_rows_of_every_routine_holding_its_states() {
    let (dir, s) = session(&[("src/Tables.al", TABLES), ("src/Chain.al", CHAIN)]);
    let ws = dir.path();
    let (_, before, _) = step("cold", &s, ws, &["A", "C"]);
    edit(
        ws,
        "src/Chain.al",
        "        Rec.Insert();",
        "        Rec.Modify();",
    );
    let (_, after, r) = step("producer edit", &s, ws, &["A", "C"]);
    assert_ne!(before, after, "precondition: A's rows move");
    assert!(
        r.pd_rows > 0 && r.pd_delta > 0,
        "precondition: rows re-folded through Δ_pd_output ({r:?})"
    );
}

const UNUSED: &str = r#"table 50102 "T Unused"
{
    fields
    {
        field(1; Id; Integer)
        {
        }
    }
}
"#;

/// Design §11 T-obj-only: renaming a table no routine declares changes no
/// routine and re-folds nothing, yet every answer still equals cold (the
/// object-level sections are rebuilt whole every revision).
#[test]
fn an_object_only_table_rename_equals_cold() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Unused.al", UNUSED),
    ]);
    let ws = dir.path();
    step("cold", &s, ws, &["Go"]);
    edit(ws, "src/Unused.al", "\"T Unused\"", "\"T Renamed\"");
    let (_, _, r) = step("rename", &s, ws, &["Go"]);
    assert_eq!(r.refolded, 0, "nothing re-folds ({r:?})");
}
