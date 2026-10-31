//! The cone's inherited capability facts, stored compactly (cone-memory
//! design step 1, `docs/superpowers/specs/2026-10-31-cone-memory-design.md`
//! §4.1-4.2, §5).
//!
//! An inherited fact is "callee fact X, reached through my first-hop edge Y".
//! There are few distinct callee facts (reps) and few distinct first hops, but
//! millions of (rep, hop) pairs: 17.0 M on the cross-app CDO model, against
//! 27,477 reps and 38,081 hops. So a fact is stored as an [`InheritedRef`], two
//! `u32` indexes (8 bytes) into the shared [`InheritedTables`], and read through
//! a borrowed [`InheritedFactRef`] whose accessors apply `retag`'s rules. Only
//! [`InheritedFactRef::materialize`] builds an owned [`CapabilityFact`] from it;
//! it is the ONE builder of an inherited fact.
//!
//! What the selection keeps and what this stores are separate: the cone still
//! picks winners over the original `TypedOutEdge`s (their `sort_key`
//! tie-breaks included). Only the winning output is interned here:
//!
//! - **reps by `Arc` identity.** The table holds a strong `Arc` per entry from
//!   the first time an address is seen, so a recorded address cannot be freed
//!   and reused for a different fact while the table lives (the census in the
//!   design's §2.8 saw 5,402 address reuses without that hold).
//! - **hops by content**, `(subject, via, callsite, operation_id, event_id)`:
//!   exactly what `retag` reads from an edge, never the edge's target or kind.
//!   `None` and `Some("")` stay distinct. The subject is part of the key, so a
//!   hop's subject is always the routine that emitted it.

use std::collections::HashMap;
use std::sync::Arc;

use super::capability_cone::{
    CapabilityExtra, CapabilityFact, TypedOutEdge, ValueSource, capability_via_for_edge_kind,
};

/// An index into [`InheritedTables::reps`].
pub type RepIx = u32;
/// An index into [`InheritedTables::hops`].
pub type HopIx = u32;
/// An index into [`InheritedTables::subjects`].
pub type SubjIx = u32;

/// Reserved: marks a direct (non-inherited) row. Never a table index; every
/// table index is built by [`table_ix`], which rejects it.
pub const NO_HOP: HopIx = u32::MAX;

/// One inherited fact: its rep and its first hop. 8 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InheritedRef {
    pub rep: RepIx,
    pub hop: HopIx,
}

const _: () = assert!(std::mem::size_of::<InheritedRef>() == 8);

/// The index of the next row of a table that holds `len` rows: `u32`, checked,
/// and never [`NO_HOP`].
pub fn table_ix(len: usize, table: &str) -> u32 {
    let ix = u32::try_from(len)
        .unwrap_or_else(|_| panic!("inherited facts: the {table} table outgrew u32 ({len} rows)"));
    assert!(
        ix != NO_HOP,
        "inherited facts: the {table} table reached the reserved index u32::MAX"
    );
    ix
}

/// What `retag` reads from a first-hop edge, plus the emitting subject. One row
/// per distinct (subject, via, callsite, operation_id, event_id); `Option`s
/// stay distinct (`None` is not `Some("")`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FirstHop {
    pub subject: SubjIx,
    pub via: &'static str,
    pub callsite: Option<String>,
    pub operation_id: Option<String>,
    pub event_id: Option<String>,
}

/// The shared tables every [`InheritedRef`] indexes.
#[derive(Debug, Default)]
pub struct InheritedTables {
    /// Internal routine ids of the emitting members, interned.
    pub subjects: Vec<String>,
    /// Interned by `Arc` identity; a strong `Arc` per entry.
    pub reps: Vec<Arc<CapabilityFact>>,
    pub hops: Vec<FirstHop>,
}

