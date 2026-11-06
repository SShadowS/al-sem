//! The **sited graph** (agent-tasks design §1.2 M0-1, §3.1): the may-execute
//! graph with one edge per site, before the export's collapse.
//!
//! The projection's builder ([`super::project`]) records every edge it adds
//! here, in the order it adds them. The exported `alsem graph` edges are the
//! COLLAPSE of these sites on `(from, to, kind)` ([`collapse`]): the strongest
//! confidence wins (its `via` and conditions with it), equal confidences keep
//! the first site's `via` and union their conditions, and the line is the
//! minimum over every site. The fold runs in site order, so the export is the
//! same bytes it was before the sited graph existed.
//!
//! Nodes, sites and source units are interned to `u32`; a node's strings live
//! once, in its [`Key`]. The graph holds no reference into the program, so a
//! resident revision keeps it after the program is dropped
//! (`ArtifactKey::Sited`).
//!
//! **Execution boundary.** Each site carries the boundary the graph alone can
//! state ([`Boundary`]); a static background start is its own,
//! [`Boundary::Background`] (F3). The finer boundaries the design names (a
//! checked against an unchecked `Codeunit.Run`, a trigger's temporary-record
//! state) come from body facts the program edges do not carry; their
//! consumers (R0, A) refine a [`Boundary::Run`] or [`Boundary::TriggerOf`]
//! site by its [`Anchor`].

use std::collections::HashMap;

use super::project::{Conf, Hub, Key, Kind};
use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, RoutineNodeId};

/// Every `via` the builder writes, by index ([`Site::via`]).
pub(crate) const VIAS: &[&str] = &[
    "call",
    "interface",
    "run",
    "implicitTrigger",
    "eventFlow",
    "lifecycle",
    "testRunner",
    "testHandler",
    "declaredRunnable",
    "parseRecovered",
    "coverageMiss",
    "hubExpansion",
    "opaqueBody",
    "dynamic",
    "unresolved",
    "dispatchingBuiltin",
    "backgroundStart",
];

/// Every route condition, in name order: a condition mask's bits, read low
/// to high, are the export's sorted `conditions`. `configuredDispatch` is the
/// projection's own (a catalogued configured-dispatch arm,
/// [`super::configured_dispatch`]); the others come from the resolver's
/// routes.
pub(crate) const CONDITIONS: &[&str] = &[
    "ambiguousDispatch",
    "configuredDispatch",
    "manualBinding",
    "onTaskFailure",
    "runTriggerGuarded",
    "skipOnMissingLicense",
    "skipOnMissingPermission",
];

/// The record operations a trigger boundary names.
pub(crate) const RECORD_OPS: &[&str] = &["insert", "modify", "delete", "rename", "validate"];

/// `via`'s index in [`VIAS`]. Panics on a `via` the table does not list: a
/// new rule adds its `via` there.
pub(crate) fn via_ix(via: &str) -> u8 {
    VIAS.iter()
        .position(|v| *v == via)
        .unwrap_or_else(|| panic!("via {via:?} is not in sited::VIAS")) as u8
}

/// The mask of `conditions` (names from [`CONDITIONS`]).
pub(crate) fn condition_mask<'a>(conditions: impl IntoIterator<Item = &'a str>) -> u8 {
    conditions.into_iter().fold(0u8, |m, c| {
        let i = CONDITIONS
            .iter()
            .position(|n| *n == c)
            .unwrap_or_else(|| panic!("condition {c:?} is not in sited::CONDITIONS"));
        m | (1 << i)
    })
}

/// The condition names of `mask`, sorted.
pub(crate) fn condition_names(mask: u8) -> impl Iterator<Item = &'static str> {
    CONDITIONS
        .iter()
        .enumerate()
        .filter(move |(i, _)| mask & (1 << i) != 0)
        .map(|(_, n)| *n)
}

