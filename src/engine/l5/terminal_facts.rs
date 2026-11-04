//! The terminal-fact index (MCP output wave 2, W2-0a; design
//! `docs/superpowers/specs/2026-11-04-mcp-output-wave2-design.md` §3.4.1): one
//! LOSSLESS record per direct capability fact of a tree, resource-less ones
//! (COMMIT, UI, HTTP, FILE, …) included.
//!
//! A witness or a count for one digest row must end at a fact of that row's
//! kind AND class. A record therefore keeps everything the class depends on:
//! the source fact's identity (its index in `capability_facts`), the effect
//! type, the resource, the owner, the exact terminal (operation or call site),
//! the ORIGINAL operation kind (`Insert`, `ModifyAll`, `Validate`, …, the
//! fact's `opSubtype`; never collapsed into the digest's three write types)
//! and the temp state in the OWNER's frame. A record whose temp state is
//! parameter-dependent is a raw producer: its class at a caller comes from the
//! producer solve (`pd_solution`), never from the record, so a reader joins it
//! in the root's frame before it counts or ends a witness there (§3.4.1;
//! `digest::is_bfs_carrier` and `enumerate_proofs` are that association
//! today). Two facts of one table that differ only in temp class (a temporary
//! and a persistent `Insert`) are two records with two classes.
//!
//! Built once per read while the tree is alive, by
//! [`crate::engine::l5::literal_reach::LiteralReach::new`], whose
//! "is a kind dead in the root's reach" test reads it
//! ([`TerminalFacts::of_kind`]). It borrows the tree's fact rows; nothing is
//! copied.

use crate::engine::l5::snapshot::{SnapCapabilityExtra, SnapTempState, SnapshotCapabilityFact};
use smallvec::SmallVec;
use std::collections::HashMap;

/// The exact place a direct fact sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalSite<'a> {
    /// A record operation, commit or error site (`…/op{N}`).
    Operation(&'a str),
    /// A call site (`…/cs{N}`): an HTTP call, a dialog, a dispatch, ….
    Callsite(&'a str),
    /// The fact names neither.
    None,
}

impl<'a> TerminalSite<'a> {
    /// The site's id; `""` for none (what the literal pass's guard lookup
    /// reads).
    pub(crate) fn id(&self) -> &'a str {
        match *self {
            TerminalSite::Operation(s) | TerminalSite::Callsite(s) => s,
            TerminalSite::None => "",
        }
    }
}

/// A record's temp state in its owner's frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalTemp {
    Known(bool),
    Unknown,
    /// A raw producer: the class at a caller comes from the producer solve.
    ParameterDependent(u32),
    /// Not a table fact, or a table fact with no temp state.
    Absent,
}

/// One direct fact, kept whole (module doc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalRecord<'a> {
    /// The fact's index in the tree's `capability_facts`.
    pub fact: usize,
    pub effect_type: &'static str,
    pub resource: Option<&'a str>,
    /// The fact's subject: the stable id of the routine the fact sits in.
    pub owner: &'a str,
    pub terminal: TerminalSite<'a>,
    /// The original operation kind (`opSubtype`), for a table fact.
    pub op_kind: Option<&'a str>,
    pub temp: TerminalTemp,
}

impl TerminalRecord<'_> {
    /// A raw producer (module doc). Read by W2-0b; until then by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn producer(&self) -> bool {
        matches!(self.temp, TerminalTemp::ParameterDependent(_))
    }
}

/// The records of one tree, by kind and by terminal id.
pub(crate) struct TerminalFacts<'a> {
    records: Vec<TerminalRecord<'a>>,
    /// Effect type → resource id (`""`: none; a resource id is never empty) →
    /// the records. Two levels keyed by `&str`, so a lookup borrows nothing
    /// from its key.
    by_kind: HashMap<&'static str, HashMap<&'a str, Vec<u32>>>,
    by_terminal: HashMap<&'a str, SmallVec<[u32; 1]>>,
}

fn temp_of(f: &SnapshotCapabilityFact) -> (Option<&str>, TerminalTemp) {
    match f.extra() {
        Some(SnapCapabilityExtra::Table {
            op_subtype,
            temp_state,
            ..
        }) => (
            op_subtype.as_deref(),
            match temp_state {
                Some(SnapTempState::Known { value }) => TerminalTemp::Known(*value),
                Some(SnapTempState::Unknown) => TerminalTemp::Unknown,
                Some(SnapTempState::ParameterDependent { parameter_index }) => {
                    TerminalTemp::ParameterDependent(*parameter_index)
                }
                None => TerminalTemp::Absent,
            },
        ),
        _ => (None, TerminalTemp::Absent),
    }
}

impl<'a> TerminalFacts<'a> {
    /// One record per direct fact of `facts` with an effect type
    /// ([`crate::engine::l5::digest::effect_type_of`]), in fact order.
    pub(crate) fn new(facts: &'a [SnapshotCapabilityFact]) -> Self {
        let mut out = TerminalFacts {
            records: Vec::new(),
            by_kind: HashMap::new(),
            by_terminal: HashMap::new(),
        };
        for (i, f) in facts.iter().enumerate() {
            if f.provenance() != "direct" {
                continue;
            }
            let Some(effect_type) = crate::engine::l5::digest::effect_type_of(f) else {
                continue;
            };
            let terminal = match (f.witness_operation_id(), f.witness_callsite_id()) {
                (Some(op), _) => TerminalSite::Operation(op),
                (None, Some(cs)) => TerminalSite::Callsite(cs),
                (None, None) => TerminalSite::None,
            };
            let (op_kind, temp) = temp_of(f);
            let ix = u32::try_from(out.records.len()).expect("a record index fits u32");
            out.by_kind
                .entry(effect_type)
                .or_default()
                .entry(f.resource_id().unwrap_or(""))
                .or_default()
                .push(ix);
            if terminal != TerminalSite::None {
                out.by_terminal.entry(terminal.id()).or_default().push(ix);
            }
            out.records.push(TerminalRecord {
                fact: i,
                effect_type,
                resource: f.resource_id(),
                owner: f.subject(),
                terminal,
                op_kind,
                temp,
            });
        }
        out
    }

    /// Every record, in fact order. Read by W2-0b; until then by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn records(&self) -> &[TerminalRecord<'a>] {
        &self.records
    }

    /// The records of `kind` (effect type, resource id), in fact order.
    pub(crate) fn of_kind<'s>(
        &'s self,
        kind: (&str, Option<&str>),
    ) -> impl Iterator<Item = &'s TerminalRecord<'a>> + use<'s, 'a> {
        let ixs: &'s [u32] = self
            .by_kind
            .get(kind.0)
            .and_then(|m| m.get(kind.1.unwrap_or("")))
            .map_or(&[], Vec::as_slice);
        ixs.iter().map(|&i| &self.records[i as usize])
    }

    /// The records sitting at terminal `id` (an operation or call-site id).
    /// Read by W2-0b; until then by the tests.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn at<'s>(
        &'s self,
        id: &str,
    ) -> impl Iterator<Item = &'s TerminalRecord<'a>> + use<'s, 'a> {
        let ixs: &'s [u32] = self.by_terminal.get(id).map_or(&[], SmallVec::as_slice);
        ixs.iter().map(|&i| &self.records[i as usize])
    }
}
