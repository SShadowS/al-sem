//! Engine limits B2 (wave 2 final review, follow-up 3): one routine node per
//! declaration of a subscriber whose declarations subscribe differently.
//!
//! One app can declare a routine more than once only where no build compiles
//! two of the declarations: `#if` arms of a member list, whole-object `#if`
//! arms, or two files (`#if A` / `#if not A`). The program graph dedups such
//! copies into ONE node (`build::dedup_routines_preserving_genuine_overloads`),
//! and a node unions its copies' event facts. When the copies subscribe to
//! DIFFERENT events, that union lets one declaration's subscription dispatch
//! into another declaration's body, a path no build has. So before the dedup,
//! each declaration of such a routine gets a node id of its own (the "split"):
//! the routine's id with its `sig_fp` mixed with a hash of the declaration's
//! place, which is its source unit ([`unit_id`]) and
//! its ordinal among the routine's declarations in that file, in document
//! order. The place depends on the declaration's own file only, so a
//! declaration's node id, and the model's ids derived from it, do not move when
//! another file changes, except when that change starts or ends the split.
//!
//! Every reader that maps a routine declaration of the IR to its node maps it
//! through [`file_declaration_nodes`].

use std::collections::HashMap;

use crate::program::node::RoutineNodeId;

/// The split routines of a graph: each split routine's id (the "base", which no
/// node carries) and its declarations' node ids, in declaration order (source
/// units in build order, then document order), and each declaration's place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SplitDecls {
    by_base: HashMap<RoutineNodeId, Vec<RoutineNodeId>>,
    /// Declaration node → (base id, the 8-hex-digit suffix of its stable id).
    by_decl: HashMap<RoutineNodeId, (RoutineNodeId, String)>,
}

/// The hash of a declaration's place: its source unit and its ordinal among
/// the routine's declarations in that unit. Shared with the model, which
/// derives its split ids from it (`workspace::adopt_declaration_ids`).
#[must_use]
pub fn place_hash(unit: &str, ordinal: u32) -> u64 {
    let hex = crate::engine::ids::sha256_hex(&format!("{unit}\u{1f}{ordinal}"));
    u64::from_str_radix(&hex[..16], 16).expect("16 hex digits")
}

/// The 8 hex digits a split declaration adds to its routine's stable id
/// (`{stable}~{suffix}`).
#[must_use]
pub fn stable_suffix(unit: &str, ordinal: u32) -> String {
    let s = format!("{:016x}", place_hash(unit, ordinal))[..8].to_string();
    // Zero is the sited graph's "no suffix" (`sited::StableEntry`).
    if s == "00000000" {
        "00000001".to_string()
    } else {
        s
    }
}

/// The node id of the declaration of routine `base` at `unit`/`ordinal`.
#[must_use]
pub fn decl_id(base: &RoutineNodeId, unit: &str, ordinal: u32) -> RoutineNodeId {
    let mut id = base.clone();
    let mixed = base.sig_fp ^ place_hash(unit, ordinal);
    // Never the base's own fingerprint (and never 0, which means "no params").
    id.sig_fp = if mixed == base.sig_fp || mixed == 0 {
        mixed.wrapping_add(1)
    } else {
        mixed
    };
    id
}

impl SplitDecls {
    /// Record the split of routine `base` into `decls` (node id, unit,
    /// ordinal), in declaration order.
    pub fn insert(&mut self, base: RoutineNodeId, decls: Vec<(RoutineNodeId, String, u32)>) {
        let mut ids = Vec::with_capacity(decls.len());
        for (id, unit, ordinal) in decls {
            self.by_decl
                .insert(id.clone(), (base.clone(), stable_suffix(&unit, ordinal)));
            ids.push(id);
        }
        // The first (smallest id) is the one a name lookup shows
        // (`is_hidden`): the node list's own order, so any file order agrees.
        ids.sort();
        self.by_base.insert(base, ids);
    }

    /// Whether `decl` is a declaration a name lookup does not show: every
    /// declaration but the first of its routine
    /// (`resolve::reads::RoutineRun`).
    #[must_use]
    pub fn is_hidden(&self, decl: &RoutineNodeId) -> bool {
        self.siblings(decl).first().is_some_and(|f| f != decl)
    }

    /// Merge another table (the dependency layer's into the graph's).
    pub fn extend(&mut self, other: &SplitDecls) {
        self.by_base
            .extend(other.by_base.iter().map(|(k, v)| (k.clone(), v.clone())));
        self.by_decl
            .extend(other.by_decl.iter().map(|(k, v)| (k.clone(), v.clone())));
    }

    /// `ids` (a file's routine ids per object, as the source spells them) with
    /// each declaration of a split routine replaced by its own node; `None`
    /// when no id of the file is split. `unit` is the file's [`unit_id`].
    #[must_use]
    pub fn file_declaration_ids(
        &self,
        ids: &[Vec<RoutineNodeId>],
        unit: &str,
    ) -> Option<Vec<Vec<RoutineNodeId>>> {
        if self.is_empty() || !ids.iter().flatten().any(|b| self.is_split(b)) {
            return None;
        }
        let mut ordinals = Ordinals::default();
        Some(
            ids.iter()
                .map(|object| {
                    object
                        .iter()
                        .map(|b| {
                            if self.is_split(b) {
                                decl_id(b, unit, ordinals.next(b))
                            } else {
                                b.clone()
                            }
                        })
                        .collect()
                })
                .collect(),
        )
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_base.is_empty()
    }

