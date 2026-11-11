//! The resolution reads facade (context-server design §6.6, plan
//! `2026-11-03-context-server-remaining.md` item S).
//!
//! Call-site resolution (`full::resolve_file_obligations` and everything it
//! reaches in `resolver`, `receiver`, `arg_dispatch`, `closed_enum`,
//! `interface_holder` and `applicability`) reads the whole-program state
//! through three traits only: [`GraphReads`] (the graph's nodes, topology and
//! whole-program flags), [`IndexReads`] (the [`ResolveIndex`] accessors) and
//! [`SurfaceReads`] (the [`DeclSurface`]). Those functions take
//! `&dyn GraphReads`, `&dyn IndexReads` and `&dyn SurfaceReads`, never the
//! concrete types, so they cannot name a graph field, an index map or a
//! surface map: every read is a trait method, and every trait method is a
//! semantic query (a key in, an answer out).
//!
//! The concrete [`ProgramGraph`], [`ResolveIndex`] and [`DeclSurface`]
//! implement the traits directly (no recording): workspace resolution, the
//! LSP and every test pass them as they are. A [`Recorder`] implements all
//! three over the same state and logs every query it answers ([`Query`]);
//! dependency-body resolution resolves each file through its own recorder,
//! so the log is the complete list of what that file's resolution read of
//! the whole-program state. A query's answer is a function of the query and
//! the state (`session::dep_record::eval`), so the S rule (`program::session`) can re-ask every
//! logged query against the next revision and keep a file whose answers all
//! stayed the same.
//!
//! **What makes the log complete.** (1) Type level: the resolution functions
//! hold only the trait objects; [`GraphReads::raw`] and [`IndexReads`]'s
//! internals need a [`Token`], which only this module can make. (2) The
//! tripwire (tests only): every raw read of the graph's node sets, the index
//! accessors, the surface and the topology calls [`probe`]. Inside a
//! recording scope ([`scope`]) a probe outside a recorder method counts as a
//! bypass; the tripwire test resolves dependency bodies under recorders and
//! asserts no bypass (and that the facade was read at all).
//!
//! **Reads through returned values.** A query that hands out a node
//! (`&ObjectNode`, `&RoutineNode`, `&RoutineMeta`, a run of routines) is
//! answered, for `session::dep_record::eval`, by the whole node's content, so any field the
//! resolution reads off the returned node afterwards is covered. A query that
//! returns ids is answered by the ids; a read through such an id is a second
//! query and is logged when it happens.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashSet};

use al_syntax::ir::ObjectKind;

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{FieldNode, ObjectNode, ObjectRef, RoutineNode};
use crate::program::node_set::NodeSetIter;
use crate::program::resolve::decl_surface::{DeclSurface, RoutineMeta};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};

/// Bumped whenever a trait method, a [`Query`] variant or `session::dep_record::eval`'s answer
/// changes meaning: a kept log of another version is never trusted.
pub const FACADE_VERSION: u32 = 2;

/// A run of routines as resolution reads it: one candidate per routine. The
/// declarations of a split subscriber (`program::split_decls`) are one routine
/// to a name lookup, so only the first of them shows; the resolver routes a
/// call that lands on it to every declaration
/// (`split_decls::expand_call_routes`). Nothing is copied unless the run holds
/// a hidden declaration.
#[derive(Clone)]
pub enum RoutineRun<'g> {
    Whole(NodeSetIter<'g, RoutineNode>),
    Filtered(std::vec::IntoIter<&'g RoutineNode>),
}

impl<'g> RoutineRun<'g> {
    fn of(run: NodeSetIter<'g, RoutineNode>, graph: &ProgramGraph) -> Self {
        let d = &graph.workspace_rows.declarations;
        if d.is_empty() || !run.clone().any(|r| d.is_hidden(&r.id)) {
            return RoutineRun::Whole(run);
        }
        let kept: Vec<&'g RoutineNode> = run.filter(|r| !d.is_hidden(&r.id)).collect();
        RoutineRun::Filtered(kept.into_iter())
    }
}

impl<'g> Iterator for RoutineRun<'g> {
    type Item = &'g RoutineNode;
    fn next(&mut self) -> Option<&'g RoutineNode> {
        match self {
            RoutineRun::Whole(i) => i.next(),
            RoutineRun::Filtered(i) => i.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            RoutineRun::Whole(i) => i.size_hint(),
            RoutineRun::Filtered(i) => i.size_hint(),
        }
    }
}

