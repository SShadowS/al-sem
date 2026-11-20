//! ENG-152: interface implementers by identity, the one home of the fact.
//!
//! [`InterfaceImplementers::build`] files every codeunit and every closed enum
//! under each interface its `implements` references bind to from its own app
//! ([`ProgramGraph::bind_interface`]), and under each of their bases,
//! transitively (ENG-142: an implementer of a derived interface implements each
//! base; `interface IBaz extends IBar`: alc 18.0.41 accepts
//! `B := <codeunit implementing IBaz>` for `B: Interface IBar`). A base binds
//! from its declaring interface's app. A reference that binds no interface node
//! goes to the unbound bucket of its folded name, whatever app it names: an
//! unbound reference is one whose interface is not loaded, so which app it
//! means cannot be checked, and an unbound receiver of that name reaches them
//! all (review, Fable: keyed by app too, a bare unbound receiver missed a
//! symbol-only implementer of `#<A>#I` with A's package absent).
//!
//! `ResolveIndex` keeps one (the resolver's reads, recorded for the session);
//! the model's dispatch meta and the impact hub build their own from the same
//! graph, so all three answer the same question the same way.

use std::collections::{HashMap, HashSet};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

use crate::program::graph::{InterfaceBinding, ProgramGraph};
use crate::program::node::ObjectNodeId;
use crate::program::node_extract::InterfaceRef;

/// The unbound bucket's key: `(closed enum?, folded name)`.
type UnboundKey = (bool, String);

/// See the module doc.
#[derive(Debug, Default, Clone)]
pub struct InterfaceImplementers {
    /// Interface → the codeunits implementing it or an interface derived from it.
    implementers: HashMap<ObjectNodeId, Vec<ObjectNodeId>>,
    /// Interface → the CLOSED enums implementing it (G24): only they carry their
    /// value mappings (`ObjectNode::enum_impls`). An enum has no methods to
    /// dispatch to (S9.0e), so it is never among `implementers`.
    closed_enum_implementers: HashMap<ObjectNodeId, Vec<ObjectNodeId>>,
    /// The codeunits and closed enums whose reference binds no interface node.
    unbound: HashMap<UnboundKey, Vec<ObjectNodeId>>,
    /// Each interface's bases, bound from its own app.
    bases: HashMap<ObjectNodeId, Vec<ObjectNodeId>>,
}

impl InterfaceImplementers {
    /// Build over `graph` (see the module doc). A header split across `#if`
    /// arms lists an interface once per arm, so each object is recorded once
    /// per interface; an `extends` cycle (alc refuses one) ends at the
    /// interfaces seen. Each list holds an interface's own implementers first,
    /// then those of the interfaces derived from it, each part in object order
    /// (ENG-142's order: two passes).
    pub fn build(graph: &ProgramGraph) -> Self {
        let bases: HashMap<ObjectNodeId, Vec<ObjectNodeId>> = graph
            .objects
            .iter()
            .filter(|o| o.id.kind == ObjectKind::Interface && !o.interface_bases.is_empty())
            .map(|d| {
                let b = d
                    .interface_bases
                    .iter()
                    .flat_map(|b| graph.bind_interface(d.id.app, b).ids().to_vec())
                    .collect();
                (d.id.clone(), b)
            })
            .collect();
        let mut out = Self {
            bases,
            ..Self::default()
        };
        let push = |list: &mut Vec<ObjectNodeId>, id: &ObjectNodeId| {
            if !list.contains(id) {
                list.push(id.clone());
            }
        };
        // Each implementer's references, bound once: (object, its bindings).
        let bound: Vec<(&ObjectNodeId, bool, Vec<ObjectNodeId>)> = graph
            .objects
            .iter()
            .filter(|o| o.id.kind == ObjectKind::Codeunit || o.closed_enum)
            .map(|obj| {
                let mut direct = Vec::new();
                for r in &obj.implements {
                    let binding = graph.bind_interface(obj.id.app, r);
                    if binding == InterfaceBinding::Unbound {
                        let key = (obj.closed_enum, r.name.fold_identifier());
                        push(out.unbound.entry(key).or_default(), &obj.id);
                    }
                    direct.extend(binding.ids().iter().cloned());
                }
                (&obj.id, obj.closed_enum, direct)
            })
            .collect();
        // Pass 1: the interfaces each object names itself.
        for (id, closed_enum, direct) in &bound {
            let map = if *closed_enum {
                &mut out.closed_enum_implementers
            } else {
                &mut out.implementers
            };
            for iface in direct {
                push(map.entry(iface.clone()).or_default(), id);
            }
        }
        // Pass 2: their bases, transitively.
        for (id, closed_enum, direct) in &bound {
            let mut seen: HashSet<ObjectNodeId> = direct.iter().cloned().collect();
            let mut stack: Vec<ObjectNodeId> = direct
                .iter()
                .flat_map(|d| out.bases.get(d).cloned().unwrap_or_default())
                .collect();
            stack.reverse();
            while let Some(iface) = stack.pop() {
                if !seen.insert(iface.clone()) {
                    continue;
                }
                if let Some(more) = out.bases.get(&iface) {
                    stack.extend(more.iter().rev().cloned());
                }
                let map = if *closed_enum {
                    &mut out.closed_enum_implementers
                } else {
                    &mut out.implementers
                };
                push(map.entry(iface).or_default(), id);
            }
        }
        out
    }

    /// The codeunits implementing interface `iface` or one derived from it.
    pub fn implementers_of(&self, iface: &ObjectNodeId) -> &[ObjectNodeId] {
        self.implementers
            .get(iface)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// The codeunits (`closed_enum: false`) or closed enums (`true`) whose
    /// reference binds no interface node, filed by `r`'s folded name.
    pub fn unbound_of(&self, closed_enum: bool, r: &InterfaceRef) -> &[ObjectNodeId] {
        self.unbound
            .get(&(closed_enum, r.name.fold_identifier()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// What a receiver bound `binding` (typed by `r`) dispatches to: the
    /// codeunits (`closed_enum: false`) or closed enums (`true`) of every
    /// interface it may name, then the unbound bucket of `r`'s name when the
    /// interface may be one no loaded app declares (`Unseen`, `Unbound`).
    /// Deduplicated, first occurrence kept, in binding order.
    pub fn for_binding(
        &self,
        closed_enum: bool,
        binding: &InterfaceBinding,
        r: &InterfaceRef,
    ) -> Vec<ObjectNodeId> {
        let map = if closed_enum {
            &self.closed_enum_implementers
        } else {
            &self.implementers
        };
        let mut lists: Vec<&[ObjectNodeId]> = binding
            .ids()
            .iter()
            .map(|i| map.get(i).map(Vec::as_slice).unwrap_or(&[]))
            .collect();
        if binding.may_be_unloaded() {
            lists.push(self.unbound_of(closed_enum, r));
        }
        let mut seen: HashSet<&ObjectNodeId> = HashSet::new();
        lists
            .into_iter()
            .flatten()
            .filter(|id| seen.insert(*id))
            .cloned()
            .collect()
    }

    /// Whether interface `derived` is `base` or derives from it, transitively.
    pub fn is_or_derives(&self, derived: &ObjectNodeId, base: &ObjectNodeId) -> bool {
        let mut seen: HashSet<&ObjectNodeId> = HashSet::new();
        let mut stack = vec![derived];
        while let Some(i) = stack.pop() {
            if i == base {
                return true;
            }
            if seen.insert(i) {
                stack.extend(self.bases.get(i).into_iter().flatten());
            }
        }
        false
    }
}
