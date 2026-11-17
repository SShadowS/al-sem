//! ENG-125 Task 2: pass 1's fields equal each root's own walk. The oracle is the per-root
//! visitor: one `Up` per root over its own single walk (`walk_multi`), then `summary`. Every
//! pass-1 field of every root is compared (plan §Per-field plan, the P1 rows), with the
//! nearest ancestors and the page-action sites.
//!
//! DISCRIMINATION (2026-11-17, `cargo test -p al-sem --lib multi_p1`), each red, then green on
//! revert:
//! 1. the worst tier taken as a node's best: `ancestors`;
//! 2. node class masks without the Local split: `byRelation`;
//! 3. the root's own nodes left out of its external callers: `externalCode`;
//! 4. a hub's location from a site whose caller the root did not reach: `hubs`.
//!
//! LIMIT, a fifth break that stayed green: canonical node entries counted for the root's own
//! nodes as well (`canon` given every root of the event, not `rest`). That exclusion (`Up::entry`
//! returns early for a root node) is redundant with `changed`: every root node's canonical entry
//! is a changed entry, in `up()` (every node with a changed stable id) and here (every root
//! node), so no root counts it either way. Kept to match `Up`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use serde_json::Value;

use super::super::{Captions, Keep, Shared, Up, summary};
use super::UpMulti;
use crate::program::impact_graph::SitedGraph;
use crate::program::impact_graph::multi_walk::{multi_walk, tests::every_shape};
use crate::program::impact_graph::reverse::{NodeIx, WalkOptions, WalkScratch, walk_multi};

/// The keys pass 1 fills completely.
const FULL: [&str; 21] = [
    "rootEntryKinds",
    "rootEntryWord",
    "rootUserOrJob",
    "hubs",
    "programEntries",
    "entryBuckets",
    "states",
    "maxHops",
    "ancestors",
    "userOrJobEntries",
    "hidden",
    "limited",
    "relationRecorded",
    "byRelation",
    "entriesByKind",
    "entriesByContext",
    "programEntriesByKind",
    "programShare",
    "externalCode",
    "entryNotes",
    "nearestAncestors",
];

/// `v` without its `first` lists (pass 2's), at any depth.
fn counts_only(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| k.as_str() != "first")
                .map(|(k, v)| (k.clone(), counts_only(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(counts_only).collect()),
        other => other.clone(),
    }
}

/// What a comparison saw, for a test's preconditions.
#[derive(Default)]
pub(crate) struct Seen {
    pub(crate) roots: usize,
    pub(crate) limited: usize,
    pub(crate) page_action_rows: usize,
    pub(crate) publisher_rows: usize,
    pub(crate) external_rows: usize,
    pub(crate) hub_rows: usize,
    pub(crate) candidate_rows: usize,
    pub(crate) full_ancestor_lists: usize,
}

