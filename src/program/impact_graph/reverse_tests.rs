//! The reverse walker's contract (agent-tasks design §3.7, R0-1..R0-9).
//! Hand-built worlds state every precondition literally; the fixture tests
//! read `tests/fixtures/test-selection-graph` through a real build.

use std::collections::HashSet;
use std::path::PathBuf;

use al_syntax::ir::ObjectKind;

use super::*;
use crate::program::impact_graph::project::tests::{World, sited_of, unknown_route};
use crate::program::impact_graph::{RoutineFacts, SitedGraph};
use crate::program::node::{AppRef, RoutineNodeId};
use crate::program::node_extract::Access;
use crate::program::resolve::edge::{DispatchShape, EdgeKind};
use crate::snapshot::TrustTier;

fn walk(g: &SitedGraph, roots: &[NodeIx], opts: &WalkOptions<'_>) -> Recorder {
    let mut rec = Recorder::default();
    walk_multi(g, roots, opts, &mut WalkScratch::default(), &mut rec);
    rec
}

fn node(g: &SitedGraph, r: &RoutineNodeId) -> NodeIx {
    g.routine_node(r).expect("the routine is a node")
}

fn mask(kinds: &[&str]) -> u16 {
    kinds
        .iter()
        .map(|k| crate::engine::root_classification::root_kind_bit(k).expect("a kind"))
        .fold(0, |m, b| m | b)
}

/// A workspace app `W` and a dependency `D` (embedded source).
fn two_apps(w: &mut World) -> (AppRef, AppRef) {
    let a = w.app("W");
    let d = w.app("D");
    w.deps.push((a, d));
    (a, d)
}

/// R0-5: a node reached by a local path (3 hops) and by a shorter path
/// through dependency code (2 hops) has a `Local` state with the local hop
/// count and a `ThroughDependency` state with its own.
///
/// DISCRIMINATION (2026-11-05): keying `WalkScratch::insert` on the node
/// alone (`let bit = 1u8;`, one state per node) fails "a Local state": the
/// node keeps only the first-found (dependency) state.
#[test]
fn a_node_keeps_a_state_per_tier() {
    let mut w = World::default();
    let (a, d) = two_apps(&mut w);
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let dep = w.obj(d, ObjectKind::Codeunit, 2, "Dep", TrustTier::EmbeddedSource);
    let r = w.rtn(&cu, "R");
    let a1 = w.rtn(&cu, "A1");
    let a2 = w.rtn(&cu, "A2");
    let p = w.rtn(&dep, "P");
    let x = w.rtn(&cu, "X");
    w.call(&x, &a2, 1);
    w.call(&a2, &a1, 2);
    w.call(&a1, &r, 3);
    w.call(&x, &p, 4);
    w.call(&p, &r, 5);
    let g = sited_of(&w, a, &[]);
    let rec = walk(&g, &[node(&g, &r)], &WalkOptions::default());
    let xs = rec.of(node(&g, &x));
    let local = xs.iter().find(|(s, _)| s.tier == Tier::Local);
    let dep_state = xs.iter().find(|(s, _)| s.tier == Tier::ThroughDependency);
    assert_eq!(local.map(|x| x.1), Some(3), "a Local state, 3 hops: {xs:?}");
    assert_eq!(
        dep_state.map(|x| x.1),
        Some(2),
        "a ThroughDependency state, 2 hops"
    );
    assert_eq!(xs.len(), 2);
    let ps = rec.of(node(&g, &p));
    assert_eq!(ps, vec![(ps[0].0, 1)]);
    assert_eq!(ps[0].0.tier, Tier::ThroughDependency);
}

/// R0-2: a dependency leaf called only from the workspace is absent from the
/// pruned export, and its cone holds its workspace caller.
#[test]
fn a_dependency_leaf_the_export_prunes_has_a_cone() {
    let mut w = World::default();
    let (a, d) = two_apps(&mut w);
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let dep = w.obj(d, ObjectKind::Codeunit, 2, "Dep", TrustTier::EmbeddedSource);
    let leaf = w.rtn(&dep, "Leaf");
    let c = w.rtn(&cu, "C");
    w.call(&c, &leaf, 1);
    let g = sited_of(&w, a, &[]);
    let exported: HashSet<String> = w
        .with_inputs(a, crate::program::impact_graph::project)
        .nodes
        .into_iter()
        .map(|n| n.id)
        .collect();
    let l = node(&g, &leaf);
    assert!(
        !exported.contains(&g.node_id(l)),
        "the export prunes the leaf"
    );
    let rec = walk(&g, &[l], &WalkOptions::default());
    let cs = rec.of(node(&g, &c));
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].1, 1);
    // The path passes the dependency leaf itself: not local.
    assert_eq!(cs[0].0.tier, Tier::ThroughDependency);
}

