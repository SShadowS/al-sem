//! The projection: engine edges plus the may-execute rules, open-site hubs,
//! hidden-callers flags and pruning (spec §4.3-§4.6). Pure over its [`Inputs`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use super::facts::{RunTarget, SourceFacts};
use super::{Edge, Node};
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{Access, ObjectNode, ObjectRef, RoutineNode};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    AbiRoutineKey, Condition, DispatchShape, EdgeKind, EvidenceKind, OpenBound, RouteTarget,
};
use crate::program::resolve::event::PublisherKind;
use crate::program::resolve::full::{ClassifiedEdge, ObligationId, SiteFacts};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::snapshot::TrustTier;

/// Everything the projection reads.
pub struct Inputs<'a> {
    pub graph: &'a ProgramGraph,
    pub index: &'a ResolveIndex,
    pub surface: &'a DeclSurface,
    /// The workspace app.
    pub primary: AppRef,
    /// The workspace's call-site and event-flow edges plus the dependency
    /// bodies' call-site edges.
    pub edges: &'a [ClassifiedEdge],
    pub site_facts: &'a HashMap<ObligationId, SiteFacts>,
    pub facts: &'a SourceFacts,
    /// Obligations the coverage contract found without an edge.
    pub missing_obligations: &'a [ObligationId],
    /// `"<app name>::<virtual path>"` of every recovered file.
    pub recovered_files: &'a HashSet<String>,
}

/// What the projection produced, before pruning counts are taken.
pub struct Projection {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Edges into the universal hub (kept ones).
    pub universal_callers: usize,
    pub hubs: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Hub {
    Any,
    Name { name_lc: String, arity: usize },
    Run(Option<ObjectKind>),
    Record(&'static str),
    Callbacks(AppRef),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Key {
    Routine(RoutineNodeId),
    Object(ObjectNodeId),
    Hub(Hub),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Kind {
    Direct,
    Event,
    Interface,
    Run,
    Trigger,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Direct => "Direct",
            Kind::Event => "Event",
            Kind::Interface => "Interface",
            Kind::Run => "Run",
            Kind::Trigger => "Trigger",
        }
    }
}

/// Ordered strongest first, so `min` keeps the strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Conf {
    Resolved,
    OverApprox,
    Open,
}

impl Conf {
    fn as_str(self) -> &'static str {
        match self {
            Conf::Resolved => "Resolved",
            Conf::OverApprox => "OverApprox",
            Conf::Open => "Open",
        }
    }
}

#[derive(Clone, Debug)]
struct E {
    conf: Conf,
    via: &'static str,
    line: Option<u32>,
    conditions: BTreeSet<&'static str>,
}

/// The entry trigger a run of `kind` always runs (`resolver::entry_trigger_name`).
fn entry_trigger(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Page => "onopenpage",
        ObjectKind::Report => "onprereport",
        ObjectKind::XmlPort => "onprexmlport",
        _ => "onrun",
    }
}

fn is_runnable(kind: ObjectKind) -> bool {
    matches!(
        kind,
        ObjectKind::Codeunit
            | ObjectKind::Page
            | ObjectKind::Report
            | ObjectKind::XmlPort
            | ObjectKind::Query
    )
}

fn condition_str(c: Condition) -> &'static str {
    match c {
        Condition::RunTriggerGuarded => "runTriggerGuarded",
        Condition::ManualBinding => "manualBinding",
        Condition::SkipOnMissingLicense => "skipOnMissingLicense",
        Condition::SkipOnMissingPermission => "skipOnMissingPermission",
        Condition::AmbiguousDispatch => "ambiguousDispatch",
    }
}

/// Trigger names a record operation of family `op` fires, and the platform table
/// events it raises (folded).
fn record_trigger_names(op: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match op {
        "insert" => (
            &["oninsert", "onbeforeinsert", "onafterinsert"],
            &["onbeforeinsertevent", "onafterinsertevent"],
        ),
        "modify" => (
            &["onmodify", "onbeforemodify", "onaftermodify"],
            &["onbeforemodifyevent", "onaftermodifyevent"],
        ),
        "delete" => (
            &["ondelete", "onbeforedelete", "onafterdelete"],
            &["onbeforedeleteevent", "onafterdeleteevent"],
        ),
        "rename" => (
            &["onrename", "onbeforerename", "onafterrename"],
            &["onbeforerenameevent", "onafterrenameevent"],
        ),
        "validate" => (
            &["onvalidate", "onbeforevalidate", "onaftervalidate"],
            &["onbeforevalidateevent", "onaftervalidateevent"],
        ),
        _ => (&[], &[]),
    }
}

/// Table-extension triggers no record operation reaches yet (F5).
fn is_unmodeled_record_trigger(name_lc: &str) -> bool {
    matches!(
        name_lc,
        "onbeforeinsert"
            | "onafterinsert"
            | "onbeforemodify"
            | "onaftermodify"
            | "onbeforedelete"
            | "onafterdelete"
            | "onbeforerename"
            | "onafterrename"
            | "onbeforevalidate"
            | "onaftervalidate"
    )
}

fn object_kind_from_lc(s: &str) -> Option<ObjectKind> {
    Some(match s {
        "codeunit" => ObjectKind::Codeunit,
        "table" => ObjectKind::Table,
        "tableextension" => ObjectKind::TableExtension,
        "page" => ObjectKind::Page,
        "pageextension" => ObjectKind::PageExtension,
        "report" => ObjectKind::Report,
        "reportextension" => ObjectKind::ReportExtension,
        "query" => ObjectKind::Query,
        "xmlport" => ObjectKind::XmlPort,
        "enum" => ObjectKind::Enum,
        "enumextension" => ObjectKind::EnumExtension,
        "interface" => ObjectKind::Interface,
        _ => return None,
    })
}

struct Builder<'a> {
    inp: &'a Inputs<'a>,
    obj_by_id: HashMap<&'a ObjectNodeId, &'a ObjectNode>,
    routines_of: HashMap<&'a ObjectNodeId, Vec<&'a RoutineNode>>,
    rtn_by_id: HashMap<&'a RoutineNodeId, &'a RoutineNode>,
    base_of: HashMap<&'a ObjectNodeId, ObjectNodeId>,
    exts_of: HashMap<ObjectNodeId, Vec<&'a ObjectNodeId>>,
    edges: BTreeMap<(Key, Key, Kind), E>,
    objects: BTreeSet<ObjectNodeId>,
}

impl<'a> Builder<'a> {
    fn new(inp: &'a Inputs<'a>) -> Self {
        let g = inp.graph;
        let obj_by_id: HashMap<&ObjectNodeId, &ObjectNode> =
            g.objects.iter().map(|o| (&o.id, o)).collect();
        let mut routines_of: HashMap<&ObjectNodeId, Vec<&RoutineNode>> = HashMap::new();
        let mut rtn_by_id = HashMap::new();
        for r in g.routines.iter() {
            routines_of.entry(&r.id.object).or_default().push(r);
            rtn_by_id.entry(&r.id).or_insert(r);
        }
        let mut base_of = HashMap::new();
        let mut exts_of: HashMap<ObjectNodeId, Vec<&ObjectNodeId>> = HashMap::new();
        for o in g.objects.iter() {
            if let (Some(bk), Some(t)) = (o.id.kind.extension_base_kind(), &o.extends_target)
                && let Some(base) = g.resolve_object(o.id.app, bk, t)
            {
                base_of.insert(&o.id, base.id.clone());
                exts_of.entry(base.id.clone()).or_default().push(&o.id);
            }
        }
        Builder {
            inp,
            obj_by_id,
            routines_of,
            rtn_by_id,
            base_of,
            exts_of,
            edges: BTreeMap::new(),
            objects: BTreeSet::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        from: Key,
        to: Key,
        kind: Kind,
        conf: Conf,
        via: &'static str,
        line: Option<u32>,
        conditions: BTreeSet<&'static str>,
    ) {
        if from == to {
            return;
        }
        if let Key::Object(o) = &to {
            self.objects.insert(o.clone());
        }
        if let Key::Object(o) = &from {
            self.objects.insert(o.clone());
        }
        let new = E {
            conf,
            via,
            line,
            conditions,
        };
        match self.edges.entry((from, to, kind)) {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(new);
            }
            std::collections::btree_map::Entry::Occupied(mut o) => {
                let old = o.get_mut();
                let line = match (old.line, new.line) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                if new.conf < old.conf {
                    *old = new;
                } else if new.conf == old.conf {
                    old.conditions.extend(new.conditions);
                }
                old.line = line;
            }
        }
    }

