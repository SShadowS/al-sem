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

use crate::engine::l5::digest::DigestEffectResult;
use crate::engine::l5::literal_reach::{LiteralReach, LiveTree};
use crate::engine::l5::snapshot::SnapshotGraphEdge;
use crate::program::model::workspace::Model;
use std::collections::HashMap;

/// Edge kinds and routine names, to render a path.
pub(crate) struct GroupPaths<'a> {
    /// (from, to) → the first edge kind between them.
    kind: HashMap<(&'a str, &'a str), &'a str>,
    /// Stable routine id → `Object.Routine`.
    display: HashMap<&'a str, String>,
}

impl<'a> GroupPaths<'a> {
    pub(crate) fn new(edges: &'a [SnapshotGraphEdge], model: &'a Model) -> Self {
        let mut kind = HashMap::new();
        for e in edges {
            if let Some(to) = e.edge_to() {
                kind.entry((e.edge_from(), to)).or_insert(e.edge_kind());
            }
        }
        let object_names: HashMap<&str, &str> = model
            .workspace
            .objects
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str()))
            .collect();
        let mut display = HashMap::new();
        for r in &model.workspace.routines {
            display
                .entry(r.stable_routine_id.as_str())
                .or_insert_with(|| match object_names.get(r.object_id.as_str()) {
                    Some(o) => format!("{o}.{}", r.name),
                    None => r.name.clone(),
                });
        }
        GroupPaths { kind, display }
    }

    /// Per group resource of `eff`, one path from the root to a routine where
    /// one of the resource's operations can run: the root's display, then
    /// `<edge kind> → <routine>` per hop. Empty off a group; a resource with an
    /// operation not known, or none reached, has none.
    pub(crate) fn paths(
        &self,
        reach: &LiteralReach<'a>,
        tree: &LiveTree<'a>,
        eff: &DigestEffectResult,
    ) -> Vec<(String, Vec<String>)> {
        if !eff.boundary_group {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (resource, ops) in &eff.group_resource_ops {
            let Some(ops) = ops else {
                continue;
            };
            let Some(routines) = reach.live_path(tree, ops) else {
                continue;
            };
            let mut hops = vec![self.name(routines[0])];
            for w in routines.windows(2) {
                let kind = self.kind.get(&(w[0], w[1])).copied().unwrap_or("call");
                hops.push(format!("{kind} → {}", self.name(w[1])));
            }
            out.push((resource.clone(), hops));
        }
        out
    }

    fn name(&self, stable: &str) -> String {
        self.display
            .get(stable)
            .cloned()
            .unwrap_or_else(|| stable.to_string())
    }
}