impl ExactSizeIterator for RoutineRun<'_> {}

/// The capability to read the raw state behind a trait object. Only this
/// module can make one, so resolution code cannot reach the concrete graph
/// through [`GraphReads::raw`].
pub struct Token(());

/// The routine ids of a run of routines (what
/// [`IndexReads::routines_in_object`] yields).
pub type RidIter<'g> = std::iter::Map<RoutineRun<'g>, fn(&'g RoutineNode) -> &'g RoutineNodeId>;

fn rid_of(r: &RoutineNode) -> &RoutineNodeId {
    &r.id
}

/// The graph reads of call-site resolution. See the module doc.
pub trait GraphReads {
    /// The object with this id (binary search; [`ProgramGraph::object`]).
    fn object(&self, id: &ObjectNodeId) -> Option<&ObjectNode>;
    /// The FIRST object with this id in the merged order (what a scan from
    /// the front finds).
    fn object_first(&self, id: &ObjectNodeId) -> Option<&ObjectNode>;
    /// A routine with this id (binary search over the merged order: the same
    /// element the old flat `Vec`'s search landed on).
    fn routine(&self, rid: &RoutineNodeId) -> Option<&RoutineNode>;
    /// Every routine with exactly this id, in merged order.
    fn routines_with_id(&self, rid: &RoutineNodeId) -> NodeSetIter<'_, RoutineNode>;
    /// Every routine of `object`, in merged order, one per routine
    /// ([`RoutineRun`]).
    fn routines_of(&self, object: &ObjectNodeId) -> RoutineRun<'_>;
    /// Every routine of `object` named `name_lc`, in merged order, one per
    /// routine ([`RoutineRun`]).
    fn routines_named(&self, object: &ObjectNodeId, name_lc: &str) -> RoutineRun<'_>;
    /// Every declaration node of the split subscriber that `rid` is one
    /// declaration of (`program::split_decls`), `rid` included; empty for any
    /// other routine.
    fn declarations_of(&self, rid: &RoutineNodeId) -> &[RoutineNodeId];
    /// [`ProgramGraph::resolve_object`].
    fn resolve_object(&self, from: AppRef, kind: ObjectKind, name: &str) -> Option<&ObjectNode>;
    /// [`ProgramGraph::resolve_object_by_number`].
    fn resolve_object_by_number(
        &self,
        from: AppRef,
        kind: ObjectKind,
        number: i64,
    ) -> Option<&ObjectNode>;
    /// `app`'s dependency closure, itself included.
    fn closure(&self, app: AppRef) -> BTreeSet<AppRef>;
    /// Whether `exposing` names `caller` an `internalsVisibleTo` friend.
    fn is_friend(&self, exposing: AppRef, caller: AppRef) -> bool;
    /// [`ProgramGraph::workspace_friendless`].
    fn workspace_friendless(&self) -> bool;
    /// Whether `id` is in [`ProgramGraph::closed_internal_interfaces`].
    fn is_closed_internal_interface(&self, id: &ObjectNodeId) -> bool;
    /// `id`'s entry in [`ProgramGraph::lookup_pages`].
    fn lookup_pages(&self, id: &ObjectNodeId) -> Option<&[String]>;
    /// The raw graph: only for this module's implementations.
    #[doc(hidden)]
    fn raw(&self, token: Token) -> &ProgramGraph;
}

/// The [`ResolveIndex`] reads of call-site resolution: its accessors, one for
/// one, each over `graph`. See the module doc.
#[allow(clippy::too_many_arguments)]
pub trait IndexReads {
    fn routines_in_object<'g>(
        &self,
        graph: &'g dyn GraphReads,
        obj: &ObjectNodeId,
        name_lc: &str,
    ) -> RidIter<'g>;
    fn object_by_number(
        &self,
        graph: &dyn GraphReads,
        from: AppRef,
        kind: ObjectKind,
        declared_id: i64,
    ) -> Option<ObjectNodeId>;
    fn resolve_object_ref(
        &self,
        graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution;
    fn resolve_object_ref_strict(
        &self,
        graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution;
    fn name_is_unique(&self, kind: ObjectKind, name_lc: &str) -> bool;
    fn unique_object(&self, kind: ObjectKind, name_lc: &str) -> Option<&ObjectNodeId>;
    fn table_extensions_of(&self, base_table_name_lc: &str) -> &[ObjectNodeId];
    fn page_extensions_of(&self, base_page_name_lc: &str) -> &[ObjectNodeId];
    fn report_extensions_of(&self, base_report_name_lc: &str) -> &[ObjectNodeId];
    fn implementers_of(&self, interface_name_lc: &str) -> &[ObjectNodeId];
    fn unmapped_enum_implementers(
        &self,
        graph: &dyn GraphReads,
        interface_name_lc: &str,
    ) -> Vec<ObjectNodeId>;
    fn field_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<FieldNode>;
    fn field_with_owner_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<(FieldNode, ObjectNodeId)>;
    /// [`Self::field_with_owner_in_table`] by the field's number.
    fn field_numbered_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        number: i64,
    ) -> Option<(FieldNode, ObjectNodeId)>;
    fn table_scope_has_routine(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        name_lc: &str,
    ) -> bool;
    fn table_scope_may_have_field(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> bool;
    fn untargeted_table_extensions_in(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
    ) -> Vec<&ObjectNodeId>;
    fn object_extends(
        &self,
        graph: &dyn GraphReads,
        from: &ObjectNodeId,
        target: &ObjectNodeId,
    ) -> bool;
}

/// The [`DeclSurface`] reads of call-site resolution.
pub trait SurfaceReads {
    fn get(&self, id: &RoutineNodeId) -> Option<&RoutineMeta>;
    fn get_with_path(&self, id: &RoutineNodeId) -> Option<(&RoutineMeta, &str)> {
        self.get(id).map(|m| (m, m.virtual_path.as_str()))
    }
}

// ---------------------------------------------------------------------------
// The plain implementations (no recording)
// ---------------------------------------------------------------------------

impl GraphReads for ProgramGraph {
    fn object(&self, id: &ObjectNodeId) -> Option<&ObjectNode> {
        ProgramGraph::object(self, id)
    }
    fn object_first(&self, id: &ObjectNodeId) -> Option<&ObjectNode> {
        self.objects.run_by(|o| o.id.cmp(id)).next()
    }
    fn routine(&self, rid: &RoutineNodeId) -> Option<&RoutineNode> {
        self.routines
            .binary_search_by(|probe| probe.id.cmp(rid))
            .ok()
            .map(|i| &self.routines[i])
    }
    fn routines_with_id(&self, rid: &RoutineNodeId) -> NodeSetIter<'_, RoutineNode> {
        self.routines.run_by(|r| r.id.cmp(rid))
    }
    fn routines_of(&self, object: &ObjectNodeId) -> RoutineRun<'_> {
        RoutineRun::of(self.routines.run_by(|r| r.id.object.cmp(object)), self)
    }
    fn routines_named(&self, object: &ObjectNodeId, name_lc: &str) -> RoutineRun<'_> {
        RoutineRun::of(
            self.routines.run_by(|r| {
                r.id.object
                    .cmp(object)
                    .then_with(|| r.id.name_lc.as_str().cmp(name_lc))
            }),
            self,
        )
    }
    fn declarations_of(&self, rid: &RoutineNodeId) -> &[RoutineNodeId] {
        self.workspace_rows.declarations.siblings(rid)
    }
    fn resolve_object(&self, from: AppRef, kind: ObjectKind, name: &str) -> Option<&ObjectNode> {
        ProgramGraph::resolve_object(self, from, kind, name)
    }
    fn resolve_object_by_number(
        &self,
        from: AppRef,
        kind: ObjectKind,
        number: i64,
    ) -> Option<&ObjectNode> {
        ProgramGraph::resolve_object_by_number(self, from, kind, number)
    }
    fn closure(&self, app: AppRef) -> BTreeSet<AppRef> {
        self.topology.closure(app)
    }
    fn is_friend(&self, exposing: AppRef, caller: AppRef) -> bool {
        self.friends
            .get(&exposing)
            .is_some_and(|f| f.contains(&caller))
    }
    fn workspace_friendless(&self) -> bool {
        self.workspace_friendless
    }
    fn is_closed_internal_interface(&self, id: &ObjectNodeId) -> bool {
        self.closed_internal_interfaces.contains(id)
    }
    fn lookup_pages(&self, id: &ObjectNodeId) -> Option<&[String]> {
        self.lookup_pages.get(id).map(Vec::as_slice)
    }
    fn raw(&self, _: Token) -> &ProgramGraph {
        self
    }
}

