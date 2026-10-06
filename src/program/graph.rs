//! The whole-program node graph: app-qualified nodes + topology-scoped lookup.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use std::collections::{BTreeSet, HashMap};

use crate::program::node::{AppRef, AppRegistry};
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::node_set::NodeSet;
use crate::program::topology::DependencyGraph;

/// Index from (app, kind, lowercase-name) to position in `ProgramGraph::objects`.
///
/// Built once after `objects` is sorted; first entry wins on a same-app duplicate.
#[derive(Default)]
pub struct ObjectIndex {
    by_app_kind_name: HashMap<(AppRef, ObjectKind, String), usize>,
}

impl ObjectIndex {
    /// Build the index from `objects`, in sorted order. Each entry's position
    /// is its index in that iteration order, so the caller must iterate in the
    /// same order it later indexes by (`ProgramGraph::objects`' merged order).
    /// On a duplicate `(app, kind, name_lc)` key the first (lowest-`NodeId`) entry wins.
    pub fn build<'a>(objects: impl IntoIterator<Item = &'a ObjectNode>) -> Self {
        let mut idx = ObjectIndex::default();
        for (i, obj) in objects.into_iter().enumerate() {
            let key = (obj.id.app, obj.id.kind, obj.name.fold_identifier());
            idx.by_app_kind_name.entry(key).or_insert(i);
        }
        idx
    }
}

/// The assembled whole-program graph: app-qualified nodes + dependency topology.
#[derive(Default)]
pub struct ProgramGraph {
    pub apps: AppRegistry,
    pub topology: DependencyGraph,
    /// All object nodes, sorted by `ObjectNodeId` for determinism. A
    /// `NodeSet`: the dependency part is shared, never copied (see
    /// `program::node_set`).
    pub objects: NodeSet<ObjectNode>,
    /// All routine nodes, sorted by `RoutineNodeId` for determinism.
    pub routines: NodeSet<RoutineNode>,
    pub obj_index: ObjectIndex,
    /// `internalsVisibleTo` friend-app authorizations (Task 1.5), keyed by
    /// the app EXPOSING `internal` members → the set of caller apps its own
    /// manifest's `<InternalsVisibleTo><Module .../></InternalsVisibleTo>`
    /// declares as friends. One-directional per the DECLARING (exposing)
    /// app — `friends[B].contains(A)` means B trusts A, not the reverse.
    /// Consulted by [`crate::program::resolve::resolver`]'s per-candidate
    /// `Access::Internal` visibility rule as a cross-app fallback alongside
    /// the same-app check. Populated in [`crate::program::build::build_program_graph`]
    /// (Step 3b); empty in every in-memory test fixture that doesn't
    /// explicitly wire it (`..Default::default()`).
    pub friends: HashMap<AppRef, BTreeSet<AppRef>>,
    /// Per-app dependency-ABI ingest diagnostics (Tier-1 remediation, H-3):
    /// one entry per SymbolOnly dep whose `SymbolReference.json` could not
    /// be read or parsed. Previously this signal (`SymbolReferenceAbi::
    /// error`) existed but had ZERO production reads — a broken dependency
    /// silently ingested as an empty ABI, indistinguishable from a
    /// genuinely-empty one. Populated in `build::build_program_graph`'s Step
    /// 2b; empty on every successful ingest (the overwhelmingly common
    /// case) and in every in-memory test fixture that doesn't explicitly
    /// wire it.
    pub abi_ingest_errors: Vec<AbiIngestError>,
    /// The WORKSPACE's declarations as they occur in source, before the sort and
    /// dedup above (engine-switch S2b.3): one row per occurrence, with the node it
    /// became. Empty for dependency-only and in-memory test graphs.
    pub workspace_rows: crate::program::physical::PhysicalIndex,
}

/// One dependency-ABI ingest failure (H-3) — see
/// [`ProgramGraph::abi_ingest_errors`]'s doc.
#[derive(Debug, Clone)]
pub struct AbiIngestError {
    pub app: AppRef,
    pub message: String,
}