    fn simple(&mut self, from: Key, to: Key, kind: Kind, conf: Conf, via: &'static str) {
        self.add(from, to, kind, conf, via, None, BTreeSet::new());
    }

    /// The object a run of `oid` stands for: its base when `oid` is an extension.
    fn run_object(&self, oid: &ObjectNodeId) -> ObjectNodeId {
        self.base_of
            .get(oid)
            .cloned()
            .unwrap_or_else(|| oid.clone())
    }

    fn abi_object(&self, key: &AbiRoutineKey) -> Option<ObjectNodeId> {
        let kind = object_kind_from_lc(&key.object_type)?;
        Some(ObjectNodeId {
            app: key.app,
            kind,
            key: if key.object_number != 0 {
                ObjKey::Id(key.object_number)
            } else {
                ObjKey::Name(key.object_name_lc.as_str().into())
            },
        })
    }

    fn engine_edges(&mut self) {
        let inp = self.inp;
        for ce in inp.edges {
            let e = &ce.edge;
            let from = Key::Routine(e.from.clone());
            let line = match ce.obligation_id {
                ObligationId::CallSite { .. } => Some(e.site.span.start.line + 1),
                ObligationId::Publisher(_) => None,
            };
            let mut unknown = false;
            for r in e.may_reachable_routes() {
                if r.evidence.kind() == EvidenceKind::Unknown || r.target == RouteTarget::Unresolved
                {
                    unknown = true;
                    continue;
                }
                let conditions: BTreeSet<&'static str> =
                    r.conditions.iter().map(|c| condition_str(*c)).collect();
                match &r.target {
                    RouteTarget::Builtin(_) | RouteTarget::Unresolved => {}
                    RouteTarget::Routine(rid) => {
                        self.engine_route(&from, e.kind, e.shape, rid, line, conditions)
                    }
                    RouteTarget::AbiSymbol { key } => {
                        let Some(oid) = self.abi_object(key) else {
                            unknown = true;
                            continue;
                        };
                        let rid = RoutineNodeId {
                            object: oid.clone(),
                            name_lc: key.routine_name_lc.as_str().into(),
                            enclosing_member_lc: None,
                            params_count: key.params_count,
                            sig_fp: key.param_type_fp,
                        };
                        if self.rtn_by_id.contains_key(&rid) {
                            self.engine_route(&from, e.kind, e.shape, &rid, line, conditions);
                        } else {
                            // A symbol-only object's entry trigger (no routine node:
                            // the ABI lists no triggers) or another unlisted symbol:
                            // its object stands for it.
                            let kind = match e.kind {
                                EdgeKind::Run | EdgeKind::Call => Kind::Run,
                                EdgeKind::ImplicitTrigger => Kind::Trigger,
                                EdgeKind::EventFlow => Kind::Event,
                            };
                            let to = Key::Object(self.run_object(&oid));
                            self.add(
                                from.clone(),
                                to,
                                kind,
                                Conf::OverApprox,
                                "run",
                                line,
                                conditions,
                            );
                        }
                    }
                }
            }
            // A run of an object that declares no entry trigger still runs it.
            if let ObligationId::CallSite { .. } = ce.obligation_id
                && let Some(t) = inp
                    .site_facts
                    .get(&ce.obligation_id)
                    .and_then(|f| f.run_target.as_ref())
            {
                let to = Key::Object(self.run_object(t));
                self.add(
                    from.clone(),
                    to,
                    Kind::Run,
                    Conf::Resolved,
                    "run",
                    line,
                    BTreeSet::new(),
                );
            }
            let bounds: &[OpenBound] = match ce.obligation_id {
                ObligationId::CallSite { .. } => inp
                    .site_facts
                    .get(&ce.obligation_id)
                    .map(|f| f.open.as_slice())
                    .unwrap_or(&[]),
                ObligationId::Publisher(_) => &[],
            };
            let via = if e.shape == DispatchShape::DynamicOpen {
                "dynamic"
            } else if unknown {
                "unresolved"
            } else if e
                .routes
                .iter()
                .any(|r| matches!(r.target, RouteTarget::Builtin(_)))
            {
                "dispatchingBuiltin"
            } else {
                "unresolved"
            };
            for b in bounds {
                let (hub, kind) = match b {
                    OpenBound::Name { name_lc, arity } => (
                        Hub::Name {
                            name_lc: name_lc.clone(),
                            arity: *arity,
                        },
                        Kind::Direct,
                    ),
                    OpenBound::Run { kind } => (Hub::Run(*kind), Kind::Run),
                    OpenBound::Record { op } => (Hub::Record(op), Kind::Trigger),
                    OpenBound::Any => (Hub::Any, Kind::Direct),
                };
                self.add(
                    from.clone(),
                    Key::Hub(hub),
                    kind,
                    Conf::Open,
                    via,
                    line,
                    BTreeSet::new(),
                );
            }
            if unknown && bounds.is_empty() {
                self.add(
                    from.clone(),
                    Key::Hub(Hub::Any),
                    Kind::Direct,
                    Conf::Open,
                    "unresolved",
                    line,
                    BTreeSet::new(),
                );
            }
        }
    }

    fn engine_route(
        &mut self,
        from: &Key,
        kind: EdgeKind,
        shape: DispatchShape,
        rid: &RoutineNodeId,
        line: Option<u32>,
        conditions: BTreeSet<&'static str>,
    ) {
        let to_trigger = self.rtn_by_id.get(rid).is_some_and(|r| r.is_trigger);
        let conf = if conditions.is_empty() {
            Conf::Resolved
        } else {
            Conf::OverApprox
        };
        match kind {
            EdgeKind::Run => {
                let to = Key::Object(self.run_object(&rid.object));
                self.add(
                    from.clone(),
                    to,
                    Kind::Run,
                    Conf::Resolved,
                    "run",
                    line,
                    BTreeSet::new(),
                );
            }
            EdgeKind::Call if to_trigger => {
                // A typed-variable run (`Cu.Run()`, `PageVar.RunModal()`): no
                // other call reaches a trigger (`resolve_in_object`).
                let to = Key::Object(self.run_object(&rid.object));
                self.add(
                    from.clone(),
                    to,
                    Kind::Run,
                    Conf::Resolved,
                    "run",
                    line,
                    BTreeSet::new(),
                );
            }
            EdgeKind::ImplicitTrigger => self.add(
                from.clone(),
                Key::Routine(rid.clone()),
                Kind::Trigger,
                conf,
                "implicitTrigger",
                line,
                conditions,
            ),
            EdgeKind::EventFlow => self.add(
                from.clone(),
                Key::Routine(rid.clone()),
                Kind::Event,
                conf,
                "eventFlow",
                line,
                conditions,
            ),
            EdgeKind::Call => {
                let (k, c, via) = match shape {
                    DispatchShape::Polymorphic => (Kind::Interface, Conf::OverApprox, "interface"),
                    DispatchShape::AmbiguousOverload => (Kind::Direct, Conf::OverApprox, "call"),
                    _ => (Kind::Direct, conf, "call"),
                };
                self.add(
                    from.clone(),
                    Key::Routine(rid.clone()),
                    k,
                    c,
                    via,
                    line,
                    conditions,
                );
            }
        }
    }

