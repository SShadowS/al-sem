//! The whole-program node graph: app-qualified nodes + topology-scoped lookup.

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId};
use crate::program::node_extract::{InterfaceRef, ObjectNode, RoutineNode};
use crate::program::node_set::NodeSet;
use crate::program::topology::DependencyGraph;
use serde::{Deserialize, Serialize};

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
    /// G28: the workspace interfaces declared `Access = Internal` while every
    /// `internalsVisibleTo` friend of the workspace is loaded. No app outside
    /// the program can implement one, so a call on it dispatches only to the
    /// implementers the program holds. Empty in dependency-only and in-memory
    /// test graphs.
    pub closed_internal_interfaces: HashSet<ObjectNodeId>,
    /// G30: the workspace manifest names no `internalsVisibleTo` friend, so
    /// only workspace code calls its `internal` routines. False in
    /// dependency-only and in-memory test graphs.
    pub workspace_friendless: bool,
    /// Every table with source (any app) → the `LookupPageId` values (raw
    /// property text) it and the loaded table extensions of it declare; empty
    /// when none does. `Page.Run(0, Rec)` runs that page (`resolve::full`).
    /// Empty in dependency-only and in-memory test graphs.
    pub lookup_pages: HashMap<ObjectNodeId, Vec<String>>,
    /// Per-app dependency-ABI ingest diagnostics (Tier-1 remediation, H-3):
    /// one entry per SymbolOnly dep whose `SymbolReference.json` could not
    /// be read or parsed. Previously this signal (`SymbolReferenceAbi::
    /// error`) existed but had ZERO production reads — a broken dependency
    /// silently ingested as an empty ABI, indistinguishable from a
    /// genuinely-empty one. Populated in `build::build_program_graph`'s Step
    /// 2b; empty on every successful ingest (the overwhelmingly common
    /// case) and in every in-memory test fixture that doesn't explicitly
    /// wire it.
    ///
    /// A package whose symbols the loader cannot parse at all is dropped before
    /// the graph build (`dependencies::load_all_apps`, Phase 3) and never
    /// appears here; `FreshCoverage` reports it. A package the loader's typed
    /// parse accepts but the ABI reader's stricter whole-value check rejects (a
    /// `1e400` or a lone surrogate in a member the loader skips) is loaded and
    /// lands here (TLS-47; the two rules differ, ENG-68). Printed by
    /// `aldump --program-call-graph-stats` and the dependency ledger.
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

/// An interface name as interface binding compares it (ENG-152): folded, with
/// AL's escaped quote (`""`) read as one quote, so a source spelling (`A""B`)
/// and a symbol spelling (`A"B`) of one name agree. shortcut: local to
/// interfaces; ENG-169 gives every identifier one spelling and removes this.
pub fn interface_name_key(name: &str) -> String {
    name.fold_identifier().replace("\"\"", "\"")
}

/// What an interface reference binds to (ENG-152, [`ProgramGraph::bind_interface`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InterfaceBinding {
    /// The one interface it names: proven.
    Exact(ObjectNodeId),
    /// Several it may name (sorted): reach them all, prove nothing.
    Visible(Vec<ObjectNodeId>),
    /// None of its name is visible from its scope: every interface of the name
    /// in the program (sorted), and the one it names may be in an app that is
    /// not loaded, so the unbound implementers of the name count too (review,
    /// sol). Reach them all, prove nothing, and never read it as a cataloged
    /// interface (G29).
    Unseen(Vec<ObjectNodeId>),
    /// No interface node it can name.
    Unbound,
}

impl InterfaceBinding {
    /// The interfaces it may name.
    pub fn ids(&self) -> &[ObjectNodeId] {
        match self {
            Self::Exact(id) => std::slice::from_ref(id),
            Self::Visible(ids) | Self::Unseen(ids) => ids,
            Self::Unbound => &[],
        }
    }

    /// Whether the interface it names may be one no loaded app declares:
    /// dispatch then reaches the unbound implementers of its name too.
    pub fn may_be_unloaded(&self) -> bool {
        matches!(self, Self::Unseen(_) | Self::Unbound)
    }

    /// The interface it proves, if any.
    pub fn exact(&self) -> Option<&ObjectNodeId> {
        match self {
            Self::Exact(id) => Some(id),
            _ => None,
        }
    }
}