impl IndexReads for ResolveIndex {
    fn routines_in_object<'g>(
        &self,
        graph: &'g dyn GraphReads,
        obj: &ObjectNodeId,
        name_lc: &str,
    ) -> RidIter<'g> {
        graph
            .routines_named(obj, name_lc)
            .map(rid_of as fn(&'g RoutineNode) -> &'g RoutineNodeId)
    }
    fn object_by_number(
        &self,
        graph: &dyn GraphReads,
        from: AppRef,
        kind: ObjectKind,
        declared_id: i64,
    ) -> Option<ObjectNodeId> {
        ResolveIndex::object_by_number(self, graph.raw(Token(())), from, kind, declared_id)
    }
    fn resolve_object_ref(
        &self,
        graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution {
        ResolveIndex::resolve_object_ref(self, graph.raw(Token(())), from, kind, r)
    }
    fn resolve_object_ref_strict(
        &self,
        graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution {
        ResolveIndex::resolve_object_ref_strict(self, graph.raw(Token(())), from, kind, r)
    }
    fn name_is_unique(&self, kind: ObjectKind, name_lc: &str) -> bool {
        ResolveIndex::name_is_unique(self, kind, name_lc)
    }
    fn unique_object(&self, kind: ObjectKind, name_lc: &str) -> Option<&ObjectNodeId> {
        ResolveIndex::unique_object(self, kind, name_lc)
    }
    fn table_extensions_of(&self, base_table_name_lc: &str) -> &[ObjectNodeId] {
        ResolveIndex::table_extensions_of(self, base_table_name_lc)
    }
    fn page_extensions_of(&self, base_page_name_lc: &str) -> &[ObjectNodeId] {
        ResolveIndex::page_extensions_of(self, base_page_name_lc)
    }
    fn report_extensions_of(&self, base_report_name_lc: &str) -> &[ObjectNodeId] {
        ResolveIndex::report_extensions_of(self, base_report_name_lc)
    }
    fn implementers_of(&self, interface_name_lc: &str) -> &[ObjectNodeId] {
        ResolveIndex::implementers_of(self, interface_name_lc)
    }
    fn unmapped_enum_implementers(
        &self,
        graph: &dyn GraphReads,
        interface_name_lc: &str,
    ) -> Vec<ObjectNodeId> {
        ResolveIndex::unmapped_enum_implementers(self, graph.raw(Token(())), interface_name_lc)
    }
    fn field_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<FieldNode> {
        ResolveIndex::field_in_table(self, graph.raw(Token(())), from_object, base, field_lc)
    }
    fn field_with_owner_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<(FieldNode, ObjectNodeId)> {
        ResolveIndex::field_with_owner_in_table(
            self,
            graph.raw(Token(())),
            from_object,
            base,
            field_lc,
        )
    }
    fn field_numbered_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        number: i64,
    ) -> Option<(FieldNode, ObjectNodeId)> {
        ResolveIndex::field_numbered_in_table_from(
            self,
            graph.raw(Token(())),
            from_object.id.app,
            base,
            number,
        )
    }
    fn table_scope_has_routine(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        name_lc: &str,
    ) -> bool {
        ResolveIndex::table_scope_has_routine(
            self,
            graph.raw(Token(())),
            from_object,
            base,
            name_lc,
        )
    }
    fn table_scope_may_have_field(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> bool {
        ResolveIndex::table_scope_may_have_field(
            self,
            graph.raw(Token(())),
            from_object,
            base,
            field_lc,
        )
    }
    fn untargeted_table_extensions_in(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
    ) -> Vec<&ObjectNodeId> {
        ResolveIndex::untargeted_table_extensions_in(self, graph.raw(Token(())), from_object)
            .collect()
    }
    fn object_extends(
        &self,
        graph: &dyn GraphReads,
        from: &ObjectNodeId,
        target: &ObjectNodeId,
    ) -> bool {
        ResolveIndex::object_extends(self, graph.raw(Token(())), from, target)
    }
}

impl SurfaceReads for DeclSurface {
    fn get(&self, id: &RoutineNodeId) -> Option<&RoutineMeta> {
        DeclSurface::get(self, id)
    }
}

// ---------------------------------------------------------------------------
// Queries and the recorder
// ---------------------------------------------------------------------------

/// An [`ObjectRef`] as a query key (both spellings kept, so `session::dep_record::eval` asks
/// with exactly the reference the resolution asked with).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RefKey {
    Name { raw: String, normalized_lc: String },
    Id(i64),
}

