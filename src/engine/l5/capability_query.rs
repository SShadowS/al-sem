//! Capability-query helpers — faithful port of al-sem
//! `src/detectors/capability-query.ts`.
//!
//! Pure functions over a `FullRoutineSummary`. The tri-state helpers honour G6
//! coverage semantics: when a fact is absent AND the inherited cone is not
//! "complete", they return `Unknown` rather than `No` (absence of evidence is not
//! evidence of absence when the cone is partial / coverage data is missing).
//!
//! ## ⟨C1 Task 3 — R6⟩ What is left, and what was retired
//!
//! The al-sem-shaped RAW helpers — `find_capabilities`, `has_capability`,
//! `writes_tables_of`, `writes_physical_tables_of`, `publishes_events_of` — all
//! scanned `reachable(s) = capability_facts_direct ∪ capability_facts_inherited`.
//! Task 3 stops materializing the inherited half on the analyze path, so those
//! functions no longer have a correct input: left in place they would keep
//! compiling and silently return a DIRECT-ONLY view. They are **deleted**, not
//! demoted. Their production consumers had already moved to the pooled
//! [`ConeDerivedStore`] equivalents (`store.writes_tables_of(id)` and friends) in
//! Task 2, and their semantics — sorted, deduped, unresolved-id and foreign-kind
//! facts dropped, known-temp writes excluded from the physical set — are pinned by
//! `l4::cone_derived`'s own fold tests.
//!
//! What survives:
//!   - [`touches_db_derived`] / [`may_commit_derived`] — the LIVE tri-states.
//!     They take a `&ConeDerivedStore` for the presence half and the summary for
//!     the coverage half (the only two derived queries that need something off the
//!     summary as well as off the cone).
//!   - [`reachable_coverage`] — reads `coverage` only; never touched the facts.
//!   - [`fact_is_known_temp`] — the shared temp gate, re-exported from the fold.
//!   - `touches_db_of` / `may_commit`, kept `#[cfg(test)]` ONLY: they are the
//!     raw-scan oracle side of the two tri-states for hand-built fixture
//!     summaries (which carry their inherited facts as INPUT), notably d1's
//!     `touches_db_memoized` parity test. They are unreachable from shipping code
//!     by construction, and on a real derived-only summary they panic through
//!     `inherited_raw()` rather than answering direct-only.

use crate::engine::l4::cone_derived::ConeDerivedStore;
use crate::engine::l5::full_summary::FullRoutineSummary;

/// True when a capability fact is a write/read on a PROVABLY temporary record.
/// ⟨C1⟩ The implementation lives in `l4::cone_derived` (the fold applies the same
/// gate per fact); re-exported here so the raw helpers and the derived fold
/// cannot drift apart.
pub use crate::engine::l4::cone_derived::fact_is_known_temp;

/// Tri-state effect presence (al-sem `EffectPresence = "yes" | "no" | "unknown"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectPresence {
    Yes,
    No,
    Unknown,
}

/// ⟨C1 Task 3⟩ FIXTURE-ONLY raw oracle for [`may_commit_derived`]: `Yes` when any
/// fact in `direct ∪ inherited_raw` is a commit on the transaction resource;
/// otherwise the coverage arm. Mirrors al-sem `mayCommit`.
///
/// # Panics
/// Panics (via `FullRoutineSummary::inherited_raw`) on a summary whose cone ran
/// `DerivedOnly` — i.e. on anything the production path builds. That is the point:
/// it can only be applied to hand-built summaries that own their inherited facts.
#[cfg(test)]
pub fn may_commit(s: &FullRoutineSummary) -> EffectPresence {
    let found = s
        .capability_facts_direct
        .iter()
        .chain(s.inherited_raw())
        .any(|f| f.op == "commit" && f.resource_kind == "transaction");
    presence(found, s)
}