impl InheritedTables {
    fn view(&self, r: InheritedRef) -> InheritedFactRef<'_> {
        InheritedFactRef {
            rep: &self.reps[r.rep as usize],
            hop: &self.hops[r.hop as usize],
            tables: self,
        }
    }

    /// The seven fields of `sort_inherited`'s key for `r`, in key order: the
    /// order is their `|`-joined byte order (`cmp_joined`). `None` is `""`.
    fn sort_fields(&self, r: InheritedRef) -> [&str; 7] {
        let f = self.view(r);
        [
            f.op(),
            f.resource_kind(),
            f.resource_id().unwrap_or_default(),
            f.confidence(),
            f.via(),
            f.witness_callsite_id().unwrap_or_default(),
            f.witness_operation_id().unwrap_or_default(),
        ]
    }

    /// Heap bytes of the tables (the census): each table's buffer at capacity
    /// plus every row's owned strings, and each rep's fact once.
    pub(crate) fn heap_bytes(&self) -> u64 {
        let opt = |s: &Option<String>| s.as_ref().map_or(0, |s| s.len() as u64);
        let subjects: u64 = self.subjects.iter().map(|s| s.len() as u64).sum::<u64>()
            + (self.subjects.capacity() * std::mem::size_of::<String>()) as u64;
        let reps: u64 = self
            .reps
            .iter()
            .map(|r| {
                std::mem::size_of::<CapabilityFact>() as u64
                    + super::cone_census::capability_fact_heap_bytes(r)
            })
            .sum::<u64>()
            + (self.reps.capacity() * std::mem::size_of::<Arc<CapabilityFact>>()) as u64;
        let hops: u64 = self
            .hops
            .iter()
            .map(|h| opt(&h.callsite) + opt(&h.operation_id) + opt(&h.event_id))
            .sum::<u64>()
            + (self.hops.capacity() * std::mem::size_of::<FirstHop>()) as u64;
        subjects + reps + hops
    }
}

/// Per-subject inherited facts, each subject's refs in `sort_inherited` order.
/// One entry per member the scope's `raw` contained, empty or not: `get` is
/// `None` only for a subject the composition never walked, and `Some(empty)`
/// for a composed subject with nothing. Owned keys; the tables are shared.
#[derive(Debug, Clone, Default)]
pub struct InheritedFacts {
    tables: Arc<InheritedTables>,
    by_subject: HashMap<String, Vec<InheritedRef>>,
}

impl InheritedFacts {
    /// `id`'s facts: `None` when the composition never walked `id`.
    pub fn get(&self, id: &str) -> Option<InheritedSlice<'_>> {
        let (key, refs) = self.by_subject.get_key_value(id)?;
        // The subject contract (design §4.1): every ref stored under `key` was
        // emitted for `key`, so its hop's subject is `key`.
        debug_assert!(
            refs.iter().all(|r| self.tables.subjects
                [self.tables.hops[r.hop as usize].subject as usize]
                == *key),
            "inherited facts: a ref under {key:?} has another subject's hop"
        );
        Some(InheritedSlice {
            refs,
            tables: &self.tables,
        })
    }

    pub fn contains_key(&self, id: &str) -> bool {
        self.by_subject.contains_key(id)
    }

    /// The number of subjects (empty ones included).
    pub fn len(&self) -> usize {
        self.by_subject.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_subject.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.by_subject.keys().map(String::as_str)
    }

    /// Every subject with its facts, in map order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, InheritedSlice<'_>)> {
        self.by_subject.iter().map(|(k, refs)| {
            (
                k.as_str(),
                InheritedSlice {
                    refs,
                    tables: &self.tables,
                },
            )
        })
    }

    /// The facts of all subjects together.
    pub fn total_facts(&self) -> usize {
        self.by_subject.values().map(Vec::len).sum()
    }

    /// Move the entries of `ids` (those present) into a new `InheritedFacts`
    /// that shares these tables: each drained vector stays paired with its
    /// owned subject and a table owner.
    pub fn take_subjects<'i>(&mut self, ids: impl IntoIterator<Item = &'i str>) -> InheritedFacts {
        let by_subject = ids
            .into_iter()
            .filter_map(|id| self.by_subject.remove_entry(id))
            .collect();
        InheritedFacts {
            tables: Arc::clone(&self.tables),
            by_subject,
        }
    }

    pub fn tables(&self) -> &Arc<InheritedTables> {
        &self.tables
    }

    /// Bytes of this map for the `witness.raw_bytes` census: key heap, each
    /// ref vector at capacity, and the shared tables once.
    pub(crate) fn census_bytes(&self) -> u64 {
        self.by_subject
            .iter()
            .map(|(k, v)| (k.len() + v.capacity() * std::mem::size_of::<InheritedRef>()) as u64)
            .sum::<u64>()
            + self.tables.heap_bytes()
    }
}

