//! G35: one example path to each resource of a dependency boundary group.
//!
//! Under rule B a witness path ends where it enters dependency code, and a
//! boundary group lists every table that code reaches. A `prove` answer on such
//! a group named the boundary only: Continia DO's `writes-table:CDO E-Mail Log`
//! was `yes` through `Base Application.CalcInvDiscForHeader`, with no way to
//! see how Base Application code reaches a DO table.
//!
//! The path comes from the guarded search G23/G33 already run over the same
//! graph the cone was built over ([`LiteralReach::live_tree`]): from the root,
//! through call sites its known arguments do not rule out, to the first state
//! of a routine owning one of the resource's originating operations where that
//! operation can run. So a branch G33 proves dead (an approval `case` on the
//! wrong table) is never shown. It is an example of how the resource is
//! reached, not the only way, and like any witness it is not checked further
//! for feasibility.

//!
//! MCP output wave 2, W2-0b (design §3.4.3, G1): the path must TRAVERSE the
//! group's crossing. Before, it was the first state in the root's search
//! order owning the operation, so a shorter workspace route to the same
//! operation won and a group "beyond `Codeunit X::Y`" was shown a path that
//! never entered `X::Y` (Continia DO: `RunReportID`'s `CDO E-Mail Log` beyond
//! `Codeunit Sales-Calc. Discount::OnRun`). Now the path is the root's
//! search up to one of the crossing's typed edges, then the search from that
//! entry with the bindings it had ([`LiteralReach::group_witness`]). The
//! `hops` strings keep their shape; [`GroupPath`] adds the structured
//! witness beside them.

use crate::engine::l5::digest::DigestEffectResult;
use crate::engine::l5::literal_reach::{
    CrossingEntries, EdgeSite, GroupWitness, LiteralReach, LiveTree, Step, Stop, SuffixCache,
};
use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::model::workspace::Model;
use std::collections::HashMap;

/// One structured hop of a group witness: the edge kind, its site id (call
/// site, firing operation or event), the routine entered (stable id and
/// owner-qualified display), and where the edge leaves from in its CALLING
/// routine's source (0-based line, as the model keeps it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessStep {
    pub edge: String,
    pub edge_id: Option<String>,
    pub to: String,
    pub display: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// The operation a group witness ends at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessTerminal {
    pub operation_id: String,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// One group resource's path (G35, W2-0b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupPath {
    /// `prove`'s `groupPath.hops`, unchanged in shape: the root's
    /// `Object.Routine`, then `<edge kind> → <Object.Routine>` per hop.
    pub hops: Vec<String>,
    /// The path traverses the group's crossing.
    pub through_entry_point: bool,
    /// Why not, when it does not (`search-cap`, `cancelled`).
    pub reason: Option<&'static str>,
    /// The crossing's boundary identity.
    pub crossing: String,
    pub steps: Vec<WitnessStep>,
    pub terminal: WitnessTerminal,
}

/// A group resource's path, or why it has none: `no-witness` (the searches
/// completed and none traverses the crossing to a live operation),
/// `search-cap` / `cancelled` (a search stopped early and the root has no
/// path either), `no-operation-id` (a fact of the resource names none).
pub type GroupPathOutcome = Result<GroupPath, NoGroupPath>;

/// Why a group resource has no path, and the crossing it was searched
/// through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoGroupPath {
    pub reason: &'static str,
    pub crossing: String,
}

/// Edge kinds and routine names, to render a path.
pub(crate) struct GroupPaths<'a> {
    /// (from, to) → the first edge kind between them.
    kind: HashMap<(&'a str, &'a str), &'a str>,
    /// Stable routine id → `Object.Routine`.
    display: HashMap<&'a str, String>,
    /// Stable routine id → `<Type> <Object>::<Routine>` (MCP output plan 1d).
    qualified: HashMap<&'a str, String>,
}

impl<'a> GroupPaths<'a> {
    pub(crate) fn new(edges: &'a [SnapshotGraphEdge], model: &'a Model) -> Self {
        let mut kind = HashMap::new();
        for e in edges {
            if let Some(to) = e.edge_to() {
                kind.entry((e.edge_from(), to)).or_insert(e.edge_kind());
            }
        }
        let objects: HashMap<&str, (&str, &str)> = model
            .workspace
            .objects
            .iter()
            .map(|o| (o.id.as_str(), (o.object_type.as_str(), o.name.as_str())))
            .collect();
        let mut display = HashMap::new();
        let mut qualified = HashMap::new();
        for r in &model.workspace.routines {
            let object = objects.get(r.object_id.as_str());
            display
                .entry(r.stable_routine_id.as_str())
                .or_insert_with(|| match object {
                    Some((_, o)) => format!("{o}.{}", r.name),
                    None => r.name.clone(),
                });
            qualified
                .entry(r.stable_routine_id.as_str())
                .or_insert_with(|| match object {
                    Some((t, o)) => format!("{t} {o}::{}", r.name),
                    None => r.name.clone(),
                });
        }
        GroupPaths {
            kind,
            display,
            qualified,
        }
    }

