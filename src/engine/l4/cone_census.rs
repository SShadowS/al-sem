//! ⟨C1 residual census⟩ `C1_CONE_CENSUS=1` — an env-gated, value-tested
//! (mirrors `REGEN_TEMP_GOLDENS`'s `=1` contract, not presence-tested) one-shot
//! byte census of what `context.capability_cones` still retains after Task 3
//! stopped materializing the per-routine raw `Vec<CapabilityFact>` cone (that
//! Vec dropped the span's `rss_delta` from 10,941 MB to 2,151 MB on the 8020
//! corpus — see `capability_cone.rs`'s module doc / `cone_derived.rs`). This
//! module answers "what is the remaining 2,151 MB made of" by MEASUREMENT
//! rather than by choosing between the two standing hypotheses (H1:
//! `CoverageRecord.unknown_targets` reproduces the same per-routine-O(cone)
//! pathology the raw Vec had; H2: `capability_facts_direct` itself, which was
//! never touched by Task 3, is simply large on its own).
//!
//! Purely diagnostic: `enabled()` is a single `OnceLock` env read, matching
//! `perf_trace`'s disabled-path contract — no output change, no behavior
//! change, no allocation at all when `C1_CONE_CENSUS` is unset.
//!
//! ## Accounting convention (read this before reading the numbers)
//!
//! Every byte is classified into exactly one of two buckets, and every printed
//! total says which bucket(s) it sums:
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
//! headers inline (Rust never boxes a plain struct field), so
//! `summaries_entry_struct_bytes` (`entries * size_of::<FullRoutineSummary>()`)
//! already counts one full `CoverageRecord`'s inline shape (3 `String` headers
//! plus 2 `Vec<String>` headers) per routine.
//!
//! The `CoverageRecord` section below therefore reports its own
//! `size_of::<CoverageRecord>()` line as informational only (marked
//! `[informational]`, already counted above, NOT additive), and treats only
//! the string CONTENT plus the `Vec<String>` backing buffers (the array of
//! `String` headers the Vec header points to — a separate allocation from the
//! Vec header itself) as the additive contribution.
//!
//! `capability_facts_direct`'s `Vec<CapabilityFact>` header is likewise
//! inline/already-counted; its backing buffer (`fact_struct_bytes`) is the
//! separate, additive allocation. `grand_total_bytes` sums exactly the
//! additive lines, never the informational ones, so it does not overlap
//! itself.
//!
//! HashMap container cost is approximated, not modeled exactly:
//! `entries * size_of::<(String, FullRoutineSummary)>()` ignores hashbrown's
//! one-control-byte-per-slot overhead and its load-factor slack (typically
//! another ~15-30% on top). Flagged inline; not worth modeling precisely for
//! an attribution census.
//!
//! Capacity vs length: every collection is measured by `len()`, never
//! `capacity()` — this under-counts by whatever slack the allocator left,
//! usually small relative to the totals here.

use std::collections::HashMap;
use std::mem::size_of;
use std::sync::OnceLock;

use crate::engine::l4::capability_cone::{
    CapabilityExtra, CapabilityFact, CoverageRecord, ValueSource,
};
use crate::engine::l4::cone_derived::ConeDerivedStore;
use crate::engine::l5::full_summary::FullRoutineSummary;

/// True when `C1_CONE_CENSUS=1` (value-tested — `=0`/absent/anything else is
/// off, matching `REGEN_TEMP_GOLDENS`'s convention, NOT a presence test).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("C1_CONE_CENSUS").as_deref() == Ok("1"))
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

// ---------------------------------------------------------------------------
// Recursive heap-byte accounting for one `CapabilityFact` (shared with the
// `fact_cones`/`cov_cones` residual census AND the `direct` dedup-keyed
// census, both in `capability_cone.rs` — one definition so both census sites
// agree on what a fact "costs"). A third call site, the `direct_in`
// transient-duplicate census in `detector_context.rs`, was deleted alongside
// `direct_in` itself (Task 4) — see the tombstone below at
// `emit_direct_in_residual`'s old location.
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