impl ProgramGraph {
    /// ENG-152: what interface reference `r`, read in `from`'s scope, names.
    /// - Qualified (`r.app`, from symbols or synthesized text): the interface
    ///   of that name in that app, `Exact`; that app or name absent: `Unbound`
    ///   (never rebound to another app's interface of the name), unless the
    ///   app was only INFERRED (`r.inferred`: a bare symbol reference read as the
    ///   package's own), which then binds as a bare reference below.
    /// - Bare (hand-written source, namespaces erased: ENG-158): the
    ///   interfaces of the name in `from` and its dependency closure. One:
    ///   `Exact` (alc refuses two without namespaces, AL0275, and the own app
    ///   does not shadow, AL0197). Several: `Visible(all)`. None visible: every
    ///   interface of the name in the program, `Unseen` (a missing topology
    ///   edge or package must not lose reach), or `Unbound` when there is none.
    pub fn bind_interface(&self, from: AppRef, r: &InterfaceRef) -> InterfaceBinding {
        let kind = ObjectKind::Interface;
        // A name holding a quote has two spellings (ENG-169): a source
        // declaration and every reference keep AL's escape (`A""B`), a
        // symbol `Name` does not (`A"B`). Both are looked up.
        let name_lc = interface_name_key(&r.name);
        let escaped = name_lc.replace('"', "\"\"");
        let at = |app: AppRef| {
            [&name_lc, &escaped].into_iter().find_map(|n| {
                self.obj_index
                    .by_app_kind_name
                    .get(&(app, kind, n.clone()))
                    .map(|&i| self.objects[i].id.clone())
            })
        };
        if let Some(guid) = &r.app {
            let mut ids: Vec<ObjectNodeId> = self.apps.find_by_guid(guid).filter_map(at).collect();
            ids.sort();
            match ids.len() {
                // A WRITTEN app that lacks the name stays unbound (never
                // rebound); an INFERRED one (a bare symbol reference read as the
                // package's own) was a wrong inference: bind it as bare.
                0 if r.inferred => {}
                0 => return InterfaceBinding::Unbound,
                1 => return InterfaceBinding::Exact(ids.remove(0)),
                // Two versions of one app interned: either may be meant.
                _ => return InterfaceBinding::Visible(ids),
            }
        }
        let mut apps = self.topology.closure(from);
        apps.insert(from);
        let mut ids: Vec<ObjectNodeId> = apps.into_iter().filter_map(at).collect();
        if ids.is_empty() {
            ids = self
                .objects
                .iter()
                .filter(|o| o.id.kind == kind && interface_name_key(&o.name) == name_lc)
                .map(|o| o.id.clone())
                .collect();
            ids.sort();
            ids.dedup();
            return if ids.is_empty() {
                InterfaceBinding::Unbound
            } else {
                InterfaceBinding::Unseen(ids)
            };
        }
        ids.sort();
        if ids.len() == 1 {
            InterfaceBinding::Exact(ids.remove(0))
        } else {
            InterfaceBinding::Visible(ids)
        }
    }

    /// The object node with this id ([`NodeSet::find`]: what a binary search over the
    /// id-sorted `objects` finds).
    pub fn object(&self, id: &ObjectNodeId) -> Option<&ObjectNode> {
        self.objects.find(id).map(|i| &self.objects[i])
    }