impl RefKey {
    fn of(r: &ObjectRef) -> Self {
        match r {
            ObjectRef::Name { raw, normalized_lc } => RefKey::Name {
                raw: raw.clone(),
                normalized_lc: normalized_lc.clone(),
            },
            ObjectRef::Id(n) => RefKey::Id(*n),
        }
    }

    /// The reference this key was made from.
    #[must_use]
    pub fn to_ref(&self) -> ObjectRef {
        match self {
            RefKey::Name { raw, normalized_lc } => ObjectRef::Name {
                raw: raw.clone(),
                normalized_lc: normalized_lc.clone(),
            },
            RefKey::Id(n) => ObjectRef::Id(*n),
        }
    }
}

/// One read of the whole-program state, as its key: every trait method of
/// the facade, one variant each. The index accessors that take a
/// `from_object` read only its app (`from_object.id.app`), so that is the
/// key. `session::dep_record::eval` answers a query against a state.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Query {
    Object(ObjectNodeId),
    ObjectFirst(ObjectNodeId),
    Routine(RoutineNodeId),
    RoutinesWithId(RoutineNodeId),
    RoutinesOf(ObjectNodeId),
    RoutinesNamed(ObjectNodeId, String),
    DeclarationsOf(RoutineNodeId),
    ResolveObject(AppRef, ObjectKind, String),
    ResolveObjectByNumber(AppRef, ObjectKind, i64),
    Closure(AppRef),
    IsFriend(AppRef, AppRef),
    WorkspaceFriendless,
    ClosedInternalInterface(ObjectNodeId),
    LookupPages(ObjectNodeId),
    ObjectByNumber(AppRef, ObjectKind, i64),
    ResolveObjectRef(ObjectNodeId, ObjectKind, RefKey),
    ResolveObjectRefStrict(ObjectNodeId, ObjectKind, RefKey),
    NameIsUnique(ObjectKind, String),
    UniqueObject(ObjectKind, String),
    TableExtensionsOf(String),
    PageExtensionsOf(String),
    ReportExtensionsOf(String),
    ImplementersOf(String),
    UnmappedEnumImplementers(String),
    FieldWithOwnerInTable(AppRef, ObjectNodeId, String),
    FieldNumberedInTable(AppRef, ObjectNodeId, i64),
    TableScopeHasRoutine(AppRef, ObjectNodeId, String),
    TableScopeMayHaveField(AppRef, ObjectNodeId, String),
    UntargetedTableExtensionsIn(AppRef),
    ObjectExtends(ObjectNodeId, ObjectNodeId),
    SurfaceGet(RoutineNodeId),
}

