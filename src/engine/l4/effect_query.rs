//! `DbEffectQuery` — the engine-side query surface over the compact db-effect
//! store, and the ONLY thing outside `l4` that should touch
//! [`ReverseEffectIndex`] directly.
//!
//! ## The one question this answers
//!
//! > *Is table X touched by any DB **action** — read or write, temporary or
//! > physical — transitively, up or down the call stack from routine R?*
//!
//! State that scope before reaching for this module, because a NEIGHBOURING
//! substrate already answers a NARROWER question better:
//! [`crate::engine::l4::cone_derived::ConeDerivedStore`] is live on every
//! `analyze` run and exposes transitive `touches_table` /
//! `writes_physical_tables_of` / `physical_table_reads_of`. If the question is
//! **"does this write a physical table?"** use that one — do not build a second
//! answer to it. What it structurally cannot cover, and what sends you here:
//!
//! | carried by `db_effects` | DO evidence | `cone_derived` |
//! |---|---|---|
//! | reads on TEMP tables | 77 009 of 222 483 memberships (34.6%) are `temp = known(true)`; `physical_table_reads` excludes every one | absent |
//! | the full read verb set (`Get`/`Next`/`FindSet`/`CalcFields`/`IsEmpty`/`LockTable`/…) | 38 410 / 37 673 / 32 904 / 25 795 / 15 171 / 2 296 … | collapsed to one physical-read id set |
//! | parameter-dependent temp state | 493 memberships | not modeled |
//! | per-operation identity (`operation_id`) | 2 281 distinct | not carried |
//! | `via` provenance | direct 2 281 / implicit-trigger 452 / event-subscriber 23 / inherited 219 727 | not carried |
//!
//! ## Three queries, very different support
//!
//! | query | formally | how |
//! |---|---|---|
//! | **D — down** | does R's transitive effect cone touch X? | [`Self::touches_table`] (a posting probe) + [`Self::touches`] (the witnesses) |
//! | **U-global** | which routines ANYWHERE touch X? | [`Self::routines_touching`] |
//! | **U-scoped** | does a transitive CALLER of R touch X, through a branch that does not go through R? | [`Self::ancestors_touching`] |
//!
//! Summaries are transitive-DOWN, so `touches_table(R,X)` implies
//! `touches_table(A,X)` for every ancestor `A`. U-scoped is therefore only
//! *informative* when R itself does **not** touch X — the answer "your caller
//! writes X through a different branch". Shape an answer accordingly:
//!
//! - `touches_table(R,T)` **true** ⇒ report the down witnesses; the up answer is
//!   trivially "yes, all callers" and should be STATED, never enumerated.
//! - `touches_table(R,T)` **false** ⇒ "R does not touch T, but N of its M
//!   transitive callers do, through other branches" — [`Self::ancestors`] gives
//!   M, [`Self::ancestors_touching`] gives N, nearest-first.
//!
//! U-global alone is not a feature and must not be rendered raw: on DO the
//! per-table `routines_touching` cardinality has a **median of 377** of 3 685
//! routines (top 1 051 / 955 / 885 / 681, excluding the `"unknown"` bucket).
//! The scoping is what turns the substrate into an answer.
//!
//! ## Two design rules, held here so consumers cannot break them
//!
//! 1. **Membership from the index, WITNESSES from the bundle.**
//!    [`Self::touches_table`] is a posting probe; [`Self::touches`] re-reads
//!    [`SummaryBundle::db_effects`] for the hits, because the postings drop
//!    [`ViaRank`] and 98.8% of real memberships are `inherited` — so "yes" alone
//!    answers the less useful half of the user's question ("does THIS routine do
//!    it, or something twelve frames down?").
//! 2. **This facade returns [`RoutineIx`] + effect facts, never rendered
//!    names.** The join to `ModelRoutine`'s `name` / `object_type` /
//!    `stable_routine_id` / `source_anchor` belongs to the CONSUMER (see
//!    `effect_query_cli.rs`), because `l4` should not own presentation and the
//!    two eventual consumers want different shapes (a JSON row vs. an LSP
//!    `Location`).
//!
//! [`ReverseEffectIndex::class_members`] is never exposed past this facade —
//! see that module's doc for why rendering it would be a lie.

use std::collections::VecDeque;