    fn resolve_ref(
        &self,
        from: &ObjectNodeId,
        kind: ObjectKind,
        r: &ObjectRef,
    ) -> Option<RunTarget> {
        match self
            .inp
            .index
            .resolve_object_ref(self.inp.graph, from.clone(), kind, r)
        {
            ObjectRefResolution::Unique(id) => Some(RunTarget::Object(id)),
            ObjectRefResolution::Ambiguous => Some(RunTarget::AnyOf(kind)),
            _ => None,
        }
    }

    fn run_target_key(&self, t: &RunTarget) -> Key {
        match t {
            RunTarget::Object(id) => Key::Object(self.run_object(id)),
            RunTarget::AnyOf(k) => Key::Hub(Hub::Run(Some(*k))),
        }
    }

    /// Every trigger of `oid` and of each extension of it.
    fn triggers_with_extensions(&self, oid: &ObjectNodeId) -> Vec<&'a RoutineNode> {
        let mut out: Vec<&RoutineNode> = Vec::new();
        let mut objs: Vec<&ObjectNodeId> = vec![oid];
        if let Some(exts) = self.exts_of.get(oid) {
            objs.extend(exts.iter().copied());
        }
        for o in objs {
            if let Some(rs) = self.routines_of.get(o) {
                out.extend(rs.iter().copied().filter(|r| r.is_trigger));
            }
        }
        out
    }

    /// Rule 1 (lifecycle) for one object node.
    fn lifecycle(&mut self, oid: &ObjectNodeId) {
        let from = Key::Object(oid.clone());
        let Some(obj) = self.obj_by_id.get(oid).copied() else {
            return;
        };
        let entry = entry_trigger(oid.kind);
        for t in self.triggers_with_extensions(oid) {
            let conf = if t.id.object == *oid
                && t.id.enclosing_member_lc.is_none()
                && t.id.name_lc.as_str() == entry
            {
                Conf::Resolved
            } else {
                Conf::OverApprox
            };
            self.simple(
                from.clone(),
                Key::Routine(t.id.clone()),
                Kind::Trigger,
                conf,
                "lifecycle",
            );
        }
        // The object's own synthetic platform events (a page's OnOpenPageEvent, …).
        let own_platform: Vec<RoutineNodeId> = self
            .routines_of
            .get(oid)
            .into_iter()
            .flatten()
            .filter(|r| r.publisher_kind == Some(PublisherKind::Platform))
            .map(|r| r.id.clone())
            .collect();
        for p in own_platform {
            self.simple(
                from.clone(),
                Key::Routine(p),
                Kind::Trigger,
                Conf::OverApprox,
                "lifecycle",
            );
        }
        let mut members: Vec<&ObjectNodeId> = vec![oid];
        if let Some(exts) = self.exts_of.get(oid) {
            members.extend(exts.iter().copied());
        }
        let mut opens: Vec<Key> = Vec::new();
        let mut tables: Vec<ObjectNodeId> = Vec::new();
        for m in &members {
            if let Some(f) = self.inp.facts.objects.get(*m) {
                opens.extend(f.opens.iter().map(|t| self.run_target_key(t)));
                tables.extend(f.tables.iter().cloned());
            }
            if let Some(mo) = self.obj_by_id.get(*m) {
                for c in &mo.page_controls {
                    if c.kind == crate::program::node_extract::PageControlKind::Part
                        && let Some(t) = self.resolve_ref(m, ObjectKind::Page, &c.target)
                    {
                        opens.push(self.run_target_key(&t));
                    }
                }
            }
        }
        if oid.kind == ObjectKind::Page
            && let Some(st) = &obj.source_table
            && let Some(RunTarget::Object(table)) = self.resolve_ref(oid, ObjectKind::Table, st)
        {
            if let Some(f) = self.inp.facts.objects.get(&table) {
                opens.extend(f.lookup_pages.iter().map(|t| self.run_target_key(t)));
            }
            tables.push(table);
        }
        for to in opens {
            self.simple(
                from.clone(),
                to,
                Kind::Trigger,
                Conf::OverApprox,
                "lifecycle",
            );
        }
        for table in tables {
            let mut targets: Vec<RoutineNodeId> = self
                .triggers_with_extensions(&table)
                .into_iter()
                .map(|r| r.id.clone())
                .collect();
            targets.extend(
                self.routines_of
                    .get(&table)
                    .into_iter()
                    .flatten()
                    .filter(|r| r.publisher_kind == Some(PublisherKind::Platform))
                    .map(|r| r.id.clone()),
            );
            for t in targets {
                self.simple(
                    from.clone(),
                    Key::Routine(t),
                    Kind::Trigger,
                    Conf::OverApprox,
                    "lifecycle",
                );
            }
        }
        if obj.tier == TrustTier::SymbolOnly {
            self.simple(
                from,
                Key::Hub(Hub::Callbacks(oid.app)),
                Kind::Event,
                Conf::Open,
                "opaqueBody",
            );
        }
    }

    /// Rules 2-4 and the opaque-body edges, per routine.
    fn routine_rules(&mut self) {
        let inp = self.inp;
        for r in inp.graph.routines.iter() {
            let from = Key::Routine(r.id.clone());
            if r.tier == TrustTier::SymbolOnly {
                // Opaque body. A publisher's body only raises; its event flow says so.
                if r.publisher_kind.is_none() {
                    self.simple(
                        from,
                        Key::Hub(Hub::Callbacks(r.id.object.app)),
                        Kind::Event,
                        Conf::Open,
                        "opaqueBody",
                    );
                }
                continue;
            }
            let mut runs: Vec<RunTarget> = Vec::new();
            if let Some(f) = inp.facts.routines.get(&r.id) {
                runs.extend(f.runs.iter().cloned());
                if f.test {
                    self.simple(
                        from.clone(),
                        Key::Object(r.id.object.clone()),
                        Kind::Run,
                        Conf::Resolved,
                        "testRunner",
                    );
                }
                let handlers: Vec<RoutineNodeId> = self
                    .routines_of
                    .get(&r.id.object)
                    .into_iter()
                    .flatten()
                    .filter(|h| {
                        !h.is_trigger && f.handlers.iter().any(|n| n == h.id.name_lc.as_str())
                    })
                    .map(|h| h.id.clone())
                    .collect();
                for h in handlers {
                    self.simple(
                        from.clone(),
                        Key::Routine(h),
                        Kind::Event,
                        Conf::OverApprox,
                        "testHandler",
                    );
                }
            }
            if let Some(of) = inp.facts.objects.get(&r.id.object) {
                runs.extend(of.runs.iter().cloned());
            }
            runs.sort();
            runs.dedup();
            for t in runs {
                let to = self.run_target_key(&t);
                self.simple(
                    from.clone(),
                    to,
                    Kind::Run,
                    Conf::OverApprox,
                    "declaredRunnable",
                );
            }
        }
    }

    /// Edges into the universal hub that no call site makes: unreliable parses
    /// and coverage-contract misses.
    fn universal_callers(&mut self) {
        let inp = self.inp;
        for r in inp.graph.routines.iter() {
            let parse_bad = self
                .obj_by_id
                .get(&r.id.object)
                .is_some_and(|o| o.parse_incomplete)
                || inp.surface.get_with_path(&r.id).is_some_and(|(m, path)| {
                    m.parse_incomplete
                        || inp.recovered_files.contains(&format!(
                            "{}::{}",
                            inp.graph.apps.resolve(r.id.object.app).name,
                            path
                        ))
                });
            if parse_bad {
                self.simple(
                    Key::Routine(r.id.clone()),
                    Key::Hub(Hub::Any),
                    Kind::Direct,
                    Conf::Open,
                    "parseRecovered",
                );
            }
        }
        for o in inp.missing_obligations {
            let caller = match o {
                ObligationId::CallSite { caller, .. } => caller,
                ObligationId::Publisher(p) => p,
            };
            self.simple(
                Key::Routine(caller.clone()),
                Key::Hub(Hub::Any),
                Kind::Direct,
                Conf::Open,
                "coverageMiss",
            );
        }
    }

    fn hub_expansions(&mut self) {
        let hubs: BTreeSet<Hub> = self
            .edges
            .keys()
            .filter_map(|(_, to, _)| match to {
                Key::Hub(h) => Some(h.clone()),
                _ => None,
            })
            .collect();
        let g = self.inp.graph;
        let mut by_name: HashMap<(&str, usize), Vec<&RoutineNodeId>> = HashMap::new();
        for r in g.routines.iter().filter(|r| !r.is_trigger) {
            by_name
                .entry((r.id.name_lc.as_str(), r.id.params_count))
                .or_default()
                .push(&r.id);
        }
        // Publishers that have a subscriber in the program.
        let subscribed: HashSet<RoutineNodeId> = self
            .edges
            .iter()
            .filter(|((_, _, k), e)| *k == Kind::Event && e.via == "eventFlow")
            .filter_map(|((from, _, _), _)| match from {
                Key::Routine(r) => Some(r.clone()),
                _ => None,
            })
            .collect();
        let runnable: Vec<ObjectNodeId> = g
            .objects
            .iter()
            .filter(|o| is_runnable(o.id.kind))
            .map(|o| o.id.clone())
            .collect();
        for hub in hubs {
            let from = Key::Hub(hub.clone());
            let x = "hubExpansion";
            match &hub {
                Hub::Any => {}
                Hub::Name { name_lc, arity } => {
                    let targets: Vec<RoutineNodeId> = by_name
                        .get(&(name_lc.as_str(), *arity))
                        .into_iter()
                        .flatten()
                        .map(|r| (*r).clone())
                        .collect();
                    for t in targets {
                        self.simple(from.clone(), Key::Routine(t), Kind::Direct, Conf::Open, x);
                    }
                }
                Hub::Run(kind) => {
                    for o in runnable.iter().filter(|o| kind.is_none_or(|k| o.kind == k)) {
                        self.simple(
                            from.clone(),
                            Key::Object(o.clone()),
                            Kind::Run,
                            Conf::Open,
                            x,
                        );
                    }
                }
                Hub::Record(op) => {
                    let (triggers, events) = record_trigger_names(op);
                    let targets: Vec<RoutineNodeId> = g
                        .routines
                        .iter()
                        .filter(|r| {
                            matches!(
                                r.id.object.kind,
                                ObjectKind::Table | ObjectKind::TableExtension
                            ) && ((r.is_trigger && triggers.contains(&r.id.name_lc.as_str()))
                                || (r.publisher_kind == Some(PublisherKind::Platform)
                                    && events.contains(&r.id.name_lc.as_str())))
                        })
                        .map(|r| r.id.clone())
                        .collect();
                    for t in targets {
                        self.simple(from.clone(), Key::Routine(t), Kind::Trigger, Conf::Open, x);
                    }
                }
                Hub::Callbacks(app) => {
                    let mut apps = g.topology.closure(*app);
                    apps.insert(*app);
                    let mut targets: Vec<(Key, Kind)> = Vec::new();
                    // Publishers declared in those apps that have a subscriber.
                    for r in g.routines.iter() {
                        if apps.contains(&r.id.object.app)
                            && r.publisher_kind.is_some()
                            && subscribed.contains(&r.id)
                        {
                            targets.push((Key::Routine(r.id.clone()), Kind::Event));
                        }
                    }
                    // Implementations of interfaces declared there.
                    let ifaces: HashSet<String> = g
                        .objects
                        .iter()
                        .filter(|o| o.id.kind == ObjectKind::Interface && apps.contains(&o.id.app))
                        .map(|o| o.name.fold_identifier())
                        .collect();
                    if !ifaces.is_empty() {
                        for o in g.objects.iter() {
                            if o.implements
                                .iter()
                                .any(|i| ifaces.contains(&i.fold_identifier()))
                            {
                                for r in self.routines_of.get(&o.id).into_iter().flatten() {
                                    if !r.is_trigger && r.access == Access::Public {
                                        targets.push((Key::Routine(r.id.clone()), Kind::Interface));
                                    }
                                }
                            }
                        }
                    }
                    // Triggers of extensions of their objects.
                    for o in g.objects.iter().filter(|o| apps.contains(&o.id.app)) {
                        for ext in self.exts_of.get(&o.id).into_iter().flatten() {
                            for r in self.routines_of.get(*ext).into_iter().flatten() {
                                if r.is_trigger {
                                    targets.push((Key::Routine(r.id.clone()), Kind::Trigger));
                                }
                            }
                        }
                    }
                    for (to, kind) in targets {
                        self.simple(from.clone(), to, kind, Conf::Open, x);
                    }
                }
            }
        }
    }

    /// `hiddenCallers` per routine (spec §4.5), over the unpruned graph.
    fn hidden_callers(&self) -> HashMap<RoutineNodeId, Vec<&'static str>> {
        let mut incoming_any: HashSet<&RoutineNodeId> = HashSet::new();
        let mut incoming_event: HashSet<&RoutineNodeId> = HashSet::new();
        let mut outgoing_event: HashSet<&RoutineNodeId> = HashSet::new();
        for ((from, to, kind), e) in &self.edges {
            if let Key::Routine(t) = to {
                incoming_any.insert(t);
                if *kind == Kind::Event {
                    incoming_event.insert(t);
                }
            }
            if let Key::Routine(f) = from
                && e.via == "eventFlow"
            {
                outgoing_event.insert(f);
            }
        }
        let mut out: HashMap<RoutineNodeId, Vec<&'static str>> = HashMap::new();
        for r in self.inp.graph.routines.iter() {
            let mut flags: Vec<&'static str> = Vec::new();
            match r.publisher_kind {
                Some(PublisherKind::Platform)
                    if matches!(
                        r.id.object.kind,
                        ObjectKind::Table | ObjectKind::TableExtension
                    ) =>
                {
                    flags.push("unraisedPublisher");
                }
                Some(PublisherKind::Platform) | None => {}
                Some(_) => {
                    if outgoing_event.contains(&r.id) && !incoming_any.contains(&r.id) {
                        flags.push("unraisedPublisher");
                    }
                }
            }
            if !r.event_subscribers.is_empty() && !incoming_event.contains(&r.id) {
                flags.push("unboundSubscriber");
            }
            if r.is_trigger
                && r.id.object.kind == ObjectKind::TableExtension
                && is_unmodeled_record_trigger(r.id.name_lc.as_str())
            {
                flags.push("unmodeledRecordTrigger");
            }
            if !flags.is_empty() {
                out.insert(r.id.clone(), flags);
            }
        }
        out
    }
}

