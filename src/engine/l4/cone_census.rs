//! ⟨C1 residual census⟩ The per-fact byte accounting of the capability cone,
//! and its two residual-census sites in `capability_cone.rs`, gated on
//! `ALSEM_RETAINED_CENSUS=1` (#160 spec §4.1.5; value-tested, mirroring
//! `REGEN_TEMP_GOLDENS`'s `=1` contract, not presence-tested). It answered
//! "what is the remaining 2,151 MB of `context.capability_cones` made of" on
//! the 8020 corpus by MEASUREMENT (see `capability_cone.rs`'s module doc /
//! `cone_derived.rs`).
//!
//! Until #160 this module printed its own `[C1_CONE_CENSUS]` report under
//! `C1_CONE_CENSUS=1`. That flag and report are gone: the retained census
//! (`crate::engine::census`) prints `context.summaries` and
//! `context.cone_derived` as buckets, and [`summaries_tally`] below is the
//! old report's grand total for the summaries map.
//!
//! Purely diagnostic: `enabled()` is a single `OnceLock` env read, matching
//! `perf_trace`'s disabled-path contract — no output change, no behavior
//! change, no allocation at all when the flag is unset.
//!
//! ## Accounting convention (read this before reading the numbers)
//!
//! Every byte is classified into exactly one of two buckets:
//!   - **`struct_bytes`** — `count * size_of::<T>()`, the STACK-SHAPE footprint
//!     of `T` sitting inside its OWN container's backing buffer (a `Vec<T>`'s
//!     element array, a `HashMap<K, V>`'s bucket table). This is a real,
//!     separate heap allocation distinct from the container it lives in — the
//!     Vec header itself is a further 24 B living in the PARENT struct, not
//!     counted twice here (see the "no double-counting" note below).
//!   - **`heap_bytes`** — content actually pointed to and OWNED:
//!     `String::len()` (never capacity — a length-only convention, per
//!     instruction), recursing through every `Option`/`Box`/enum payload
//!     (`ValueSource`, `CapabilityExtra`) to their own owned strings. A
//!     `&'static str` field is NOT counted (⟨C1 Task 4⟩ — see
//!     `capability_fact_heap_bytes`): its bytes live once in the binary's
//!     read-only data, not once per value.
//!
//! No double-counting, by construction. `FullRoutineSummary` embeds its
//! `Option<CoverageRecord>` and its `Vec<CapabilityFact>`/`Option<Vec<_>>`
//! headers inline (Rust never boxes a plain struct field), so the map's entry
//! bytes (`entries * size_of::<FullRoutineSummary>()`) already count one full
//! `CoverageRecord`'s inline shape per routine; only the string CONTENT and
//! the `Vec<String>` backing buffers are added for it.
//!
//! HashMap container cost is approximated, not modeled exactly:
//! `entries * size_of::<(String, FullRoutineSummary)>()` ignores hashbrown's
//! one-control-byte-per-slot overhead and its load-factor slack (typically
//! another ~15-30% on top).
//!
//! Capacity vs length: every collection is measured by `len()`, never
//! `capacity()` — this under-counts by whatever slack the allocator left,
//! usually small relative to the totals here.

use std::collections::HashMap;
use std::mem::size_of;

use crate::engine::census::Tally;
use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact, ValueSource};
use crate::engine::l5::full_summary::FullRoutineSummary;

/// True when `ALSEM_RETAINED_CENSUS=1` (value-tested — `=0`/absent/anything
/// else is off, matching `REGEN_TEMP_GOLDENS`'s convention, NOT a presence test).
pub fn enabled() -> bool {
    crate::engine::census::enabled()
}

// ---------------------------------------------------------------------------
// Recursive heap-byte accounting for one `CapabilityFact` (shared with the
// `fact_cones`/`cov_cones` residual census AND the `direct` dedup-keyed
// census, both in `capability_cone.rs`, and with `pd_solve`'s byte functions
// — one definition so every census site agrees on what a fact "costs"). A
// further call site, the `direct_in` transient-duplicate census in
// `detector_context.rs`, was deleted alongside `direct_in` itself (Task 4).
// ---------------------------------------------------------------------------

/// Owned heap bytes reachable from one [`ValueSource`] (recurses through
/// `ConstantVar`'s boxed initializer, counting the box's own
/// `size_of::<ValueSource>()` allocation plus its content).
pub(crate) fn value_source_heap_bytes(vs: &ValueSource) -> u64 {
    match vs {
        ValueSource::Literal { value } => value.len() as u64,
        ValueSource::Enum { enum_name, member } => {
            enum_name.len() as u64 + member.as_ref().map_or(0, |m| m.len() as u64)
        }
        ValueSource::ConstantVar {
            var_name,
            initializer,
        } => {
            var_name.len() as u64
                + size_of::<ValueSource>() as u64 // the Box's own heap allocation
                + value_source_heap_bytes(initializer)
        }
        ValueSource::Parameter { var_name, .. } => var_name.len() as u64,
        ValueSource::TableField {
            table_id,
            field_name,
        } => table_id.len() as u64 + field_name.len() as u64,
        ValueSource::Expression | ValueSource::Unknown => 0,
    }
}

