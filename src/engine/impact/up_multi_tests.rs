//! ENG-125 Tasks 2-3: every root's row from the two multi-walk passes is the same bytes as its
//! own walk's. The oracle is the per-root visitor: one `Up` per root over its own single walk
//! (`walk_multi`), then `summary`. The comparison is the printed JSON text (key order counts),
//! with real captions, plus the nearest ancestors.
//!
//! DISCRIMINATION, pass 1 (2026-11-17, `cargo test -p al-sem --lib multi_p1`, on the
//! per-field form of this test), each red, then green on revert:
//! 1. the worst tier taken as a node's best: `ancestors`;
//! 2. node class masks without the Local split: `byRelation`;
//! 3. the root's own nodes left out of its external callers: `externalCode`;
//! 4. a hub's location from a site whose caller the root did not reach: `hubs`.
//!
//! DISCRIMINATION, pass 2 (2026-11-17, `cargo test -p al-sem --lib multi_rows`), each red, then
//! green on revert:
//! 1. the only-open partition admitting every open arrival, not only nodes whose final
//!    category is only open: `entries`;
//! 2. list items without their arrival number (ties not broken by walk order): `external`,
//!    `entries`.
//!
//! Two pass-2 breaks stayed green here and got their own worlds in `up_tests`, each red then
//! green: a full list that never takes a newcomer (`a_full_list_takes_a_nearer_arrival_from_
//! later_in_the_level`), and a class witness admitted by the arrival's class instead of the
//! entry's final class (`an_entry_counts_only_in_its_best_population`). The ancestor witness's
//! `no_entries` mask is a cost guard only (`fill` reads the ancestor only when the population
//! has no entry), so it has no proof.
//!
//! LIMIT, a fifth pass-1 break that stayed green: canonical node entries counted for the root's own
//! nodes as well (`canon` given every root of the event, not `rest`). That exclusion (`Up::entry`
//! returns early for a root node) is redundant with `changed`: every root node's canonical entry
//! is a changed entry, in `up()` (every node with a changed stable id) and here (every root
//! node), so no root counts it either way. Kept to match `Up`.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use super::super::{Captions, Keep, Shared, Up, row_json, summary};
use super::{UpMulti, UpMultiP2};
use crate::engine::test_select::select::display;
use crate::program::impact_graph::SitedGraph;
use crate::program::impact_graph::multi_walk::{multi_walk, tests::every_shape};
use crate::program::impact_graph::reverse::{NodeIx, WalkOptions, WalkScratch, walk_multi};

/// What a comparison saw, for a test's preconditions.
#[derive(Default)]
pub(crate) struct Seen {
    pub(crate) roots: usize,
    pub(crate) limited: usize,
    pub(crate) captions: usize,
    pub(crate) publishers: usize,
    pub(crate) externals: usize,
    pub(crate) full_lists: usize,
    pub(crate) open_only_firsts: usize,
    pub(crate) candidates: usize,
    pub(crate) ancestor_nearest: usize,
    pub(crate) full_ancestor_lists: usize,
}

/// The nearest-ancestor list as `root_row` prints it.
fn ancestors_json(g: &SitedGraph, list: impl Iterator<Item = (NodeIx, Option<u32>)>) -> Value {
    json!(
        list.filter_map(|(n, hops)| {
            g.stable_id(n)
                .map(|id| json!({"stableId": id, "display": display(g, n), "hops": hops}))
        })
        .collect::<Vec<_>>()
    )
}