    /// Resolve `(kind, name)` as seen FROM `from` — fail-closed (I1).
    ///
    /// Exactly ONE match among `from`'s dependency closure, `from` itself
    /// included and NOT preferred (ENG-193: alc ranks no app first by name; an
    /// own-app shadow bound the workspace's `W.X` where `namespace C; using A;`
    /// binds a dependency's `A.X`, a false `Source` route). More than one
    /// visible match is an unprovable collision and this DECLINES (`None`)
    /// rather than guessing — a confident WRONG pick is the cardinal sin (I1).
    /// An object declared `ObsoleteState = Moved`/`Removed`
    /// ([`ObjectNode::obsolete_gone`]) counts only when no other match is
    /// visible, as in `ResolveIndex::build` (Base Application's `Moved` stub of
    /// `No. Series` gives way to Business Foundation's table).
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
        // `by_app_kind_name` holds at most one entry per `(app, kind, name_lc)`
        // (first/lowest-id wins on a same-app duplicate — `ObjectIndex::build`),
        // so at most one match comes from any single app.
        let mut found: Vec<usize> = self
            .topology
            .closure(from)
            .into_iter()
            .filter_map(|app| {
                self.obj_index
                    .by_app_kind_name
                    .get(&(app, kind, name_lc.clone()))
                    .copied()
            })
            .collect();
        if found.iter().any(|&i| !self.objects[i].obsolete_gone) {
            found.retain(|&i| !self.objects[i].obsolete_gone);
        }
        match found.as_slice() {
            [i] => Some(&self.objects[*i]),
            _ => None,
        }
    }

    /// [`Self::resolve_object`] for an object named by NUMBER (an
    /// `[EventSubscriber(ObjectType::Codeunit, 50, …)]`; engine-switch S4.3b):
    /// `from`'s own object wins, else exactly one app in `from`'s dependency
    /// closure; more than one declines.
    pub fn resolve_object_by_number(
        &self,
        from: AppRef,
        kind: ObjectKind,
        number: i64,
    ) -> Option<&ObjectNode> {
        let find = |app: AppRef| {
            let id = ObjectNodeId {
                app,
                kind,
                key: ObjKey::Id(number),
            };
            self.objects.find(&id)
        };
        if let Some(i) = find(from) {
            return Some(&self.objects[i]);
        }
        let mut found: Option<usize> = None;
        for &app in &self.topology.closure(from) {
            if app == from {
                continue;
            }
            if let Some(i) = find(app) {
                if found.is_some() {
                    return None; // >1 dependency declares this (kind, number) — decline.
                }
                found = Some(i);
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
                key: ObjKey::Name(name.to_ascii_lowercase().into()),
            },
            name: name.into(),
            declared_id: None,
            extends_target: None,
            implements: vec![],
            interface_bases: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            query_columns: Vec::new(),
            dataset: Default::default(),
            protected_vars: Vec::new(),
            parse_incomplete: false,
            closed_enum: false,
            internal: false,
            enum_impls: Vec::new(),
            lookup_pages: Vec::new(),
            obsolete_gone: false,
            page_ui: Default::default(),
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
        // App A depends on B. Both define codeunit "Util". From A both are
        // visible and neither is preferred (ENG-193: no own-app shadow), so A
        // declines; B cannot see A's Util at all, so B resolves its own.
        let g = build_two_app_fixture();
        let a = g.app_ref_by_name("AppA");
        let b = g.app_ref_by_name("AppB");

        assert!(
            g.resolve_object(a, ObjectKind::Codeunit, "Util").is_none(),
            "A sees two Utils: declines"
        );

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
                    None => ObjKey::Name(name.to_ascii_lowercase().into()),
                },
            },
            name: name.into(),
            declared_id,
            extends_target: None,
            implements: vec![],
            interface_bases: vec![],
            tier: TrustTier::Workspace,
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields: vec![],
            dataitems: vec![],
            query_columns: Vec::new(),
            dataset: Default::default(),
            protected_vars: Vec::new(),
            parse_incomplete: false,
            closed_enum: false,
            internal: false,
            enum_impls: Vec::new(),
            lookup_pages: Vec::new(),
            obsolete_gone: false,
            page_ui: Default::default(),
        };

        let mut objects = vec![
            make_obj(b, Some(100), "Shared"),
            make_obj(c, Some(101), "Shared"),
            make_obj(a, Some(200), "OwnShadow"),
            make_obj(b, Some(201), "OwnShadow"),
            // By-number collisions (S4.3b): 300 in two dependencies; 301 in A
            // and in a dependency.
            make_obj(b, Some(300), "NumB"),
            make_obj(c, Some(300), "NumC"),
            make_obj(a, Some(301), "NumA"),
            make_obj(b, Some(301), "NumB2"),
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

    /// ENG-193: AppA declares its own "OwnShadow" and its dependency AppB a
    /// same-name table. alc ranks no app first (a file's `namespace`/`using`
    /// may bind B's), so A declines. Discrimination: the old own-app shortcut
    /// restored resolves A's.
    #[test]
    fn resolve_object_own_and_dependency_of_one_name_decline() {
        let g = build_three_app_ambiguous_fixture();
        let a = g.app_ref_by_name("AppA");
        assert!(
            g.resolve_object(a, ObjectKind::Table, "OwnShadow")
                .is_none(),
            "an own and a dependency object of one name: decline"
        );
    }

    /// ENG-193: an own object declared `ObsoleteState = Moved`
    /// (`obsolete_gone`) gives way to the dependency's live object of its name
    /// (Base Application's `No. Series` stub and Business Foundation's table).
    /// Discrimination: dropping the `obsolete_gone` filter in `resolve_object`
    /// makes it decline.
    #[test]
    fn resolve_object_a_moved_own_stub_gives_way_to_the_live_object() {
        let mut g = build_three_app_ambiguous_fixture();
        let a = g.app_ref_by_name("AppA");
        let b = g.app_ref_by_name("AppB");
        let objects: Vec<ObjectNode> = g
            .objects
            .iter()
            .cloned()
            .map(|mut o| {
                if o.id.app == a && o.name.eq_ignore_ascii_case("OwnShadow") {
                    o.obsolete_gone = true;
                }
                o
            })
            .collect();
        g.obj_index = ObjectIndex::build(&objects);
        g.objects = objects.into();
        let resolved = g.resolve_object(a, ObjectKind::Table, "OwnShadow").unwrap();
        assert_eq!(resolved.id.app, b, "the live dependency table binds");
    }

    #[test]
    fn resolve_object_by_number_prefers_own_app_and_declines_on_collision() {
        let g = build_three_app_ambiguous_fixture();
        let a = g.app_ref_by_name("AppA");
        let b = g.app_ref_by_name("AppB");
        let by = |n| {
            g.resolve_object_by_number(a, ObjectKind::Table, n)
                .map(|o| o.id.app)
        };
        assert_eq!(by(301), Some(a), "own app wins over a dependency");
        assert_eq!(by(201), Some(b), "one dependency declares 201");
        assert_eq!(by(300), None, "two dependencies declare 300: decline");
        assert_eq!(by(999), None);
    }
}