/// A recording view of one revision's state: answers every facade read
/// exactly as the plain implementations do, and logs it. One per resolved
/// file (it is not `Sync`; each file is resolved on one thread).
pub struct Recorder<'a> {
    graph: &'a ProgramGraph,
    index: &'a ResolveIndex,
    surface: &'a DeclSurface,
    log: RefCell<HashSet<Query>>,
}

impl<'a> Recorder<'a> {
    #[must_use]
    pub fn new(graph: &'a ProgramGraph, index: &'a ResolveIndex, surface: &'a DeclSurface) -> Self {
        Recorder {
            graph,
            index,
            surface,
            log: RefCell::new(HashSet::new()),
        }
    }

    /// The queries logged so far.
    #[must_use]
    pub fn into_log(self) -> HashSet<Query> {
        self.log.into_inner()
    }

    fn note(&self, q: Query) -> FacadeGuard {
        self.log.borrow_mut().insert(q);
        FacadeGuard::enter()
    }
}

impl GraphReads for Recorder<'_> {
    fn object(&self, id: &ObjectNodeId) -> Option<&ObjectNode> {
        let _g = self.note(Query::Object(id.clone()));
        GraphReads::object(self.graph, id)
    }
    fn object_first(&self, id: &ObjectNodeId) -> Option<&ObjectNode> {
        let _g = self.note(Query::ObjectFirst(id.clone()));
        GraphReads::object_first(self.graph, id)
    }
    fn routine(&self, rid: &RoutineNodeId) -> Option<&RoutineNode> {
        let _g = self.note(Query::Routine(rid.clone()));
        GraphReads::routine(self.graph, rid)
    }
    fn routines_with_id(&self, rid: &RoutineNodeId) -> NodeSetIter<'_, RoutineNode> {
        let _g = self.note(Query::RoutinesWithId(rid.clone()));
        GraphReads::routines_with_id(self.graph, rid)
    }
    fn routines_of(&self, object: &ObjectNodeId) -> RoutineRun<'_> {
        let _g = self.note(Query::RoutinesOf(object.clone()));
        GraphReads::routines_of(self.graph, object)
    }
    fn routines_named(&self, object: &ObjectNodeId, name_lc: &str) -> RoutineRun<'_> {
        let _g = self.note(Query::RoutinesNamed(object.clone(), name_lc.to_string()));
        GraphReads::routines_named(self.graph, object, name_lc)
    }
    fn declarations_of(&self, rid: &RoutineNodeId) -> &[RoutineNodeId] {
        let out = GraphReads::declarations_of(self.graph, rid);
        // Logged only when `rid` IS split. A routine that later splits has
        // lost its id, which the run or lookup that found it reports.
        if !out.is_empty() {
            let _g = self.note(Query::DeclarationsOf(rid.clone()));
        }
        out
    }
    fn resolve_object(&self, from: AppRef, kind: ObjectKind, name: &str) -> Option<&ObjectNode> {
        let _g = self.note(Query::ResolveObject(from, kind, name.to_string()));
        GraphReads::resolve_object(self.graph, from, kind, name)
    }
    fn resolve_object_by_number(
        &self,
        from: AppRef,
        kind: ObjectKind,
        number: i64,
    ) -> Option<&ObjectNode> {
        let _g = self.note(Query::ResolveObjectByNumber(from, kind, number));
        GraphReads::resolve_object_by_number(self.graph, from, kind, number)
    }
    fn closure(&self, app: AppRef) -> BTreeSet<AppRef> {
        let _g = self.note(Query::Closure(app));
        GraphReads::closure(self.graph, app)
    }
    fn is_friend(&self, exposing: AppRef, caller: AppRef) -> bool {
        let _g = self.note(Query::IsFriend(exposing, caller));
        GraphReads::is_friend(self.graph, exposing, caller)
    }
    fn workspace_friendless(&self) -> bool {
        let _g = self.note(Query::WorkspaceFriendless);
        GraphReads::workspace_friendless(self.graph)
    }
    fn is_closed_internal_interface(&self, id: &ObjectNodeId) -> bool {
        let _g = self.note(Query::ClosedInternalInterface(id.clone()));
        GraphReads::is_closed_internal_interface(self.graph, id)
    }
    fn lookup_pages(&self, id: &ObjectNodeId) -> Option<&[String]> {
        let _g = self.note(Query::LookupPages(id.clone()));
        GraphReads::lookup_pages(self.graph, id)
    }
    fn raw(&self, _: Token) -> &ProgramGraph {
        self.graph
    }
}