    /// Whether `base` is a split routine (no node carries its id).
    #[must_use]
    pub fn is_split(&self, base: &RoutineNodeId) -> bool {
        self.by_base.contains_key(base)
    }

    /// Every declaration node of split routine `base`, in declaration order.
    #[must_use]
    pub fn declarations(&self, base: &RoutineNodeId) -> &[RoutineNodeId] {
        self.by_base.get(base).map_or(&[], Vec::as_slice)
    }

    /// The base and the stable-id suffix of declaration node `decl`; `None`
    /// for any other node.
    #[must_use]
    pub fn base_of(&self, decl: &RoutineNodeId) -> Option<(&RoutineNodeId, &str)> {
        self.by_decl.get(decl).map(|(b, s)| (b, s.as_str()))
    }

    /// The declarations of the routine `decl` is a declaration of (itself
    /// included); empty for a node that is no split declaration.
    #[must_use]
    pub fn siblings(&self, decl: &RoutineNodeId) -> &[RoutineNodeId] {
        self.base_of(decl)
            .map_or(&[], |(base, _)| self.declarations(base))
    }
}

/// A source file's unit for a declaration's place: its app's guid and its
/// virtual path. The same for a workspace file and a dependency's.
#[must_use]
pub fn unit_id(app_guid: &str, path: &str) -> String {
    format!("{app_guid}:{path}")
}

/// [`unit_id`] of a parsed file.
#[must_use]
pub fn unit_of_parsed(pf: &crate::snapshot::parse::ParsedFile) -> String {
    unit_id(&pf.provenance.app.guid, &pf.virtual_path)
}

/// The node of every routine declaration of `file`, indexed by (object index,
/// routine index). `unit` is the file's [`unit_id`]. A declaration keeps its
/// `source_routine_node_id` while a node carries that id; when none does and
/// the graph split the routine, it is the declaration's own node. Both reads
/// go through `graph`, so a recording reader (the session's per-file
/// resolution) notes a split's start or end as a change. No ordinal is
/// counted for a routine a node carries (nearly all), so a file with no split
/// routine pays one lookup per routine.
#[must_use]
pub fn file_declaration_nodes(
    graph: &dyn crate::program::resolve::reads::GraphReads,
    file: &al_syntax::ir::AlFile,
    app: crate::program::node::AppRef,
    unit: &str,
) -> Vec<Vec<RoutineNodeId>> {
    let mut ordinals = Ordinals::default();
    file.objects
        .iter()
        .map(|obj| {
            let object = crate::program::node::ObjectNodeId {
                app,
                kind: obj.kind,
                key: crate::program::node::ObjKey::from_source(obj),
            };
            obj.routines
                .iter()
                .map(|r| {
                    let base = crate::program::sig_fp::source_routine_node_id(object.clone(), r);
                    if graph.routine(&base).is_some() {
                        return base;
                    }
                    let d = decl_id(&base, unit, ordinals.next(&base));
                    if graph.routine(&d).is_some() { d } else { base }
                })
                .collect()
        })
        .collect()
}

/// The routes of a resolved site with each route to a split subscriber's
/// declaration turned into one route per declaration of it: a direct call
/// reaches the routine in every build that declares it, so every declaration
/// is a possible target. An `EventFlow` edge is per subscription and already
/// names the declaration that subscribes, so it stays as it is.
#[must_use]
pub fn expand_call_routes(
    graph: &dyn crate::program::resolve::reads::GraphReads,
    kind: crate::program::resolve::edge::EdgeKind,
    routes: Vec<crate::program::resolve::edge::Route>,
) -> Vec<crate::program::resolve::edge::Route> {
    use crate::program::resolve::edge::{EdgeKind, RouteTarget};
    if kind == EdgeKind::EventFlow {
        return routes;
    }
    let mut out = Vec::with_capacity(routes.len());
    for r in routes {
        let decls = match &r.target {
            RouteTarget::Routine(t) => graph.declarations_of(t),
            _ => &[],
        };
        if decls.is_empty() {
            out.push(r);
            continue;
        }
        out.extend(decls.iter().map(|d| {
            let mut route = r.clone();
            route.target = RouteTarget::Routine(d.clone());
            route
        }));
    }
    out
}

/// The ordinal of each routine declaration among the declarations of the same
/// routine id in one source unit, in document order. One per unit walk.
#[derive(Debug, Default)]
pub struct Ordinals(HashMap<RoutineNodeId, u32>);

impl Ordinals {
    /// The next ordinal of routine `base` in this unit (0 for its first).
    pub fn next(&mut self, base: &RoutineNodeId) -> u32 {
        let n = self.0.entry(base.clone()).or_insert(0);
        let k = *n;
        *n += 1;
        k
    }
}
