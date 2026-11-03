//! Stage 5's cone store (design §6.10, §11): hand-stated counterexamples,
//! each a resident session over a temporary single-app workspace edited step
//! by step. After every step every served answer equals a cold standalone run
//! over a fresh read of the edited disk, and the revision's fold (coverage,
//! derived rows) and every `Raw` artifact it published (plan 2026-11-03 L6:
//! emitted on demand, or adopted from the previous revision) equal a cold
//! composition over the same model.
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

/// The revision's fold as text, with every `Raw` artifact it published (plan
/// 2026-11-03 L6: the fold emits no raw facts; each view's are emitted on
/// demand, or adopted), and a cold composition's over the same model whose
/// raw scope is every published artifact's subjects; and the fold's cone
/// report. Each artifact's entries are listed under its scope, so an entry
/// adopted from an earlier revision is compared too.
fn fold_and_cold(rev: &Revision) -> (String, String, ConeReport) {
    use crate::engine::l4::inherited_facts::InheritedFacts;
    let snap = rev.snapshot();
    let Some(ArtifactValue::Fold(f)) = snap.get(&ArtifactKey::Fold).map(|a| &a.value) else {
        panic!("the fold is published");
    };
    let raws: Vec<(&std::collections::BTreeSet<String>, &InheritedFacts)> = snap
        .keys()
        .filter_map(|k| match (k, snap.get(k).map(|a| &a.value)) {
            (ArtifactKey::Raw(scope), Some(ArtifactValue::Raw(r))) => Some((scope, r)),
            _ => None,
        })
        .collect();
    let raws_text = |of: &dyn Fn(&std::collections::BTreeSet<String>) -> String| {
        raws.iter()
            .map(|(scope, _)| format!("raw artifact {scope:?}\n{}", of(scope)))
            .collect::<String>()
    };
    let no_raw = InheritedFacts::default();
    let warm = fold_text(&f.caps, &no_raw)
        + &raws_text(&|scope| {
            let (_, r) = raws.iter().find(|(s, _)| *s == scope).expect("listed");
            fold_text_raw(r)
        });
    let target = rev.target();
    let model = target.resolved();
    let subjects: std::collections::BTreeSet<String> =
        raws.iter().flat_map(|(s, _)| s.iter().cloned()).collect();
    let scope = crate::engine::l4::cone_derived::ConeScope {
        derived: true,
        raw: crate::engine::l4::cone_derived::WitnessScope::Subjects(subjects),
    };
    let (caps, raw) = crate::engine::session::compose_cone(model, &relation_of(target), scope);
    let cold = fold_text(&caps, &no_raw)
        + &raws_text(&|scope| {
            let mut out = String::new();
            for k in scope {
                if let Some(facts) = raw.get(k) {
                    writeln!(out, "raw {k} {:?}", facts.materialize()).unwrap();
                }
            }
            out
        });
    (warm, cold, f.cone_report)
}

/// [`fold_text`]'s raw half alone.
fn fold_text_raw(raw: &crate::engine::l4::inherited_facts::InheritedFacts) -> String {
    let mut out = String::new();
    let mut keys: Vec<&str> = raw.keys().collect();
    keys.sort_unstable();
    for k in keys {
        let facts = raw.get(k).expect("a listed subject").materialize();
        writeln!(out, "raw {k} {facts:?}").unwrap();
    }
    out
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

// ---------------------------------------------------------------------------
// The tree store (design §6.11)
// ---------------------------------------------------------------------------

/// A refresh frees the published revision's trees before it builds (they
/// seed nothing in the next revision): the old revision keeps its fold (the
/// next fold's seed) but no tree and no base.
///
/// DISCRIMINATION (2026-11-03): removing the two `reclaim_kind` calls in
/// the refresh fails "the old revision's trees are freed"; reverted, green.
#[test]
fn a_refresh_frees_the_published_revisions_trees() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
    ]);
    let ws = dir.path();
    let (old, _, _) = step("cold", &s, ws, &["Go"]);
    let kinds = |r: &Revision| -> Vec<&'static str> {
        r.snapshot().keys().map(ArtifactKey::kind).collect()
    };
    assert!(
        kinds(&old).contains(&"tree") && kinds(&old).contains(&"tree_base"),
        "precondition: the answers filled trees: {:?}",
        kinds(&old)
    );
    edit(
        ws,
        "src/Leaf.al",
        "    begin\n        R.Insert();",
        "    begin\n        // a comment\n        R.Insert();",
    );
    step("comment", &s, ws, &["Go"]);
    let after = kinds(&old);
    assert!(
        !after.contains(&"tree") && !after.contains(&"tree_base") && after.contains(&"fold"),
        "the old revision's trees are freed, its fold kept: {after:?}"
    );
}