/// Owned heap bytes reachable from one [`CapabilityExtra`].
pub(crate) fn capability_extra_heap_bytes(e: &CapabilityExtra) -> u64 {
    match e {
        CapabilityExtra::Table {
            record_variable_id,
            temp_state,
            op_subtype,
        } => {
            record_variable_id.as_ref().map_or(0, |s| s.len() as u64)
                + temp_state.as_ref().map_or(0, |ts| ts.kind.len() as u64)
                + op_subtype.as_ref().map_or(0, |s| s.len() as u64)
        }
        CapabilityExtra::Dispatch { object_type, .. } => object_type.len() as u64,
        CapabilityExtra::Event { event_class, .. } => event_class.len() as u64,
        CapabilityExtra::Http {
            method,
            body_arg_source,
        } => method.len() as u64 + body_arg_source.as_ref().map_or(0, value_source_heap_bytes),
        CapabilityExtra::Storage {
            key_arg_source,
            value_arg_source,
            scope,
        } => {
            key_arg_source.as_ref().map_or(0, value_source_heap_bytes)
                + value_arg_source.as_ref().map_or(0, value_source_heap_bytes)
                + scope.as_ref().map_or(0, |s| s.len() as u64)
        }
    }
}

/// Owned heap bytes for one [`CapabilityFact`] — every OWNED
/// `String`/`Option<String>` field plus the recursive
/// `resource_arg_source`/`extra` payloads. Does NOT include
/// `size_of::<CapabilityFact>()` itself (that is the caller's `struct_bytes`
/// line, counted once per element in the owning `Vec`'s backing buffer).
///
/// ⟨C1 Task 4⟩ `op` / `resource_kind` / `confidence` / `provenance` / `via` are
/// deliberately NOT counted any more: they are `&'static str` since Task 4, so
/// their content lives once in the binary's read-only data and is shared by
/// every fact in the process. Charging each fact for its length would inflate
/// this total with bytes no fact owns — and would hide exactly the allocation
/// the shrink removed. Their 16 B pointer+len pair is already inside
/// `size_of::<CapabilityFact>()`.
pub(crate) fn capability_fact_heap_bytes(f: &CapabilityFact) -> u64 {
    f.subject.len() as u64
        + f.resource_id.as_ref().map_or(0, |s| s.len() as u64)
        + f.witness_operation_id
            .as_ref()
            .map_or(0, |s| s.len() as u64)
        + f.witness_callsite_id.as_ref().map_or(0, |s| s.len() as u64)
        + f.witness_first_hop_operation_id
            .as_ref()
            .map_or(0, |s| s.len() as u64)
        + f.witness_first_hop_event_id
            .as_ref()
            .map_or(0, |s| s.len() as u64)
        + f.resource_arg_source
            .as_ref()
            .map_or(0, value_source_heap_bytes)
        + f.extra.as_ref().map_or(0, capability_extra_heap_bytes)
}

/// `(struct_bytes, heap_bytes)` for a slice of facts — `struct_bytes` is the
/// facts' own backing-buffer footprint (`len() * size_of::<CapabilityFact>()`),
/// `heap_bytes` the sum of [`capability_fact_heap_bytes`] over each element.
pub(crate) fn facts_bytes(facts: &[CapabilityFact]) -> (u64, u64) {
    let struct_bytes = facts.len() as u64 * size_of::<CapabilityFact>() as u64;
    let heap_bytes: u64 = facts.iter().map(capability_fact_heap_bytes).sum();
    (struct_bytes, heap_bytes)
}

/// The `summaries` map's bytes by the convention above (the old
/// `[C1_CONE_CENSUS]` report's grand total, minus `cone_derived`, which is its
/// own bucket): the map's entries and keys, each summary's `routine_id`, its
/// direct facts, its raw inherited facts when the cone kept them, and its
/// coverage record's string content and `Vec<String>` buffers.
pub(crate) fn summaries_tally(summaries: &HashMap<String, FullRoutineSummary>) -> Tally {
    let mut t = Tally::default();
    t.buf::<(String, FullRoutineSummary)>(summaries.len())
        .strs(summaries.keys());
    let facts = |t: &mut Tally, f: &[CapabilityFact]| {
        let (sb, hb) = facts_bytes(f);
        t.bytes += (sb + hb) as usize;
        t.allocs += usize::from(!f.is_empty());
    };
    for s in summaries.values() {
        t.strs([&s.routine_id]);
        facts(&mut t, &s.capability_facts_direct);
        let (sb, hb) = s.census_inherited_bytes();
        t.bytes += (sb + hb) as usize;
        t.allocs += usize::from(sb > 0);
        if let Some(cov) = &s.coverage {
            t.strs([&cov.subject, &cov.direct_status, &cov.inherited_status])
                .buf::<String>(cov.reasons.len())
                .strs(&cov.reasons)
                .buf::<String>(cov.unknown_targets.len())
                .strs(&cov.unknown_targets);
        }
    }
    t
}