/// One subject's inherited facts, borrowed.
#[derive(Clone, Copy)]
pub struct InheritedSlice<'a> {
    refs: &'a [InheritedRef],
    tables: &'a InheritedTables,
}

/// Prints the facts as they read (materialized), not the indexes.
impl std::fmt::Debug for InheritedSlice<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.materialize()).finish()
    }
}

impl<'a> InheritedSlice<'a> {
    pub fn len(&self) -> usize {
        self.refs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    pub fn refs(&self) -> &'a [InheritedRef] {
        self.refs
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = InheritedFactRef<'a>> + use<'a> {
        let tables = self.tables;
        self.refs.iter().map(move |r| tables.view(*r))
    }

    /// Owned facts, in order: for the projections and tests only.
    pub fn materialize(&self) -> Vec<CapabilityFact> {
        self.iter().map(|f| f.materialize()).collect()
    }
}

/// One inherited fact, borrowed. The accessors restate `retag`'s rules: the
/// subject is the emitting routine; `provenance` is `"inherited"`; `via`, the
/// call-site witness and both first-hop ids come from the first hop (`None`
/// when the hop has none, never the rep's); every other field is the rep's,
/// `witness_operation_id` included.
#[derive(Clone, Copy)]
pub struct InheritedFactRef<'a> {
    rep: &'a CapabilityFact,
    hop: &'a FirstHop,
    tables: &'a InheritedTables,
}

impl<'a> InheritedFactRef<'a> {
    pub fn subject(&self) -> &'a str {
        &self.tables.subjects[self.hop.subject as usize]
    }
    pub fn op(&self) -> &'static str {
        self.rep.op
    }
    pub fn resource_kind(&self) -> &'static str {
        self.rep.resource_kind
    }
    pub fn resource_id(&self) -> Option<&'a str> {
        self.rep.resource_id.as_deref()
    }
    pub fn resource_arg_source(&self) -> Option<&'a ValueSource> {
        self.rep.resource_arg_source.as_ref()
    }
    pub fn confidence(&self) -> &'static str {
        self.rep.confidence
    }
    pub fn provenance(&self) -> &'static str {
        "inherited"
    }
    pub fn via(&self) -> &'static str {
        self.hop.via
    }
    /// The rep's: `retag` keeps it.
    pub fn witness_operation_id(&self) -> Option<&'a str> {
        self.rep.witness_operation_id.as_deref()
    }
    pub fn witness_callsite_id(&self) -> Option<&'a str> {
        self.hop.callsite.as_deref()
    }
    pub fn witness_first_hop_operation_id(&self) -> Option<&'a str> {
        self.hop.operation_id.as_deref()
    }
    pub fn witness_first_hop_event_id(&self) -> Option<&'a str> {
        self.hop.event_id.as_deref()
    }
    pub fn extra(&self) -> Option<&'a CapabilityExtra> {
        self.rep.extra.as_ref()
    }
    pub fn rep(&self) -> &'a CapabilityFact {
        self.rep
    }

    /// The owned fact, exactly `retag(rep, subject, edge)`: the ONE builder of
    /// an inherited `CapabilityFact`.
    pub fn materialize(&self) -> CapabilityFact {
        CapabilityFact {
            subject: self.subject().to_string(),
            provenance: "inherited",
            via: self.hop.via,
            witness_callsite_id: self.hop.callsite.clone(),
            // Set for EVERY hop, never inherited from the rep (#96 D5, N4; #98
            // D1): a stale first-hop id on the rep cannot leak through.
            witness_first_hop_operation_id: self.hop.operation_id.clone(),
            witness_first_hop_event_id: self.hop.event_id.clone(),
            ..self.rep.clone()
        }
    }
}