/// What kind of execution a site enters (see the module doc for what is left
/// to the consumers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Boundary {
    /// A call in the caller's own execution: a direct or interface call.
    Sync,
    /// A run of an object (`Codeunit.Run`, `Page.RunModal`, a typed-variable
    /// run, a declared runnable): checked or not is a body fact.
    Run,
    /// A static background start (`StartSession`, `TaskScheduler.CreateTask`,
    /// `CurrPage.EnqueueBackgroundTask` with a literal codeunit; F3): the
    /// target codeunit's object runs in a new session or task. No transaction
    /// is shared, the target has no UI, and its errors do not propagate back
    /// to the caller. A dynamic codeunit stays an `Open` site into the
    /// `run:codeunit` hub, whose anchor is the same call.
    Background,
    /// A publisher raising its subscribers.
    EventDispatch,
    /// A record operation firing a table trigger or a platform table event:
    /// the operation's index in [`RECORD_OPS`] when the graph names it.
    TriggerOf { op: Option<u8> },
    /// An object node running its triggers, parts and opened pages.
    Lifecycle,
    /// A test running a handler it lists.
    TestHandler,
    /// A test running its codeunit.
    TestRunner,
    /// An open site: into a hub, out of a hub, or out of an opaque body.
    Open,
}

/// A site's source anchor: its unit and span (0-based, the resolver's
/// [`crate::program::resolve::edge::CanonicalSpan`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Anchor {
    /// Index into [`SitedGraph::unit`].
    pub unit: u32,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// One site: one edge the builder added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    pub from: u32,
    pub to: u32,
    pub(crate) kind: Kind,
    pub(crate) conf: Conf,
    /// Index into [`VIAS`].
    pub(crate) via: u8,
    /// A [`CONDITIONS`] mask.
    pub(crate) conditions: u8,
    pub boundary: Boundary,
    /// `None` for an edge no call site makes (a rule, a hub expansion, a
    /// publisher's event flow).
    pub anchor: Option<Anchor>,
}

impl Site {
    #[must_use]
    pub fn via(&self) -> &'static str {
        VIAS[self.via as usize]
    }

    /// The export's 1-based line of this site.
    #[must_use]
    pub fn line(&self) -> Option<u32> {
        self.anchor.map(|a| a.start_line + 1)
    }
}

/// Per-node flag bits ([`SitedGraph::flags`]).
pub mod flag {
    /// The node is in the workspace app or an app loaded from local source (a
    /// test universe's production app): a path of such nodes is `Local`.
    pub const LOCAL: u8 = 1;
    /// A hub (an open site's bounded stand-in).
    pub const HUB: u8 = 2;
    /// The universal hub `h:any`: a caller of every node.
    pub const UNIVERSAL: u8 = 4;
    /// A routine carrying `[Test]`.
    pub const TEST: u8 = 8;
    /// A `LOCAL` procedure apps outside the program may call: public access
    /// on an object that is not `Access = Internal`.
    pub const EXT_PUBLIC: u8 = 16;
    /// A `LOCAL` `protected` procedure on an object that is not internal: an
    /// extension object in an app outside the program may call it.
    pub const EXT_PROTECTED: u8 = 32;
    /// A `LOCAL` procedure only the app's `internalsVisibleTo` friends may call
    /// from outside (an `internal` procedure, or a public one on an internal
    /// object), in an app that names a friend.
    pub const EXT_FRIEND: u8 = 64;
    /// A symbol-only routine or object (no body; no source facts).
    pub const SYMBOL_ONLY: u8 = 128;
}

/// One routine's StableRoutineId, interned: its object part's index in
/// [`SitedGraph::stable_object`] and the 32 bytes of its signature hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct StableEntry {
    object: u32,
    hash: [u8; 32],
    node: u32,
}

/// The program's sited graph (see the module doc). A site's id is its index
/// in [`SitedGraph::sites`].
pub struct SitedGraph {
    pub(crate) nodes: Vec<Key>,
    node_ix: HashMap<Key, u32>,
    pub(crate) sites: Vec<Site>,
    units: Vec<Box<str>>,
    /// Each app's `(guid, name)`, for node ids.
    apps: HashMap<AppRef, (String, String)>,
    /// `hiddenCallers` flags per node, over the unpruned graph.
    pub(crate) hidden: HashMap<u32, Vec<&'static str>>,
    pub(crate) primary_app: AppRef,
    /// Per node: its root kinds from source, a mask over
    /// `root_classification::ROOT_KIND_VALUES` (0: none, or no source).
    root_kinds: Vec<u16>,
    /// Per node: [`flag`] bits.
    flags: Vec<u8>,
    /// The object parts of the stable ids, interned.
    stable_objects: Vec<Box<str>>,
    stable_object_ix: HashMap<Box<str>, u32>,
    /// Every routine node with a StableRoutineId, sorted (a stable id can
    /// name several nodes: `#if` arms the program keeps apart).
    stable: Vec<StableEntry>,
    /// Per node: its index in `stable`, `u32::MAX` for none.
    stable_of: Vec<u32>,
    /// The lifecycle sites a page's action makes (`RunObject`), sorted.
    action_run_sites: Vec<u32>,
    /// The reverse index: the sites into node `n` are
    /// `rev_sites[rev_off[n]..rev_off[n + 1]]`, ascending.
    rev_off: Vec<u32>,
    rev_sites: Vec<u32>,
    /// The source-cased names of each local routine node (workspace and
    /// local-source apps): what a test-selection answer prints.
    local_names: HashMap<u32, LocalName>,
    /// The platform's table-event publishers (`OnAfterModifyEvent`, …: the
    /// synthetic publishers a subscriber binds to), ascending.
    platform_publishers: Vec<u32>,
    /// The sites carrying `configuredDispatch`, ascending, with the catalog
    /// entry and arm that labelled each ([`super::configured_dispatch`]).
    configured_arms: Vec<(u32, ConfiguredArm)>,
    /// The configured-dispatch catalog's outcome in this program.
    catalog: super::configured_dispatch::CatalogReport,
}