use crate::engine::l4::combined_graph::CombinedGraph;
use crate::engine::l4::effect_lattice::TempStateKind;
use crate::engine::l4::effect_store::{SummaryBundle, ViaRank};
use crate::engine::l4::reverse_index::{GraphSccIx, ReverseEffectIndex, graph_scc_of};
use crate::engine::l4::routine_interner::RoutineIx;
use crate::engine::l4::scc::SccResult;

/// The literal sentinel `summary_runner`'s base extraction substitutes when a
/// record operation's target table could not be determined (`op.table_id` was
/// `None`) — see that module's `base_intraprocedural_summary`. It is **NOT a
/// table id**: it is the "effect is real, target unresolved" bucket, and on DO
/// it is the LARGEST posting of all (1 334 of 3 685 routines).
///
/// Consequences every surface must honour:
///
/// - It is never resolvable through `ws.tables`, so a name→id lookup must not
///   silently fall through to it.
/// - It is a legitimate thing to QUERY (`--table unknown` answers "which
///   routines have a db effect whose table we could not determine?"), so it is
///   not filtered out — [`DbEffectQuery`] treats it like any other key.
/// - It must be LABELLED when rendered ([`is_unknown_table`]); showing it as
///   though it were a table id would assert a resolution that never happened.
pub const UNKNOWN_TABLE_ID: &str = "unknown";

/// True iff `table_id` is the [`UNKNOWN_TABLE_ID`] sentinel rather than a real
/// table identity.
pub fn is_unknown_table(table_id: &str) -> bool {
    table_id == UNKNOWN_TABLE_ID
}

/// One WITNESSED db-effect touch, borrowed from the bundle's dictionaries.
///
/// `via` is what makes an answer actionable and it comes from
/// [`SummaryBundle::db_effects`], **not** from the posting lists (which drop it
/// — see the module doc's rule 1).
///
/// `table_id` is carried even though [`DbEffectQuery::touches`] already knows
/// it, so that ONE witness type serves both the table-filtered query and
/// [`DbEffectQuery::all_effects`]'s unfiltered down-list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableTouch<'a> {
    pub routine: RoutineIx,
    /// `Insert` / `Modify` / `Get` / `FindSet` / `CalcFields` / …
    pub op: &'a str,
    pub table_id: &'a str,
    pub operation_id: &'a str,
    pub temp_state: &'a TempStateKind,
    /// `direct` | `implicit-trigger` | `event-subscriber` | `dynamic` |
    /// `inherited`. 98.8% of real memberships are `inherited`.
    pub via: ViaRank,
}

/// A [`TableTouch`] performed by a transitive CALLER of the routine asked
/// about, plus how far up it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AncestorTouch<'a> {
    pub touch: TableTouch<'a>,
    /// BFS distance in the ORIGINAL Tarjan condensation: `0` = a cycle-mate
    /// (same SCC as the routine asked about), `1` = a direct caller's SCC, and
    /// so on. Lets a UI show nearest-first and cap the list honestly instead of
    /// dumping 1 051 rows.
    pub depth: u32,
}

/// Everything one db-effect question needs, assembled once per workspace
/// snapshot: the finished bundle, its transpose, the ORIGINAL Tarjan
/// condensation, and that condensation REVERSED.
pub struct DbEffectQuery<'a> {
    bundle: &'a SummaryBundle,
    index: ReverseEffectIndex,
    scc: &'a SccResult,
    /// `GraphSccIx` -> its predecessor SCCs (the reverse condensation), each
    /// list sorted + deduped so the BFS below is deterministic regardless of
    /// `graph.edges_by_from`'s `HashMap` iteration order. O(V + E) to build.
    rev_condensation: Vec<Vec<GraphSccIx>>,
}

impl<'a> DbEffectQuery<'a> {
    /// Build the query surface. `scc` MUST be the Tarjan result for `graph`
    /// (the same pair the summary solve consumed) — the ancestor walk reads
    /// both and they are meaningless apart.
    pub fn build(bundle: &'a SummaryBundle, scc: &'a SccResult, graph: &CombinedGraph) -> Self {
        let index = ReverseEffectIndex::build(bundle);
        let mut rev_condensation: Vec<Vec<GraphSccIx>> = vec![Vec::new(); scc.sccs.len()];
        for (from, edges) in &graph.edges_by_from {
            let Some(&from_scc) = scc.scc_id_by_routine.get(from) else {
                continue; // an edge out of a node Tarjan never saw — skip.
            };
            for e in edges {
                let Some(&to_scc) = scc.scc_id_by_routine.get(&e.to) else {
                    continue;
                };
                if from_scc != to_scc {
                    rev_condensation[to_scc].push(GraphSccIx(from_scc as u32));
                }
            }
        }
        for preds in &mut rev_condensation {
            preds.sort_unstable();
            preds.dedup();
        }
        DbEffectQuery {
            bundle,
            index,
            scc,
            rev_condensation,
        }
    }