/// Asserts that the two passes over one multi walk of `roots` give every root the bytes its
/// own `Up` gives.
pub(crate) fn assert_rows_equal(
    g: &SitedGraph,
    roots: &[Vec<NodeIx>],
    opts: &WalkOptions<'_>,
    keep: Keep,
    captions: &Captions,
) -> Seen {
    let changed: BTreeSet<NodeIx> = roots.iter().flatten().copied().collect();
    let shared = Shared::new(g, &changed);
    let mut p1 = UpMulti::new(g, keep, &shared, opts, roots);
    let stats = multi_walk(g, roots, opts, &mut p1);
    let (mut rows, finals) = p1.rows_p1(&stats.roots);
    let mut p2 = UpMultiP2::new(&p1, &finals);
    multi_walk(g, roots, opts, &mut p2);
    p2.fill(&mut rows);
    let mut seen = Seen::default();
    let mut scratch = WalkScratch::default();
    for (k, lanes) in roots.iter().enumerate() {
        let mut up = Up::new(g, keep, lanes, &shared);
        walk_multi(g, lanes, opts, &mut scratch, &mut up);
        let mut want = summary(&up, g, captions);
        want["nearestAncestors"] = ancestors_json(
            g,
            up.ancestors
                .iter()
                .map(|&n| (n, up.best.get(&n).map(|b| b.hops))),
        );
        let mut got = row_json(&rows[k], g, &shared, captions);
        got["nearestAncestors"] = ancestors_json(
            g,
            p1.nearest_ancestors(k)
                .iter()
                .map(|&(n, _, hops)| (n, Some(hops))),
        );
        let (want_s, got_s) = (want.to_string(), got.to_string());
        if want_s != got_s {
            let key = want
                .as_object()
                .and_then(|w| w.keys().find(|key| want[key.as_str()] != got[key.as_str()]))
                .cloned()
                .unwrap_or_else(|| "(key order)".into());
            panic!(
                "root {k} {lanes:?}: `{key}` differs\n  want {}\n  got  {}",
                want.get(&key).unwrap_or(&Value::Null),
                got.get(&key).unwrap_or(&Value::Null)
            );
        }
        seen.roots += 1;
        seen.limited += usize::from(!want["limited"].is_null());
        seen.captions += usize::from(want_s.contains("\"caption\":"));
        seen.publishers += usize::from(want["eventPublishers"]["count"] != 0);
        seen.externals += usize::from(want["external"].as_array().is_some_and(|a| !a.is_empty()));
        let lists = want["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(want["external"].as_array().into_iter().flatten());
        for l in lists {
            let first = l["first"].as_array().map_or(0, Vec::len);
            seen.full_lists +=
                usize::from(first == keep.per_list && l["count"].as_u64() > Some(first as u64));
            seen.open_only_firsts += usize::from(
                l["first"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|n| n["tier"] == "onlyOpen"),
            );
        }
        let c = &want["byRelation"]["ancestors"]["candidates"];
        seen.candidates += usize::from(
            c["overHub"] != 0
                || c["overRule"] != 0
                || c["overConfiguration"]["code"] != 0
                || c["overConfiguration"]["uiNavigation"] != 0,
        );
        seen.ancestor_nearest += want["nearest"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, v)| !v.is_null() && v.get("entry").is_none())
            .count();
        seen.full_ancestor_lists += usize::from(up.ancestors.len() == keep.ancestors);
    }
    seen
}

/// A fixture's sited graph and its captions (the analyzed source's texts).
pub(crate) fn fixture(dir: &str) -> (SitedGraph, Captions) {
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir),
        &crate::engine::resident::session::REVISION_READ,
    );
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    let g = crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built");
    let texts: HashMap<String, Arc<str>> = inputs
        .al_files
        .iter()
        .filter_map(|f| {
            f.text
                .as_ref()
                .ok()
                .map(|t| (f.rel_posix.clone(), Arc::clone(t)))
        })
        .collect();
    (g, Captions::new(texts))
}