impl ProgramGraph {
    /// Resolve `(kind, name)` as seen FROM `from` — fail-closed (I1).
    ///
    /// Search order:
    /// 1. An object declared in `from` itself always wins (own-app shadow) —
    ///    short-circuits before any cross-app ambiguity check.
    /// 2. Otherwise, exactly ONE match among `from`'s dependency closure
    ///    resolves; more than one VISIBLE dependency match is an unprovable
    ///    cross-app collision and this DECLINES (`None`) rather than guessing
    ///    — a confident WRONG pick (the old lowest-`ObjectNodeId` tiebreak) is
    ///    the cardinal sin (I1: a false `Source` route).
    ///
    /// An app that is **not** in `from`'s transitive dependency closure is
    /// never matched — topology-scoped, never flat-global.
    pub fn resolve_object(
        &self,
        from: AppRef,
        kind: ObjectKind,
        name: &str,
    ) -> Option<&ObjectNode> {
        let name_lc = name.fold_identifier();

        // Prefer `from` itself — short-circuit before building the full closure.
        if let Some(&idx) = self
            .obj_index
            .by_app_kind_name
            .get(&(from, kind, name_lc.clone()))
        {
            return Some(&self.objects[idx]);
        }

        // No own-app declaration: search the rest of the closure. More than
        // one VISIBLE dependency match is ambiguous — decline rather than
        // pick the lowest `ObjectNodeId`. `by_app_kind_name` already holds at
        // most one entry per `(app, kind, name_lc)` (first/lowest-id wins on
        // a same-app duplicate — see `ObjectIndex::build`), so at most one
        // match can come from any single app in the closure.
        let closure = self.topology.closure(from);
        let mut found: Option<usize> = None;
        for &app in &closure {
            if app == from {
                continue;
            }
            if let Some(&idx) = self
                .obj_index
                .by_app_kind_name
                .get(&(app, kind, name_lc.clone()))
            {
                if found.is_some() {
                    return None; // >1 dependency declares this (kind, name) — decline.
                }
                found = Some(idx);
            }
        }
        found.map(|i| &self.objects[i])
    }

    /// Look up an interned `AppRef` by name (case-insensitive).
    /// Panics if the name is not present — intended for tests and CLI helpers.
    pub fn app_ref_by_name(&self, name: &str) -> AppRef {
        self.apps
            .find_by_name(name)
            .unwrap_or_else(|| panic!("app not found in registry: {name}"))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use al_syntax::ir::ObjectKind;

    use crate::program::node::{ObjKey, ObjectNodeId};
    use crate::program::node_extract::ObjectNode;
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, TrustTier};