/// ⟨C1 Task 3⟩ FIXTURE-ONLY raw oracle for [`touches_db_derived`]: `Yes` when any
/// fact in `direct ∪ inherited_raw` has `resource_kind == "table"` (regardless of
/// op — read or write); otherwise the coverage arm. Mirrors al-sem `touchesDbOf`.
///
/// # Panics
/// See [`may_commit`].
#[cfg(test)]
pub fn touches_db_of(s: &FullRoutineSummary) -> EffectPresence {
    let found = s
        .capability_facts_direct
        .iter()
        .chain(s.inherited_raw())
        .any(|f| f.resource_kind == "table");
    presence(found, s)
}

/// Returns the routine's inherited coverage status (`coverage.inherited_status`),
/// or "unknown" when there is no coverage record. The optional `kind` is accepted
/// for al-sem signature parity but (as in al-sem Phase 1a) does NOT narrow — the
/// per-routine overall status is the only roll-up maintained. Mirrors al-sem
/// `reachableCoverage`.
pub fn reachable_coverage<'a>(s: &'a FullRoutineSummary, _kind: Option<&str>) -> &'a str {
    s.inherited_status()
}

// ===========================================================================
// ⟨C1⟩ DERIVED-substrate tri-states — the LIVE pair. The presence half reads the
// folded cone flag instead of scanning raw facts; the absence half is UNCHANGED
// (`coverage.inherited_status`, which C1 does not touch).
//
// ⟨C1 Task 2⟩ These two live HERE, not on the store, because they are the only
// derived queries that need something off the SUMMARY (`coverage`) as well as
// off the cone. Everything else a detector reads — the id-sets
// (`writes_tables_of` / `writes_physical_tables_of` / `physical_table_reads_of`
// / `physical_table_write_ops_of` / `publishes_events_of`) and the presence
// flags (`touches_table` / `may_commit_flag` / `touches_io`) — is a pure
// function of the routine id and is called directly on
// `ctx.cone_derived`. No consumer routes an id-set through this module.
// ===========================================================================

/// `touches_db_of` over the derived substrate. (`touches_db_of` is
/// `#[cfg(test)]`-only, so this cannot be an intra-doc link under a non-test
/// build — see M-2 in the C1 Task 3 review.)
pub fn touches_db_derived(store: &ConeDerivedStore, s: &FullRoutineSummary) -> EffectPresence {
    presence(store.touches_table(&s.routine_id), s)
}

/// `may_commit` over the derived substrate. (`may_commit` is `#[cfg(test)]`-only,
/// so this cannot be an intra-doc link under a non-test build — see M-2 in the
/// C1 Task 3 review.)
pub fn may_commit_derived(store: &ConeDerivedStore, s: &FullRoutineSummary) -> EffectPresence {
    presence(store.may_commit_flag(&s.routine_id), s)
}

/// The shared tri-state arm: present ⇒ `Yes`; absent ⇒ `No` only when the
/// inherited cone is "complete", else `Unknown` (G6 honesty).
fn presence(found: bool, s: &FullRoutineSummary) -> EffectPresence {
    if found {
        EffectPresence::Yes
    } else if s.inherited_status() == "complete" {
        EffectPresence::No
    } else {
        EffectPresence::Unknown
    }
}

