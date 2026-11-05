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
//! state ([`Boundary`]). The finer boundaries the design names (a checked
//! against an unchecked `Codeunit.Run`, a background session or task, a
//! trigger's temporary-record state) come from body facts the program edges do
//! not carry; their consumers (R0, A) refine a [`Boundary::Run`] or
//! [`Boundary::TriggerOf`] site by its [`Anchor`].

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
];

/// Every route condition, in name order: a condition mask's bits, read low
/// to high, are the export's sorted `conditions`.
pub(crate) const CONDITIONS: &[&str] = &[
    "ambiguousDispatch",
    "manualBinding",
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
}

impl SitedGraph {
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
        (nodes + sites + units) as u64
    }
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

    pub(crate) fn finish(
        self,
        graph: &ProgramGraph,
        hidden: HashMap<u32, Vec<&'static str>>,
        primary_app: AppRef,
    ) -> SitedGraph {
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