/// Build the export's nodes and edges.
#[must_use]
pub fn project(inp: &Inputs<'_>) -> Projection {
    let mut b = Builder::new(inp);
    b.engine_edges();
    b.routine_rules();
    b.universal_callers();
    // Object nodes: every runnable object, plus any an edge names. Lifecycle may
    // name more (parts, opened pages), so run it to a fixed point.
    for o in inp.graph.objects.iter().filter(|o| is_runnable(o.id.kind)) {
        b.objects.insert(o.id.clone());
    }
    let mut done: BTreeSet<ObjectNodeId> = BTreeSet::new();
    loop {
        let todo: Vec<ObjectNodeId> = b.objects.difference(&done).cloned().collect();
        if todo.is_empty() {
            break;
        }
        for o in todo {
            b.lifecycle(&o);
            done.insert(o);
        }
    }
    b.hub_expansions();
    // Hub expansion adds object nodes only from the runnable set, already done.
    let hidden = b.hidden_callers();

    // ── Pruning (spec §4.6) ────────────────────────────────────────────────
    let mut succ: HashMap<&Key, Vec<&Key>> = HashMap::new();
    let mut pred: HashMap<&Key, Vec<&Key>> = HashMap::new();
    for (from, to, _) in b.edges.keys() {
        succ.entry(from).or_default().push(to);
        pred.entry(to).or_default().push(from);
    }
    let workspace: Vec<Key> = inp
        .graph
        .routines
        .iter()
        .filter(|r| r.id.object.app == inp.primary)
        .map(|r| Key::Routine(r.id.clone()))
        .collect();
    let any = Key::Hub(Hub::Any);
    let any_used = pred.contains_key(&any);
    let walk = |start: &[Key], next: &HashMap<&Key, Vec<&Key>>| -> HashSet<Key> {
        let mut seen: HashSet<Key> = start.iter().cloned().collect();
        let mut queue: VecDeque<Key> = start.iter().cloned().collect();
        while let Some(k) = queue.pop_front() {
            for n in next.get(&k).into_iter().flatten() {
                if seen.insert((*n).clone()) {
                    queue.push_back((*n).clone());
                }
            }
        }
        seen
    };
    let forward = walk(&workspace, &succ);
    let mut backward = walk(&workspace, &pred);
    if any_used && !backward.is_empty() {
        // The universal hub is a caller of every node, so of every node here.
        backward.extend(walk(std::slice::from_ref(&any), &pred));
    }
    let universal_reached = forward.contains(&any);
    let keep: HashSet<Key> = backward
        .iter()
        .filter(|k| {
            universal_reached
                || forward.contains(*k)
                || matches!(k, Key::Routine(r) if hidden.contains_key(r))
        })
        .cloned()
        .chain(workspace.iter().cloned())
        .collect();

    // ── Rows ───────────────────────────────────────────────────────────────
    let ids: HashMap<&Key, String> = keep
        .iter()
        .map(|k| (k, super::key_id(k, inp.graph)))
        .collect();
    let mut nodes: Vec<Node> = keep
        .iter()
        .map(|k| super::render_node(k, &ids[k], inp, &hidden))
        .collect();
    nodes.sort_by(|a, c| a.id.cmp(&c.id));
    let mut edges: Vec<Edge> = Vec::new();
    let mut universal_callers = 0usize;
    for ((from, to, kind), e) in &b.edges {
        let (Some(f), Some(t)) = (ids.get(from), ids.get(to)) else {
            continue;
        };
        if *to == any {
            universal_callers += 1;
        }
        edges.push(Edge {
            caller: f.clone(),
            callee: t.clone(),
            kind: kind.as_str(),
            confidence: e.conf.as_str(),
            via: e.via,
            line: e.line,
            conditions: e.conditions.iter().copied().collect(),
        });
    }
    edges.sort_by(|a, c| {
        (a.caller.as_str(), a.callee.as_str(), a.kind).cmp(&(
            c.caller.as_str(),
            c.callee.as_str(),
            c.kind,
        ))
    });
    let hubs = keep.iter().filter(|k| matches!(k, Key::Hub(_))).count();
    Projection {
        nodes,
        edges,
        universal_callers,
        hubs,
    }
}