/// The catalog entry and arm a `configuredDispatch` site lies in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfiguredArm {
    /// The entry's index in [`super::configured_dispatch::CATALOG`].
    pub entry: u16,
    /// The arm's label as written (`PostDocumentCode()`, `else`).
    pub label: Box<str>,
}

impl ConfiguredArm {
    /// The entry's name.
    #[must_use]
    pub fn entry_name(&self) -> &'static str {
        super::configured_dispatch::CATALOG[self.entry as usize].name
    }
}

/// A local routine node's names as its source spells them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalName {
    pub object: Box<str>,
    pub routine: Box<str>,
    /// The file declaring it (its unit's virtual path).
    pub file: Box<str>,
    /// The routine is a trigger (`OnRun`, `OnAction`, …).
    pub trigger: bool,
    /// Its overload handle (`/1`, `/2[1: Integer]`, `/0#2`), empty when no
    /// other routine of its object has its name
    /// ([`crate::engine::l5::routine_rollup::overload_short`]).
    pub handle: Box<str>,
}

impl SitedGraph {
    /// A local routine's source-cased names ([`LocalName`]).
    #[must_use]
    pub fn local_name(&self, ix: u32) -> Option<&LocalName> {
        self.local_names.get(&ix)
    }

    /// Whether site `s` carries condition `name` (one of [`CONDITIONS`]).
    #[must_use]
    pub fn has_condition(&self, s: u32, name: &str) -> bool {
        let mask = self.sites[s as usize].conditions;
        condition_names(mask).any(|n| n == name)
    }

    /// The catalog entry and arm of a `configuredDispatch` site.
    #[must_use]
    pub fn configured_arm(&self, s: u32) -> Option<&ConfiguredArm> {
        self.configured_arms
            .binary_search_by_key(&s, |(x, _)| *x)
            .ok()
            .map(|i| &self.configured_arms[i].1)
    }

    /// The configured-dispatch catalog's outcome in this program.
    #[must_use]
    pub fn catalog(&self) -> &super::configured_dispatch::CatalogReport {
        &self.catalog
    }

    /// Whether `ix` is a platform table-event publisher (`OnAfterModifyEvent`
    /// and the like, raised by the platform on a record operation).
    #[must_use]
    pub fn is_platform_publisher(&self, ix: u32) -> bool {
        self.platform_publishers.binary_search(&ix).is_ok()
    }

    /// Every loaded app's `(guid, name)` (the apps the node table names).
    pub fn apps(&self) -> impl Iterator<Item = &(String, String)> {
        self.apps.values()
    }

    /// A node's [`flag`] bits.
    #[must_use]
    pub fn flags(&self, ix: u32) -> u8 {
        self.flags[ix as usize]
    }

    /// A node's root kinds from source (a `root_classification` mask).
    #[must_use]
    pub fn root_kinds(&self, ix: u32) -> u16 {
        self.root_kinds[ix as usize]
    }

    /// The sites into `ix` (ids), ascending.
    #[must_use]
    pub fn sites_into(&self, ix: u32) -> &[u32] {
        let (a, b) = (
            self.rev_off[ix as usize] as usize,
            self.rev_off[ix as usize + 1] as usize,
        );
        &self.rev_sites[a..b]
    }

    /// `true` when a [`Boundary::Background`] site enters `ix`: a static
    /// background start's target (F3).
    #[must_use]
    pub fn is_background_target(&self, ix: u32) -> bool {
        self.sites_into(ix)
            .iter()
            .any(|&s| self.sites[s as usize].boundary == Boundary::Background)
    }

