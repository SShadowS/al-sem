//! **Canonical entries** (output-validation round 2, r2 §2.1 steps 3-4): one
//! identity per way a run starts, shared by what a reverse walk reaches (the
//! numerator) and what the program holds (the denominator), so a share of
//! the program's entries never divides a sum over overlapping kinds by a
//! distinct count.
//!
//! - **Identity.** A routine node, the `#if` arms of one StableRoutineId as
//!   one entry. A page action is one entry, `(the object declaring it, its
//!   name)`, whether it runs through its `OnAction` trigger or through its
//!   `RunObject` (an action-run site, [`SitedGraph::action_run_actions`]); an
//!   action with both counts once. An action-run site whose actions the facts
//!   do not name stands for the page that declares them (one entry per page,
//!   noted: [`NOTE_UNNAMED_ACTIONS`]). A static background target (the
//!   codeunit object a start enters) and the `OnRun` it runs are one entry;
//!   a target codeunit with no `OnRun` node is its own entry, noted
//!   ([`NOTE_BACKGROUND_WITHOUT_ON_RUN`]).
//! - **Kind.** One per entry: the first of its kinds in [`EntryKind::ALL`]'s
//!   order (page action before page trigger, …), so the kinds are disjoint.
//!   Every kind the entry has is kept for the execution contexts.
//! - **What counts.** A node whose root kinds hold an entry kind, a static
//!   background target ([`super::sited::Boundary::Background`]), and the
//!   actions of every action-run site: what R0's reverse walk reports as
//!   entries ([`super::reverse`]'s `report`), with no roots.config overlay
//!   (impact walks without one). Public procedures are not entries (a walk
//!   reports them as external callers).
//!
//! The inventory is built once per graph ([`SitedGraph::entry_inventory`]).

use std::collections::{BTreeMap, HashMap};

use super::project::Key;
use super::reverse::{EntryKind, NodeIx, SiteIx};
use super::sited::SitedGraph;
use crate::program::node::ObjectNodeId;

/// One identity (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum EntryKey {
    /// A routine with a StableRoutineId (every `#if` arm of it).
    Stable(String),
    /// A node with no StableRoutineId (a symbol-only routine, an object).
    Node(NodeIx),
    /// A page action: the object declaring it and its name, folded.
    Action(ObjectNodeId, String),
    /// An action-run site whose actions are not named: the page declaring
    /// them.
    ActionRunPage(NodeIx),
}

/// An entry that stands for a page whose action-run site names no action
/// (one entry per page, not per action; §R2.4 landing change 2).
pub const NOTE_UNNAMED_ACTIONS: u8 = 1;
/// A background target codeunit with no `OnRun` node: counted apart, not
/// merged with its `OnRun` (§R2.4 landing change 1).
pub const NOTE_BACKGROUND_WITHOUT_ON_RUN: u8 = 2;

/// The bit of `k` in an entry's kind mask.
fn kind_bit(k: EntryKind) -> u16 {
    1 << EntryKind::ALL
        .iter()
        .position(|x| *x == k)
        .expect("every kind is listed")
}

/// The program's canonical entries (see the module doc). An entry is an
/// index `0..len()`.
#[derive(Debug, Default)]
pub struct EntryInventory {
    /// Per entry: its kinds, a mask over [`EntryKind::ALL`].
    kinds: Vec<u16>,
    /// Per entry: the node that shows it (the first node found for it).
    witness: Vec<NodeIx>,
    /// Per entry: the action's declared name, for an action an action-run
    /// site stands for.
    action: Vec<Option<Box<str>>>,
    /// Per entry: [`NOTE_UNNAMED_ACTIONS`] / [`NOTE_BACKGROUND_WITHOUT_ON_RUN`]
    /// bits, for the totals that must say they include such entries.
    note: Vec<u8>,
    /// How many action-run sites name no action (each counted as its page).
    unnamed_sites: usize,
    /// The entry of each node that is one.
    node_entry: HashMap<NodeIx, u32>,
    /// The entries each action-run site stands for.
    site_entries: HashMap<SiteIx, Vec<u32>>,
}