/// R0-4: an `h:any` caller. The hub is reached once, one hop from the roots,
/// as an `OnlyOpen` state; its caller once, two hops; the hub is reported
/// once as the universal hidden channel, for one root or two.
///
/// DISCRIMINATION (2026-11-05): deleting the universal hub's seeding in
/// `walk_multi` (`universal.filter(|_| false)`) fails "the hub once": the
/// hub and its caller are dropped.
#[test]
fn the_universal_hub_is_walked_once_as_only_open() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let r = w.rtn(&cu, "R");
    let r2 = w.rtn(&cu, "R2");
    let c = w.rtn(&cu, "C");
    let u = w.rtn(&cu, "U");
    w.call(&c, &r, 1);
    w.call(&c, &r2, 2);
    w.edge(
        &u,
        EdgeKind::Call,
        DispatchShape::Exact,
        vec![unknown_route()],
        3,
    );
    let g = sited_of(&w, a, &[]);
    let hub = g.universal_hub().expect("an edge names h:any");
    for roots in [vec![node(&g, &r)], vec![node(&g, &r), node(&g, &r2)]] {
        let rec = walk(&g, &roots, &WalkOptions::default());
        let hubs = rec.of(hub);
        assert_eq!(hubs.len(), 1, "the hub once");
        assert_eq!((hubs[0].0.tier, hubs[0].1), (Tier::OnlyOpen, 1));
        let us = rec.of(node(&g, &u));
        assert_eq!(us.len(), 1, "the hub's caller is walked, once");
        assert_eq!((us[0].0.tier, us[0].1), (Tier::OnlyOpen, 2));
        let universal: Vec<_> = rec
            .hidden
            .iter()
            .filter(|h| h.2 == HiddenChannel::Universal)
            .collect();
        assert_eq!(universal.len(), 1, "one universal hidden channel");
        assert!(
            rec.states
                .iter()
                .any(|s| s.1.node == hub && s.3 == Via::Universal)
        );
    }
    // Without open sites the hub is not walked.
    let rec = walk(
        &g,
        &[node(&g, &r)],
        &WalkOptions {
            follow_open: false,
            ..Default::default()
        },
    );
    assert!(rec.of(hub).is_empty() && rec.of(node(&g, &u)).is_empty());
}

/// Counts the reversed sites a walk steps over, per site.
#[derive(Default)]
struct Steps(Vec<SiteIx>);

impl ConeVisitor for Steps {
    fn step(&mut self, _: &SitedGraph, from: WalkState, site: SiteIx) -> Option<u8> {
        self.0.push(site);
        Some(from.ext)
    }
}

/// R0-7: two calls `A -> R` in one routine (one before a `Modify`, one
/// after, under different guards: their own anchors) are two reverse sites
/// with their own ids and anchors, each stepped over by the walk.
///
/// DISCRIMINATION (2026-11-05): keeping only the first site per `(from, to)`
/// in the reverse index (`finish`: skip a site whose `(from, to)` an earlier
/// site had) fails `assert_ne!(anchors[0], anchors[1])`: the walk sees one
/// site twice, never the second call's.
#[test]
fn two_calls_between_the_same_routines_are_two_reverse_sites() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let r = w.rtn(&cu, "R");
    let caller = w.rtn(&cu, "A");
    w.call(&caller, &r, 10);
    w.call(&caller, &r, 14);
    let g = sited_of(&w, a, &[]);
    let into: Vec<SiteIx> = g
        .sites_into(node(&g, &r))
        .iter()
        .copied()
        .filter(|s| g.sites()[*s as usize].from == node(&g, &caller))
        .collect();
    assert_eq!(into.len(), 2, "two reverse sites");
    let anchors: Vec<_> = into.iter().map(|s| g.sites()[*s as usize].anchor).collect();
    assert_ne!(anchors[0], anchors[1]);
    let mut steps = Steps::default();
    walk_one(
        &g,
        node(&g, &r),
        &WalkOptions::default(),
        &mut WalkScratch::default(),
        &mut steps,
    );
    for s in &into {
        assert!(steps.0.contains(s), "site {s} stepped over");
    }
}