/// ENG-125 Task 4, on the pinned CDO baseline: every root's row from the two passes, in one
/// chunk, is the bytes of its own walk's, for an even sample of about 256 workspace routines
/// (four words of roots), with real captions.
#[test]
fn multi_rows_equal_the_per_root_visitor_on_cdo() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    crate::big_stack::run_with_big_stack(move || {
        let inputs = crate::program::inputs::WorkspaceInputs::read(
            &ws,
            &crate::engine::resident::session::REVISION_READ,
        );
        let p = crate::program::resolved::ResolvedProgram::build(
            &inputs,
            crate::program::resolved::ProgramScope::CrossApp,
        )
        .expect("CDO builds");
        let g = crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built");
        let texts: HashMap<String, Arc<str>> = inputs
            .al_files
            .iter()
            .filter_map(|f| {
                f.text
                    .as_ref()
                    .ok()
                    .map(|t| (f.rel_posix.clone(), Arc::clone(t)))
            })
            .collect();
        let workspace: Vec<NodeIx> = (0..g.node_count() as u32)
            .filter(|&n| g.stable_id(n).is_some() && g.node_app(n) == Some(g.primary_app()))
            .collect();
        let step = (workspace.len() / 256).max(1);
        let roots: Vec<Vec<NodeIx>> = workspace.iter().step_by(step).map(|&n| vec![n]).collect();
        let seen = assert_rows_equal(
            &g,
            &roots,
            &WalkOptions::default(),
            Keep::new(false),
            &Captions::new(texts),
        );
        assert!(
            seen.roots > 192,
            "precondition: more than three words of roots"
        );
        assert!(
            seen.publishers > 0 && seen.externals > 0,
            "precondition: real lists"
        );
        assert!(
            seen.full_lists > 0,
            "precondition: a list with more nodes than it shows"
        );
    });
}

/// Every root's row, as bytes, on five real fixtures: with the default options, with no
/// dependents (no external callers), at a small state budget, with ancestor lists of two (they
/// fill), and with the detail lists (ten witnesses).
#[test]
fn multi_rows_equal_the_per_root_visitor() {
    let mut total = Seen::default();
    for dir in [
        "tests/fixtures/test-selection-graph",
        "tests/fixtures/test-universe/App",
        "tests/fixtures/test-universe/Test",
        "tests/fixtures/dependency-callers/ws",
        "tests/fixtures/up-parallel/ws",
    ] {
        let (g, captions) = fixture(dir);
        let roots = every_shape(&g);
        let no_dependents = WalkOptions {
            assume_no_dependents: true,
            ..WalkOptions::default()
        };
        let budget = WalkOptions {
            state_budget: 7,
            ..WalkOptions::default()
        };
        let short = Keep {
            per_list: 1,
            ancestors: 2,
        };
        for (opts, keep) in [
            (WalkOptions::default(), Keep::new(false)),
            (no_dependents, Keep::new(false)),
            (budget, Keep::new(false)),
            (WalkOptions::default(), short),
            (WalkOptions::default(), Keep::new(true)),
        ] {
            let s = assert_rows_equal(&g, &roots, &opts, keep, &captions);
            total.roots += s.roots;
            total.limited += s.limited;
            total.captions += s.captions;
            total.publishers += s.publishers;
            total.externals += s.externals;
            total.full_lists += s.full_lists;
            total.open_only_firsts += s.open_only_firsts;
            total.ancestor_nearest += s.ancestor_nearest;
            total.full_ancestor_lists += s.full_ancestor_lists;
        }
    }
    assert!(
        total.roots > 64,
        "precondition: more than one word of roots"
    );
    assert!(total.limited > 0, "precondition: a limited root");
    assert!(total.captions > 0, "precondition: a page-action caption");
    assert!(total.publishers > 0, "precondition: an event publisher");
    assert!(total.externals > 0, "precondition: an external caller");
    assert!(
        total.full_lists > 0,
        "precondition: a list with more nodes than it shows"
    );
    assert!(
        total.ancestor_nearest > 0,
        "precondition: a nearest witness that is an ancestor"
    );
    assert!(
        total.full_ancestor_lists > 0,
        "precondition: a full ancestor list"
    );
    eprintln!("open-only firsts {}", total.open_only_firsts);
}