/// A direct or an inherited fact, with the same accessor names. Policy and
/// `derive_permissions` read through it, so neither materializes.
#[derive(Clone, Copy)]
pub enum FactRef<'a> {
    Direct(&'a CapabilityFact),
    Inherited(InheritedFactRef<'a>),
}

impl<'a> FactRef<'a> {
    pub fn subject(&self) -> &'a str {
        match self {
            FactRef::Direct(f) => &f.subject,
            FactRef::Inherited(v) => v.subject(),
        }
    }
    pub fn op(&self) -> &'static str {
        match self {
            FactRef::Direct(f) => f.op,
            FactRef::Inherited(v) => v.op(),
        }
    }
    pub fn resource_kind(&self) -> &'static str {
        match self {
            FactRef::Direct(f) => f.resource_kind,
            FactRef::Inherited(v) => v.resource_kind(),
        }
    }
    pub fn resource_id(&self) -> Option<&'a str> {
        match self {
            FactRef::Direct(f) => f.resource_id.as_deref(),
            FactRef::Inherited(v) => v.resource_id(),
        }
    }
    pub fn resource_arg_source(&self) -> Option<&'a ValueSource> {
        match self {
            FactRef::Direct(f) => f.resource_arg_source.as_ref(),
            FactRef::Inherited(v) => v.resource_arg_source(),
        }
    }
    pub fn confidence(&self) -> &'static str {
        match self {
            FactRef::Direct(f) => f.confidence,
            FactRef::Inherited(v) => v.confidence(),
        }
    }
    pub fn provenance(&self) -> &'static str {
        match self {
            FactRef::Direct(f) => f.provenance,
            FactRef::Inherited(v) => v.provenance(),
        }
    }
    pub fn via(&self) -> &'static str {
        match self {
            FactRef::Direct(f) => f.via,
            FactRef::Inherited(v) => v.via(),
        }
    }
    pub fn witness_operation_id(&self) -> Option<&'a str> {
        match self {
            FactRef::Direct(f) => f.witness_operation_id.as_deref(),
            FactRef::Inherited(v) => v.witness_operation_id(),
        }
    }
    pub fn witness_callsite_id(&self) -> Option<&'a str> {
        match self {
            FactRef::Direct(f) => f.witness_callsite_id.as_deref(),
            FactRef::Inherited(v) => v.witness_callsite_id(),
        }
    }
    pub fn witness_first_hop_operation_id(&self) -> Option<&'a str> {
        match self {
            FactRef::Direct(f) => f.witness_first_hop_operation_id.as_deref(),
            FactRef::Inherited(v) => v.witness_first_hop_operation_id(),
        }
    }
    pub fn witness_first_hop_event_id(&self) -> Option<&'a str> {
        match self {
            FactRef::Direct(f) => f.witness_first_hop_event_id.as_deref(),
            FactRef::Inherited(v) => v.witness_first_hop_event_id(),
        }
    }
    pub fn extra(&self) -> Option<&'a CapabilityExtra> {
        match self {
            FactRef::Direct(f) => f.extra.as_ref(),
            FactRef::Inherited(v) => v.extra(),
        }
    }
    /// `cone_derived::fact_is_known_temp`'s rule (it reads `extra` alone,
    /// which an inherited fact takes from its rep unchanged).
    pub fn is_known_temp(&self) -> bool {
        match self {
            FactRef::Direct(f) => super::cone_derived::fact_is_known_temp(f),
            FactRef::Inherited(v) => super::cone_derived::fact_is_known_temp(v.rep()),
        }
    }
}

/// The interner a composition emits through: reps by `Arc` identity, hops by
/// content with their subject, subjects by id. One per composition; single
/// threaded (the SCC walk is).
#[derive(Default)]
pub(crate) struct InheritedBuilder {
    tables: InheritedTables,
    subject_ix: HashMap<String, SubjIx>,
    /// Keyed by the address of an `Arc` the table holds, so it stays valid.
    rep_ix: HashMap<*const CapabilityFact, RepIx>,
    hop_ix: HashMap<FirstHop, HopIx>,
    /// A memo in front of `hop_ix`: (subject, edge address) → hop. The graph's
    /// edges live for the whole composition and never move. It only saves the
    /// content key's clones; `hop_ix` decides identity.
    edge_memo: HashMap<(SubjIx, *const TypedOutEdge), HopIx>,
    by_subject: HashMap<String, Vec<InheritedRef>>,
}