    /// `true` when site `s` is a page action's `RunObject` run.
    #[must_use]
    pub fn is_action_run(&self, s: u32) -> bool {
        self.action_run_sites.binary_search(&s).is_ok()
    }

    /// The universal hub's node, when an edge names it.
    #[must_use]
    pub fn universal_hub(&self) -> Option<u32> {
        self.node_ix.get(&Key::Hub(Hub::Any)).copied()
    }

    /// The StableRoutineId of a routine node with source.
    #[must_use]
    pub fn stable_id(&self, ix: u32) -> Option<String> {
        let i = *self.stable_of.get(ix as usize)?;
        let e = self.stable.get(i as usize)?;
        let hex: String = e.hash.iter().map(|b| format!("{b:02x}")).collect();
        Some(format!("{}#{hex}", self.stable_objects[e.object as usize]))
    }

    /// The nodes whose StableRoutineId is `id` (more than one for `#if` arms
    /// the program keeps apart), ascending; empty when none has it. The test
    /// universe's lookup of a production routine (agent-tasks design §4.3).
    #[must_use]
    pub fn nodes_with_stable_id(&self, id: &str) -> Vec<u32> {
        let Some((object, hash)) = split_stable_id(id) else {
            return Vec::new();
        };
        let Some(&object) = self.stable_object_ix.get(object) else {
            return Vec::new();
        };
        let lo = self
            .stable
            .partition_point(|e| (e.object, e.hash) < (object, hash));
        self.stable[lo..]
            .iter()
            .take_while(|e| (e.object, e.hash) == (object, hash))
            .map(|e| e.node)
            .collect()
    }

    /// A node's display name: `Codeunit Name::Routine` for a routine,
    /// `Codeunit Name` for an object, the hub id for a hub.
    #[must_use]
    pub fn display(&self, ix: u32) -> String {
        let object = |o: &crate::program::node::ObjectNodeId| {
            let key = match &o.key {
                crate::program::node::ObjKey::Id(n) => n.to_string(),
                crate::program::node::ObjKey::Name(s) => s.to_string(),
            };
            format!(
                "{} {key}",
                crate::program::graphify_export::object_kind_str(o.kind)
            )
        };
        match &self.nodes[ix as usize] {
            Key::Routine(r) => match &r.enclosing_member_lc {
                Some(m) => format!("{}::{m}::{}", object(&r.object), r.name_lc),
                None => format!("{}::{}", object(&r.object), r.name_lc),
            },
            Key::Object(o) => object(o),
            Key::Hub(_) => self.node_id(ix),
        }
    }

    /// The app of a routine or object node.
    #[must_use]
    pub fn node_app(&self, ix: u32) -> Option<AppRef> {
        match &self.nodes[ix as usize] {
            Key::Routine(r) => Some(r.object.app),
            Key::Object(o) => Some(o.app),
            Key::Hub(Hub::Callbacks(a)) => Some(*a),
            Key::Hub(_) => None,
        }
    }