impl EntryInventory {
    /// The inventory of `g`: one pass over the nodes and the action-run
    /// sites.
    #[must_use]
    pub fn of(g: &SitedGraph) -> EntryInventory {
        let mut inv = EntryInventory::default();
        let mut ids: HashMap<EntryKey, u32> = HashMap::new();
        let mut intern = |inv: &mut EntryInventory, key: EntryKey, witness: NodeIx| -> u32 {
            *ids.entry(key).or_insert_with(|| {
                inv.kinds.push(0);
                inv.witness.push(witness);
                inv.action.push(None);
                inv.note.push(0);
                (inv.kinds.len() - 1) as u32
            })
        };
        // Each codeunit's `OnRun` node (round-2 r2 §R2.4 landing change 1: a
        // background start's target object and the `OnRun` it runs are one
        // entry).
        let mut on_run: HashMap<&ObjectNodeId, NodeIx> = HashMap::new();
        for n in 0..g.node_count() as u32 {
            if let Some(Key::Routine(r)) = g.nodes.get(n as usize)
                && r.enclosing_member_lc.is_none()
                && r.name_lc.as_str() == "onrun"
            {
                on_run.entry(&r.object).or_insert(n);
            }
        }
        let routine_key = |n: NodeIx, mask: u16| match g.nodes.get(n as usize) {
            Some(Key::Routine(r))
                if mask & kind_bit(EntryKind::PageAction) != 0
                    && r.enclosing_member_lc.is_some() =>
            {
                EntryKey::Action(
                    r.object.clone(),
                    al_syntax::fold_identifier(
                        r.enclosing_member_lc.as_deref().unwrap_or_default(),
                    ),
                )
            }
            _ => g.stable_id(n).map_or(EntryKey::Node(n), EntryKey::Stable),
        };
        for n in 0..g.node_count() as u32 {
            let mut mask = EntryKind::of_root_kinds(g.root_kinds(n))
                .into_iter()
                .fold(0u16, |m, k| m | kind_bit(k));
            let background = g.is_background_target(n);
            if background {
                mask |= kind_bit(EntryKind::BackgroundStart);
            }
            if mask == 0 {
                continue;
            }
            let (key, note) = match g.nodes.get(n as usize) {
                // A background target object: the entry of the `OnRun` it
                // starts, or apart (noted) when the codeunit has none.
                Some(Key::Object(o)) if background => match on_run.get(o) {
                    Some(&r) => (routine_key(r, 0), 0),
                    None => (EntryKey::Node(n), NOTE_BACKGROUND_WITHOUT_ON_RUN),
                },
                _ => (routine_key(n, mask), 0),
            };
            let e = intern(&mut inv, key, n);
            inv.kinds[e as usize] |= mask;
            inv.note[e as usize] |= note;
            inv.node_entry.insert(n, e);
        }
        for &s in g.action_run_sites() {
            let page = g.sites()[s as usize].from;
            let acts = g.action_run_actions(s);
            let mut es: Vec<u32> = Vec::new();
            if acts.is_empty() {
                let e = intern(&mut inv, EntryKey::ActionRunPage(page), page);
                inv.note[e as usize] |= NOTE_UNNAMED_ACTIONS;
                inv.unnamed_sites += 1;
                es.push(e);
            } else {
                for (object, name) in acts {
                    let e = intern(
                        &mut inv,
                        EntryKey::Action(object.clone(), al_syntax::fold_identifier(name)),
                        page,
                    );
                    if inv.action[e as usize].is_none() && inv.kinds[e as usize] == 0 {
                        inv.action[e as usize] = Some(name.clone());
                    }
                    es.push(e);
                }
            }
            for &e in &es {
                inv.kinds[e as usize] |= kind_bit(EntryKind::PageAction);
            }
            es.sort_unstable();
            es.dedup();
            inv.site_entries.insert(s, es);
        }
        inv
    }

    /// An entry's note bits ([`NOTE_UNNAMED_ACTIONS`],
    /// [`NOTE_BACKGROUND_WITHOUT_ON_RUN`]).
    #[must_use]
    pub fn note(&self, e: u32) -> u8 {
        self.note[e as usize]
    }

    /// How many action-run sites name no action.
    #[must_use]
    pub fn unnamed_sites(&self) -> usize {
        self.unnamed_sites
    }

    /// How many entries the program holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// The entry node `n` is, if it is one.
    #[must_use]
    pub fn of_node(&self, n: NodeIx) -> Option<u32> {
        self.node_entry.get(&n).copied()
    }

    /// The entries action-run site `s` stands for (empty for any other
    /// site).
    #[must_use]
    pub fn of_site(&self, s: SiteIx) -> &[u32] {
        self.site_entries.get(&s).map_or(&[], Vec::as_slice)
    }

    /// An entry's one kind (the first of its kinds in [`EntryKind::ALL`]).
    #[must_use]
    pub fn kind(&self, e: u32) -> EntryKind {
        let m = self.kinds[e as usize];
        EntryKind::ALL[m.trailing_zeros() as usize]
    }

    /// Every kind an entry has, in [`EntryKind::ALL`]'s order.
    pub fn kinds(&self, e: u32) -> impl Iterator<Item = EntryKind> + '_ {
        let m = self.kinds[e as usize];
        EntryKind::ALL
            .into_iter()
            .filter(move |k| m & kind_bit(*k) != 0)
    }

    /// The node that shows an entry.
    #[must_use]
    pub fn witness(&self, e: u32) -> NodeIx {
        self.witness[e as usize]
    }

    /// The declared name of an action only an action-run site stands for.
    #[must_use]
    pub fn action(&self, e: u32) -> Option<&str> {
        self.action[e as usize].as_deref()
    }

    /// The count per kind over the entries `keep` accepts.
    pub fn by_kind(&self, keep: impl Fn(u32) -> bool) -> BTreeMap<EntryKind, usize> {
        let mut out: BTreeMap<EntryKind, usize> = BTreeMap::new();
        for e in 0..self.len() as u32 {
            if keep(e) {
                *out.entry(self.kind(e)).or_default() += 1;
            }
        }
        out
    }

    /// The inventory's heap bytes (capacities).
    #[must_use]
    pub fn bytes(&self) -> usize {
        use std::mem::size_of;
        self.kinds.capacity() * size_of::<u16>()
            + self.note.capacity()
            + self.witness.capacity() * size_of::<NodeIx>()
            + self.action.capacity() * size_of::<Option<Box<str>>>()
            + self.node_entry.capacity() * (2 * size_of::<u32>() + 8)
            + self
                .site_entries
                .values()
                .map(|v| v.capacity() * size_of::<u32>() + size_of::<(SiteIx, Vec<u32>)>() + 8)
                .sum::<usize>()
    }
}