    /// The bundle this query reads — for a consumer that needs
    /// [`SummaryBundle::routine_id`] to join a [`RoutineIx`] back to a routine.
    pub fn bundle(&self) -> &'a SummaryBundle {
        self.bundle
    }

    /// Look up a routine's [`RoutineIx`] by its internal id.
    pub fn routine_ix(&self, routine_id: &str) -> Option<RoutineIx> {
        self.bundle.routine_ix(routine_id)
    }

    // -- D: down ------------------------------------------------------------

    /// **D.** Does `r`'s transitive effect cone touch `table_id`? A pure
    /// posting-list probe — never decompresses `r`'s effect set.
    pub fn touches_table(&self, r: RoutineIx, table_id: &str) -> bool {
        self.index.touches_table(self.bundle, r, table_id)
    }

    /// **D, witnessed.** Every touch of `table_id` in `r`'s transitive cone, in
    /// the bundle's own `(effect_key, operation_id)` emit order.
    ///
    /// Membership could be answered by [`Self::touches_table`] alone; this
    /// re-reads the bundle precisely so `via` / `op` / `temp_state` survive
    /// (module-doc rule 1). Empty iff `touches_table` is false — asserted by the
    /// differential, not merely intended.
    pub fn touches(&self, r: RoutineIx, table_id: &str) -> Vec<TableTouch<'a>> {
        self.bundle
            .db_effects(r)
            .filter(|e| e.table_id == table_id)
            .map(|e| TableTouch {
                routine: r,
                op: e.op,
                table_id: e.table_id,
                operation_id: e.operation_id,
                temp_state: e.temp_state,
                via: e.via,
            })
            .collect()
    }

    /// **D, unfiltered.** `r`'s complete transitive effect cone — the same rows
    /// [`Self::touches`] filters, in the same order. Backs `alsem query
    /// effects`.
    pub fn all_effects(&self, r: RoutineIx) -> Vec<TableTouch<'a>> {
        self.bundle
            .db_effects(r)
            .map(|e| TableTouch {
                routine: r,
                op: e.op,
                table_id: e.table_id,
                operation_id: e.operation_id,
                temp_state: e.temp_state,
                via: e.via,
            })
            .collect()
    }

    // -- U-global -----------------------------------------------------------

    /// **U-global.** Every routine in the workspace that touches `table_id`,
    /// ascending [`RoutineIx`].
    ///
    /// WORKSPACE-WIDE and unscoped: on DO the median answer is 377 routines.
    /// Report the count and scope it (see [`Self::ancestors_touching`]) rather
    /// than rendering the list raw.
    pub fn routines_touching(&self, table_id: &str) -> Vec<RoutineIx> {
        self.index.up_table(table_id)
    }

    // -- U-scoped: the ancestor walk ----------------------------------------

    /// Every TRANSITIVE CALLER of `r` as `(depth, routine)` — its BFS depth in
    /// the ORIGINAL Tarjan condensation paired with the routine — ascending, so
    /// nearest callers come first. Depth leads the tuple so the natural `Ord`
    /// of an element is exactly this order.
    ///
    /// **`r` itself is excluded by definition**, including when `r` sits in a
    /// recursive SCC (where it genuinely does transitively call itself): a
    /// hover about `r` must not list `r`. Every OTHER member of `r`'s own SCC
    /// IS included, at depth 0 — an SCC with >= 2 members is strongly
    /// connected, so each such member really does reach `r`.
    ///
    /// Returns every INTERNED transitive caller — including one that has no
    /// compact row (a routine the solve never settled). That is the honest
    /// graph answer, and it is the right denominator for "N of M transitive
    /// callers": a rowless caller is a real caller we simply cannot state
    /// effects for. It can never survive into [`Self::ancestors_touching`],
    /// since no row means [`Self::touches_table`] is false. A graph node that
    /// was never interned at all (absent from the workspace routine set) is
    /// skipped — nothing in this bundle can name it.
    ///
    /// ## Notion discipline (the reason [`GraphSccIx`] exists)
    ///
    /// This walk uses the ORIGINAL Tarjan condensation, NEVER the effect-
    /// sharing `EffectClassIx` DAG. Effective SCCs are formed AFTER fixed
    /// leaves and missing routines are removed from the induced subgraph, and
    /// leaf removal changes REACHABILITY — an edge routed through a removed
    /// leaf disappears from the effective-SCC DAG, so an ancestor set computed
    /// there would be strictly too small. Pinned by
    /// `ancestors_are_computed_on_the_tarjan_condensation_not_the_effect_class_dag`.
    pub fn ancestors(&self, r: RoutineIx) -> Vec<(u32, RoutineIx)> {
        let Some(start) = graph_scc_of(self.scc, self.bundle, r) else {
            return Vec::new(); // r is not a node of the condensation at all.
        };
        let mut out: Vec<(u32, RoutineIx)> = Vec::new();
        for (scc_ix, depth) in self.ancestor_sccs(start) {
            for member_id in &self.scc.sccs[scc_ix.0 as usize].members {
                let Some(member) = self.bundle.routine_ix(member_id) else {
                    continue; // never interned => no row => not answerable.
                };
                if member == r {
                    continue;
                }
                out.push((depth, member));
            }
        }
        // DEPTH FIRST in the tuple, deliberately: the natural `Ord` of the
        // returned element then IS the documented order, so a caller can sort,
        // merge or binary-search the result without re-deriving the key — and
        // cannot write an ordering check that silently compares by the wrong
        // field (which is exactly what happened when this returned
        // `(RoutineIx, u32)` sorted by `(depth, ix)`).
        out.sort_unstable();
        out
    }

    /// **U-scoped** — the query the hover exists for: which transitive callers
    /// of `r` touch `table_id`, nearest first, WITH witnesses.
    ///
    /// Ordered `(depth, RoutineIx, bundle emit order)`; `r` itself is never
    /// included (see [`Self::ancestors`]).
    ///
    /// Read the result against [`Self::touches_table`]: when `r` itself touches
    /// `table_id` every ancestor trivially does too (summaries are
    /// transitive-down), so the informative case is the one where `r` does not.
    pub fn ancestors_touching(&self, r: RoutineIx, table_id: &str) -> Vec<AncestorTouch<'a>> {
        let mut out: Vec<AncestorTouch<'a>> = Vec::new();
        for (depth, ancestor) in self.ancestors(r) {
            // Membership from the INDEX (cheap posting probe) — witnesses from
            // the bundle, and only for the routines that passed.
            if !self.touches_table(ancestor, table_id) {
                continue;
            }
            for touch in self.touches(ancestor, table_id) {
                out.push(AncestorTouch { touch, depth });
            }
        }
        out
    }

    /// BFS over the reverse condensation from `start`, yielding each reachable
    /// SCC exactly once with its MINIMUM depth (`start` itself at depth 0).
    /// Deterministic: `rev_condensation`'s adjacency lists are sorted+deduped
    /// at build.
    fn ancestor_sccs(&self, start: GraphSccIx) -> Vec<(GraphSccIx, u32)> {
        let n = self.rev_condensation.len();
        let start_ix = start.0 as usize;
        if start_ix >= n {
            return Vec::new();
        }
        let mut depth: Vec<Option<u32>> = vec![None; n];
        let mut out: Vec<(GraphSccIx, u32)> = Vec::new();
        let mut queue: VecDeque<GraphSccIx> = VecDeque::new();
        depth[start_ix] = Some(0);
        queue.push_back(start);
        while let Some(cur) = queue.pop_front() {
            let d = depth[cur.0 as usize].expect("enqueued nodes always carry a depth");
            out.push((cur, d));
            for &pred in &self.rev_condensation[cur.0 as usize] {
                if depth[pred.0 as usize].is_none() {
                    depth[pred.0 as usize] = Some(d + 1);
                    queue.push_back(pred);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::combined_graph::CombinedEdge;
    use crate::engine::l4::effect_store::{SummaryBundleBuilder, set_bit};
    use crate::engine::l4::effect_universe::{EffectId, EffectIdentity, GrowingEffectUniverse};
    use crate::engine::l4::routine_interner::RoutineInterner;
    use crate::engine::l4::scc::{SccInputGraph, tarjan_scc};

    fn ident(op: &str, table: &str, opid: &str) -> EffectIdentity {
        EffectIdentity {
            op: op.into(),
            table_id: table.into(),
            operation_id: opid.into(),
            temp: TempStateKind::Known(true),
        }
    }

    fn bits_of(ids: &[EffectId]) -> Vec<u64> {
        let mut b = Vec::new();
        for &id in ids {
            set_bit(&mut b, id);
        }
        b
    }

    fn combined(nodes: &[&str], edges: &[(&str, &str)]) -> CombinedGraph {
        let mut edges_by_from: HashMap<String, Vec<CombinedEdge>> = HashMap::new();
        let mut edges_from_order: Vec<String> = Vec::new();
        for (from, to) in edges {
            if !edges_by_from.contains_key(*from) {
                edges_from_order.push((*from).to_string());
            }
            edges_by_from
                .entry((*from).to_string())
                .or_default()
                .push(CombinedEdge {
                    from: (*from).to_string(),
                    to: (*to).to_string(),
                    kind: "direct".to_string(),
                    callsite_id: None,
                    operation_id: None,
                    event_id: None,
                    subscriber_app_id: None,
                    resolution: "resolved".to_string(),
                });
        }
        let mut node_vec: Vec<String> = nodes.iter().map(|n| (*n).to_string()).collect();
        node_vec.sort();
        CombinedGraph {
            nodes: node_vec,
            edges_by_from,
            edges_from_order,
            uncertainty_edges: Vec::new(),
            typed_edges: Vec::new(),
        }
    }

    fn tarjan_of(graph: &CombinedGraph) -> SccResult {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for (from, list) in &graph.edges_by_from {
            adjacency.insert(from.clone(), list.iter().map(|e| e.to.clone()).collect());
        }
        tarjan_scc(&SccInputGraph {
            nodes: &graph.nodes,
            edges_by_from: &adjacency,
        })
    }

    /// A 4-node chain `top -> mid -> low -> sink` where ONLY `top` and `sink`
    /// touch `TCUST`. Every precondition is hand-stated (the bundle rows are
    /// literal), so nothing here depends on the solver producing a particular
    /// shape.
    struct Chain {
        bundle: SummaryBundle,
        graph: CombinedGraph,
        scc: SccResult,
        top: RoutineIx,
        mid: RoutineIx,
        low: RoutineIx,
        sink: RoutineIx,
    }

    fn chain() -> Chain {
        let mut u = GrowingEffectUniverse::new();
        let cust_top = u.intern(&ident("Insert", "TCUST", "op_top"));
        let cust_sink = u.intern(&ident("Get", "TCUST", "op_sink"));
        let other = u.intern(&ident("Modify", "TOTHER", "op_mid"));

        let mut interner = RoutineInterner::new();
        let top = interner.intern("top");
        let mid = interner.intern("mid");
        let low = interner.intern("low");
        let sink = interner.intern("sink");

        let mut b = SummaryBundleBuilder::new();
        let s_top = b.push_terminal_set(bits_of(&[cust_top]));
        b.push_row(top, s_top, vec![ViaRank::Direct], vec![], vec![]);
        let s_mid = b.push_terminal_set(bits_of(&[other]));
        b.push_row(mid, s_mid, vec![ViaRank::Inherited], vec![], vec![]);
        let s_low = b.push_terminal_set(bits_of(&[]));
        b.push_row(low, s_low, vec![], vec![], vec![]);
        let s_sink = b.push_terminal_set(bits_of(&[cust_sink]));
        b.push_row(sink, s_sink, vec![ViaRank::Direct], vec![], vec![]);

        let graph = combined(
            &["top", "mid", "low", "sink"],
            &[("top", "mid"), ("mid", "low"), ("low", "sink")],
        );
        let scc = tarjan_of(&graph);
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        Chain {
            bundle: b.finish(u.freeze(), interner, rvid),
            graph,
            scc,
            top,
            mid,
            low,
            sink,
        }
    }

    #[test]
    fn ancestors_are_nearest_first_and_exclude_the_routine_itself() {
        let fx = chain();
        let q = DbEffectQuery::build(&fx.bundle, &fx.scc, &fx.graph);

        assert_eq!(
            q.ancestors(fx.sink),
            vec![(1, fx.low), (2, fx.mid), (3, fx.top)],
            "sink's callers, nearest first; sink itself absent"
        );
        assert_eq!(q.ancestors(fx.top), Vec::new(), "top has no callers");
        assert_eq!(q.ancestors(fx.mid), vec![(1, fx.top)]);
    }

    #[test]
    fn ancestors_touching_finds_the_caller_that_touches_through_another_branch() {
        let fx = chain();
        let q = DbEffectQuery::build(&fx.bundle, &fx.scc, &fx.graph);

        // `low` does NOT touch TCUST itself — the informative case.
        assert!(!q.touches_table(fx.low, "TCUST"));
        let ups = q.ancestors_touching(fx.low, "TCUST");
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].touch.routine, fx.top);
        assert_eq!(ups[0].depth, 2, "top is 2 SCCs above low");
        assert_eq!(ups[0].touch.op, "Insert");
        assert_eq!(ups[0].touch.via, ViaRank::Direct);

        // `mid` touches TOTHER directly; its only ancestor is `top`.
        assert!(q.touches_table(fx.mid, "TOTHER"));
        assert!(q.ancestors_touching(fx.mid, "TOTHER").is_empty());
    }

    #[test]
    fn touches_carries_the_via_the_postings_drop_and_agrees_with_touches_table() {
        let fx = chain();
        let q = DbEffectQuery::build(&fx.bundle, &fx.scc, &fx.graph);

        let hits = q.touches(fx.sink, "TCUST");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].op, "Get");
        assert_eq!(hits[0].operation_id, "op_sink");
        assert_eq!(hits[0].table_id, "TCUST");
        assert_eq!(hits[0].temp_state, &TempStateKind::Known(true));

        // The bidirectional agreement the differential then asserts wholesale.
        for r in [fx.top, fx.mid, fx.low, fx.sink] {
            for t in ["TCUST", "TOTHER", "TABSENT"] {
                assert_eq!(
                    q.touches_table(r, t),
                    !q.touches(r, t).is_empty(),
                    "touches_table and touches must never disagree"
                );
            }
        }
        assert!(q.touches(fx.low, "TABSENT").is_empty());
    }

    #[test]
    fn routines_touching_is_workspace_global_and_ascending() {
        let fx = chain();
        let q = DbEffectQuery::build(&fx.bundle, &fx.scc, &fx.graph);
        assert_eq!(q.routines_touching("TCUST"), vec![fx.top, fx.sink]);
        assert_eq!(q.routines_touching("TOTHER"), vec![fx.mid]);
        assert!(q.routines_touching("TABSENT").is_empty());
    }

    /// The `"unknown"` bucket is a first-class, queryable answer — not filtered
    /// out and not silently rendered as a table.
    #[test]
    fn unknown_table_bucket_is_queryable_and_labelled() {
        let mut u = GrowingEffectUniverse::new();
        let unk = u.intern(&ident("Insert", UNKNOWN_TABLE_ID, "op_u"));
        let mut interner = RoutineInterner::new();
        let r = interner.intern("r");
        let mut b = SummaryBundleBuilder::new();
        let s = b.push_terminal_set(bits_of(&[unk]));
        b.push_row(r, s, vec![ViaRank::Direct], vec![], vec![]);
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);

        let graph = combined(&["r"], &[]);
        let scc = tarjan_of(&graph);
        let q = DbEffectQuery::build(&bundle, &scc, &graph);

        assert!(q.touches_table(r, UNKNOWN_TABLE_ID));
        assert_eq!(q.routines_touching(UNKNOWN_TABLE_ID), vec![r]);
        assert!(is_unknown_table(q.touches(r, UNKNOWN_TABLE_ID)[0].table_id));
        assert!(!is_unknown_table("TCUST"));
    }

    /// ⟨scope §2.4⟩ THE notion-discipline test. A 3-node cycle `a -> b -> c ->
    /// a` in which `b` is a FIXED LEAF: `effective_sccs` (the real production
    /// function) splits the cycle into two singleton effect classes, so `a` and
    /// `c` land in DIFFERENT `EffectClassIx`es — and an ancestor walk computed
    /// on the effect-class DAG would find NO path from `a` to `c` at all.
    /// Tarjan's condensation, which `ancestors` actually walks, still sees one
    /// 3-member SCC, so `a` IS an ancestor of `c` (and vice versa).
    ///
    /// This is the single mistake the two newtypes exist to prevent, so the
    /// test asserts the premise (classes differ, Tarjan SCC identical) BEFORE
    /// asserting the answer.
    #[test]
    fn ancestors_are_computed_on_the_tarjan_condensation_not_the_effect_class_dag() {
        use crate::engine::l4::db_effect_solver::effective_sccs;
        use crate::engine::l4::reverse_index::class_of;

        let graph = combined(&["a", "b", "c"], &[("a", "b"), ("b", "c"), ("c", "a")]);
        let scc = tarjan_of(&graph);
        assert_eq!(scc.sccs.len(), 1, "Tarjan sees ONE 3-member cycle");

        // `b` is the fixed leaf — the real production splitter.
        let eff = effective_sccs(&scc.sccs[0], &graph, &|id: &str| id != "b");
        assert_eq!(
            eff.len(),
            2,
            "leaf removal splits the cycle in the EFFECT DAG"
        );

        let mut u = GrowingEffectUniverse::new();
        let e_a = u.intern(&ident("Insert", "TA", "op_a"));
        let e_c = u.intern(&ident("Insert", "TC", "op_c"));
        let mut interner = RoutineInterner::new();
        let ia = interner.intern("a");
        let _ib = interner.intern("b");
        let ic = interner.intern("c");
        let mut b = SummaryBundleBuilder::new();
        let s_a = b.push_terminal_set(bits_of(&[e_a]));
        b.push_row(ia, s_a, vec![ViaRank::Direct], vec![], vec![]);
        let s_c = b.push_terminal_set(bits_of(&[e_c]));
        b.push_row(ic, s_c, vec![ViaRank::Direct], vec![], vec![]);
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);

        // Premise: DIFFERENT effect classes, SAME Tarjan SCC.
        assert_ne!(class_of(&bundle, ia), class_of(&bundle, ic));
        assert_eq!(
            scc.scc_id_by_routine.get("a"),
            scc.scc_id_by_routine.get("c")
        );

        let q = DbEffectQuery::build(&bundle, &scc, &graph);
        // Cycle-mates are ancestors at depth 0. `b` is interned but ROWLESS —
        // it is still a real caller, so `ancestors` reports it (see that
        // method's contract) while `ancestors_touching` cannot: no row means
        // no effect class, so `touches_table` is false for it.
        let ib = bundle.routine_ix("b").expect("b was interned");
        assert_eq!(q.ancestors(ic), vec![(0, ia), (0, ib)]);
        assert_eq!(q.ancestors(ia), vec![(0, ib), (0, ic)]);
        assert!(
            !q.touches_table(ib, "TA"),
            "a rowless routine touches nothing"
        );
        // The payoff: `c` does not touch TA, but its cycle-mate `a` does.
        assert!(!q.touches_table(ic, "TA"));
        let ups = q.ancestors_touching(ic, "TA");
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].touch.routine, ia);
        assert_eq!(ups[0].depth, 0);
    }

    /// A routine with a row but no place in the condensation (never a graph
    /// node) answers "no ancestors" instead of panicking.
    #[test]
    fn a_routine_outside_the_condensation_has_no_ancestors() {
        let mut u = GrowingEffectUniverse::new();
        let e = u.intern(&ident("Insert", "T", "op"));
        let mut interner = RoutineInterner::new();
        let orphan = interner.intern("orphan");
        let mut b = SummaryBundleBuilder::new();
        let s = b.push_terminal_set(bits_of(&[e]));
        b.push_row(orphan, s, vec![ViaRank::Direct], vec![], vec![]);
        let rvid: HashMap<String, Option<String>> = HashMap::new();
        let bundle = b.finish(u.freeze(), interner, rvid);

        let graph = combined(&["someone-else"], &[]);
        let scc = tarjan_of(&graph);
        let q = DbEffectQuery::build(&bundle, &scc, &graph);
        assert!(q.ancestors(orphan).is_empty());
        assert!(q.ancestors_touching(orphan, "T").is_empty());
    }
}