/// Asserts that pass 1 over one multi walk of `roots` gives every root its own `Up`'s fields.
pub(crate) fn assert_p1_equal(
    g: &SitedGraph,
    roots: &[Vec<NodeIx>],
    opts: &WalkOptions<'_>,
    keep: Keep,
) -> Seen {
    let changed: BTreeSet<NodeIx> = roots.iter().flatten().copied().collect();
    let shared = Shared::new(g, &changed);
    let captions = Captions::new(HashMap::new());
    let mut multi = UpMulti::new(g, keep, &shared, opts, roots);
    let stats = multi_walk(g, roots, opts, &mut multi);
    let rows = multi.rows_p1(&stats.roots);
    let mut seen = Seen::default();
    let mut scratch = WalkScratch::default();
    for (k, lanes) in roots.iter().enumerate() {
        let mut up = Up::new(g, keep, lanes, &shared);
        walk_multi(g, lanes, opts, &mut scratch, &mut up);
        let mut want = summary(&up, g, &captions);
        want["nearestAncestors"] = serde_json::json!(
            up.ancestors
                .iter()
                .filter_map(|&n| g.stable_id(n).map(|id| serde_json::json!({
                    "stableId": id,
                    "display": crate::engine::test_select::select::display(g, n),
                    "hops": up.best.get(&n).map(|b| b.hops)
                })))
                .collect::<Vec<_>>()
        );
        let got = &rows[k].fields;
        for key in FULL {
            assert_eq!(got.get(key), want.get(key), "root {k} {lanes:?}: `{key}`");
        }
        for key in ["entries", "external", "eventPublishers"] {
            assert_eq!(
                got.get(key).map(counts_only),
                want.get(key).map(counts_only),
                "root {k} {lanes:?}: `{key}` (counts)"
            );
        }
        let sites: BTreeMap<NodeIx, u32> = up.action_site.iter().map(|(&n, &s)| (n, s)).collect();
        assert_eq!(
            rows[k].action_site, sites,
            "root {k} {lanes:?}: page-action sites"
        );
        seen.roots += 1;
        seen.limited += usize::from(!want["limited"].is_null());
        seen.page_action_rows += usize::from(!up.action_site.is_empty());
        seen.publisher_rows += usize::from(want["eventPublishers"]["count"] != 0);
        seen.external_rows +=
            usize::from(want["external"].as_array().is_some_and(|a| !a.is_empty()));
        seen.hub_rows += usize::from(want["hubs"].as_array().is_some_and(|a| !a.is_empty()));
        let c = &want["byRelation"]["ancestors"]["candidates"];
        let candidate_ancestors = c["overHub"].as_u64().unwrap_or(0)
            + c["overRule"].as_u64().unwrap_or(0)
            + c["overConfiguration"]["code"].as_u64().unwrap_or(0)
            + c["overConfiguration"]["uiNavigation"].as_u64().unwrap_or(0);
        seen.candidate_rows += usize::from(
            candidate_ancestors != 0 || want["programShare"]["candidates"]["reached"] != 0,
        );
        seen.full_ancestor_lists += usize::from(up.ancestors.len() == keep.ancestors);
    }
    seen
}

fn fixture_graph(dir: &str) -> SitedGraph {
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir),
        &crate::engine::resident::session::REVISION_READ,
    );
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built")
}

/// Every pass-1 field of every root on three real fixtures: with the default options, with
/// no dependents (no external callers), at a small state budget, and with ancestor lists of
/// two (they fill).
#[test]
fn multi_p1_rows_equal_the_per_root_visitor() {
    let mut total = Seen::default();
    for dir in [
        "tests/fixtures/test-selection-graph",
        "tests/fixtures/test-universe/App",
        "tests/fixtures/test-universe/Test",
        "tests/fixtures/dependency-callers/ws",
    ] {
        let g = fixture_graph(dir);
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
            per_list: 3,
            ancestors: 2,
        };
        for (opts, keep) in [
            (WalkOptions::default(), Keep::new(false)),
            (no_dependents, Keep::new(false)),
            (budget, Keep::new(false)),
            (WalkOptions::default(), short),
        ] {
            let s = assert_p1_equal(&g, &roots, &opts, keep);
            total.roots += s.roots;
            total.limited += s.limited;
            total.page_action_rows += s.page_action_rows;
            total.publisher_rows += s.publisher_rows;
            total.external_rows += s.external_rows;
            total.hub_rows += s.hub_rows;
            total.candidate_rows += s.candidate_rows;
            total.full_ancestor_lists += s.full_ancestor_lists;
        }
    }
    assert!(
        total.roots > 64,
        "precondition: more than one word of roots"
    );
    assert!(total.limited > 0, "precondition: a limited root");
    assert!(
        total.page_action_rows > 0,
        "precondition: a page-action site"
    );
    assert!(total.publisher_rows > 0, "precondition: an event publisher");
    assert!(total.external_rows > 0, "precondition: an external caller");
    assert!(
        total.full_ancestor_lists > 0,
        "precondition: a full ancestor list"
    );
    eprintln!("hub rows {}", total.hub_rows);
}