// ---------------------------------------------------------------------------
// The main census: `summaries` (capability_facts_direct + CoverageRecord) +
// `ConeDerivedStore` + the summaries map's own container overhead.
// ---------------------------------------------------------------------------

/// Emit the full `[C1_CONE_CENSUS]` report for the `summaries` map + the
/// derived cone substrate, once, when `build_detector_context`'s cone build
/// completes. No-op when [`enabled`] is false.
pub fn emit_full_census(
    summaries: &HashMap<String, FullRoutineSummary>,
    cone_derived: &ConeDerivedStore,
) {
    if !enabled() {
        return;
    }

    let routine_count = summaries.len() as u64;

    let mut fact_count: u64 = 0;
    let mut fact_struct_bytes: u64 = 0;
    let mut fact_heap_bytes: u64 = 0;
    let mut routine_id_field_heap_bytes: u64 = 0;

    let mut coverage_present: u64 = 0;
    let mut coverage_subject_status_heap: u64 = 0;
    let mut reasons_count: u64 = 0;
    let mut reasons_content_bytes: u64 = 0;
    let mut unknown_count_total: u64 = 0;
    let mut unknown_content_bytes: u64 = 0;
    let mut unknown_max: usize = 0;

    for s in summaries.values() {
        routine_id_field_heap_bytes += s.routine_id.len() as u64;
        let (sb, hb) = facts_bytes(&s.capability_facts_direct);
        fact_count += s.capability_facts_direct.len() as u64;
        fact_struct_bytes += sb;
        fact_heap_bytes += hb;

        if let Some(cov) = &s.coverage {
            coverage_present += 1;
            coverage_subject_status_heap += cov.subject.len() as u64
                + cov.direct_status.len() as u64
                + cov.inherited_status.len() as u64;
            reasons_count += cov.reasons.len() as u64;
            reasons_content_bytes += cov.reasons.iter().map(|s| s.len() as u64).sum::<u64>();
            let ulen = cov.unknown_targets.len();
            unknown_count_total += ulen as u64;
            unknown_content_bytes += cov
                .unknown_targets
                .iter()
                .map(|s| s.len() as u64)
                .sum::<u64>();
            unknown_max = unknown_max.max(ulen);
        }
    }

    let reasons_vec_buffer_bytes = reasons_count * size_of::<String>() as u64;
    let unknown_vec_buffer_bytes = unknown_count_total * size_of::<String>() as u64;
    let coverage_informational_struct_bytes = coverage_present * size_of::<CoverageRecord>() as u64;
    let unknown_mean_per_covered_routine = if coverage_present > 0 {
        unknown_count_total as f64 / coverage_present as f64
    } else {
        0.0
    };

    // HashMap<String, FullRoutineSummary> container — approximated (hashbrown
    // control bytes/load factor not modeled, see module doc).
    let map_key_heap_bytes: u64 = summaries.keys().map(|k| k.len() as u64).sum();
    let map_entry_struct_bytes =
        routine_count * (size_of::<String>() + size_of::<FullRoutineSummary>()) as u64;

    let cd = cone_derived.census();
    let cd_total_bytes = cd.rows_key_heap_bytes
        + cd.rows_struct_bytes
        + cd.interner_heap_bytes
        + cd.writes_all_bytes
        + cd.phys_writes_bytes
        + cd.phys_reads_bytes
        + cd.events_bytes;

    let grand_total_bytes = map_entry_struct_bytes
        + map_key_heap_bytes
        + routine_id_field_heap_bytes
        + fact_struct_bytes
        + fact_heap_bytes
        + coverage_subject_status_heap
        + reasons_vec_buffer_bytes
        + reasons_content_bytes
        + unknown_vec_buffer_bytes
        + unknown_content_bytes
        + cd_total_bytes;

    eprintln!("[C1_CONE_CENSUS] ==================================================");
    eprintln!("[C1_CONE_CENSUS] C1 residual census — context.capability_cones");
    eprintln!(
        "[C1_CONE_CENSUS] convention: struct_bytes=count*size_of::<T>() (own backing-buffer \
         footprint); heap_bytes=OWNED String content (len(), not capacity), recursing through \
         Option/Box/enum payloads — a &'static str field owns nothing and is NOT counted \
         (C1 Task 4). Lines marked [informational] are NOT in grand_total (see module doc — \
         they are already embedded inline in a struct counted elsewhere)."
    );
    eprintln!("[C1_CONE_CENSUS] routine_count={routine_count}");
    eprintln!("[C1_CONE_CENSUS] --------------------------------------------------");
    eprintln!(
        "[C1_CONE_CENSUS] section: capability_facts_direct (FullRoutineSummary.capability_facts_direct)"
    );
    eprintln!("[C1_CONE_CENSUS]   fact_count={fact_count}");
    eprintln!(
        "[C1_CONE_CENSUS]   fact_struct_bytes={fact_struct_bytes} ({:.2} MB)  # {fact_count} * size_of::<CapabilityFact>()={} B",
        mb(fact_struct_bytes),
        size_of::<CapabilityFact>()
    );
    eprintln!(
        "[C1_CONE_CENSUS]   fact_heap_bytes={fact_heap_bytes} ({:.2} MB)",
        mb(fact_heap_bytes)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   fact_total_bytes={} ({:.2} MB)  # struct+heap, additive",
        fact_struct_bytes + fact_heap_bytes,
        mb(fact_struct_bytes + fact_heap_bytes)
    );
    eprintln!("[C1_CONE_CENSUS] --------------------------------------------------");
    eprintln!("[C1_CONE_CENSUS] section: CoverageRecord (FullRoutineSummary.coverage)");
    eprintln!(
        "[C1_CONE_CENSUS]   coverage_present={coverage_present} (of {routine_count} routines)"
    );
    eprintln!(
        "[C1_CONE_CENSUS]   coverage_struct_bytes={coverage_informational_struct_bytes} ({:.2} MB)  # {coverage_present} * size_of::<CoverageRecord>()={} B [informational — already inside summaries_entry_struct_bytes below]",
        mb(coverage_informational_struct_bytes),
        size_of::<CoverageRecord>()
    );
    eprintln!(
        "[C1_CONE_CENSUS]   coverage_subject_status_heap_bytes={coverage_subject_status_heap} ({:.2} MB)  # subject+direct_status+inherited_status content",
        mb(coverage_subject_status_heap)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   reasons_count={reasons_count} reasons_vec_buffer_bytes={reasons_vec_buffer_bytes} reasons_content_bytes={reasons_content_bytes} ({:.2} MB total)",
        mb(reasons_vec_buffer_bytes + reasons_content_bytes)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   unknown_targets_count={unknown_count_total} unknown_targets_vec_buffer_bytes={unknown_vec_buffer_bytes} unknown_targets_content_bytes={unknown_content_bytes} ({:.2} MB total)",
        mb(unknown_vec_buffer_bytes + unknown_content_bytes)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   unknown_targets_max_per_routine={unknown_max} unknown_targets_mean_per_covered_routine={unknown_mean_per_covered_routine:.4}"
    );
    let coverage_additive_total = coverage_subject_status_heap
        + reasons_vec_buffer_bytes
        + reasons_content_bytes
        + unknown_vec_buffer_bytes
        + unknown_content_bytes;
    eprintln!(
        "[C1_CONE_CENSUS]   coverage_additive_total_bytes={coverage_additive_total} ({:.2} MB)  # content only — struct shape counted via summaries_entry_struct_bytes",
        mb(coverage_additive_total)
    );
    eprintln!("[C1_CONE_CENSUS] --------------------------------------------------");
    eprintln!("[C1_CONE_CENSUS] section: ConeDerivedStore");
    eprintln!(
        "[C1_CONE_CENSUS]   rows={} rows_key_heap_bytes={} rows_struct_bytes={} ({:.2} MB)",
        cd.rows,
        cd.rows_key_heap_bytes,
        cd.rows_struct_bytes,
        mb(cd.rows_key_heap_bytes + cd.rows_struct_bytes)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   interner_strings={} interner_heap_bytes={} ({:.2} MB)  # BOTH copies (strings Vec + by_str reverse-map keys)",
        cd.interner_strings,
        cd.interner_heap_bytes,
        mb(cd.interner_heap_bytes)
    );
    eprintln!(
        "[C1_CONE_CENSUS]   writes_all_pool_len={} bytes={} phys_writes_pool_len={} bytes={} phys_reads_pool_len={} bytes={} events_pool_len={} bytes={}",
        cd.writes_all_len,
        cd.writes_all_bytes,
        cd.phys_writes_len,
        cd.phys_writes_bytes,
        cd.phys_reads_len,
        cd.phys_reads_bytes,
        cd.events_len,
        cd.events_bytes
    );
    eprintln!(
        "[C1_CONE_CENSUS]   cone_derived_total_bytes={cd_total_bytes} ({:.2} MB)",
        mb(cd_total_bytes)
    );
    eprintln!("[C1_CONE_CENSUS] --------------------------------------------------");
    eprintln!(
        "[C1_CONE_CENSUS] section: summaries map container (HashMap<String, FullRoutineSummary>)"
    );
    eprintln!(
        "[C1_CONE_CENSUS]   entries={routine_count} map_key_heap_bytes={map_key_heap_bytes} routine_id_field_heap_bytes={routine_id_field_heap_bytes}  # TWO separate owned copies of the same routine-id content (the HashMap key + FullRoutineSummary.routine_id)"
    );
    eprintln!(
        "[C1_CONE_CENSUS]   map_entry_struct_bytes={map_entry_struct_bytes} ({:.2} MB)  # {routine_count} * (size_of::<String>()+size_of::<FullRoutineSummary>()) = {routine_count} * {} B — APPROXIMATE, hashbrown control-byte/load-factor overhead NOT modeled; this line already embeds one CoverageRecord shape + the capability_facts_direct/inherited Vec headers per entry",
        mb(map_entry_struct_bytes),
        size_of::<String>() + size_of::<FullRoutineSummary>()
    );
    eprintln!(
        "[C1_CONE_CENSUS]   size_of::<FullRoutineSummary>()={} B  size_of::<CapabilityFact>()={} B  size_of::<CoverageRecord>()={} B",
        size_of::<FullRoutineSummary>(),
        size_of::<CapabilityFact>(),
        size_of::<CoverageRecord>()
    );
    eprintln!("[C1_CONE_CENSUS] --------------------------------------------------");
    eprintln!(
        "[C1_CONE_CENSUS] grand_total_bytes={grand_total_bytes} ({:.2} MB)  # map_entry_struct_bytes + map_key_heap_bytes + routine_id_field_heap_bytes + fact_total_bytes + coverage_additive_total_bytes + cone_derived_total_bytes",
        mb(grand_total_bytes)
    );
    eprintln!("[C1_CONE_CENSUS] ==================================================");
}

// ⟨C1 Task 4⟩ `emit_direct_in_residual` lived here: a census of the
// `direct_in` map `build_detector_context` cloned alongside `direct_full` (one
// copy fed `compose_cone_over_graph`, the other was moved into the summaries)
// and then held, dead, for the rest of the `if need_summaries` block — 79.66 MB
// of pure duplicate on the 8020 corpus. Task 4 deleted the clone itself: the
// walk now reads `direct_full` directly, so there is no second map left to
// measure. Removed rather than kept pointing at `direct_full`, which is a
// LIVE structure already reported by [`emit_full_census`]'s
// `capability_facts_direct` section.