// ===========================================================================
// Native oracles — ground-truth-free invariants on synthetic inputs.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact};
    use crate::engine::l5::test_support::{coverage, fact, summary};

    /// Build a `table` write fact carrying a `CapabilityExtra::Table` with the
    /// given temp_state kind/value — for the physical-vs-temp gate tests.
    fn temp_table_write_fact(resource_id: &str, kind: &str, value: Option<bool>) -> CapabilityFact {
        let mut f = fact("insert", "table", Some(resource_id));
        f.extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(crate::engine::l2::features::PTempState {
                kind: kind.to_string(),
                value,
                parameter_index: None,
            }),
            op_subtype: Some("Insert".to_string()),
        });
        f
    }

    #[test]
    fn fact_is_known_temp_is_exact() {
        assert!(fact_is_known_temp(&temp_table_write_fact(
            "t/A",
            "known",
            Some(true)
        )));
        assert!(!fact_is_known_temp(&temp_table_write_fact(
            "t/A",
            "known",
            Some(false)
        )));
        assert!(!fact_is_known_temp(&temp_table_write_fact(
            "t/A", "unknown", None
        )));
        // No extra at all → not known-temp.
        assert!(!fact_is_known_temp(&fact("insert", "table", Some("t/A"))));
        // #34: the rest of the CapabilityFact truth table.
        assert!(!fact_is_known_temp(&temp_table_write_fact(
            "t/A", "known", None
        )));
        assert!(!fact_is_known_temp(&temp_table_write_fact(
            "t/A",
            "unknown",
            Some(true)
        )));
        assert!(!fact_is_known_temp(&temp_table_write_fact(
            "t/A",
            "parameter-dependent",
            None
        )));
    }

    #[test]
    fn may_commit_yes_on_matching_fact() {
        let s = summary("r", vec![fact("commit", "transaction", None)], vec![], None);
        assert_eq!(may_commit(&s), EffectPresence::Yes);
    }

    #[test]
    fn may_commit_no_only_when_absent_and_complete() {
        // absent + complete → No
        let s = summary("r", vec![], vec![], Some(coverage("complete")));
        assert_eq!(may_commit(&s), EffectPresence::No);
        // absent + partial → Unknown
        let s = summary("r", vec![], vec![], Some(coverage("partial")));
        assert_eq!(may_commit(&s), EffectPresence::Unknown);
        // absent + no coverage record → Unknown
        let s = summary("r", vec![], vec![], None);
        assert_eq!(may_commit(&s), EffectPresence::Unknown);
    }

    #[test]
    fn touches_db_yes_no_unknown() {
        // any table fact (even a read) → Yes
        let s = summary(
            "r",
            vec![fact("read", "table", Some("t/A"))],
            vec![],
            Some(coverage("complete")),
        );
        assert_eq!(touches_db_of(&s), EffectPresence::Yes);
        // no table fact + complete → No
        let s = summary(
            "r",
            vec![fact("commit", "transaction", None)],
            vec![],
            Some(coverage("complete")),
        );
        assert_eq!(touches_db_of(&s), EffectPresence::No);
        // no table fact + partial → Unknown
        let s = summary("r", vec![], vec![], Some(coverage("partial")));
        assert_eq!(touches_db_of(&s), EffectPresence::Unknown);
    }

    /// ⟨C1 Task 3⟩ The two surviving raw oracles span `direct ∪ inherited_raw`
    /// — the property that made them a meaningful oracle side. Pinned here
    /// because the id-set helpers that used to carry it are deleted.
    #[test]
    fn raw_oracles_span_direct_and_inherited() {
        // The table fact lives ONLY in the inherited half.
        let s = summary(
            "r",
            vec![fact("send", "http", None)],
            vec![fact("modify", "table", Some("t/inherited"))],
            Some(coverage("complete")),
        );
        assert_eq!(touches_db_of(&s), EffectPresence::Yes);
        // The commit fact likewise.
        let s = summary(
            "r",
            vec![fact("send", "http", None)],
            vec![fact("commit", "transaction", None)],
            Some(coverage("complete")),
        );
        assert_eq!(may_commit(&s), EffectPresence::Yes);
    }

    /// ⟨C1 Task 3 — R6⟩ Applying a raw oracle to a summary whose cone was never
    /// materialized must PANIC, not quietly answer from the direct half alone.
    #[test]
    #[should_panic(expected = "RAW_INHERITED_FACTS")]
    fn raw_oracle_panics_on_a_derived_only_summary() {
        let s = FullRoutineSummary::new(
            "r".to_string(),
            vec![fact("send", "http", None)],
            None,
            Some(coverage("complete")),
        );
        let _ = touches_db_of(&s);
    }

    #[test]
    fn reachable_coverage_reports_inherited_status_or_unknown() {
        let s = summary("r", vec![], vec![], Some(coverage("complete")));
        assert_eq!(reachable_coverage(&s, None), "complete");
        let s = summary("r", vec![], vec![], None);
        assert_eq!(reachable_coverage(&s, Some("table")), "unknown");
    }
}