impl InheritedBuilder {
    /// The index of subject `id` (interned on first sight).
    pub(crate) fn subject(&mut self, id: &str) -> SubjIx {
        if let Some(&ix) = self.subject_ix.get(id) {
            return ix;
        }
        let ix = table_ix(self.tables.subjects.len(), "subject");
        self.tables.subjects.push(id.to_string());
        self.subject_ix.insert(id.to_string(), ix);
        ix
    }

    /// The index of `rep`, by `Arc` identity; the table keeps a strong `Arc`.
    pub(crate) fn rep(&mut self, rep: &Arc<CapabilityFact>) -> RepIx {
        let p = Arc::as_ptr(rep);
        if let Some(&ix) = self.rep_ix.get(&p) {
            return ix;
        }
        let ix = table_ix(self.tables.reps.len(), "rep");
        self.tables.reps.push(Arc::clone(rep));
        self.rep_ix.insert(p, ix);
        ix
    }

    /// The index of the first hop `edge` gives `subject`, by content.
    pub(crate) fn hop(&mut self, subject: SubjIx, edge: &TypedOutEdge) -> HopIx {
        let memo = (subject, edge as *const TypedOutEdge);
        if let Some(&ix) = self.edge_memo.get(&memo) {
            return ix;
        }
        let key = FirstHop {
            subject,
            via: capability_via_for_edge_kind(&edge.kind),
            callsite: edge.callsite.clone(),
            operation_id: edge.operation_id.clone(),
            event_id: edge.event_id.clone(),
        };
        let ix = match self.hop_ix.get(&key) {
            Some(&ix) => ix,
            None => {
                let ix = table_ix(self.tables.hops.len(), "hop");
                self.tables.hops.push(key.clone());
                self.hop_ix.insert(key, ix);
                ix
            }
        };
        self.edge_memo.insert(memo, ix);
        ix
    }

    /// One fact: `rep` reached by `subject` through `edge`.
    pub(crate) fn fact(
        &mut self,
        subject: SubjIx,
        rep: &Arc<CapabilityFact>,
        edge: &TypedOutEdge,
    ) -> InheritedRef {
        InheritedRef {
            rep: self.rep(rep),
            hop: self.hop(subject, edge),
        }
    }

    /// Stable sort by `inherited_output_sort_key`'s fields, compared as one
    /// `|`-joined string (`cmp_joined`), as `sort_inherited` always did.
    pub(crate) fn sort(&self, refs: &mut [InheritedRef]) {
        refs.sort_by(|a, b| {
            crate::engine::joined_key::cmp_joined(
                &self.tables.sort_fields(*a),
                &self.tables.sort_fields(*b),
            )
        });
    }

    /// Record `id`'s facts (an empty Vec included: "composed, nothing").
    pub(crate) fn insert(&mut self, id: String, refs: Vec<InheritedRef>) {
        self.by_subject.insert(id, refs);
    }

    /// The tables so far (a test hook materializes through them).
    #[cfg(test)]
    pub(crate) fn tables(&self) -> &InheritedTables {
        &self.tables
    }

    pub(crate) fn finish(self) -> InheritedFacts {
        let mut tables = self.tables;
        tables.subjects.shrink_to_fit();
        tables.reps.shrink_to_fit();
        tables.hops.shrink_to_fit();
        InheritedFacts {
            tables: Arc::new(tables),
            by_subject: self.by_subject,
        }
    }
}

