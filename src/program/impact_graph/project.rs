//! The projection: engine edges plus the may-execute rules, open-site hubs,
//! hidden-callers flags and pruning (spec §4.3-§4.6). Pure over its [`Inputs`].

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use super::facts::{RunTarget, SourceFacts};
use super::sited::{
    Anchor, Boundary, Collapsed, Site, SitedBuilder, SitedGraph, collapse, condition_mask,
    condition_names, via_ix,
};
use super::{Edge, Node};
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{Access, ObjectNode, ObjectRef, RoutineNode};
use crate::program::resolve::decl_surface::DeclSurface;
use crate::program::resolve::edge::{
    AbiRoutineKey, Condition, DispatchShape, EdgeKind, EvidenceKind, OpenBound, Route, RouteTarget,
};
use crate::program::resolve::event::PublisherKind;
use crate::program::resolve::full::{FieldRefValidate, ObligationId};
use crate::program::resolve::index::{ObjectRefResolution, ResolveIndex};
use crate::program::resolved::EdgeView;
use crate::snapshot::TrustTier;

/// Everything the projection reads.
pub struct Inputs<'a> {
    pub graph: &'a ProgramGraph,
    pub index: &'a ResolveIndex,
    pub surface: &'a DeclSurface,
    /// The workspace app (`primary_app_ref`), the workspace's call-site and
    /// event-flow edges, the dependency bodies' call-site edges, and both sets'
    /// site facts. Read only through `all_edges` (its order is a contract).
    pub edges: EdgeView<'a>,
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
    Name {
        name_lc: String,
        arity: usize,
    },
    Run(Option<ObjectKind>),
    Record(&'static str),
    /// Round-3 c02: the platform table events of a record operation that runs
    /// no trigger (`OpenBound::RecordEvents`), `h:record-events:<op>`.
    RecordEvents(&'static str),
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

/// The entry trigger a run of `kind` always runs (`resolver::entry_trigger_name`).
fn entry_trigger(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Page => "onopenpage",
        ObjectKind::Report => "onprereport",
        ObjectKind::XmlPort => "onprexmlport",
        ObjectKind::Query => "onbeforeopen",
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
        Condition::OnTaskFailure => "onTaskFailure",
    }
}

/// Whether a table trigger named `name_lc` runs for a record operation of family
/// `op`: `On<Op>`, a table extension's `OnBefore<Op>`/`OnAfter<Op>` (the shared
/// stem rule, [`crate::program::resolve::edge::trigger_family`]; ENG-15).
fn is_record_trigger_of(name_lc: &str, op: &str) -> bool {
    crate::program::resolve::edge::trigger_family(name_lc) == Some(op)
}

/// Whether a platform table event named `name_lc` is one a record operation of
/// family `op` raises (`resolver::table_event_names`).
fn is_record_event_of(name_lc: &str, op: &str) -> bool {
    crate::program::resolve::resolver::table_event_names(op).is_some_and(|e| e.contains(&name_lc))
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
    /// Every site added, in order: the sited graph.
    sited: SitedBuilder,
    /// The sites collapsed so far (the rules after the call sites read the
    /// collapsed edges: hub expansion, `hiddenCallers`).
    edges: HashMap<(u32, u32, Kind), Collapsed>,
    objects: BTreeSet<ObjectNodeId>,
    /// Polled every [`CANCEL_EVERY`] sites; once it says stop, no site is
    /// added and the build is reported cancelled.
    cancel: &'a dyn Fn() -> bool,
    cancelled: bool,
    added: u64,
    /// The [`Site::guard`] bits of the sites being added (set around one
    /// engine route in [`Self::engine_edges`]; 0 otherwise).
    site_guard: u8,
    /// What each page blocks a user from doing to its source table's records
    /// through it ([`crate::program::page_ui`]); pages blocking nothing are
    /// absent.
    page_ui: HashMap<ObjectNodeId, crate::program::page_ui::PageUiBlocks>,
}

/// How many sites a sited-graph build adds between cancellation checks
/// (literal reach's interval).
pub(crate) const CANCEL_EVERY: u64 = 4096;

/// The trigger boundary of a record trigger or platform table event named
/// `name_lc`: the operation that fires it.
pub(crate) fn trigger_op(name_lc: &str) -> Option<u8> {
    super::sited::RECORD_OPS
        .iter()
        .position(|op| is_record_trigger_of(name_lc, op) || is_record_event_of(name_lc, op))
        .map(|i| i as u8)
}

impl<'a> Builder<'a> {
    #[cfg(test)]
    fn new(inp: &'a Inputs<'a>) -> Self {
        Self::with_cancel(inp, &|| false)
    }

    fn with_cancel(inp: &'a Inputs<'a>, cancel: &'a dyn Fn() -> bool) -> Self {
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
            sited: SitedBuilder::new(g),
            edges: HashMap::new(),
            objects: BTreeSet::new(),
            cancel,
            cancelled: false,
            added: 0,
            site_guard: 0,
            page_ui: crate::program::page_ui::page_ui_blocks(g),
        }
    }

    /// The collapsed edge `(from, to, kind)`, if any site made it.
    #[cfg(test)]
    fn edge(&self, from: &Key, to: &Key, kind: Kind) -> Option<&Collapsed> {
        let (f, t) = (self.sited.ix(from)?, self.sited.ix(to)?);
        self.edges.get(&(f, t, kind))
    }

    /// One site (the sited graph's edge) and its collapse into the export's
    /// edge. `line` is the site's anchor (`None`: no call site makes it).
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        from: Key,
        to: Key,
        kind: Kind,
        conf: Conf,
        via: &'static str,
        line: Option<Anchor>,
        conditions: BTreeSet<&'static str>,
    ) {
        if from == to || self.cancelled {
            return;
        }
        self.added += 1;
        if self.added.is_multiple_of(CANCEL_EVERY) && (self.cancel)() {
            self.cancelled = true;
            return;
        }
        if let Key::Object(o) = &to {
            self.objects.insert(o.clone());
        }
        if let Key::Object(o) = &from {
            self.objects.insert(o.clone());
        }
        let boundary = match (via, kind, &to) {
            (_, _, Key::Hub(Hub::Record(op) | Hub::RecordEvents(op))) => Boundary::TriggerOf {
                op: super::sited::RECORD_OPS
                    .iter()
                    .position(|o| o == op)
                    .map(|i| i as u8),
            },
            ("call" | "interface", _, _) => Boundary::Sync,
            ("run", Kind::Trigger, _) => Boundary::TriggerOf { op: None },
            ("run", Kind::Event, _) => Boundary::EventDispatch,
            ("run" | "declaredRunnable", _, _) => Boundary::Run,
            // F1: a record op raising its table's platform event belongs to
            // that op, as its trigger does (`trigger_op` knows the events).
            ("implicitTrigger" | "tableEvent", _, Key::Routine(r)) => Boundary::TriggerOf {
                op: trigger_op(r.name_lc.as_str()),
            },
            ("eventFlow", _, _) => Boundary::EventDispatch,
            ("backgroundStart", _, _) => Boundary::Background,
            ("lifecycle" | "clientEvent", _, _) => Boundary::Lifecycle,
            ("testRunner", _, _) => Boundary::TestRunner,
            ("testHandler", _, _) => Boundary::TestHandler,
            _ => Boundary::Open,
        };
        let mut site = Site {
            from: self.sited.node(&from),
            to: self.sited.node(&to),
            kind,
            conf,
            via: via_ix(via),
            conditions: condition_mask(conditions),
            boundary,
            anchor: line,
            guard: self.site_guard,
        };
        // A catalogued configured-dispatch arm (design r2 §4.3): a label in
        // the conditions mask; boundary, `via` and confidence stay the site's.
        if let (Key::Routine(r), Some(a)) = (&from, line)
            && let Some(region) = self.inp.facts.configured.get(r)
            && let Some(arm) = region.arm_at(a.start_line)
        {
            site.conditions |=
                condition_mask([super::configured_dispatch::CONDITION].iter().copied());
            self.sited.configured_arms.push((
                self.sited.sites.len() as u32,
                super::sited::ConfiguredArm {
                    entry: region.entry,
                    label: arm.label.as_str().into(),
                },
            ));
        }
        self.sited.sites.push(site);
        match self.edges.entry((site.from, site.to, kind)) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Collapsed::of(&site));
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                o.get_mut().merge(Collapsed::of(&site));
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
            key: key.object_node_key(),
        })
    }

    fn engine_edges(&mut self) {
        let inp = self.inp;
        for ce in inp.edges.all_edges() {
            let e = &ce.edge;
            let from = Key::Routine(e.from.clone());
            let line = match ce.obligation_id {
                ObligationId::CallSite { .. }
                | ObligationId::Background { .. }
                | ObligationId::TableEvent { .. } => Some(Anchor {
                    unit: self.sited.unit(&e.site.span.unit),
                    start_line: e.site.span.start.line,
                    start_col: e.site.span.start.col,
                    end_line: e.site.span.end.line,
                    end_col: e.site.span.end.col,
                }),
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
                        self.site_guard = self.table_event_guard(&ce.obligation_id, rid);
                        self.engine_route(&from, e.kind, e.shape, rid, line, conditions);
                        self.site_guard = 0;
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
                            let (kind, via) = match e.kind {
                                EdgeKind::Run | EdgeKind::Call => (Kind::Run, "run"),
                                EdgeKind::ImplicitTrigger => (Kind::Trigger, "run"),
                                EdgeKind::EventFlow => (Kind::Event, "run"),
                                EdgeKind::BackgroundStart => (Kind::Run, "backgroundStart"),
                                EdgeKind::TableEvent => (Kind::Event, "tableEvent"),
                            };
                            let to = Key::Object(self.run_object(&oid));
                            self.add(
                                from.clone(),
                                to,
                                kind,
                                Conf::OverApprox,
                                via,
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
                    .edges
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
            // B6 round 2: the platform's own dataset operations at a run (an
            // XmlPort import's insert or modify) may run the table's
            // `OnInsert`/`OnModify` and raise its table events: the edges the
            // model's cone already follows (`program_calls`'s dataset
            // operations), so a test that imports into a table is selected for
            // a change to that table's trigger or to a table-event subscriber.
            // ENG-12 / ENG-06: so does a record operation the site facts carry
            // instead of the edge (a call result's, `GetRef().Modify(true)`; a
            // FieldRef's `Validate`), its trigger rule already applied.
            if let ObligationId::CallSite { .. } = ce.obligation_id
                && let Some(facts) = inp.edges.site_facts.get(&ce.obligation_id)
            {
                let mut ops: Vec<(&[Route], &[Route], Conf)> = Vec::new();
                for eff in facts.dataset_run.iter().flat_map(|run| &run.effects) {
                    ops.push((&eff.triggers, &eff.table_events, Conf::OverApprox));
                }
                if let Some(cr) = &facts.call_result_record {
                    ops.push((&cr.triggers, &cr.table_events, Conf::Resolved));
                }
                if let Some(FieldRefValidate::Decided {
                    triggers,
                    table_events,
                    ..
                }) = &facts.field_ref_validate
                {
                    ops.push((triggers, table_events, Conf::Resolved));
                }
                for (triggers, table_events, conf) in ops {
                    let routes = triggers
                        .iter()
                        .map(|r| (Kind::Trigger, "implicitTrigger", r))
                        .chain(table_events.iter().map(|r| (Kind::Event, "tableEvent", r)));
                    for (kind, via, r) in routes {
                        let to = match &r.target {
                            RouteTarget::Routine(rid) => Key::Routine(rid.clone()),
                            RouteTarget::AbiSymbol { key } => {
                                let Some(oid) = self.abi_object(key) else {
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
                                    Key::Routine(rid)
                                } else {
                                    Key::Object(self.run_object(&oid))
                                }
                            }
                            RouteTarget::Builtin(_) | RouteTarget::Unresolved => continue,
                        };
                        // A dataset run's: whether the platform's own insert
                        // runs `OnInsert` is not documented (the operation's
                        // `Maybe` trigger edge), so OverApprox. The others, as
                        // an edge's route (`engine_route`): a conditioned
                        // route is OverApprox.
                        let conditions: BTreeSet<&'static str> = match conf {
                            Conf::OverApprox => BTreeSet::new(),
                            _ => r.conditions.iter().map(|c| condition_str(*c)).collect(),
                        };
                        let conf = if conditions.is_empty() {
                            conf
                        } else {
                            Conf::OverApprox
                        };
                        self.add(from.clone(), to, kind, conf, via, line, conditions);
                    }
                }
            }
            // Rule 2: a dispatching builtin that runs an object, called on a
            // receiver typed as one object (`ReportVar.SaveAsPdf()`,
            // `QueryVar.Open()`), runs that object. The site's open bound
            // below still names the kind's hub.
            if let ObligationId::CallSite { .. } = ce.obligation_id
                && let Some(t) = self.receiver_run(e, &ce.obligation_id)
            {
                let to = self.run_target_key(&t);
                self.add(
                    from.clone(),
                    to,
                    Kind::Run,
                    Conf::OverApprox,
                    "declaredRunnable",
                    line,
                    BTreeSet::new(),
                );
            }
            let bounds: &[OpenBound] = match ce.obligation_id {
                ObligationId::CallSite { .. } => inp
                    .edges
                    .site_facts
                    .get(&ce.obligation_id)
                    .map(|f| f.open.as_slice())
                    .unwrap_or(&[]),
                // A background start's dynamic codeunits are its catalog
                // call's open bound (`full::resolve_file_obligations`).
                ObligationId::Publisher(_)
                | ObligationId::Background { .. }
                | ObligationId::TableEvent { .. } => &[],
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
                    OpenBound::RecordEvents { op } => (Hub::RecordEvents(op), Kind::Trigger),
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

    /// The [`Site::guard`] bits of the route to `rid` of the edge `ob`
    /// ([`crate::program::table_event_guard`]): for a record operation's
    /// table-event route, what the operation proves (read off its call site's
    /// facts); for a platform publisher's route to a subscriber, what the
    /// subscriber exits on first thing (its source facts). 0 otherwise.
    fn table_event_guard(&self, ob: &ObligationId, rid: &RoutineNodeId) -> u8 {
        match ob {
            ObligationId::TableEvent {
                caller,
                span,
                callee_fp,
            } => self
                .inp
                .edges
                .site_facts
                .get(&ObligationId::CallSite {
                    caller: caller.clone(),
                    span: span.clone(),
                    callee_fp: *callee_fp,
                })
                .and_then(|f| f.table_event_raise)
                .map_or(0, |c| c.as_exits().bits()),
            ObligationId::Publisher(p)
                if self.rtn_by_id.get(p).is_some_and(|n| {
                    n.publisher_kind
                        == Some(crate::program::resolve::event::PublisherKind::Platform)
                }) =>
            {
                self.inp
                    .facts
                    .routines
                    .get(rid)
                    .map_or(0, |f| f.table_event_exits.bits())
            }
            _ => 0,
        }
    }

    /// What a call site runs through a dispatching builtin on a typed
    /// receiver (rule 2): the site resolved to a builtin that runs an object
    /// of kind K ([`dispatching_builtin`], `Run(K)`, a `Page`, `Report`,
    /// `XmlPort` or `Query` instance method) and the resolver typed its
    /// receiver as an object of kind K; the receiver's name resolves from the
    /// caller's object. `None` for every other site, and for a receiver the
    /// caller's closure does not resolve (its open bound still names the hub).
    fn receiver_run(
        &self,
        e: &crate::program::resolve::edge::Edge,
        ob: &ObligationId,
    ) -> Option<RunTarget> {
        use crate::program::resolve::full::ReceiverFactType;
        use crate::program::resolve::member_catalog::{DispatchingBuiltin, dispatching_builtin};
        let kind = e.routes.iter().find_map(|r| match &r.target {
            RouteTarget::Builtin(bid) => match dispatching_builtin(bid) {
                Some(DispatchingBuiltin::Run(
                    k @ (ObjectKind::Page
                    | ObjectKind::Report
                    | ObjectKind::XmlPort
                    | ObjectKind::Query),
                )) => Some(k),
                _ => None,
            },
            _ => None,
        })?;
        let facts = self.inp.edges.site_facts.get(ob)?;
        let ReceiverFactType::Object { kind: rk, name } = &facts.receiver.as_ref()?.ty else {
            return None;
        };
        if *rk != kind {
            return None;
        }
        self.resolve_ref(
            &e.from.object,
            kind,
            &super::facts::property_object_ref(name),
        )
    }

    fn engine_route(
        &mut self,
        from: &Key,
        kind: EdgeKind,
        shape: DispatchShape,
        rid: &RoutineNodeId,
        line: Option<Anchor>,
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
            // F1 (spec 2026-11-06-db-event-subscribers): a record operation
            // raises its table's platform event; the publisher's own
            // `eventFlow` edges go on to the subscribers.
            EdgeKind::TableEvent => self.add(
                from.clone(),
                Key::Routine(rid.clone()),
                Kind::Event,
                conf,
                "tableEvent",
                line,
                conditions,
            ),
            // F3: a run of the codeunit's object, as `Codeunit.Run` (rule 1),
            // across a background boundary ([`Boundary::Background`]). A
            // failure codeunit's route carries `onTaskFailure`: `OverApprox`.
            EdgeKind::BackgroundStart => {
                let to = Key::Object(self.run_object(&rid.object));
                self.add(
                    from.clone(),
                    to,
                    Kind::Run,
                    conf,
                    "backgroundStart",
                    line,
                    conditions,
                );
            }
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

    /// The folded names of the `usercontrol` (control add-in) layout controls
    /// of `oid` and of each extension of it: a trigger whose enclosing member
    /// is one of them is an add-in event. AL control names are unique within
    /// a page and its extensions, so no field or action shares one.
    fn usercontrol_names(&self, oid: &ObjectNodeId) -> HashSet<&'a str> {
        let mut objs: Vec<&ObjectNodeId> = vec![oid];
        if let Some(exts) = self.exts_of.get(oid) {
            objs.extend(exts.iter().copied());
        }
        objs.into_iter()
            .filter_map(|o| self.obj_by_id.get(o).copied())
            .flat_map(|o| o.page_controls.iter())
            .filter(|c| c.kind == crate::program::node_extract::PageControlKind::UserControl)
            .map(|c| c.name_lc.as_str())
            .collect()
    }

    /// Rule 1 (lifecycle) for one object node.
    fn lifecycle(&mut self, oid: &ObjectNodeId) {
        let from = Key::Object(oid.clone());
        let Some(obj) = self.obj_by_id.get(oid).copied() else {
            return;
        };
        let entry = entry_trigger(oid.kind);
        let addin_controls = self.usercontrol_names(oid);
        for t in self.triggers_with_extensions(oid) {
            let conf = if t.id.object == *oid
                && t.id.enclosing_member_lc.is_none()
                && t.id.name_lc.as_str() == entry
            {
                Conf::Resolved
            } else {
                Conf::OverApprox
            };
            // A control add-in's event trigger (`usercontrol(Body; X) {
            // trigger EditorInitialized() … }`) runs only when the add-in's
            // JavaScript raises it in a client: a user's page run may, a
            // `TestPage` cannot (it hosts no add-in). Its own `via` lets a
            // test walk refuse it (`test_select::select`) while impact keeps
            // it (round-1 c21).
            let via = if t
                .id
                .enclosing_member_lc
                .as_ref()
                .is_some_and(|m| addin_controls.contains(m.as_str()))
            {
                "clientEvent"
            } else {
                "lifecycle"
            };
            self.simple(
                from.clone(),
                Key::Routine(t.id.clone()),
                Kind::Trigger,
                conf,
                via,
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
        let mut action_runs: HashSet<Key> = HashSet::new();
        // Per run target: the actions that run it, each as `(the object
        // declaring it, its name)` (round-2 r2 §2.1 step 3).
        let mut action_names: HashMap<Key, Vec<(ObjectNodeId, Box<str>)>> = HashMap::new();
        let mut tables: Vec<ObjectNodeId> = Vec::new();
        for m in &members {
            if let Some(f) = self.inp.facts.objects.get(*m) {
                opens.extend(f.opens.iter().map(|t| self.run_target_key(t)));
                action_runs.extend(f.action_runs.iter().map(|t| self.run_target_key(t)));
                for (t, name) in &f.action_run_names {
                    action_names
                        .entry(self.run_target_key(t))
                        .or_default()
                        .push(((*m).clone(), Box::from(name.as_str())));
                }
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
        // A page declared `SourceTableTemporary = true` works on temporary
        // records of its source table: the table events its record operations
        // raise come with the record temporary (`Site::guard`).
        let mut temporary_source: Option<ObjectNodeId> = None;
        // The page's source table: what its UI permissions apply to.
        let mut page_source: Option<ObjectNodeId> = None;
        if oid.kind == ObjectKind::Page
            && let Some(st) = &obj.source_table
            && let Some(RunTarget::Object(table)) = self.resolve_ref(oid, ObjectKind::Table, st)
        {
            if let Some(f) = self.inp.facts.objects.get(&table) {
                opens.extend(f.lookup_pages.iter().map(|t| self.run_target_key(t)));
            }
            if self
                .inp
                .facts
                .objects
                .get(oid)
                .is_some_and(|f| f.source_table_temporary)
            {
                temporary_source = Some(table.clone());
            }
            page_source = Some(table.clone());
            tables.push(table);
        }
        for to in opens {
            let action = action_runs.contains(&to);
            let names = if action {
                action_names.get(&to).cloned().unwrap_or_default()
            } else {
                Vec::new()
            };
            let before = self.sited.sites.len();
            self.simple(
                from.clone(),
                to,
                Kind::Trigger,
                Conf::OverApprox,
                "lifecycle",
            );
            if action && self.sited.sites.len() > before {
                // A user starts this run from one of the page's actions
                // (`RunObject`): the reverse walk's page-action entry.
                self.sited.action_run_sites.push(before as u32);
                if !names.is_empty() {
                    let mut names = names;
                    names.sort();
                    names.dedup();
                    self.sited.action_run_actions.push((before as u32, names));
                }
            }
        }
        for table in tables {
            let mut targets: Vec<(RoutineNodeId, u8)> = self
                .triggers_with_extensions(&table)
                .into_iter()
                .map(|r| (r.id.clone(), 0))
                .collect();
            let event_guard = if temporary_source.as_ref() == Some(&table) {
                crate::program::table_event_guard::EntryExits::TEMPORARY_REC.bits()
            } else {
                0
            };
            targets.extend(
                self.routines_of
                    .get(&table)
                    .into_iter()
                    .flatten()
                    .filter(|r| r.publisher_kind == Some(PublisherKind::Platform))
                    .map(|r| (r.id.clone(), event_guard)),
            );
            // What the page lets a user do to these records through it
            // (`page_ui`): a blocked operation's triggers and table events
            // get no site, a candidate's get a site marked so.
            let blocks = if page_source.as_ref() == Some(&table) {
                self.page_ui.get(oid).copied().unwrap_or_default()
            } else {
                crate::program::page_ui::PageUiBlocks::default()
            };
            for (t, guard) in targets {
                let op =
                    trigger_op(t.name_lc.as_str()).map_or(0, crate::program::page_ui::op::of_index);
                if blocks.certain & op != 0 {
                    continue;
                }
                let before = self.sited.sites.len();
                self.site_guard = guard;
                self.simple(
                    from.clone(),
                    Key::Routine(t),
                    Kind::Trigger,
                    Conf::OverApprox,
                    "lifecycle",
                );
                self.site_guard = 0;
                if blocks.candidate & op != 0 && self.sited.sites.len() > before {
                    self.sited.ui_candidate_sites.push(before as u32);
                }
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
            // The test pages it (or, for a global, its object) declares; the
            // other runnable variables run at their sites (`receiver_run`).
            let mut runs: Vec<RunTarget> = Vec::new();
            if let Some(f) = inp.facts.routines.get(&r.id) {
                runs.extend(f.test_pages.iter().cloned());
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
                runs.extend(of.test_pages.iter().cloned());
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
                ObligationId::CallSite { caller, .. }
                | ObligationId::Background { caller, .. }
                | ObligationId::TableEvent { caller, .. } => caller,
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
        let nodes = &self.sited.nodes;
        let hubs: BTreeSet<Hub> = self
            .edges
            .keys()
            .filter_map(|(_, to, _)| match &nodes[*to as usize] {
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
            .filter(|((_, _, k), e)| *k == Kind::Event && e.via() == "eventFlow")
            .filter_map(|((from, _, _), _)| match &nodes[*from as usize] {
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
                Hub::Record(op) | Hub::RecordEvents(op) => {
                    // A RunTrigger-false operation reaches no trigger (c02).
                    let triggers_run = !matches!(hub, Hub::RecordEvents(_));
                    let targets: Vec<RoutineNodeId> = g
                        .routines
                        .iter()
                        .filter(|r| {
                            matches!(
                                r.id.object.kind,
                                ObjectKind::Table | ObjectKind::TableExtension
                            ) && ((triggers_run
                                && r.is_trigger
                                && is_record_trigger_of(r.id.name_lc.as_str(), op))
                                || (r.publisher_kind == Some(PublisherKind::Platform)
                                    && is_record_event_of(r.id.name_lc.as_str(), op)))
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
        // A routine with an incoming edge from a non-hub node: a site that is
        // shown, not only a hub's "anything of this kind may".
        let mut incoming_shown: HashSet<&RoutineNodeId> = HashSet::new();
        let nodes = &self.sited.nodes;
        for ((from, to, kind), e) in &self.edges {
            if let Key::Routine(t) = &nodes[*to as usize] {
                incoming_any.insert(t);
                if *kind == Kind::Event {
                    incoming_event.insert(t);
                }
                if !matches!(nodes[*from as usize], Key::Hub(_)) {
                    incoming_shown.insert(t);
                }
            }
            if let Key::Routine(f) = &nodes[*from as usize]
                && e.via() == "eventFlow"
            {
                outgoing_event.insert(f);
            }
        }
        let mut out: HashMap<RoutineNodeId, Vec<&'static str>> = HashMap::new();
        for r in self.inp.graph.routines.iter() {
            let mut flags: Vec<&'static str> = Vec::new();
            // A synthetic platform TABLE publisher (rule (a) until F1, spec
            // 2026-11-06-db-event-subscribers): each record operation on its
            // table now raises it through a `tableEvent` edge, so it is flagged
            // only when no shown site raises it; a hub edge (a record op on a
            // table not known) still does not clear the flag. A page event
            // fires exactly when its page runs (lifecycle), never flagged.
            match r.publisher_kind {
                Some(PublisherKind::Platform)
                    if matches!(
                        r.id.object.kind,
                        ObjectKind::Table | ObjectKind::TableExtension
                    ) =>
                {
                    if !incoming_shown.contains(&r.id) {
                        flags.push("unraisedPublisher");
                    }
                }
                Some(PublisherKind::Platform) | None => {}
                Some(_) => {
                    if outgoing_event.contains(&r.id) && !incoming_any.contains(&r.id) {
                        flags.push("unraisedPublisher");
                    }
                }
            }
            // An `[EventSubscriber]` the engine cannot read may be for any
            // event, so even a node with bound events has hidden callers.
            if !r.event_subscribers.is_empty()
                && (!incoming_event.contains(&r.id)
                    || r.event_subscribers.iter().any(|s| s.is_unreadable()))
            {
                flags.push("unboundSubscriber");
            }
            if !flags.is_empty() {
                out.insert(r.id.clone(), flags);
            }
        }
        out
    }
}

/// The sited graph a build stopped by its cancellation did not finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Every site of the may-execute graph (see [`super::sited`]), and the
/// unpruned graph's `hiddenCallers`. `cancel` is polled every
/// [`CANCEL_EVERY`] sites; a `true` stops the build.
fn build<'a>(
    inp: &'a Inputs<'a>,
    cancel: &'a dyn Fn() -> bool,
) -> Result<(Builder<'a>, HashMap<RoutineNodeId, Vec<&'static str>>), Cancelled> {
    let mut b = Builder::with_cancel(inp, cancel);
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
    if b.cancelled || cancel() {
        return Err(Cancelled);
    }
    Ok((b, hidden))
}

/// The program's sited graph (see [`super::sited`]), or [`Cancelled`] when
/// `cancel` said stop during the build. `friended` holds the apps whose
/// manifest names an `internalsVisibleTo` friend, loaded or not (their
/// `internal` routines are callable from outside the program).
pub fn sited_graph(
    inp: &Inputs<'_>,
    friended: &HashSet<AppRef>,
    cancel: &dyn Fn() -> bool,
) -> Result<SitedGraph, Cancelled> {
    let (b, hidden) = build(inp, cancel)?;
    let hidden: HashMap<u32, Vec<&'static str>> = hidden
        .into_iter()
        .filter_map(|(r, f)| b.sited.ix(&Key::Routine(r)).map(|i| (i, f)))
        .collect();
    Ok(b.sited.finish(inp, hidden, friended))
}

/// Build the export's nodes and edges: the sited graph, collapsed
/// ([`super::sited::collapse`]), pruned.
#[must_use]
pub fn project(inp: &Inputs<'_>) -> Projection {
    project_keeping(inp, false)
}

/// [`project`]; with `keep_local_sources`, every routine of an app loaded
/// from local source (a source-folder dependency) survives pruning too
/// (agent-tasks design §4.8: a consumer joins changed production files on
/// `(appId, sourcePath)`, so no routine of such an app may be pruned).
#[must_use]
pub fn project_keeping(inp: &Inputs<'_>, keep_local_sources: bool) -> Projection {
    let (b, hidden) = build(inp, &|| false).expect("an uncancellable build finishes");
    let keys = &b.sited.nodes;
    let collapsed = collapse(&b.sited.sites);

    // ── Pruning (spec §4.6) ────────────────────────────────────────────────
    let mut succ: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut pred: HashMap<u32, Vec<u32>> = HashMap::new();
    for (from, to, _) in collapsed.keys() {
        succ.entry(*from).or_default().push(*to);
        pred.entry(*to).or_default().push(*from);
    }
    let workspace: Vec<u32> = inp
        .graph
        .routines
        .iter()
        .filter(|r| r.id.object.app == inp.edges.primary_app_ref)
        .filter_map(|r| b.sited.ix(&Key::Routine(r.id.clone())))
        .collect();
    let local_sources: Vec<u32> = if keep_local_sources {
        inp.graph
            .routines
            .iter()
            .filter(|r| {
                r.id.object.app != inp.edges.primary_app_ref
                    && matches!(
                        r.tier,
                        crate::snapshot::TrustTier::LocalSourceVerified
                            | crate::snapshot::TrustTier::LocalSourceApproximate
                    )
            })
            .filter_map(|r| b.sited.ix(&Key::Routine(r.id.clone())))
            .collect()
    } else {
        Vec::new()
    };
    let any = b.sited.ix(&Key::Hub(Hub::Any));
    let any_used = any.is_some_and(|a| pred.contains_key(&a));
    let walk = |start: &[u32], next: &HashMap<u32, Vec<u32>>| -> HashSet<u32> {
        let mut seen: HashSet<u32> = start.iter().copied().collect();
        let mut queue: VecDeque<u32> = start.iter().copied().collect();
        while let Some(k) = queue.pop_front() {
            for n in next.get(&k).into_iter().flatten() {
                if seen.insert(*n) {
                    queue.push_back(*n);
                }
            }
        }
        seen
    };
    let forward = walk(&workspace, &succ);
    let mut backward = walk(&workspace, &pred);
    if let Some(a) = any
        && any_used
        && !backward.is_empty()
    {
        // The universal hub is a caller of every node, so of every node here.
        backward.extend(walk(&[a], &pred));
    }
    let universal_reached = any.is_some_and(|a| forward.contains(&a));
    let keep: HashSet<u32> = backward
        .iter()
        .filter(|k| {
            universal_reached
                || forward.contains(*k)
                || matches!(&keys[**k as usize], Key::Routine(r) if hidden.contains_key(r))
        })
        .copied()
        .chain(workspace.iter().copied())
        .chain(local_sources.iter().copied())
        .collect();

    // ── Rows ───────────────────────────────────────────────────────────────
    let ids: HashMap<u32, String> = keep
        .iter()
        .map(|k| (*k, super::key_id(&keys[*k as usize], inp.graph)))
        .collect();
    let mut nodes: Vec<Node> = keep
        .iter()
        .map(|k| super::render_node(&keys[*k as usize], &ids[k], inp, &hidden))
        .collect();
    nodes.sort_by(|a, c| a.id.cmp(&c.id));
    let mut edges: Vec<Edge> = Vec::new();
    let mut universal_callers = 0usize;
    for ((from, to, kind), e) in &collapsed {
        let (Some(f), Some(t)) = (ids.get(from), ids.get(to)) else {
            continue;
        };
        if Some(*to) == any {
            universal_callers += 1;
        }
        edges.push(Edge {
            caller: f.clone(),
            callee: t.clone(),
            kind: kind.as_str(),
            confidence: e.conf.as_str(),
            via: e.via(),
            line: e.line,
            conditions: condition_names(e.conditions).collect(),
        });
    }
    edges.sort_by(|a, c| {
        (a.caller.as_str(), a.callee.as_str(), a.kind).cmp(&(
            c.caller.as_str(),
            c.callee.as_str(),
            c.kind,
        ))
    });
    let hubs = keep
        .iter()
        .filter(|k| matches!(keys[**k as usize], Key::Hub(_)))
        .count();
    Projection {
        nodes,
        edges,
        universal_callers,
        hubs,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    //! The projection contract over hand-built graphs: every precondition is
    //! stated here, none is produced by the resolver.

    use super::*;
    use crate::program::graph::ObjectIndex;
    use crate::program::impact_graph::{ObjectFacts, RoutineFacts};
    use crate::program::node::{AppRegistry, ObjKey};
    use crate::program::node_extract::{AbiParams, PageControlKind, PageControlNode};
    use crate::program::resolve::edge::{
        CanonicalSpan, Edge as EngineEdge, Evidence, OpenWorldReason, Route, SetCompleteness,
        SiteId, SourcePos, UnknownReason, Witness,
    };
    use crate::program::resolve::event::ParsedSubscriberArgs;
    use crate::program::resolve::full::{ClassifiedEdge, SiteFacts};
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::AppId;

    /// A hand-built program (also the reverse walker's test world).
    #[derive(Default)]
    pub(crate) struct World {
        pub(crate) apps: AppRegistry,
        pub(crate) deps: Vec<(AppRef, AppRef)>,
        pub(crate) objects: Vec<ObjectNode>,
        pub(crate) routines: Vec<RoutineNode>,
        pub(crate) edges: Vec<ClassifiedEdge>,
        /// The second edge set (`EdgeView::dependency_body_edges`): `project`
        /// reads it after `edges`.
        pub(crate) dependency_body_edges: Vec<ClassifiedEdge>,
        pub(crate) site_facts: HashMap<ObligationId, SiteFacts>,
        pub(crate) facts: SourceFacts,
        pub(crate) missing: Vec<ObligationId>,
    }

    /// The sited graph of `w` with `primary` as the workspace app.
    pub(crate) fn sited_of(w: &World, primary: AppRef, friended: &[AppRef]) -> SitedGraph {
        let friended: HashSet<AppRef> = friended.iter().copied().collect();
        w.with_inputs(primary, |inp| {
            sited_graph(inp, &friended, &|| false).expect("not cancelled")
        })
    }

    pub(crate) fn unknown_route() -> Route {
        unknown()
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
        pub(crate) fn app(&mut self, name: &str) -> AppRef {
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
                dataset: Default::default(),
                protected_vars: Vec::new(),
                parse_incomplete: false,
                closed_enum: false,
                internal: false,
                enum_impls: Vec::new(),
                lookup_pages: Vec::new(),
                obsolete_gone: false,
                page_ui: Default::default(),
            });
            oid
        }

        pub(crate) fn obj(
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

        pub(crate) fn object_mut(&mut self, oid: &ObjectNodeId) -> &mut ObjectNode {
            self.objects
                .iter_mut()
                .find(|o| o.id == *oid)
                .expect("object")
        }

        pub(crate) fn rtn_with(
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
                publisher_isolated: false,
                abi_routine_kind: None,
                abi_event_kind: None,
                param_sig_key: Default::default(),
                return_type: None,
                return_type_id: None,
                abi_overload_collapsed: false,
                source_overload_aliased: false,
                preproc_context: Box::default(),
                source_unit: None,
                abi_params: AbiParams::Missing,
                abi_method_id: None,
            };
            f(&mut r);
            let id = r.id.clone();
            self.routines.push(r);
            id
        }

        pub(crate) fn rtn(&mut self, o: &ObjectNodeId, name: &str) -> RoutineNodeId {
            self.rtn_with(o, name, |_| {})
        }

        pub(crate) fn trigger(&mut self, o: &ObjectNodeId, name: &str) -> RoutineNodeId {
            self.rtn_with(o, name, |r| r.is_trigger = true)
        }

        pub(crate) fn edge(
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
                site: None,
            });
            ob
        }

        /// A static background start's own edge (F3), under its `Background`
        /// obligation, as `full::resolve_file_obligations` emits it.
        pub(crate) fn background(&mut self, from: &RoutineNodeId, routes: Vec<Route>, line: u32) {
            let ob = self.edge(
                from,
                EdgeKind::BackgroundStart,
                DispatchShape::Multicast,
                routes,
                line,
            );
            let ObligationId::CallSite {
                caller,
                span,
                callee_fp,
            } = ob
            else {
                unreachable!("`edge` makes a call-site obligation");
            };
            self.edges.last_mut().expect("just pushed").obligation_id = ObligationId::Background {
                caller,
                span,
                callee_fp,
            };
        }

        pub(crate) fn call(&mut self, from: &RoutineNodeId, to: &RoutineNodeId, line: u32) {
            self.edge(
                from,
                EdgeKind::Call,
                DispatchShape::Exact,
                vec![src(to)],
                line,
            );
        }

        pub(crate) fn event(&mut self, publisher: &RoutineNodeId, subscribers: &[&RoutineNodeId]) {
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
                site: None,
            });
        }

        fn project(&self, primary: AppRef) -> Out {
            self.with_inputs(primary, |inp| Out(project(inp)))
        }

        /// The projection's `Inputs` over this world, handed to `f`.
        pub(crate) fn with_inputs<R>(
            &self,
            primary: AppRef,
            f: impl FnOnce(&Inputs<'_>) -> R,
        ) -> R {
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
            f(&Inputs {
                graph: &graph,
                index: &index,
                surface: &surface,
                edges: EdgeView::for_test(
                    primary,
                    &self.edges,
                    &self.dependency_body_edges,
                    &self.site_facts,
                ),
                facts: &self.facts,
                missing_obligations: &self.missing,
                recovered_files: &recovered,
            })
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

    /// F3: a static background start is a `Run` of the started codeunit's
    /// object (`via: backgroundStart`), its site a `Background` boundary; the
    /// failure codeunit's route is `OverApprox` with `onTaskFailure`.
    ///
    /// DISCRIMINATION (2026-11-05): mapping the `backgroundStart` via to
    /// `Boundary::Run` in `add` fails the boundary assertion; mapping a
    /// `BackgroundStart` route like a `Call` in `engine_route` fails the
    /// first edge assertion (a `Direct` edge to the trigger).
    #[test]
    fn background_starts_run_the_codeunit_across_a_background_boundary() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let bg = w.obj(a, ObjectKind::Codeunit, 2, "Bg", TrustTier::Workspace);
        let fail = w.obj(a, ObjectKind::Codeunit, 3, "Fail", TrustTier::Workspace);
        let caller = w.rtn(&cu, "Caller");
        let bg_run = w.trigger(&bg, "OnRun");
        let fail_run = w.trigger(&fail, "OnRun");
        let mut on_failure = src(&fail_run);
        on_failure.conditions = vec![Condition::OnTaskFailure];
        w.background(&caller, vec![src(&bg_run), on_failure], 7);
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.Caller", "Bg"),
            ("Run", "Resolved", "backgroundStart")
        );
        assert_eq!(o.edges("Cu.Caller", "Bg")[0].line, Some(8), "1-based");
        assert_eq!(
            o.edge("Cu.Caller", "Fail"),
            ("Run", "OverApprox", "backgroundStart")
        );
        assert_eq!(
            o.edges("Cu.Caller", "Fail")[0].conditions,
            vec!["onTaskFailure"]
        );
        assert!(o.edges("Cu.Caller", "Bg.OnRun").is_empty());
        let g = sited_of(&w, a, &[]);
        let from = g.routine_node(&caller).expect("caller");
        let starts: Vec<_> = g.sites().iter().filter(|s| s.from == from).collect();
        assert_eq!(starts.len(), 2, "{starts:?}");
        assert!(
            starts
                .iter()
                .all(|s| s.boundary == Boundary::Background && s.via() == "backgroundStart"),
            "{starts:?}"
        );
        assert!(starts.iter().all(|s| g.is_background_target(s.to)));
        assert!(!g.is_background_target(from));
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
                test_pages: vec![RunTarget::Object(page.clone())],
                ..Default::default()
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

    /// Rule 2 for a `Page`/`Report`/`XmlPort`/`Query` variable is a SITE, not a
    /// declaration (round-1 c12): `Document Errors Mgt.` declares a global
    /// `Page "Sales Invoice"` and only calls `RunBackgroundCheck()` on it, so
    /// neither that routine nor its empty `OnRun` runs the page. A dispatching
    /// builtin that runs a report, called on a receiver the resolver typed as
    /// that report, does run it; a builtin that runs nothing (`SetTableView`)
    /// on the same receiver does not; a receiver the caller's closure does not
    /// resolve gives no precise edge. Every site is stated by hand.
    ///
    /// DISCRIMINATION (2026-11-06): `receiver_run` returning `None` fails
    /// "Print runs Rep"; dropping its `dispatching_builtin` filter (any
    /// builtin route counts) fails "SetTableView runs nothing". Restoring the
    /// old declaration rule (an object's globals run from every routine) is
    /// no longer expressible: the facts no longer carry a `Page` global.
    #[test]
    fn a_runnable_variable_runs_only_where_a_site_runs_it() {
        use crate::program::resolve::edge::BuiltinId;
        use crate::program::resolve::full::{ReceiverFact, ReceiverFactType};
        let mut w = World::default();
        let a = w.app("A");
        let errs = w.obj(a, ObjectKind::Codeunit, 1, "Errs", TrustTier::Workspace);
        let card = w.obj(a, ObjectKind::Page, 2, "Card", TrustTier::Workspace);
        let rep = w.obj(a, ObjectKind::Report, 3, "Rep", TrustTier::Workspace);
        let sink = w.rtn(&errs, "Sink");
        let card_open = w.trigger(&card, "OnOpenPage");
        let rep_pre = w.trigger(&rep, "OnPreReport");
        w.call(&card_open, &sink, 1);
        w.call(&rep_pre, &sink, 2);
        let bg = w.rtn(&card, "RunBackgroundCheck");
        w.call(&bg, &sink, 3);
        w.trigger(&errs, "OnRun");
        let check = w.rtn(&errs, "Check");
        w.call(&check, &bg, 4);
        let builtin = |id: &str| Route {
            target: RouteTarget::Builtin(BuiltinId(id.into())),
            evidence: Evidence::Catalog,
            conditions: vec![],
            witness: Witness::None,
            receiver_tier: None,
        };
        let typed = |kind: ObjectKind, name: &str| SiteFacts {
            receiver: Some(ReceiverFact {
                type_text: Some(format!("{kind:?} {name}")),
                ty: ReceiverFactType::Object {
                    kind,
                    name: name.into(),
                },
            }),
            ..Default::default()
        };
        for (name, bid, recv, line) in [
            ("Print", "ReportInstance::saveaspdf", "Rep", 5),
            ("Filter", "ReportInstance::settableview", "Rep", 6),
            ("Lost", "ReportInstance::saveaspdf", "Nope", 7),
        ] {
            let r = w.rtn(&errs, name);
            let s = w.edge(
                &r,
                EdgeKind::Call,
                DispatchShape::Exact,
                vec![builtin(bid)],
                line,
            );
            w.site_facts.insert(s, typed(ObjectKind::Report, recv));
        }
        let o = w.project(a);
        assert_eq!(
            o.edge("Errs.Print", "Rep"),
            ("Run", "OverApprox", "declaredRunnable"),
            "Print runs Rep"
        );
        assert_eq!(
            o.edge("Errs.Check", "Card.RunBackgroundCheck"),
            ("Direct", "Resolved", "call")
        );
        let runs_from = |from: &str| -> Vec<&Edge> {
            let f = o.node(from).id.clone();
            o.0.edges
                .iter()
                .filter(|e| e.caller == f && e.kind == "Run")
                .collect()
        };
        assert!(
            runs_from("Errs.Filter").is_empty(),
            "SetTableView runs nothing"
        );
        for r in ["Errs.OnRun", "Errs.Check", "Errs.Lost"] {
            assert!(runs_from(r).is_empty(), "{r} runs no object");
        }
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

    /// Round-3 c02: a record operation on a table not resolved whose
    /// RunTrigger is left out or `false` (`RecordRef.Modify();`) runs no
    /// trigger of any table, only the platform table events. Its hub,
    /// `h:record-events:modify`, expands to `Tab.OnAfterModifyEvent` and
    /// never to `Tab.OnModify`; a RunTrigger-true site keeps the trigger hub.
    /// The precondition is stated by hand: the site facts carry the bound.
    /// Discrimination (2026-11-07): expanding `RecordEvents` to the triggers
    /// too fails "a RunTrigger-false operation reaches no trigger"; the
    /// resolver side is `full::tests::open_call_sites_record_their_bounds`.
    #[test]
    fn a_run_trigger_false_hub_reaches_table_events_only() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let table = w.obj(a, ObjectKind::Table, 3, "Tab", TrustTier::Workspace);
        let no_trigger = w.rtn(&cu, "NoTrigger");
        let with_trigger = w.rtn(&cu, "WithTrigger");
        let on_modify = w.trigger(&table, "OnModify");
        let event = w.rtn_with(&table, "OnAfterModifyEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform)
        });
        let sink = w.rtn(&cu, "Sink");
        w.call(&on_modify, &sink, 1);
        let sub = w.rtn(&cu, "Sub");
        w.event(&event, &[&sub]);
        for (caller, bound) in [
            (&no_trigger, OpenBound::RecordEvents { op: "modify" }),
            (&with_trigger, OpenBound::Record { op: "modify" }),
        ] {
            let s = w.edge(
                caller,
                EdgeKind::Call,
                DispatchShape::Exact,
                vec![unknown()],
                1,
            );
            w.site_facts.insert(
                s,
                SiteFacts {
                    open: vec![bound],
                    ..Default::default()
                },
            );
        }
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.NoTrigger", "h:record-events:modify").0,
            "Trigger"
        );
        assert_eq!(
            o.edge("h:record-events:modify", "Tab.OnAfterModifyEvent"),
            ("Trigger", "Open", "hubExpansion")
        );
        assert!(
            o.edges("h:record-events:modify", "Tab.OnModify").is_empty(),
            "a RunTrigger-false operation reaches no trigger"
        );
        assert_eq!(
            o.edge("h:record:modify", "Tab.OnModify"),
            ("Trigger", "Open", "hubExpansion"),
            "a RunTrigger-true one still does"
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
    /// table-extension `OnAfterInsert` carries no flag (ENG-15 retired
    /// `unmodeledRecordTrigger`).
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
        // The caller may run the page (a `TestPage` variable).
        w.facts.routines.insert(
            caller.clone(),
            RoutineFacts {
                test_pages: vec![RunTarget::Object(page.clone())],
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
        // F1: a record op that raises the table's modify event (a `TableEvent`
        // edge, as the resolver emits it) clears that publisher's flag.
        // DISCRIMINATION (2026-11-06): flagging every table publisher (the old
        // rule (a)) fails "a record op raising it clears the flag"; letting any
        // incoming edge clear it (`incoming_any`) fails "a hub edge into it must
        // not clear the flag".
        let tmodify = w.rtn_with(&table, "OnAfterModifyEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform)
        });
        let s3 = w.rtn(&cu, "S3");
        w.event(&tmodify, &[&s3]);
        w.edge(
            &caller,
            EdgeKind::TableEvent,
            DispatchShape::Multicast,
            vec![src(&tmodify)],
            2,
        );
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.Caller", "Tab.OnAfterModifyEvent"),
            ("Event", "Resolved", "tableEvent")
        );
        assert!(
            o.node("Tab.OnAfterModifyEvent").hidden_callers.is_empty(),
            "a record op raising it clears the flag"
        );
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
        // ENG-15 (F5 closed): record operations reach a table extension's
        // `OnAfterInsert`, so it carries no flag; the insert hub reaches it by
        // the shared stem rule.
        assert!(
            o.node("TabExt.OnAfterInsert").hidden_callers.is_empty(),
            "{:?}",
            o.node("TabExt.OnAfterInsert").hidden_callers
        );
        assert_eq!(
            o.edge("h:record:insert", "TabExt.OnAfterInsert"),
            ("Trigger", "Open", "hubExpansion")
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

    /// #160 spec §8.3: the merge on `(from, to, kind)` keeps the FIRST edge's
    /// `via` when the two have the same `conf`. Two hand-built edges, added in
    /// order.
    #[test]
    fn add_keeps_first_via_on_conf_tie() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let from = Key::Routine(w.rtn(&cu, "From"));
        let to = Key::Hub(Hub::Any);
        let via = w.with_inputs(a, |inp| {
            let mut b = Builder::new(inp);
            for via in ["unresolved", "dynamic"] {
                b.add(
                    from.clone(),
                    to.clone(),
                    Kind::Direct,
                    Conf::Open,
                    via,
                    None,
                    BTreeSet::new(),
                );
            }
            b.edge(&from, &to, Kind::Direct).expect("the edge").via()
        });
        assert_eq!(via, "unresolved");
    }

    /// M0-T2 and M0-T1's merge rule (agent-tasks design §1.3): two calls from
    /// X to Y keep two sites with their own ids and anchors in the sited
    /// graph, and the export collapses them to one edge whose line is the
    /// MINIMUM. The call at line 9 is added first, so a collapse keeping the
    /// first site's line fails here.
    #[test]
    fn sited_graph_keeps_each_site_and_the_export_collapses_them() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let x = w.rtn(&cu, "X");
        let y = w.rtn(&cu, "Y");
        w.call(&x, &y, 9);
        w.call(&x, &y, 3);
        let o = w.project(a);
        let es = o.edges("Cu.X", "Cu.Y");
        assert_eq!(es.len(), 1);
        assert_eq!(es[0].line, Some(4), "the minimum line over both sites");
        let g = w.with_inputs(a, |inp| {
            sited_graph(inp, &HashSet::new(), &|| false).expect("not cancelled")
        });
        let (fx, fy) = (
            g.routine_node(&x).expect("X"),
            g.routine_node(&y).expect("Y"),
        );
        let ids = g.sites_between(fx, fy);
        assert_eq!(ids.len(), 2, "one site per call");
        let (s0, s1) = (&g.sites()[ids[0] as usize], &g.sites()[ids[1] as usize]);
        assert_ne!(s0.anchor, s1.anchor);
        assert_eq!((s0.line(), s1.line()), (Some(10), Some(4)));
        assert_eq!((s0.boundary, s0.via()), (Boundary::Sync, "call"));
        // The collapse of the sited graph is the export's edge.
        let c = collapse(g.sites());
        assert_eq!(c[&(fx, fy, Kind::Direct)].line, Some(4));
        assert_eq!(g.node_id(fx), o.node("Cu.X").id);
    }

    /// M0-T8's builder half: the build polls its cancellation every
    /// [`CANCEL_EVERY`] sites, not only at the end. The flag says stop from
    /// its second poll: polled in the loop, the build stops at site 8192 and
    /// yields no graph (two polls); polled only at the end, its one poll says
    /// go and the graph is returned.
    #[test]
    fn a_cancelled_sited_build_yields_no_graph() {
        let mut w = World::default();
        let a = w.app("A");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let x = w.rtn(&cu, "X");
        let ys: Vec<RoutineNodeId> = (0..2 * CANCEL_EVERY as usize + 10)
            .map(|i| w.rtn(&cu, &format!("Y{i}")))
            .collect();
        for (i, y) in ys.iter().enumerate() {
            w.call(&x, y, i as u32);
        }
        let polls = std::cell::Cell::new(0u32);
        let out = w.with_inputs(a, |inp| {
            sited_graph(inp, &HashSet::new(), &|| {
                polls.set(polls.get() + 1);
                polls.get() >= 2
            })
            .map(|g| g.site_count())
        });
        assert_eq!(out, Err(Cancelled));
        assert_eq!(polls.get(), 2, "stopped at the second check, in the loop");
        let full = w.with_inputs(a, |inp| {
            sited_graph(inp, &HashSet::new(), &|| false).map(|g| g.site_count())
        });
        assert!(full.is_ok_and(|n| n > CANCEL_EVERY as usize));
    }

    /// #160 spec §4.3.2 / §8.3: a tie ACROSS the two edge sets keeps the via of
    /// the edge in `edges`, because `project` reads `edges` first. One
    /// dependency routine R: in `edges`, its event-flow edge with an unresolved
    /// route (a publisher has no open bounds, so `(R, h:any, Direct, Open)` via
    /// `unresolved`); in `dependency_body_edges`, its `DynamicOpen` call site
    /// with an `OpenBound::Any` site fact (the same key, via `dynamic`).
    #[test]
    fn cross_set_tie_keeps_the_first_sets_via() {
        let mut w = World::default();
        let a = w.app("A");
        let d = w.app("D");
        w.deps.push((a, d));
        let ws = w.obj(a, ObjectKind::Codeunit, 1, "Ws", TrustTier::Workspace);
        let dep = w.obj(d, ObjectKind::Codeunit, 2, "Dep", TrustTier::Workspace);
        let caller = w.rtn(&ws, "Caller");
        let r = w.rtn(&dep, "R");
        // The workspace reaches R, so pruning keeps it.
        w.call(&caller, &r, 1);
        w.edges.push(ClassifiedEdge {
            obligation_id: ObligationId::Publisher(r.clone()),
            edge: EngineEdge {
                from: r.clone(),
                site: SiteId {
                    caller: r.clone(),
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
                routes: vec![unknown()],
            },
            site: None,
        });
        let site = w.edge(
            &r,
            EdgeKind::Call,
            DispatchShape::DynamicOpen,
            vec![unknown()],
            2,
        );
        // Moved into the second set by hand.
        let moved = w.edges.pop().expect("the site just added");
        w.dependency_body_edges.push(moved);
        w.site_facts.insert(
            site,
            SiteFacts {
                open: vec![OpenBound::Any],
                ..Default::default()
            },
        );
        let o = w.project(a);
        assert_eq!(o.edge("Dep.R", "h:any").2, "unresolved");
    }

    /// ENG-12: a record operation on a call result (`GetRef().Modify(true)`)
    /// carries its trigger and table-event routes in the site facts only (the
    /// edge routes nowhere); the export runs the trigger and raises the event,
    /// so a test reaching it is selected for a change to either. Hand-stated:
    /// the site's edge has no route, its facts hold a `CallResultRecord`.
    ///
    /// DISCRIMINATION (2026-11-13): skipping the `call_result_record` push in
    /// `Projector`'s fact-op loop fails "the call result runs the trigger";
    /// dropping the route's conditions there fails "a conditioned trigger
    /// route is OverApprox".
    #[test]
    fn a_call_result_record_op_runs_its_trigger_and_raises_its_table_event() {
        use crate::program::resolve::full::CallResultRecord;
        let mut w = World::default();
        let a = w.app("W");
        let cu = w.obj(a, ObjectKind::Codeunit, 1, "Cu", TrustTier::Workspace);
        let table = w.obj(a, ObjectKind::Table, 50, "T", TrustTier::Workspace);
        let on_modify = w.trigger(&table, "OnModify");
        let on_insert = w.trigger(&table, "OnInsert");
        let event = w.rtn_with(&table, "OnAfterModifyEvent", |r| {
            r.publisher_kind = Some(PublisherKind::Platform)
        });
        let p = w.rtn(&cu, "P");
        let site = w.edge(&p, EdgeKind::Call, DispatchShape::Multicast, vec![], 1);
        let mut guarded = src(&on_insert);
        guarded.conditions = vec![Condition::RunTriggerGuarded];
        w.site_facts.insert(
            site,
            SiteFacts {
                call_result_record: Some(CallResultRecord {
                    op: "Modify",
                    table: table.clone(),
                    temporary: None,
                    triggers: vec![src(&on_modify), guarded],
                    table_events: vec![src(&event)],
                }),
                ..Default::default()
            },
        );
        let o = w.project(a);
        assert_eq!(
            o.edge("Cu.P", "T.OnModify"),
            ("Trigger", "Resolved", "implicitTrigger"),
            "the call result runs the trigger"
        );
        assert_eq!(
            o.edge("Cu.P", "T.OnAfterModifyEvent"),
            ("Event", "Resolved", "tableEvent"),
            "the call result raises the table event"
        );
        // A conditioned route is no unconditional edge (astra round 4).
        assert_eq!(
            o.edge("Cu.P", "T.OnInsert"),
            ("Trigger", "OverApprox", "implicitTrigger"),
            "a conditioned trigger route is OverApprox"
        );
    }
}