#[cfg(test)]
mod tests {
    //! The projection contract over hand-built graphs: every precondition is
    //! stated here, none is produced by the resolver.

    use super::*;
    use crate::program::graph::ObjectIndex;
    use crate::program::impact_graph::{ObjectFacts, RoutineFacts};
    use crate::program::node::AppRegistry;
    use crate::program::node_extract::{AbiParams, PageControlKind, PageControlNode};
    use crate::program::resolve::edge::{
        CanonicalSpan, Edge as EngineEdge, Evidence, OpenWorldReason, Route, SetCompleteness,
        SiteId, SourcePos, UnknownReason, Witness,
    };
    use crate::program::resolve::event::ParsedSubscriberArgs;
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::AppId;

    #[derive(Default)]
    struct World {
        apps: AppRegistry,
        deps: Vec<(AppRef, AppRef)>,
        objects: Vec<ObjectNode>,
        routines: Vec<RoutineNode>,
        edges: Vec<ClassifiedEdge>,
        site_facts: HashMap<ObligationId, SiteFacts>,
        facts: SourceFacts,
        missing: Vec<ObligationId>,
    }

    fn src(rid: &RoutineNodeId) -> Route {
        Route {
            target: RouteTarget::Routine(rid.clone()),
            evidence: Evidence::Source,
            conditions: vec![],
            witness: Witness::None,
            receiver_tier: None,
        }
    }

    fn unknown() -> Route {
        Route {
            target: RouteTarget::Unresolved,
            evidence: Evidence::Unknown(UnknownReason::UnclassifiedCallee),
            conditions: vec![],
            witness: Witness::None,
            receiver_tier: None,
        }
    }

    fn subscriber_args(object_type: &str, name: &str, event: &str) -> ParsedSubscriberArgs {
        ParsedSubscriberArgs {
            publisher_object_type: object_type.into(),
            publisher_name: name.into(),
            publisher_id: None,
            event_name: event.into(),
            element: None,
            skip_on_missing_license: false,
            skip_on_missing_permission: false,
        }
    }

    impl World {
        fn app(&mut self, name: &str) -> AppRef {
            self.apps.intern(&AppId {
                guid: format!("{name}-guid"),
                name: name.into(),
                publisher: "P".into(),
                version: "1.0.0.0".into(),
            })
        }

        fn push_obj(&mut self, oid: ObjectNodeId, name: &str, tier: TrustTier) -> ObjectNodeId {
            self.objects.push(ObjectNode {
                id: oid.clone(),
                name: name.into(),
                declared_id: match oid.key {
                    ObjKey::Id(n) => Some(n),
                    ObjKey::Name(_) => None,
                },
                extends_target: None,
                implements: vec![],
                tier,
                source_table: None,
                table_no: None,
                source_table_temporary: false,
                page_controls: vec![],
                fields: vec![],
                dataitems: vec![],
                query_columns: Vec::new(),
                protected_vars: Vec::new(),
                parse_incomplete: false,
            });
            oid
        }

        fn obj(
            &mut self,
            app: AppRef,
            kind: ObjectKind,
            id: i64,
            name: &str,
            tier: TrustTier,
        ) -> ObjectNodeId {
            let oid = ObjectNodeId {
                app,
                kind,
                key: ObjKey::Id(id),
            };
            self.push_obj(oid, name, tier)
        }

        fn object_mut(&mut self, oid: &ObjectNodeId) -> &mut ObjectNode {
            self.objects
                .iter_mut()
                .find(|o| o.id == *oid)
                .expect("object")
        }