    /// Construct a minimal two-app `ProgramGraph` entirely in memory (no I/O).
    ///
    /// Topology: A depends on B; B has no dependencies.
    /// Objects:
    ///   - codeunit "Util"   in A
    ///   - codeunit "Util"   in B
    ///   - codeunit "OnlyInA" in A
    fn build_two_app_fixture() -> ProgramGraph {
        let mut apps = AppRegistry::default();
        let a_id = AppId {
            guid: String::new(),
            name: "AppA".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let b_id = AppId {
            guid: String::new(),
            name: "AppB".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let a = apps.intern(&a_id);
        let b = apps.intern(&b_id);

        let mut topology = DependencyGraph::default();
        topology.add_dependency(a, b); // A → B; B has no reverse dep on A.

        let make_obj = |app: AppRef, name: &str| ObjectNode {
            id: ObjectNodeId {
                app,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Name(name.to_ascii_lowercase()),
            },
            name: name.to_string(),
            declared_id: None,
            extends_target: None,
            implements: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        };

        let mut objects = vec![
            make_obj(a, "Util"),
            make_obj(b, "Util"),
            make_obj(a, "OnlyInA"),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);

        ProgramGraph {
            apps,
            topology,
            objects: objects.into(),
            routines: vec![].into(),
            obj_index,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_object_is_topology_scoped_not_global() {
        // App A depends on B. Both define codeunit "Util". A call from A must
        // resolve to A's own Util (nearest), and B cannot see A's Util at all.
        let g = build_two_app_fixture();
        let a = g.app_ref_by_name("AppA");
        let b = g.app_ref_by_name("AppB");

        let from_a = g.resolve_object(a, ObjectKind::Codeunit, "Util").unwrap();
        assert_eq!(from_a.id.app, a, "A resolves its own Util");

        let from_b = g.resolve_object(b, ObjectKind::Codeunit, "Util").unwrap();
        assert_eq!(from_b.id.app, b, "B resolves its own Util, never A's");

        // B does NOT depend on A, so an A-only object is invisible from B.
        assert!(
            g.resolve_object(b, ObjectKind::Codeunit, "OnlyInA")
                .is_none(),
            "OnlyInA must not be visible from B (B's closure excludes A)"
        );
    }

    /// Construct a three-app `ProgramGraph` entirely in memory (no I/O), for the
    /// I1 root-fix tests (cross-app dependency collision must DECLINE, never
    /// silently pick the lowest `ObjectNodeId`).
    ///
    /// Topology: A depends on B and C (B, C have no dependencies of their own).
    /// Objects:
    ///   - table "Shared"    in B and C (collides; neither is A's own app)
    ///   - table "OwnShadow" in A AND B (A's own declaration must still win)
    fn build_three_app_ambiguous_fixture() -> ProgramGraph {
        let mut apps = AppRegistry::default();
        let mk_id = |name: &str| AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let a = apps.intern(&mk_id("AppA"));
        let b = apps.intern(&mk_id("AppB"));
        let c = apps.intern(&mk_id("AppC"));

        let mut topology = DependencyGraph::default();
        topology.add_dependency(a, b);
        topology.add_dependency(a, c);

        let make_obj = |app: AppRef, declared_id: Option<i64>, name: &str| ObjectNode {
            id: ObjectNodeId {
                app,
                kind: ObjectKind::Table,
                key: match declared_id {
                    Some(n) => ObjKey::Id(n),
                    None => ObjKey::Name(name.to_ascii_lowercase()),
                },
            },
            name: name.to_string(),
            declared_id,
            extends_target: None,
            implements: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            parse_incomplete: false,
        };

        let mut objects = vec![
            make_obj(b, Some(100), "Shared"),
            make_obj(c, Some(101), "Shared"),
            make_obj(a, Some(200), "OwnShadow"),
            make_obj(b, Some(201), "OwnShadow"),
        ];
        objects.sort_by(|x, y| x.id.cmp(&y.id));

        let obj_index = ObjectIndex::build(&objects);

        ProgramGraph {
            apps,
            topology,
            objects: objects.into(),
            routines: vec![].into(),
            obj_index,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_object_declines_on_cross_app_dependency_collision_never_lowest_id() {
        // AppB and AppC both declare table "Shared" — neither is A's own app.
        // The pre-fix behavior silently picked the lowest ObjectNodeId (a
        // confident WRONG guess — I1); the fixed behavior must decline
        // (`None`) instead of guessing which dependency "wins".
        let g = build_three_app_ambiguous_fixture();
        let a = g.app_ref_by_name("AppA");

        assert!(
            g.resolve_object(a, ObjectKind::Table, "Shared").is_none(),
            "cross-app dependency collision must decline (None), never silently pick the lowest id"
        );
    }

    #[test]
    fn resolve_object_own_app_shadow_survives_dependency_collision() {
        // AppA declares its own "OwnShadow"; AppB ALSO declares a same-name
        // table — A's own declaration must still win outright (own-app
        // shadow preserved even though a dependency collision exists too).
        let g = build_three_app_ambiguous_fixture();
        let a = g.app_ref_by_name("AppA");
        let b = g.app_ref_by_name("AppB");

        let resolved = g.resolve_object(a, ObjectKind::Table, "OwnShadow").unwrap();
        assert_eq!(
            resolved.id.app, a,
            "own-app declaration must shadow a colliding dependency"
        );
        assert_ne!(resolved.id.app, b);
    }
}