/// A tree as text: every serialized section in order, the routine order
/// frames, the pd arena (`by_routine` sorted) and rule B's boundary (sorted).
fn tree_text(t: &crate::engine::l5::snapshot::CapabilitySnapshot) -> String {
    let j = |v: serde_json::Result<String>| v.expect("a tree section serializes");
    let mut out = String::new();
    for (name, text) in [
        ("identities", j(serde_json::to_string(&t.identities))),
        ("facts", j(serde_json::to_string(&t.capability_facts))),
        ("edges", j(serde_json::to_string(&t.typed_edges))),
        ("ops", j(serde_json::to_string(&t.operation_index))),
        ("sites", j(serde_json::to_string(&t.callsite_index))),
        ("res", j(serde_json::to_string(&t.callsite_resolutions))),
        ("gaps", j(serde_json::to_string(&t.analysis_gaps))),
        ("coverage", j(serde_json::to_string(&t.coverage))),
        ("events", j(serde_json::to_string(&t.event_declarations))),
        ("roots", j(serde_json::to_string(&t.root_classifications))),
        ("frames", j(serde_json::to_string(&t.routine_order_frames))),
    ] {
        writeln!(out, "{name} {text}").unwrap();
    }
    let by: BTreeMap<&String, &Vec<usize>> = t.pd_solution.by_routine.iter().collect();
    writeln!(
        out,
        "pd {:?} {:?} {by:?}",
        t.pd_solution.states, t.pd_solution.links
    )
    .unwrap();
    let boundary: Option<BTreeMap<&String, &Arc<str>>> = t
        .boundary
        .as_ref()
        .map(|b| b.dependency_app.iter().collect());
    writeln!(out, "boundary {boundary:?}").unwrap();
    out
}

/// Every tree the revision's store holds, as text, each with a cold
/// one-shot tree over the same view (a cold composition, the subjects' raw
/// facts, the pd arena): `(subjects, served, cold)`.
fn trees_and_cold(rev: &Revision) -> Vec<(Vec<String>, String, String)> {
    use crate::engine::gate::run::AnalysisTarget;
    use crate::engine::l5::snapshot::{SubjectRaw, TreeRaw, compose_tree};
    let target = rev.target();
    let model = target.resolved();
    let rel = relation_of(target);
    let primary = crate::engine::session::WitnessRequest::primary();
    let (caps, raw) = crate::engine::session::compose_cone(
        model,
        &rel,
        crate::engine::l4::cone_derived::ConeScope {
            derived: true,
            raw: primary.scope(model),
        },
    );
    let primary = primary.subjects(model);
    let cross = match target {
        AnalysisTarget::CrossApp(b) => Some(&**b),
        AnalysisTarget::SingleApp(_) => None,
    };
    let calls = cross
        .and_then(|b| b.dependency_calls.as_deref())
        .map_or(&[][..], Vec::as_slice);
    let snap = rev.snapshot();
    snap.keys()
        .filter_map(|k| match (k, snap.get(k).map(|a| &a.value)) {
            (ArtifactKey::Tree(scope), Some(ArtifactValue::Tree(t))) => Some((scope, t)),
            _ => None,
        })
        .map(|(scope, served)| {
            let own;
            let map = if scope.is_subset(&primary) {
                &raw
            } else {
                own = crate::engine::session::emit_raw_ids(model, &rel, scope.clone());
                &own
            };
            let cold = compose_tree(
                model,
                rel.cone(),
                calls,
                &caps,
                TreeRaw::Borrow(SubjectRaw::new(map, scope)),
                std::borrow::Cow::Borrowed(caps.pd.as_ref().expect("an arena")),
                &crate::engine::session::tree_apps(cross),
            );
            (
                scope.iter().cloned().collect(),
                tree_text(served),
                tree_text(&cold),
            )
        })
        .collect()
}

/// Every served tree equals its cold one; returns the trees' texts.
fn assert_trees_equal_cold(what: &str, rev: &Revision) -> Vec<String> {
    let trees = trees_and_cold(rev);
    assert!(!trees.is_empty(), "precondition: {what}: some tree served");
    trees
        .into_iter()
        .map(|(scope, served, cold)| {
            assert_eq!(served, cold, "{what}: the tree over {scope:?} equals cold");
            served
        })
        .collect()
}

const CALLER: &str = r#"codeunit 50109 "Caller"
{
    procedure Call()
    var
        A: Codeunit "Aside";
    begin
        A.Alone();
    end;
}
"#;