    /// Per group resource of `eff`, one path from the root that traverses
    /// the group's crossing (`entries`, from [`LiteralReach::crossing_entries`])
    /// to a routine where one of the resource's operations (`candidates`:
    /// per resource, the operations that could write it, `None` when one
    /// cannot be judged; W2-0c: [`LiteralReach::candidate_ops`], every direct
    /// fact of the kind in the group's class, so the path and the resource's
    /// status read one set) can run (module doc). Empty off a group; a
    /// resource with an operation not known, or no witness, has none. A path
    /// that does not traverse the crossing comes only from a search that
    /// stopped early, and says so.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paths(
        &self,
        reach: &LiteralReach<'a>,
        tree: &LiveTree<'a>,
        entries: &CrossingEntries<'a>,
        eff: &DigestEffectResult,
        candidates: &[(String, Option<Vec<String>>)],
        cache: &mut SuffixCache<'a>,
        cancel: &dyn Fn() -> bool,
    ) -> Vec<(String, GroupPathOutcome)> {
        let Some(crossing) = eff.crossing.as_deref().filter(|_| eff.boundary_group) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (resource, ops) in candidates {
            let none = |reason| NoGroupPath {
                reason,
                crossing: crossing.id.clone(),
            };
            let Some(ops) = ops else {
                out.push((resource.clone(), Err(none("no-operation-id"))));
                continue;
            };
            let w = reach.group_witness(tree, crossing, entries, ops, cache, cancel);
            let (steps, terminal, through, reason) = match &w {
                GroupWitness::Through {
                    steps, terminal, ..
                } => (steps, terminal, true, None),
                GroupWitness::Outside {
                    steps,
                    terminal,
                    reason,
                } => (
                    steps,
                    terminal,
                    false,
                    Some(match reason {
                        Stop::Capped => "search-cap",
                        Stop::Cancelled => "cancelled",
                    }),
                ),
                GroupWitness::Unavailable(stop) => {
                    out.push((
                        resource.clone(),
                        Err(none(match stop {
                            None => "no-witness",
                            Some(Stop::Capped) => "search-cap",
                            Some(Stop::Cancelled) => "cancelled",
                        })),
                    ));
                    continue;
                }
            };
            let routines = w.routines();
            let mut hops = vec![self.name(routines[0])];
            for p in routines.windows(2) {
                let kind = self.kind.get(&(p[0], p[1])).copied().unwrap_or("call");
                hops.push(format!("{kind} → {}", self.name(p[1])));
            }
            out.push((
                resource.clone(),
                Ok(GroupPath {
                    hops,
                    through_entry_point: through,
                    reason,
                    crossing: crossing.id.clone(),
                    steps: steps.iter().map(|s| self.step(reach, s)).collect(),
                    terminal: self.terminal(reach, terminal),
                }),
            ));
        }
        out
    }

    /// A witness step, structured.
    fn step(&self, reach: &LiteralReach<'a>, s: &Step<'a>) -> WitnessStep {
        let (edge_id, anchor) = match s.site {
            Some(EdgeSite::Guarded(id)) => {
                let caller = reach.routine(s.from);
                let anchor = caller.and_then(|r| {
                    r.call_sites
                        .iter()
                        .find(|c| c.id == id)
                        .map(|c| &c.source_anchor)
                        .or_else(|| {
                            r.record_operations
                                .iter()
                                .find(|o| o.id == id)
                                .map(|o| &o.source_anchor)
                        })
                });
                (Some(id.to_string()), anchor)
            }
            Some(EdgeSite::Event(id)) => (Some(id.to_string()), None),
            None => (None, None),
        };
        WitnessStep {
            edge: s.kind.to_string(),
            edge_id,
            to: s.to.to_string(),
            display: self
                .qualified
                .get(s.to)
                .cloned()
                .unwrap_or_else(|| s.to.to_string()),
            file: anchor.map(|a| a.source_unit_id.clone()),
            line: anchor.map(|a| a.start_line),
        }
    }

    /// The witness's terminal operation, with its anchor.
    fn terminal(&self, reach: &LiteralReach<'a>, op: &str) -> WitnessTerminal {
        let anchor = reach
            .op_owner(op)
            .and_then(|o| reach.routine(o))
            .and_then(|r| {
                r.record_operations
                    .iter()
                    .find(|o| o.id == op)
                    .map(|o| &o.source_anchor)
                    .or_else(|| {
                        r.operation_sites
                            .iter()
                            .find(|o| o.id == op)
                            .map(|o| &o.source_anchor)
                    })
            });
        WitnessTerminal {
            operation_id: op.to_string(),
            file: anchor.map(|a| a.source_unit_id.clone()),
            line: anchor.map(|a| a.start_line),
        }
    }

    fn name(&self, stable: &str) -> String {
        self.display
            .get(stable)
            .cloned()
            .unwrap_or_else(|| stable.to_string())
    }
}