        fn rtn_with(
            &mut self,
            o: &ObjectNodeId,
            name: &str,
            f: impl FnOnce(&mut RoutineNode),
        ) -> RoutineNodeId {
            let tier = self
                .objects
                .iter()
                .find(|x| x.id == *o)
                .expect("object")
                .tier;
            let mut r = RoutineNode {
                id: RoutineNodeId {
                    object: o.clone(),
                    name_lc: name.fold_identifier().into(),
                    enclosing_member_lc: None,
                    params_count: 0,
                    sig_fp: 0,
                },
                name: name.into(),
                is_trigger: false,
                access: Access::Public,
                tier,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind: None,
                include_sender: None,
                abi_routine_kind: None,
                abi_event_kind: None,
                param_sig_key: Default::default(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                preproc_context: Box::default(),
                abi_params: AbiParams::Missing,
            };
            f(&mut r);
            let id = r.id.clone();
            self.routines.push(r);
            id
        }

        fn rtn(&mut self, o: &ObjectNodeId, name: &str) -> RoutineNodeId {
            self.rtn_with(o, name, |_| {})
        }

        fn trigger(&mut self, o: &ObjectNodeId, name: &str) -> RoutineNodeId {
            self.rtn_with(o, name, |r| r.is_trigger = true)
        }

        fn edge(
            &mut self,
            from: &RoutineNodeId,
            kind: EdgeKind,
            shape: DispatchShape,
            routes: Vec<Route>,
            line: u32,
        ) -> ObligationId {
            let span = CanonicalSpan {
                unit: "src/X.al".into(),
                start: SourcePos { line, col: 0 },
                end: SourcePos { line, col: 1 },
            };
            // The fingerprint keeps every hand-made site distinct.
            let fp = self.edges.len() as u64;
            let ob = ObligationId::CallSite {
                caller: from.clone(),
                span: span.clone(),
                callee_fp: fp,
            };
            self.edges.push(ClassifiedEdge {
                obligation_id: ob.clone(),
                edge: EngineEdge {
                    from: from.clone(),
                    site: SiteId {
                        caller: from.clone(),
                        span,
                        callee_fingerprint: fp,
                    },
                    kind,
                    shape,
                    completeness: SetCompleteness::Partial {
                        reason: OpenWorldReason::ReverseDependentExtensions,
                    },
                    routes,
                },
            });
            ob
        }

        fn call(&mut self, from: &RoutineNodeId, to: &RoutineNodeId, line: u32) {
            self.edge(
                from,
                EdgeKind::Call,
                DispatchShape::Exact,
                vec![src(to)],
                line,
            );
        }

        fn event(&mut self, publisher: &RoutineNodeId, subscribers: &[&RoutineNodeId]) {
            self.edges.push(ClassifiedEdge {
                obligation_id: ObligationId::Publisher(publisher.clone()),
                edge: EngineEdge {
                    from: publisher.clone(),
                    site: SiteId {
                        caller: publisher.clone(),
                        span: CanonicalSpan {
                            unit: String::new(),
                            start: SourcePos { line: 0, col: 0 },
                            end: SourcePos { line: 0, col: 0 },
                        },
                        callee_fingerprint: 0,
                    },
                    kind: EdgeKind::EventFlow,
                    shape: DispatchShape::Multicast,
                    completeness: SetCompleteness::Partial {
                        reason: OpenWorldReason::ReverseDependentSubscribers,
                    },
                    routes: subscribers.iter().map(|s| src(s)).collect(),
                },
            });
        }

        fn project(&self, primary: AppRef) -> Out {
            let mut objects = self.objects.clone();
            objects.sort_by(|a, b| a.id.cmp(&b.id));
            let mut routines = self.routines.clone();
            routines.sort_by(|a, b| a.id.cmp(&b.id));
            let mut topology = DependencyGraph::default();
            for (a, b) in &self.deps {
                topology.add_dependency(*a, *b);
            }
            let obj_index = ObjectIndex::build(&objects);
            let graph = ProgramGraph {
                apps: self.apps.clone(),
                topology,
                objects: objects.into(),
                routines: routines.into(),
                obj_index,
                ..Default::default()
            };
            let index = ResolveIndex::build(&graph);
            let surface = DeclSurface::build(&graph, &[]);
            let recovered = HashSet::new();
            Out(project(&Inputs {
                graph: &graph,
                index: &index,
                surface: &surface,
                primary,
                edges: &self.edges,
                site_facts: &self.site_facts,
                facts: &self.facts,
                missing_obligations: &self.missing,
                recovered_files: &recovered,
            }))
        }
    }

    struct Out(Projection);

    impl Out {
        /// Nodes labelled `Object.member`, `Object` (an object node) or a hub id.
        fn find(&self, label: &str) -> Vec<&Node> {
            if label.starts_with("h:") {
                return self.0.nodes.iter().filter(|n| n.id == label).collect();
            }
            let (obj, member) = match label.split_once('.') {
                Some((o, m)) => (o, Some(m)),
                None => (label, None),
            };
            self.0
                .nodes
                .iter()
                .filter(|n| {
                    n.object_name.as_deref() == Some(obj)
                        && n.member_name.as_deref() == member
                        && (member.is_some() || n.kind == "Object")
                })
                .collect()
        }

        fn has(&self, label: &str) -> bool {
            !self.find(label).is_empty()
        }

        fn node(&self, label: &str) -> &Node {
            let hits = self.find(label);
            assert_eq!(hits.len(), 1, "{label}: {hits:?}");
            hits[0]
        }

        fn edges(&self, from: &str, to: &str) -> Vec<&Edge> {
            let (f, t) = (self.node(from).id.clone(), self.node(to).id.clone());
            self.0
                .edges
                .iter()
                .filter(|e| e.caller == f && e.callee == t)
                .collect()
        }

        fn edge(&self, from: &str, to: &str) -> (&'static str, &'static str, &'static str) {
            let es = self.edges(from, to);
            assert_eq!(es.len(), 1, "{from} -> {to}: {es:?}");
            (es[0].kind, es[0].confidence, es[0].via)
        }
    }