/// Design §6.11 (T-tree, T-typed-edges, T-callsite-res, T-obj-only): the
/// heavy answers on one revision share its trees — one tree per distinct
/// view, the whole sections derived once — and every served tree equals a
/// cold one-shot tree over the same view after each step: a new call (the
/// typed edges move), a call to no routine (the call-site resolutions
/// move), and a rename of a table no routine declares (the identities move).
/// `Go` and `Call` have different subjects, so each step serves two views.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): reading
/// the first published tree whatever the group's subjects (one tree for
/// every view) fails an answer's "equals cold" (and the producer-chain
/// test's, whose two roots are two views); without the second `Go` this
/// test stayed green, which is why it answers `Go` twice. Keeping
/// the first revision's whole sections for every later revision (a base
/// held across revisions) fails "new call: the tree over [Go] equals cold"
/// (and three cone tests' answers); each reverted, green.
#[test]
fn heavy_answers_share_trees_that_equal_cold_trees_after_each_edit() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Aside.al", ASIDE),
        ("src/Caller.al", CALLER),
        ("src/Unused.al", UNUSED),
    ]);
    let ws = dir.path();
    // `Go` again after `Call`: its answers then read a revision holding both
    // views' trees.
    let roots = ["Go", "Call", "Go"];
    let (rev, _, _) = step("cold", &s, ws, &roots);
    let fills = rev.fill_counts();
    let views = trees_and_cold(&rev).len();
    assert!(views >= 2, "precondition: two views ({views})");
    assert_eq!(
        (fills.get("tree").copied(), fills.get("tree_base").copied()),
        (Some(views as u64), Some(1)),
        "one tree per view, one base, shared by every answer: {fills:?}"
    );
    let mut before = assert_trees_equal_cold("cold", &rev);
    for (what, file, from, to) in [
        (
            "new call",
            "src/Top.al",
            "        M.CallLeaf();\n        L.WriteLog();",
            "        M.CallLeaf();\n        L.WriteLog();\n        L.WriteLog();",
        ),
        (
            "call to no routine",
            "src/Caller.al",
            "        A.Alone();",
            "        A.Alone();\n        A.NoSuchMethod();",
        ),
        (
            "object-only rename",
            "src/Unused.al",
            "\"T Unused\"",
            "\"T Renamed\"",
        ),
    ] {
        edit(ws, file, from, to);
        let (rev, _, _) = step(what, &s, ws, &roots);
        let after = assert_trees_equal_cold(what, &rev);
        assert_ne!(before, after, "precondition: {what} moves a tree");
        before = after;
    }
}

/// The `Raw` artifact over `routine`'s view on `rev`, as text.
fn raw_of(rev: &Revision, routine: &str) -> String {
    let snap = rev.snapshot();
    let model = rev.target().resolved();
    let ids: std::collections::BTreeSet<String> = model
        .workspace
        .routines
        .iter()
        .filter(|r| r.name == routine)
        .map(|r| r.id.clone())
        .collect();
    match snap.get(&ArtifactKey::Raw(ids)).map(|a| &a.value) {
        Some(ArtifactValue::Raw(r)) => fold_text_raw(r),
        _ => panic!("{routine}'s view has a raw artifact"),
    }
}

/// Plan 2026-11-03 L6 across revisions (design §6.10's re-emission rule,
/// amended): the fold emits no raw facts, and a new revision's `Raw` fill
/// re-emits a demanded subject only when the fold re-folded its rows
/// against the previous fold; every other demanded subject adopts the
/// previous revision's entry. A body edit in `WriteLog` (insert → modify)
/// changes `Go`'s inherited facts: the next revision re-emits `Go` and
/// adopts `Call` (nothing it reaches moved), both answers equal cold, and
/// every published raw entry equals a cold composition's (`step`). A
/// second edit chains it: `Call`'s entry is adopted from an entry that was
/// itself adopted.
///
/// DISCRIMINATION (break with Edit, run, revert, run; 2026-11-03): (d)
/// adopting the previous revision's entry whatever the fold re-folded (the
/// `!store.changed(id)` test made `true ||` in the `Raw` fill's `reuse`)
/// serves `Go`'s stale raw facts: "insert to modify: Explain on Go equals
/// cold" fails (and seven other cone and incremental tests' answers). (e)
/// the changed set missing Δ_pd_output (a member recorded only when its SCC
/// re-folded) fails `a_changed_producer_refolds_the_rows_of_every_routine_
/// holding_its_states` ("producer edit: Explain on A equals cold"). Each
/// reverted, green.
#[test]
fn a_changed_subject_is_re_emitted_and_an_unchanged_one_adopted() {
    let (dir, s) = session(&[
        ("src/Tables.al", TABLES),
        ("src/Leaf.al", LEAF),
        ("src/Mid.al", MID),
        ("src/Top.al", TOP),
        ("src/Aside.al", ASIDE),
        ("src/Caller.al", CALLER),
    ]);
    let ws = dir.path();
    let roots = ["Go", "Call"];
    let (rev, _, _) = step("cold", &s, ws, &roots);
    assert_eq!(rev.raw_counts(), (2, 0), "cold: each view emitted");
    let (mut go, call) = (raw_of(&rev, "Go"), raw_of(&rev, "Call"));
    drop(rev);
    for (what, from, to) in [
        (
            "insert to modify",
            "        R.Insert();",
            "        R.Modify();",
        ),
        (
            "modify to delete",
            "        R.Modify();",
            "        R.Delete();",
        ),
    ] {
        edit(ws, "src/Leaf.al", from, to);
        let (next, _, r) = step(what, &s, ws, &roots);
        assert_partial(what, &r);
        let moved = raw_of(&next, "Go");
        assert_ne!(moved, go, "precondition: {what} moves Go's raw");
        assert_eq!(
            raw_of(&next, "Call"),
            call,
            "precondition: Call's raw is unchanged"
        );
        assert_eq!(
            next.raw_counts(),
            (1, 1),
            "{what}: Go re-emitted, Call adopted"
        );
        go = moved;
    }
}