    /// An app's `(guid, name)`.
    #[must_use]
    pub fn app_identity(&self, app: AppRef) -> (String, String) {
        self.apps.get(&app).cloned().unwrap_or_default()
    }
    /// The workspace app the graph was built for.
    #[must_use]
    pub fn primary_app(&self) -> AppRef {
        self.primary_app
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn site_count(&self) -> usize {
        self.sites.len()
    }

    #[must_use]
    pub fn sites(&self) -> &[Site] {
        &self.sites
    }

    /// A unit's name.
    #[must_use]
    pub fn unit(&self, ix: u32) -> &str {
        &self.units[ix as usize]
    }

    /// A node's wire id (the export's `id`).
    #[must_use]
    pub fn node_id(&self, ix: u32) -> String {
        super::key_id_with(&self.nodes[ix as usize], &|a| {
            self.apps.get(&a).cloned().unwrap_or_default()
        })
    }

    /// The node of a routine, if the graph has it.
    #[must_use]
    pub fn routine_node(&self, r: &RoutineNodeId) -> Option<u32> {
        self.node_ix.get(&Key::Routine(r.clone())).copied()
    }

    /// The sites out of `from` to `to` (ids), in site order.
    #[must_use]
    pub fn sites_between(&self, from: u32, to: u32) -> Vec<u32> {
        (0..self.sites.len() as u32)
            .filter(|i| {
                let s = &self.sites[*i as usize];
                s.from == from && s.to == to
            })
            .collect()
    }

    /// The object of a routine node.
    #[must_use]
    pub fn routine_object(&self, ix: u32) -> Option<crate::program::node::ObjectNodeId> {
        match self.nodes.get(ix as usize)? {
            Key::Routine(r) => Some(r.object.clone()),
            _ => None,
        }
    }

    /// The object an object node stands for (`None` for a routine or a hub).
    #[must_use]
    pub fn object_node(&self, ix: u32) -> Option<crate::program::node::ObjectNodeId> {
        match self.nodes.get(ix as usize)? {
            Key::Object(o) => Some(o.clone()),
            _ => None,
        }
    }

    /// The routine nodes whose (folded) name is `name_lc`.
    #[must_use]
    pub fn routines_named(&self, name_lc: &str) -> Vec<u32> {
        (0..self.nodes.len() as u32)
            .filter(|i| {
                matches!(&self.nodes[*i as usize], Key::Routine(r) if r.name_lc.as_str() == name_lc)
            })
            .collect()
    }

    /// Every site, one line each in site order: its ends' ids, kind,
    /// confidence, `via`, line, conditions and boundary (the cold-equality
    /// comparisons' text).
    #[must_use]
    pub fn text(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for s in &self.sites {
            let conds: Vec<&str> = condition_names(s.conditions).collect();
            let anchor = s.anchor.map(|a| {
                format!(
                    "{}:{}:{}-{}:{}",
                    self.unit(a.unit),
                    a.start_line,
                    a.start_col,
                    a.end_line,
                    a.end_col
                )
            });
            let _ = writeln!(
                out,
                "{} {} {:?} {:?} {} {:?} {:?} {:?}",
                self.node_id(s.from),
                self.node_id(s.to),
                s.kind,
                s.conf,
                s.via(),
                anchor,
                conds,
                s.boundary
            );
        }
        out
    }

    /// The `hiddenCallers` flags of a node.
    #[must_use]
    pub fn hidden_callers(&self, ix: u32) -> &[&'static str] {
        self.hidden.get(&ix).map_or(&[], Vec::as_slice)
    }

    /// The graph's resident bytes, estimated from its tables' capacities.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        use std::mem::size_of;
        let key_heap: usize = self
            .nodes
            .iter()
            .map(|k| match k {
                Key::Routine(r) => {
                    r.name_lc.len()
                        + r.enclosing_member_lc.as_ref().map_or(0, |m| m.len())
                        + object_key_heap(&r.object)
                }
                Key::Object(o) => object_key_heap(o),
                Key::Hub(Hub::Name { name_lc, .. }) => name_lc.len(),
                Key::Hub(_) => 0,
            })
            .sum();
        // Each key twice (the table and the index), the index's slots.
        let nodes = self.nodes.capacity() * size_of::<Key>()
            + self.node_ix.capacity() * (size_of::<Key>() + size_of::<u32>() + 8)
            + 2 * key_heap;
        let sites = self.sites.capacity() * size_of::<Site>();
        let units: usize = self.units.iter().map(|u| u.len() + 16).sum();
        let tables = self.root_kinds.capacity() * size_of::<u16>()
            + self.flags.capacity()
            + self.stable.capacity() * size_of::<StableEntry>()
            + self.stable_of.capacity() * size_of::<u32>()
            + self.action_run_sites.capacity() * size_of::<u32>()
            + (self.rev_off.capacity() + self.rev_sites.capacity()) * size_of::<u32>()
            + self
                .stable_objects
                .iter()
                .map(|s| 2 * s.len() + 16 + size_of::<u32>() + 8)
                .sum::<usize>();
        let names: usize = self
            .local_names
            .values()
            .map(|n| {
                n.object.len() + n.routine.len() + n.file.len() + size_of::<(u32, LocalName)>() + 8
            })
            .sum::<usize>()
            + self.platform_publishers.capacity() * size_of::<u32>();
        let configured: usize = self
            .configured_arms
            .iter()
            .map(|(_, a)| a.label.len() + size_of::<(u32, ConfiguredArm)>())
            .sum();
        (nodes + sites + units + tables + names + configured) as u64
    }
}

/// A StableRoutineId's object part and hash bytes (`None` when it is not
/// `{object}#{64 hex}`).
fn split_stable_id(id: &str) -> Option<(&str, [u8; 32])> {
    let (object, hex) = id.rsplit_once('#')?;
    if hex.len() != 64 {
        return None;
    }
    let mut hash = [0u8; 32];
    for (i, b) in hash.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some((object, hash))
}