impl IndexReads for Recorder<'_> {
    fn routines_in_object<'g>(
        &self,
        graph: &'g dyn GraphReads,
        obj: &ObjectNodeId,
        name_lc: &str,
    ) -> RidIter<'g> {
        // Through `graph`, which logs the run it hands out.
        IndexReads::routines_in_object(self.index, graph, obj, name_lc)
    }
    fn object_by_number(
        &self,
        _graph: &dyn GraphReads,
        from: AppRef,
        kind: ObjectKind,
        declared_id: i64,
    ) -> Option<ObjectNodeId> {
        let _g = self.note(Query::ObjectByNumber(from, kind, declared_id));
        IndexReads::object_by_number(self.index, self.graph, from, kind, declared_id)
    }
    fn resolve_object_ref(
        &self,
        _graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution {
        let _g = self.note(Query::ResolveObjectRef(from.clone(), kind, RefKey::of(r)));
        IndexReads::resolve_object_ref(self.index, self.graph, from, kind, r)
    }
    fn resolve_object_ref_strict(
        &self,
        _graph: &dyn GraphReads,
        from: ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> ObjectRefResolution {
        let _g = self.note(Query::ResolveObjectRefStrict(
            from.clone(),
            kind,
            RefKey::of(r),
        ));
        IndexReads::resolve_object_ref_strict(self.index, self.graph, from, kind, r)
    }
    fn name_is_unique(&self, kind: ObjectKind, name_lc: &str) -> bool {
        let _g = self.note(Query::NameIsUnique(kind, name_lc.to_string()));
        IndexReads::name_is_unique(self.index, kind, name_lc)
    }
    fn unique_object(&self, kind: ObjectKind, name_lc: &str) -> Option<&ObjectNodeId> {
        let _g = self.note(Query::UniqueObject(kind, name_lc.to_string()));
        IndexReads::unique_object(self.index, kind, name_lc)
    }
    fn table_extensions_of(&self, base_table_name_lc: &str) -> &[ObjectNodeId] {
        let _g = self.note(Query::TableExtensionsOf(base_table_name_lc.to_string()));
        IndexReads::table_extensions_of(self.index, base_table_name_lc)
    }
    fn page_extensions_of(&self, base_page_name_lc: &str) -> &[ObjectNodeId] {
        let _g = self.note(Query::PageExtensionsOf(base_page_name_lc.to_string()));
        IndexReads::page_extensions_of(self.index, base_page_name_lc)
    }
    fn report_extensions_of(&self, base_report_name_lc: &str) -> &[ObjectNodeId] {
        let _g = self.note(Query::ReportExtensionsOf(base_report_name_lc.to_string()));
        IndexReads::report_extensions_of(self.index, base_report_name_lc)
    }
    fn implementers_of(&self, interface_name_lc: &str) -> &[ObjectNodeId] {
        let _g = self.note(Query::ImplementersOf(interface_name_lc.to_string()));
        IndexReads::implementers_of(self.index, interface_name_lc)
    }
    fn unmapped_enum_implementers(
        &self,
        _graph: &dyn GraphReads,
        interface_name_lc: &str,
    ) -> Vec<ObjectNodeId> {
        let _g = self.note(Query::UnmappedEnumImplementers(
            interface_name_lc.to_string(),
        ));
        IndexReads::unmapped_enum_implementers(self.index, self.graph, interface_name_lc)
    }
    fn field_in_table(
        &self,
        graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<FieldNode> {
        IndexReads::field_with_owner_in_table(self, graph, from_object, base, field_lc)
            .map(|(field, _)| field)
    }
    fn field_with_owner_in_table(
        &self,
        _graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> Option<(FieldNode, ObjectNodeId)> {
        let _g = self.note(Query::FieldWithOwnerInTable(
            from_object.id.app,
            base.clone(),
            field_lc.to_string(),
        ));
        IndexReads::field_with_owner_in_table(self.index, self.graph, from_object, base, field_lc)
    }
    fn field_numbered_in_table(
        &self,
        _graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        number: i64,
    ) -> Option<(FieldNode, ObjectNodeId)> {
        let _g = self.note(Query::FieldNumberedInTable(
            from_object.id.app,
            base.clone(),
            number,
        ));
        IndexReads::field_numbered_in_table(self.index, self.graph, from_object, base, number)
    }
    fn table_scope_has_routine(
        &self,
        _graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        name_lc: &str,
    ) -> bool {
        let _g = self.note(Query::TableScopeHasRoutine(
            from_object.id.app,
            base.clone(),
            name_lc.to_string(),
        ));
        IndexReads::table_scope_has_routine(self.index, self.graph, from_object, base, name_lc)
    }
    fn table_scope_may_have_field(
        &self,
        _graph: &dyn GraphReads,
        from_object: &ObjectNode,
        base: &ObjectNodeId,
        field_lc: &str,
    ) -> bool {
        let _g = self.note(Query::TableScopeMayHaveField(
            from_object.id.app,
            base.clone(),
            field_lc.to_string(),
        ));
        IndexReads::table_scope_may_have_field(self.index, self.graph, from_object, base, field_lc)
    }
    fn untargeted_table_extensions_in(
        &self,
        _graph: &dyn GraphReads,
        from_object: &ObjectNode,
    ) -> Vec<&ObjectNodeId> {
        let _g = self.note(Query::UntargetedTableExtensionsIn(from_object.id.app));
        IndexReads::untargeted_table_extensions_in(self.index, self.graph, from_object)
    }
    fn object_extends(
        &self,
        _graph: &dyn GraphReads,
        from: &ObjectNodeId,
        target: &ObjectNodeId,
    ) -> bool {
        let _g = self.note(Query::ObjectExtends(from.clone(), target.clone()));
        IndexReads::object_extends(self.index, self.graph, from, target)
    }
}

impl SurfaceReads for Recorder<'_> {
    fn get(&self, id: &RoutineNodeId) -> Option<&RoutineMeta> {
        let _g = self.note(Query::SurfaceGet(id.clone()));
        SurfaceReads::get(self.surface, id)
    }
}

// ---------------------------------------------------------------------------
// The tripwire's counting hook (tests only; free otherwise)
// ---------------------------------------------------------------------------

thread_local! {
    /// Nesting depth of recording scopes on this thread.
    static SCOPE: Cell<u32> = const { Cell::new(0) };
    /// Nesting depth of recorder methods on this thread.
    #[allow(dead_code)]
    static IN_FACADE: Cell<u32> = const { Cell::new(0) };
    /// Raw reads inside a scope, outside a recorder method.
    static BYPASS: Cell<u64> = const { Cell::new(0) };
    /// Raw reads inside a scope, inside a recorder method.
    static FACADE: Cell<u64> = const { Cell::new(0) };
}

/// Marks a raw read of the whole-program state: a node set, an index
/// accessor, the surface, the topology. Counted only under `cfg(test)`, and
/// only inside a [`scope`].
#[inline]
pub(crate) fn probe() {
    #[cfg(test)]
    if SCOPE.with(Cell::get) > 0 {
        if IN_FACADE.with(Cell::get) > 0 {
            FACADE.with(|c| c.set(c.get() + 1));
        } else {
            BYPASS.with(|c| c.set(c.get() + 1));
        }
    }
}

/// Held while a recorder method answers (its raw reads are the facade's own).
pub(crate) struct FacadeGuard(());

impl FacadeGuard {
    #[inline]
    fn enter() -> Self {
        #[cfg(test)]
        IN_FACADE.with(|c| c.set(c.get() + 1));
        FacadeGuard(())
    }
}

impl Drop for FacadeGuard {
    #[inline]
    fn drop(&mut self) {
        #[cfg(test)]
        IN_FACADE.with(|c| c.set(c.get() - 1));
    }
}

/// What the counting hook saw in one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookCounts {
    /// Raw reads outside a recorder method: each one a read the log missed.
    pub bypass: u64,
    /// Raw reads made by recorder methods.
    pub facade: u64,
}

/// A recording scope on this thread: every [`probe`] until the scope ends is
/// counted ([`ScopeGuard::finish`]). Always zero outside `cfg(test)`.
pub(crate) struct ScopeGuard {
    start: HookCounts,
}

#[must_use]
pub(crate) fn scope() -> ScopeGuard {
    SCOPE.with(|c| c.set(c.get() + 1));
    ScopeGuard {
        start: HookCounts {
            bypass: BYPASS.with(Cell::get),
            facade: FACADE.with(Cell::get),
        },
    }
}

impl ScopeGuard {
    /// End the scope; the counts it saw.
    pub(crate) fn finish(self) -> HookCounts {
        HookCounts {
            bypass: BYPASS.with(Cell::get) - self.start.bypass,
            facade: FACADE.with(Cell::get) - self.start.facade,
        }
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        SCOPE.with(|c| c.set(c.get() - 1));
    }
}

#[cfg(test)]
#[path = "reads_tests.rs"]
mod tests;