/// R0-8: a page action's trigger that a lifecycle site reaches is still a
/// `page-action` entry; a public procedure with an internal caller is still
/// external (`public`). Graph sources are reported apart.
///
/// DISCRIMINATION (2026-11-05): reporting entries only for nodes with no
/// reversed site (`if !kinds.is_empty() && g.sites_into(s.node).is_empty()`
/// in `report`) fails "the action trigger is a page-action entry".
#[test]
fn entries_come_from_source_not_from_indegree() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let page = w.obj(a, ObjectKind::Page, 2, "Card", TrustTier::Workspace);
    let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
    let action = w.rtn_with(&page, "OnAction", |x| {
        x.is_trigger = true;
        x.id.enclosing_member_lc = Some("post".into());
    });
    let public = w.rtn(&cu, "Pub");
    let other = w.rtn_with(&cu, "Other", |x| x.access = Access::Local);
    w.call(&action, &r, 1);
    w.call(&public, &r, 2);
    w.call(&other, &public, 3);
    w.facts.routines.insert(
        action.clone(),
        RoutineFacts {
            root_kinds: mask(&["trigger-page", "page-action"]),
            ..Default::default()
        },
    );
    let g = sited_of(&w, a, &[]);
    let act = node(&g, &action);
    assert!(
        !g.sites_into(act).is_empty(),
        "precondition: a lifecycle site reaches the action trigger"
    );
    let rec = walk(&g, &[node(&g, &r)], &WalkOptions::default());
    let e = rec
        .entries
        .iter()
        .find(|e| e.1 == act)
        .expect("the action trigger is a page-action entry");
    assert_eq!(e.2, vec![EntryKind::PageAction, EntryKind::TriggerPage]);
    let p = node(&g, &public);
    assert!(
        rec.external
            .iter()
            .any(|x| x.1 == p && x.2 == ExternalWhy::Public),
        "a public procedure with an internal caller is external"
    );
    assert!(
        !rec.external.iter().any(|x| x.1 == node(&g, &other)),
        "a local procedure is not external"
    );
    // `Other` has no caller: a graph source, not an entry.
    let o = node(&g, &other);
    assert!(rec.sources.iter().any(|s| s.1.node == o));
    assert!(!rec.entries.iter().any(|e| e.1 == o));
}

/// F3: the codeunit object a static background start enters (a
/// `Background` site) is a `background-start` entry, from the graph; the
/// walk goes on over the start site to the starter (R0 stops nowhere; a
/// consumer's `step` does). The starter itself is no such entry.
///
/// DISCRIMINATION (2026-11-05): dropping the `is_background_target` arm in
/// `report` fails "the started codeunit is a background-start entry".
#[test]
fn a_background_start_target_is_an_entry() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let bg = w.obj(a, ObjectKind::Codeunit, 2, "Bg", TrustTier::Workspace);
    let starter = w.rtn_with(&cu, "Starter", |x| x.access = Access::Local);
    let on_run = w.trigger(&bg, "OnRun");
    let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
    w.call(&on_run, &r, 1);
    w.background(
        &starter,
        vec![crate::program::resolve::edge::Route {
            target: crate::program::resolve::edge::RouteTarget::Routine(on_run.clone()),
            evidence: crate::program::resolve::edge::Evidence::Source,
            conditions: vec![],
            witness: crate::program::resolve::edge::Witness::None,
            receiver_tier: None,
        }],
        2,
    );
    let g = sited_of(&w, a, &[]);
    let rec = walk(&g, &[node(&g, &r)], &WalkOptions::default());
    let object = (0..g.node_count() as u32)
        .find(|&n| g.object_node(n).as_ref() == Some(&bg))
        .expect("the started codeunit's object node");
    let e = rec
        .entries
        .iter()
        .find(|e| e.1 == object)
        .expect("the started codeunit is a background-start entry");
    assert_eq!(e.2, vec![EntryKind::BackgroundStart]);
    let s = node(&g, &starter);
    assert!(
        rec.states.iter().any(|x| x.1.node == s),
        "the walk reaches the starter"
    );
    assert!(!rec.entries.iter().any(|e| e.1 == s));
}