fn object_key_heap(o: &crate::program::node::ObjectNodeId) -> usize {
    match &o.key {
        crate::program::node::ObjKey::Name(s) => s.len(),
        crate::program::node::ObjKey::Id(_) => 0,
    }
}

/// The sited graph under construction (the projection's builder owns it).
pub(crate) struct SitedBuilder {
    pub(crate) nodes: Vec<Key>,
    pub(crate) node_ix: HashMap<Key, u32>,
    pub(crate) sites: Vec<Site>,
    units: Vec<Box<str>>,
    unit_ix: HashMap<Box<str>, u32>,
    /// The lifecycle sites a page action's `RunObject` makes.
    pub(crate) action_run_sites: Vec<u32>,
    /// The sites the projection labelled `configuredDispatch`, in site order.
    pub(crate) configured_arms: Vec<(u32, ConfiguredArm)>,
}

impl SitedBuilder {
    /// A builder whose node table starts with every routine of `graph`.
    pub(crate) fn new(graph: &ProgramGraph) -> SitedBuilder {
        let mut b = SitedBuilder {
            nodes: Vec::with_capacity(graph.routines.len()),
            node_ix: HashMap::with_capacity(graph.routines.len()),
            sites: Vec::new(),
            units: Vec::new(),
            unit_ix: HashMap::new(),
            action_run_sites: Vec::new(),
            configured_arms: Vec::new(),
        };
        for r in graph.routines.iter() {
            b.node(&Key::Routine(r.id.clone()));
        }
        b
    }

    pub(crate) fn node(&mut self, k: &Key) -> u32 {
        if let Some(i) = self.node_ix.get(k) {
            return *i;
        }
        let i = self.nodes.len() as u32;
        self.nodes.push(k.clone());
        self.node_ix.insert(k.clone(), i);
        i
    }

    pub(crate) fn ix(&self, k: &Key) -> Option<u32> {
        self.node_ix.get(k).copied()
    }

    pub(crate) fn unit(&mut self, unit: &str) -> u32 {
        if let Some(i) = self.unit_ix.get(unit) {
            return *i;
        }
        let i = self.units.len() as u32;
        self.units.push(unit.into());
        self.unit_ix.insert(unit.into(), i);
        i
    }