#[cfg(test)]
impl InheritedFacts {
    /// Hand-stated inherited facts (tests): each owned fact becomes a rep
    /// (itself) and a first hop carrying its `via` and its three first-hop
    /// fields, interned by content under its key's subject. Materialized,
    /// each comes back equal to the input with `subject` = its key; so every
    /// input fact must be `provenance: "inherited"` (asserted).
    pub(crate) fn from_owned(map: &HashMap<String, Vec<CapabilityFact>>) -> InheritedFacts {
        let mut b = InheritedBuilder::default();
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for id in keys {
            let s = b.subject(id);
            let refs = map[id]
                .iter()
                .map(|f| {
                    assert_eq!(f.provenance, "inherited", "a hand-stated inherited fact");
                    let hop = FirstHop {
                        subject: s,
                        via: f.via,
                        callsite: f.witness_callsite_id.clone(),
                        operation_id: f.witness_first_hop_operation_id.clone(),
                        event_id: f.witness_first_hop_event_id.clone(),
                    };
                    let hop = match b.hop_ix.get(&hop) {
                        Some(&ix) => ix,
                        None => {
                            let ix = table_ix(b.tables.hops.len(), "hop");
                            b.tables.hops.push(hop.clone());
                            b.hop_ix.insert(hop, ix);
                            ix
                        }
                    };
                    InheritedRef {
                        rep: b.rep(&Arc::new(f.clone())),
                        hop,
                    }
                })
                .collect();
            b.insert(id.clone(), refs);
        }
        b.finish()
    }

    /// `id`'s facts, materialized; panics when `id` has no entry (tests read
    /// it as they indexed the old map).
    pub(crate) fn materialized(&self, id: &str) -> Vec<CapabilityFact> {
        self.get(id)
            .unwrap_or_else(|| panic!("no inherited entry for {id:?}"))
            .materialize()
    }
}

/// Views of refs against tables the caller holds (a test hook's reads).
#[cfg(test)]
pub(crate) fn views<'a>(
    tables: &'a InheritedTables,
    refs: &'a [InheritedRef],
) -> InheritedSlice<'a> {
    InheritedSlice { refs, tables }
}

/// Cone-memory step 1, T8 (ownership half): drains and borrows keep every
/// entry, empty ones included; a drain moves owned keys and shares the
/// tables; a view's subject borrows from the tables, never from the lookup.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::cone_derived::test_facts::fact;

    fn inherited(subject: &str, t: &str) -> CapabilityFact {
        let mut f = fact(subject, "insert", "table", Some(t));
        f.provenance = "inherited";
        f.via = "call";
        f.witness_callsite_id = Some(format!("{subject}/cs0"));
        f
    }

    #[test]
    fn t8_drain_and_borrow_keep_empties_and_share_the_tables() {
        let owned: HashMap<String, Vec<CapabilityFact>> = [
            ("X".to_string(), vec![inherited("X", "t/1")]),
            ("Y".to_string(), vec![inherited("Y", "t/2")]),
            ("E".to_string(), Vec::new()),
        ]
        .into();
        let mut all = InheritedFacts::from_owned(&owned);
        assert!(all.get("E").is_some_and(|s| s.is_empty()), "E: Some(empty)");
        assert!(all.get("Z").is_none(), "Z: never composed");

        let drained = all.take_subjects(["X", "E", "Z"]);
        let mut keys: Vec<&str> = drained.keys().collect();
        keys.sort();
        assert_eq!(keys, ["E", "X"], "the present entries, empty included");
        assert!(drained.get("E").is_some_and(|s| s.is_empty()));
        assert!(all.get("X").is_none() && all.get("E").is_none());
        assert!(all.get("Y").is_some_and(|s| s.len() == 1));
        assert!(Arc::ptr_eq(drained.tables(), all.tables()));
        assert_eq!(Arc::strong_count(all.tables()), 2);
        drop(all);
        assert_eq!(
            Arc::strong_count(drained.tables()),
            1,
            "the drain owns them now"
        );

        // The subject comes from the tables, not from the lookup key.
        let lookup = String::from("X");
        let v = drained.get(&lookup).unwrap().iter().next().unwrap();
        drop(lookup);
        let subject = v.subject();
        assert_eq!(subject, "X");
        let in_table = drained
            .tables()
            .subjects
            .iter()
            .find(|s| s.as_str() == "X")
            .unwrap();
        assert!(std::ptr::eq(subject.as_ptr(), in_table.as_ptr()));
        assert_eq!(v.materialize(), owned["X"][0].clone());
    }
}