/// R0-9: an internal procedure of an app with a friend is external (reason
/// `friend`); without a friend it is not; with `assumeNoDependents` nothing
/// is external.
///
/// DISCRIMINATION (2026-11-05): dropping the friend check in `finish` (an
/// internal routine always `0`) fails "friend".
#[test]
fn internal_routines_are_external_only_with_a_friend() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let r = w.rtn_with(&cu, "R", |x| x.access = Access::Local);
    let i = w.rtn_with(&cu, "I", |x| x.access = Access::Internal);
    w.call(&i, &r, 1);
    let external_of = |g: &SitedGraph, opts: &WalkOptions<'_>| -> Vec<ExternalWhy> {
        walk(g, &[node(g, &r)], opts)
            .external
            .iter()
            .filter(|x| x.1 == node(g, &i))
            .map(|x| x.2)
            .collect()
    };
    let friended = sited_of(&w, a, &[a]);
    assert_eq!(
        external_of(&friended, &WalkOptions::default()),
        vec![ExternalWhy::Friend],
        "friend"
    );
    assert!(
        external_of(
            &friended,
            &WalkOptions {
                assume_no_dependents: true,
                ..Default::default()
            }
        )
        .is_empty(),
        "assumeNoDependents"
    );
    let alone = sited_of(&w, a, &[]);
    assert!(
        external_of(&alone, &WalkOptions::default()).is_empty(),
        "no friend"
    );
}

/// Counts the states each root label gets.
#[derive(Default)]
struct PerRoot(std::collections::HashMap<u32, usize>, usize);

impl ConeVisitor for PerRoot {
    fn reach(&mut self, root: u32, _: WalkState, _: u32, _: Via, _: Option<WalkState>) {
        *self.0.entry(root).or_default() += 1;
        self.1 += 1;
    }
}

/// R0-6: a 1,000-root `walk_multi` labels each state once, with its nearest
/// root (ties by root order), and its scratch stays O(states): a caller
/// shared by every root is one state, labelled root 0.
///
/// DISCRIMINATION (2026-11-05): resetting the visited set per root (a
/// `walk_one` per root into one visitor) reports the shared caller 1,000
/// times and fails "each state once".
#[test]
fn a_multi_root_walk_keeps_one_state_per_state_not_per_root() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let shared = w.rtn(&cu, "Shared");
    let roots: Vec<RoutineNodeId> = (0..1000).map(|i| w.rtn(&cu, &format!("R{i}"))).collect();
    for (i, r) in roots.iter().enumerate() {
        w.call(&shared, r, i as u32);
    }
    let g = sited_of(&w, a, &[]);
    let ixs: Vec<NodeIx> = roots.iter().map(|r| node(&g, r)).collect();
    let mut per = PerRoot::default();
    let mut scratch = WalkScratch::default();
    let stats = walk_multi(&g, &ixs, &WalkOptions::default(), &mut scratch, &mut per);
    assert_eq!(
        per.1, 1001,
        "each state once: 1,000 roots and the shared caller"
    );
    assert_eq!(stats.states, 1001);
    assert_eq!(per.0[&0], 2, "the shared caller is labelled with root 0");
    // The scratch is a bounded constant per state and per node, whatever the
    // root count.
    let bound = 64 * g.node_count() + 128 * stats.states + 4096;
    assert!(
        scratch.bytes() <= bound,
        "scratch {} > {bound}",
        scratch.bytes()
    );
    // And it is reused: a second walk of one root keeps the same capacity.
    let before = scratch.bytes();
    walk_one(
        &g,
        ixs[0],
        &WalkOptions::default(),
        &mut scratch,
        &mut PerRoot::default(),
    );
    assert!(scratch.bytes() <= before);
}

/// The state budget stops a walk and says so.
#[test]
fn the_state_budget_is_a_limit() {
    let mut w = World::default();
    let a = w.app("W");
    let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
    let r = w.rtn(&cu, "R");
    let c = w.rtn(&cu, "C");
    let d = w.rtn(&cu, "D");
    w.call(&c, &r, 1);
    w.call(&d, &c, 2);
    let g = sited_of(&w, a, &[]);
    let rec = walk(
        &g,
        &[node(&g, &r)],
        &WalkOptions {
            state_budget: 2,
            ..Default::default()
        },
    );
    assert_eq!(rec.limits, vec![Limit::StateBudget(2)]);
    assert_eq!(rec.states.len(), 2);
}

// ── Fixture tests (a real build) ────────────────────────────────────────────

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-selection-graph")
}

fn fixture_graph() -> (
    SitedGraph,
    std::sync::Arc<crate::program::inputs::WorkspaceInputs>,
) {
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &fixture(),
        &crate::engine::resident::session::REVISION_READ,
    );
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    let g = crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built");
    (g, inputs)
}