    /// The graph, with its node table's facts: root kinds, [`flag`] bits and
    /// StableRoutineIds, from `inp`; and its reverse index.
    pub(crate) fn finish(
        self,
        inp: &super::project::Inputs<'_>,
        hidden: HashMap<u32, Vec<&'static str>>,
        friended: &std::collections::HashSet<AppRef>,
    ) -> SitedGraph {
        use crate::snapshot::TrustTier;
        let graph = inp.graph;
        let primary_app = inp.edges.primary_app_ref;
        // The apps a `Local` path may cross: the workspace and every app
        // loaded from local source.
        let mut local_apps: std::collections::HashSet<AppRef> = graph
            .objects
            .iter()
            .filter(|o| {
                matches!(
                    o.tier,
                    TrustTier::Workspace
                        | TrustTier::LocalSourceVerified
                        | TrustTier::LocalSourceApproximate
                )
            })
            .map(|o| o.id.app)
            .collect();
        local_apps.insert(primary_app);
        let obj_of = |o: &crate::program::node::ObjectNodeId| {
            graph
                .objects
                .binary_search_by(|x| x.id.cmp(o))
                .ok()
                .map(|i| &graph.objects[i])
        };
        let n = self.nodes.len();
        let mut root_kinds = vec![0u16; n];
        let mut flags = vec![0u8; n];
        let mut stable_objects: Vec<Box<str>> = Vec::new();
        let mut stable_object_ix: HashMap<Box<str>, u32> = HashMap::new();
        let mut stable: Vec<StableEntry> = Vec::new();
        let mut local_names: HashMap<u32, LocalName> = HashMap::new();
        let mut platform_publishers: Vec<u32> = Vec::new();
        for (i, k) in self.nodes.iter().enumerate() {
            let f = &mut flags[i];
            match k {
                Key::Hub(h) => {
                    *f |= flag::HUB;
                    if matches!(h, Hub::Any) {
                        *f |= flag::UNIVERSAL;
                    }
                }
                Key::Object(o) => {
                    if local_apps.contains(&o.app) {
                        *f |= flag::LOCAL;
                        if let Some(x) = obj_of(o) {
                            local_names.insert(
                                i as u32,
                                LocalName {
                                    object: Box::from(&*x.name),
                                    ..LocalName::default()
                                },
                            );
                        }
                    }
                    if obj_of(o).is_some_and(|x| x.tier == TrustTier::SymbolOnly) {
                        *f |= flag::SYMBOL_ONLY;
                    }
                }
                Key::Routine(r) => {
                    let local = local_apps.contains(&r.object.app);
                    if local {
                        *f |= flag::LOCAL;
                    }
                    let facts = inp.facts.routines.get(r);
                    root_kinds[i] = facts.map_or(0, |x| x.root_kinds);
                    if facts.is_some_and(|x| x.test) {
                        *f |= flag::TEST;
                    }
                    let Some(node) = graph
                        .routines
                        .binary_search_by(|x| x.id.cmp(r))
                        .ok()
                        .map(|j| &graph.routines[j])
                    else {
                        continue;
                    };
                    if matches!(
                        node.publisher_kind,
                        Some(crate::program::resolve::event::PublisherKind::Platform)
                    ) {
                        platform_publishers.push(i as u32);
                    }
                    if node.tier == TrustTier::SymbolOnly {
                        *f |= flag::SYMBOL_ONLY;
                        continue;
                    }
                    if local {
                        local_names.insert(
                            i as u32,
                            LocalName {
                                object: obj_of(&r.object)
                                    .map(|o| Box::from(&*o.name))
                                    .unwrap_or_default(),
                                routine: Box::from(&*node.name),
                                trigger: node.is_trigger,
                                file: inp
                                    .surface
                                    .get_with_path(r)
                                    .map(|(_, p)| Box::from(p))
                                    .unwrap_or_default(),
                                handle: Box::default(),
                            },
                        );
                    }
                    if local
                        && !node.is_trigger
                        && r.object.kind != al_syntax::ir::ObjectKind::Interface
                    {
                        use crate::program::node_extract::Access;
                        let internal_obj = obj_of(&r.object).is_some_and(|o| o.internal);
                        *f |= match (node.access, internal_obj) {
                            (Access::Local, _) => 0,
                            (Access::Public, false) => flag::EXT_PUBLIC,
                            (Access::Protected, false) => flag::EXT_PROTECTED,
                            (Access::Internal, _) | (_, true) => {
                                if friended.contains(&r.object.app) {
                                    flag::EXT_FRIEND
                                } else {
                                    0
                                }
                            }
                        };
                    }
                    if let Some((m, _)) = inp.surface.get_with_path(r) {
                        let id = super::stable_routine_id(graph, r, node, m);
                        let parts = split_stable_id(&id);
                        debug_assert!(parts.is_some(), "a stable id is object#hex: {id}");
                        if let Some((object, hash)) = parts {
                            let next = stable_objects.len() as u32;
                            let object =
                                *stable_object_ix.entry(object.into()).or_insert_with(|| {
                                    stable_objects.push(object.into());
                                    next
                                });
                            stable.push(StableEntry {
                                object,
                                hash,
                                node: i as u32,
                            });
                        }
                    }
                }
            }
        }
        // Output validation F9: each local overload's short handle, by the
        // rule `RollupCx::handle` prints (`/n`, `/n[position: type]`,
        // `/n#k`), so a test path never prints two overloads alike.
        {
            use crate::engine::l5::routine_rollup::{OverloadShape, overload_short};
            type Group<'k> = (
                &'k crate::program::node::ObjectNodeId,
                &'k str,
                Option<&'k str>,
            );
            let mut groups: HashMap<Group<'_>, Vec<u32>> = HashMap::new();
            for (&i, l) in &local_names {
                if let (false, Key::Routine(r)) = (l.routine.is_empty(), &self.nodes[i as usize]) {
                    groups
                        .entry((
                            &r.object,
                            r.name_lc.as_str(),
                            r.enclosing_member_lc.as_deref(),
                        ))
                        .or_default()
                        .push(i);
                }
            }
            let mut handles: Vec<(u32, String)> = Vec::new();
            for members in groups.values().filter(|m| m.len() > 1) {
                let shapes: Vec<(u32, OverloadShape<'_>)> = members
                    .iter()
                    .filter_map(|&i| {
                        let Key::Routine(r) = &self.nodes[i as usize] else {
                            return None;
                        };
                        let (m, path) = inp.surface.get_with_path(r)?;
                        Some((
                            i,
                            OverloadShape {
                                types: m
                                    .params
                                    .iter()
                                    .map(|p| p.ty.as_deref().unwrap_or(""))
                                    .collect(),
                                order: (path, m.origin.start.row, m.origin.start.column),
                            },
                        ))
                    })
                    .collect();
                for (k, (i, me)) in shapes.iter().enumerate() {
                    let others: Vec<OverloadShape<'_>> = shapes
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != k)
                        .map(|(_, (_, o))| OverloadShape {
                            types: o.types.clone(),
                            order: o.order,
                        })
                        .collect();
                    handles.push((*i, overload_short(me, &others).0));
                }
            }
            for (i, h) in handles {
                if let Some(l) = local_names.get_mut(&i) {
                    l.handle = h.into();
                }
            }
        }
        stable.sort_unstable();
        let mut stable_of = vec![u32::MAX; n];
        for (j, e) in stable.iter().enumerate() {
            stable_of[e.node as usize] = j as u32;
        }
        stable.shrink_to_fit();
        // The reverse index (counting sort by target, sites ascending).
        let mut rev_off = vec![0u32; n + 1];
        for s in &self.sites {
            rev_off[s.to as usize + 1] += 1;
        }
        for j in 0..n {
            rev_off[j + 1] += rev_off[j];
        }
        let mut fill = rev_off.clone();
        let mut rev_sites = vec![0u32; self.sites.len()];
        for (j, s) in self.sites.iter().enumerate() {
            let slot = &mut fill[s.to as usize];
            rev_sites[*slot as usize] = j as u32;
            *slot += 1;
        }
        let mut action_run_sites = self.action_run_sites;
        action_run_sites.sort_unstable();
        action_run_sites.dedup();
        action_run_sites.shrink_to_fit();
        let mut apps = HashMap::new();
        for k in &self.nodes {
            let app = match k {
                Key::Routine(r) => Some(r.object.app),
                Key::Object(o) => Some(o.app),
                Key::Hub(Hub::Callbacks(a)) => Some(*a),
                Key::Hub(_) => None,
            };
            if let Some(a) = app {
                apps.entry(a)
                    .or_insert_with(|| super::app_identity(graph, a));
            }
        }
        let mut sites = self.sites;
        sites.shrink_to_fit();
        let mut nodes = self.nodes;
        nodes.shrink_to_fit();
        let mut node_ix = self.node_ix;
        node_ix.shrink_to_fit();
        SitedGraph {
            nodes,
            node_ix,
            sites,
            units: self.units,
            apps,
            hidden,
            primary_app,
            root_kinds,
            flags,
            stable_objects,
            stable_object_ix,
            stable,
            stable_of,
            action_run_sites,
            rev_off,
            rev_sites,
            local_names,
            platform_publishers,
            configured_arms: self.configured_arms,
            catalog: inp.facts.catalog.clone(),
        }
    }
}