    /// Edge kinds come from the engine `EdgeKind` first, the dispatch shape
    /// second: an implicit trigger is a `Multicast` like an event flow, and must
    /// still be `Trigger`. A `Call` route to a trigger is a typed-variable run and
    /// becomes `Run` to the object node, never a call into the trigger. The
    /// `AmbiguousOverload` row is hand-stated too, so it does not depend on the
    /// resolver producing one.
    #[test]
    fn edge_kinds_map_by_engine_kind_first() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let t = w.obj(a, ObjectKind::Table, 2, "T", TrustTier::Workspace);
        let run = w.obj(a, ObjectKind::Codeunit, 3, "Run", TrustTier::Workspace);
        let i1 = w.obj(a, ObjectKind::Codeunit, 4, "Impl", TrustTier::Workspace);
        let caller = w.rtn(&cu, "Caller");
        let exact = w.rtn(&cu, "Exact");
        let amb = w.rtn(&cu, "Amb");
        let ins = w.trigger(&t, "OnInsert");
        let on_run = w.trigger(&run, "OnRun");
        let impl1 = w.rtn(&i1, "Rate");
        let publisher = w.rtn_with(&cu, "OnEvt", |r| {
            r.publisher_kind = Some(PublisherKind::Integration)
        });
        let sub = w.rtn(&i1, "Sub");
        w.call(&caller, &exact, 1);
        w.call(&caller, &publisher, 2);
        w.edge(
            &caller,
            EdgeKind::ImplicitTrigger,
            DispatchShape::Multicast,
            vec![src(&ins)],
            3,
        );
        w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::Polymorphic,
            vec![src(&impl1)],
            4,
        );
        w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::Exact,
            vec![src(&on_run)],
            5,
        );
        let mut amb_route = src(&amb);
        amb_route.conditions = vec![Condition::AmbiguousDispatch];
        w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::AmbiguousOverload,
            vec![amb_route],
            6,
        );
        w.event(&publisher, &[&sub]);
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.Caller", "Cu.Exact"),
            ("Direct", "Resolved", "call")
        );
        assert_eq!(o.edges("Cu.Caller", "Cu.Exact")[0].line, Some(2), "1-based");
        assert_eq!(
            o.edge("Cu.Caller", "T.OnInsert"),
            ("Trigger", "Resolved", "implicitTrigger")
        );
        assert_eq!(
            o.edge("Cu.Caller", "Impl.Rate"),
            ("Interface", "OverApprox", "interface")
        );
        assert_eq!(o.edge("Cu.Caller", "Run"), ("Run", "Resolved", "run"));
        assert!(
            o.edges("Cu.Caller", "Run.OnRun").is_empty(),
            "a run never targets the trigger directly"
        );
        assert_eq!(
            o.edge("Run", "Run.OnRun"),
            ("Trigger", "Resolved", "lifecycle")
        );
        assert_eq!(
            o.edge("Cu.Caller", "Cu.Amb"),
            ("Direct", "OverApprox", "call")
        );
        assert_eq!(
            o.edges("Cu.Caller", "Cu.Amb")[0].conditions,
            vec!["ambiguousDispatch"]
        );
        assert_eq!(
            o.edge("Cu.OnEvt", "Impl.Sub"),
            ("Event", "Resolved", "eventFlow")
        );
    }

    /// A run reaches every trigger of the object, its extensions' triggers, a
    /// page's parts, its source table's triggers and the pages its actions open;
    /// only the run object's own entry trigger is `Resolved`.
    #[test]
    fn lifecycle_reaches_the_whole_object() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let table = w.obj(a, ObjectKind::Table, 2, "Tab", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 3, "Card", TrustTier::Workspace);
        let ext = w.obj(
            a,
            ObjectKind::PageExtension,
            4,
            "CardExt",
            TrustTier::Workspace,
        );
        let part = w.obj(a, ObjectKind::Page, 5, "Part", TrustTier::Workspace);
        let opened = w.obj(a, ObjectKind::Page, 6, "Opened", TrustTier::Workspace);
        w.object_mut(&page).source_table = Some(ObjectRef::Id(2));
        w.object_mut(&page).page_controls = vec![PageControlNode {
            name_lc: "lines".into(),
            kind: PageControlKind::Part,
            target: ObjectRef::Id(5),
        }];
        w.object_mut(&ext).extends_target = Some("Card".into());
        w.facts.objects.insert(
            page.clone(),
            ObjectFacts {
                opens: vec![RunTarget::Object(opened.clone())],
                ..Default::default()
            },
        );
        let sink = w.rtn(&cu, "Sink");
        let get = w.trigger(&page, "OnAfterGetRecord");
        let open = w.trigger(&page, "OnOpenPage");
        let ext_t = w.trigger(&ext, "OnOpenPage");
        let part_t = w.trigger(&part, "OnOpenPage");
        let tab_t = w.trigger(&table, "OnModify");
        let opened_t = w.trigger(&opened, "OnOpenPage");
        for t in [&get, &open, &ext_t, &part_t, &tab_t, &opened_t] {
            // Every trigger calls back into the workspace, so pruning keeps it.
            w.call(t, &sink, 9);
        }
        let runner = w.rtn(&cu, "Runner");
        w.edge(
            &runner,
            EdgeKind::Run,
            DispatchShape::Multicast,
            vec![src(&open), src(&ext_t)],
            1,
        );
        let o = w.project(a);
        assert_eq!(o.edge("Cu.Runner", "Card"), ("Run", "Resolved", "run"));
        assert!(
            !o.has("CardExt"),
            "an extension's entry trigger stands for its base object"
        );
        assert_eq!(
            o.edge("Card", "Card.OnOpenPage"),
            ("Trigger", "Resolved", "lifecycle")
        );
        for (to, what) in [
            ("Card.OnAfterGetRecord", "own trigger"),
            ("CardExt.OnOpenPage", "extension trigger"),
            ("Part", "part"),
            ("Tab.OnModify", "source-table trigger"),
            ("Opened", "opened page"),
        ] {
            assert_eq!(
                o.edge("Card", to),
                ("Trigger", "OverApprox", "lifecycle"),
                "{what}"
            );
        }
        assert_eq!(
            o.edge("Part", "Part.OnOpenPage"),
            ("Trigger", "Resolved", "lifecycle")
        );
    }

    /// Rules 2-4: a declared runnable variable, `[HandlerFunctions]` and the
    /// test runner.
    #[test]
    fn declared_runnables_handlers_and_test_runner() {
        let mut w = World::default();
        let a = w.app("A");
        let tests = w.obj(a, ObjectKind::Codeunit, 1, "Tests", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 2, "Card", TrustTier::Workspace);
        let t = w.rtn(&tests, "TestIt");
        let handler = w.rtn(&tests, "ModalHandler");
        let on_run = w.trigger(&tests, "OnRun");
        let open = w.trigger(&page, "OnOpenPage");
        w.call(&open, &handler, 1);
        w.call(&on_run, &handler, 2);
        w.facts.routines.insert(
            t.clone(),
            RoutineFacts {
                test: true,
                handlers: vec!["modalhandler".into()],
                runs: vec![RunTarget::Object(page.clone())],
            },
        );
        let o = w.project(a);
        assert_eq!(
            o.edge("Tests.TestIt", "Card"),
            ("Run", "OverApprox", "declaredRunnable")
        );
        assert_eq!(
            o.edge("Tests.TestIt", "Tests.ModalHandler"),
            ("Event", "OverApprox", "testHandler")
        );
        assert_eq!(
            o.edge("Tests.TestIt", "Tests"),
            ("Run", "Resolved", "testRunner")
        );
        assert_eq!(
            o.edge("Tests", "Tests.OnRun"),
            ("Trigger", "Resolved", "lifecycle")
        );
        assert_eq!(o.node("Tests.TestIt").test, Some(true));
    }

    /// Open sites become hub edges; each hub expands to what it stands for.
    #[test]
    fn open_bounds_become_hubs() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let other = w.obj(a, ObjectKind::Codeunit, 2, "Other", TrustTier::Workspace);
        let table = w.obj(a, ObjectKind::Table, 3, "Tab", TrustTier::Workspace);
        let caller = w.rtn(&cu, "Caller");
        w.rtn(&other, "Ping");
        let on_run = w.trigger(&other, "OnRun");
        let ins = w.trigger(&table, "OnBeforeInsert");
        let sink = w.rtn(&cu, "Sink");
        w.call(&on_run, &sink, 1);
        w.call(&ins, &sink, 1);
        let s1 = w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::DynamicOpen,
            vec![unknown()],
            1,
        );
        w.site_facts.insert(
            s1,
            SiteFacts {
                open: vec![
                    OpenBound::Name {
                        name_lc: "ping".into(),
                        arity: 0,
                    },
                    OpenBound::Run {
                        kind: Some(ObjectKind::Codeunit),
                    },
                    OpenBound::Record { op: "insert" },
                ],
                ..Default::default()
            },
        );
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.Caller", "h:name:ping/0"),
            ("Direct", "Open", "dynamic"),
            "a DynamicOpen site is `dynamic`"
        );
        assert_eq!(
            o.edge("h:name:ping/0", "Other.Ping"),
            ("Direct", "Open", "hubExpansion")
        );
        assert_eq!(
            o.edge("Cu.Caller", "h:run:codeunit"),
            ("Run", "Open", "dynamic")
        );
        assert_eq!(
            o.edge("h:run:codeunit", "Other"),
            ("Run", "Open", "hubExpansion")
        );
        assert_eq!(
            o.edge("h:record:insert", "Tab.OnBeforeInsert"),
            ("Trigger", "Open", "hubExpansion")
        );
        assert!(
            o.find("h:any").is_empty(),
            "every open site here is bounded"
        );
    }

    /// The universal hub: an unknown route with no bound, and a routine whose
    /// obligation the coverage contract misses, both call `h:any`; once a
    /// workspace routine reaches it, pruning keeps every node that reaches the
    /// workspace (the hub is a caller of each).
    #[test]
    fn universal_hub_callers_and_pruning() {
        let mut w = World::default();
        let a = w.app("A");
        let d = w.app("D");
        w.deps.push((a, d));
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let dep = w.obj(d, ObjectKind::Codeunit, 9, "Dep", TrustTier::EmbeddedSource);
        let caller = w.rtn(&cu, "Caller");
        let missed = w.rtn(&cu, "Missed");
        let target = w.rtn(&cu, "Target");
        let back = w.rtn(&dep, "Back");
        w.call(&back, &target, 1); // reaches the workspace; no edge calls it
        w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::Exact,
            vec![unknown()],
            1,
        );
        w.missing.push(ObligationId::Publisher(missed.clone()));
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.Caller", "h:any"),
            ("Direct", "Open", "unresolved")
        );
        assert_eq!(
            o.edge("Cu.Missed", "h:any"),
            ("Direct", "Open", "coverageMiss")
        );
        assert_eq!(o.node("h:any").universal, Some(true));
        assert!(
            o.has("Dep.Back"),
            "h:any may call Dep.Back, which reaches the workspace"
        );
        assert_eq!(o.0.universal_callers, 2);
    }

    /// Pruning keeps workspace -> dependency -> workspace paths and drops a
    /// dependency branch that never returns. A dependency table's synthetic
    /// platform publisher has no incoming edge at all, yet it survives with its
    /// `hiddenCallers` flag and its event edge to the workspace subscriber.
    #[test]
    fn pruning_keeps_returning_paths_and_hidden_callers() {
        let mut w = World::default();
        let a = w.app("A");
        let d = w.app("D");
        w.deps.push((a, d));
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let dep = w.obj(d, ObjectKind::Codeunit, 9, "Dep", TrustTier::EmbeddedSource);
        let dtab = w.obj(
            d,
            ObjectKind::Table,
            18,
            "Customer",
            TrustTier::EmbeddedSource,
        );
        let caller = w.rtn(&cu, "Caller");
        let back = w.rtn(&cu, "Back");
        let sub = w.rtn_with(&cu, "OnAfterInsertCustomer", |r| {
            r.event_subscribers = vec![subscriber_args("table", "customer", "onafterinsertevent")]
        });
        let via = w.rtn(&dep, "Via");
        let dead = w.rtn(&dep, "DeadEnd");
        let dead2 = w.rtn(&dep, "DeadEnd2");
        let synth = w.rtn_with(&dtab, "OnAfterInsertEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform);
            r.id.params_count = 8;
        });
        w.call(&caller, &via, 1);
        w.call(&via, &back, 2);
        w.call(&via, &dead, 3);
        w.call(&dead, &dead2, 4);
        w.event(&synth, &[&sub]);
        let o = w.project(a);
        assert!(o.has("Dep.Via"));
        assert!(
            !o.has("Dep.DeadEnd") && !o.has("Dep.DeadEnd2"),
            "a branch that never returns is pruned"
        );
        assert!(o.has("Cu.Caller") && o.has("Cu.Back"));
        let publisher = o.node("Customer.OnAfterInsertEvent");
        assert_eq!(publisher.hidden_callers, vec!["unraisedPublisher"]);
        assert_eq!(publisher.synthetic, Some(true));
        assert_eq!(publisher.dependency, Some(true));
        assert_eq!(
            o.edge("Customer.OnAfterInsertEvent", "Cu.OnAfterInsertCustomer"),
            ("Event", "Resolved", "eventFlow")
        );
    }

    /// `unraisedPublisher` stays on a synthetic TABLE publisher even when a hub
    /// reaches it (record operations still do not raise it); a synthetic PAGE
    /// publisher is not flagged (its page's runs raise it); an
    /// `[EventSubscriber]` with no incoming event edge is `unboundSubscriber`; a
    /// table-extension `OnAfterInsert` is `unmodeledRecordTrigger`.
    #[test]
    fn hidden_callers_flags() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let table = w.obj(a, ObjectKind::Table, 2, "Tab", TrustTier::Workspace);
        let page = w.obj(a, ObjectKind::Page, 3, "Pg", TrustTier::Workspace);
        let ext = w.obj(
            a,
            ObjectKind::TableExtension,
            4,
            "TabExt",
            TrustTier::Workspace,
        );
        w.object_mut(&ext).extends_target = Some("Tab".into());
        let caller = w.rtn(&cu, "Caller");
        let tsynth = w.rtn_with(&table, "OnAfterInsertEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform)
        });
        let psynth = w.rtn_with(&page, "OnOpenPageEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform)
        });
        w.rtn_with(&cu, "Unbound", |r| {
            r.event_subscribers = vec![subscriber_args("codeunit", "nowhere", "onx")]
        });
        w.trigger(&ext, "OnAfterInsert");
        let s1 = w.rtn(&cu, "S1");
        let s2 = w.rtn(&cu, "S2");
        w.event(&tsynth, &[&s1]);
        w.event(&psynth, &[&s2]);
        // The caller may run the page (a page variable).
        w.facts.routines.insert(
            caller.clone(),
            RoutineFacts {
                runs: vec![RunTarget::Object(page.clone())],
                ..Default::default()
            },
        );
        let site = w.edge(
            &caller,
            EdgeKind::Call,
            DispatchShape::Exact,
            vec![unknown()],
            1,
        );
        w.site_facts.insert(
            site,
            SiteFacts {
                open: vec![OpenBound::Record { op: "insert" }],
                ..Default::default()
            },
        );
        let o = w.project(a);
        assert_eq!(
            o.edge("h:record:insert", "Tab.OnAfterInsertEvent"),
            ("Trigger", "Open", "hubExpansion")
        );
        assert_eq!(
            o.node("Tab.OnAfterInsertEvent").hidden_callers,
            vec!["unraisedPublisher"],
            "a hub edge into it must not clear the flag"
        );
        assert_eq!(
            o.edge("Pg", "Pg.OnOpenPageEvent"),
            ("Trigger", "OverApprox", "lifecycle")
        );
        assert!(o.node("Pg.OnOpenPageEvent").hidden_callers.is_empty());
        assert_eq!(
            o.node("Cu.Unbound").hidden_callers,
            vec!["unboundSubscriber"]
        );
        assert_eq!(
            o.node("TabExt.OnAfterInsert").hidden_callers,
            vec!["unmodeledRecordTrigger"]
        );
    }

    /// Opaque bodies: a symbol-only routine calls its app's callbacks hub, a
    /// symbol-only publisher does not, and the hub reaches the app's own
    /// publishers that have a subscriber in the program.
    #[test]
    fn symbol_only_bodies_reach_the_callbacks_hub() {
        let mut w = World::default();
        let a = w.app("A");
        let s = w.app("S");
        w.deps.push((a, s));
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let sym = w.obj(s, ObjectKind::Codeunit, 7, "SymCu", TrustTier::SymbolOnly);
        let test = w.rtn(&cu, "TestIt");
        let sub = w.rtn(&cu, "Sub");
        let api = w.rtn(&sym, "Api");
        let publisher = w.rtn_with(&sym, "OnApi", |r| {
            r.publisher_kind = Some(PublisherKind::Integration)
        });
        w.call(&test, &api, 1);
        w.event(&publisher, &[&sub]);
        let o = w.project(a);
        let hub = "h:callbacks:s-guid";
        assert_eq!(o.edge("SymCu.Api", hub), ("Event", "Open", "opaqueBody"));
        assert!(
            o.edges("SymCu.OnApi", hub).is_empty(),
            "a publisher's body only raises"
        );
        assert_eq!(
            o.edge(hub, "SymCu.OnApi"),
            ("Event", "Open", "hubExpansion")
        );
        assert_eq!(o.node("SymCu.Api").symbol_only, Some(true));
        assert_eq!(o.node("SymCu.Api").source_path, None);
    }

    /// Duplicate rows merge to the strongest confidence and the smallest line;
    /// self-edges are dropped.
    #[test]
    fn dedupe_keeps_strongest_and_drops_self_edges() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let x = w.rtn(&cu, "X");
        let y = w.rtn(&cu, "Y");
        let mut manual = src(&y);
        manual.conditions = vec![Condition::ManualBinding];
        w.edge(&x, EdgeKind::Call, DispatchShape::Exact, vec![manual], 1);
        w.edge(&x, EdgeKind::Call, DispatchShape::Exact, vec![src(&y)], 7);
        w.edge(&x, EdgeKind::Call, DispatchShape::Exact, vec![src(&y)], 4);
        w.call(&x, &x, 2);
        let o = w.project(a);
        let es = o.edges("Cu.X", "Cu.Y");
        assert_eq!(es.len(), 1);
        assert_eq!((es[0].confidence, es[0].line), ("Resolved", Some(2)));
        assert!(es[0].conditions.is_empty());
        assert!(o.edges("Cu.X", "Cu.X").is_empty());
    }

    /// Ids are stable and injective: name-keyed objects whose names differ only
    /// in where a `/` falls get different ids.
    #[test]
    fn ids_are_stable_and_injective() {
        let mut w = World::default();
        let a = w.app("A");
        let name_keyed = |w: &mut World, name: &str| {
            let oid = ObjectNodeId {
                app: a,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Name(name.into()),
            };
            w.push_obj(oid, name, TrustTier::Workspace)
        };
        let o1 = name_keyed(&mut w, "a/b");
        let o2 = name_keyed(&mut w, "a");
        let r1 = w.rtn(&o1, "c");
        let r2 = w.rtn(&o2, "b/c");
        w.call(&r1, &r2, 1);
        let p1 = w.project(a);
        let p2 = w.project(a);
        assert_eq!(p1.0.nodes, p2.0.nodes);
        let (id1, id2) = (p1.node("a/b.c").id.clone(), p1.node("a.b/c").id.clone());
        assert_ne!(id1, id2);
        assert!(id1.starts_with("r:") && id1.len() == 26, "{id1}");
    }
}