fn named(g: &SitedGraph, name_lc: &str) -> NodeIx {
    let ns = g.routines_named(name_lc);
    assert_eq!(ns.len(), 1, "{name_lc}: one routine");
    ns[0]
}

/// R0-1: the cone of `ApplyDecay` holds the test `NightlyRecalcDecays`,
/// tier `Local`, at the hop count counted by hand: ApplyDecay <- OnRun
/// (call) <- the `Acc Nightly Recalc` object (lifecycle) <- RecalcNightly
/// (`Codeunit.Run`) <- NightlyRecalcDecays (call) = 4 hops.
///
/// DISCRIMINATION (2026-11-05): stepping to the site's callee instead of its
/// caller (`node: st.to` in `walk_multi`) fails "the test is in the cone".
#[test]
fn apply_decay_is_reached_by_its_test_locally_in_four_hops() {
    let (g, _) = fixture_graph();
    let root = named(&g, "applydecay");
    let test = named(&g, "nightlyrecalcdecays");
    assert!(g.flags(test) & flag::TEST != 0, "precondition: a [Test]");
    let rec = walk(&g, &[root], &WalkOptions::default());
    let ts = rec.of(test);
    assert!(!ts.is_empty(), "the test is in the cone");
    let best = ts
        .iter()
        .min_by_key(|(s, h)| (s.tier, *h))
        .expect("a state");
    assert_eq!((best.0.tier, best.1), (Tier::Local, 4));
    assert!(
        rec.entries
            .iter()
            .any(|e| e.1 == test && e.2 == vec![EntryKind::TestProcedure]),
        "a test-procedure entry"
    );
}

/// The node table's StableRoutineIds equal the export's (`stableRoutineId`)
/// for every exported routine, and each looks its node up again.
#[test]
fn stable_ids_in_the_node_table_equal_the_exports() {
    let (g, inputs) = fixture_graph();
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    let export = crate::program::impact_graph::export_from_program(&p).expect("exported");
    let by_id: std::collections::HashMap<String, NodeIx> = (0..g.node_count() as u32)
        .map(|i| (g.node_id(i), i))
        .collect();
    let mut checked = 0;
    for n in &export.nodes {
        let Some(sid) = &n.stable_routine_id else {
            continue;
        };
        let ix = by_id[&n.id];
        assert_eq!(g.stable_id(ix).as_deref(), Some(sid.as_str()), "{}", n.id);
        assert!(g.nodes_with_stable_id(sid).contains(&ix));
        checked += 1;
    }
    assert!(
        checked > 20,
        "the fixture's routines were checked: {checked}"
    );
    assert!(g.nodes_with_stable_id("no#such").is_empty());
}

/// The sited graph's root kinds equal the model's AST root classification
/// for every workspace routine (one classifier, two input shapes).
///
/// DISCRIMINATION (2026-11-05): building the IR shape with
/// `default_access: true` in `facts.rs` fails on the fixture's `local`
/// procedures (`public-procedure` appears; the first is `RoundToNickel`).
#[test]
fn root_kinds_equal_the_models_classification() {
    let (g, inputs) = fixture_graph();
    let model = crate::program::model::program_calls::assemble_and_resolve_workspace_program(
        &inputs,
        true,
        crate::program::model::workspace::AssemblyNeeds::BASE,
    )
    .expect("the model builds");
    let stable = crate::engine::l4::summary::build_routine_stable_map(&model.workspace.routines);
    let mut checked = 0;
    for rc in &model.root_classifications {
        let Some(id) = stable.get(&rc.routine_id) else {
            continue;
        };
        let want = rc
            .kinds
            .iter()
            .filter_map(|k| crate::engine::root_classification::root_kind_bit(k))
            .fold(0u16, |m, b| m | b);
        for n in g.nodes_with_stable_id(id) {
            assert_eq!(g.root_kinds(n), want, "{}", g.display(n));
            checked += 1;
        }
    }
    // And no workspace routine has kinds the model does not.
    let classified: HashSet<&str> = model
        .root_classifications
        .iter()
        .filter_map(|rc| stable.get(&rc.routine_id).map(String::as_str))
        .collect();
    for n in 0..g.node_count() as u32 {
        if g.node_app(n) == Some(g.primary_app())
            && g.root_kinds(n) != 0
            && let Some(id) = g.stable_id(n)
        {
            assert!(classified.contains(id.as_str()), "{}", g.display(n));
        }
    }
    assert!(checked > 10, "checked {checked}");
}