/// One collapsed edge (the export's row before its ids).
#[derive(Clone, Debug)]
pub(crate) struct Collapsed {
    pub(crate) conf: Conf,
    pub(crate) via: u8,
    pub(crate) line: Option<u32>,
    pub(crate) conditions: u8,
}

impl Collapsed {
    pub(crate) fn of(s: &Site) -> Collapsed {
        Collapsed {
            conf: s.conf,
            via: s.via,
            line: s.line(),
            conditions: s.conditions,
        }
    }

    /// The export's merge rule (see the module doc).
    pub(crate) fn merge(&mut self, new: Collapsed) {
        let line = match (self.line, new.line) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if new.conf < self.conf {
            *self = new;
        } else if new.conf == self.conf {
            self.conditions |= new.conditions;
        }
        self.line = line;
    }

    pub(crate) fn via(&self) -> &'static str {
        VIAS[self.via as usize]
    }
}

/// The export's edges: `sites` collapsed on `(from, to, kind)`, folded in
/// site order.
pub(crate) fn collapse(sites: &[Site]) -> HashMap<(u32, u32, Kind), Collapsed> {
    let mut out: HashMap<(u32, u32, Kind), Collapsed> = HashMap::new();
    for s in sites {
        match out.entry((s.from, s.to, s.kind)) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(Collapsed::of(s));
            }
            std::collections::hash_map::Entry::Occupied(mut o) => {
                o.get_mut().merge(Collapsed::of(s))
            }
        }
    }
    out
}
