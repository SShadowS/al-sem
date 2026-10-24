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
//!     summary as well as off the cone). ⟨issue 100⟩ [`touches_physical_table_derived`]
//!     is d2's variant (known-temp non-read facts do not count), with its
//!     `#[cfg(test)]` raw oracle `touches_physical_table_of`.
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
/// with `raw: WitnessScope::None` — i.e. on anything the production path builds. That is the point:
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

/// ⟨issue 100⟩ FIXTURE-ONLY raw oracle for [`touches_physical_table_derived`]:
/// `Yes` when any fact in `direct ∪ inherited_raw` passes
/// `cone_derived::fact_sets_physical_table_bit` (the fold's own rule); otherwise
/// the coverage arm.
///
/// # Panics
/// See [`may_commit`].
#[cfg(test)]
pub fn touches_physical_table_of(s: &FullRoutineSummary) -> EffectPresence {
    let found = s
        .capability_facts_direct
        .iter()
        .chain(s.inherited_raw())
        .any(crate::engine::l4::cone_derived::fact_sets_physical_table_bit);
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

/// ⟨issue 100⟩ d2's pruning tri-state: like [`touches_db_derived`], but a
/// known-temporary non-read fact does not count as present
/// (`ConeDerivedStore::touches_physical_table`). An incomplete cone is still
/// `Unknown`, so d2 still walks it. d1 keeps [`touches_db_derived`].
pub fn touches_physical_table_derived(
    store: &ConeDerivedStore,
    s: &FullRoutineSummary,
) -> EffectPresence {
    presence(store.touches_physical_table(&s.routine_id), s)
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
            temp_state: Some(crate::program::body::features::PTempState {
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

    #[test]
    fn reachable_coverage_reports_inherited_status_or_unknown() {
        let s = summary("r", vec![], vec![], Some(coverage("complete")));
        assert_eq!(reachable_coverage(&s, None), "complete");
        let s = summary("r", vec![], vec![], None);
        assert_eq!(reachable_coverage(&s, Some("table")), "unknown");
    }
}

/// Issue #100 acceptance row A2d (unit pins): d2's own presence bit
/// `TOUCHES_PHYSICAL_TABLE`. A `table` fact sets it UNLESS it is known-temp AND
/// its op is not `read`. The fold (`cone_derived::fold_fact`) and the raw oracle
/// `touches_physical_table_of` share ONE predicate,
/// `cone_derived::fact_sets_physical_table_bit`. These pins name symbols that do
/// not exist before #100's implementation, so until then this module is the
/// `--lib` target's compile failure.
#[cfg(test)]
mod issue100_physical_table_bit_tests {
    use super::*;
    use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact};
    use crate::engine::l4::cone_derived::{
        TOUCHES_PHYSICAL_TABLE, TOUCHES_TABLE, fact_sets_physical_table_bit,
    };
    use crate::engine::l5::test_support::{cone_store_of, coverage, fact, summary};
    use std::collections::HashMap;

    /// A table fact with an explicit temp state.
    fn table_fact(op: &'static str, rid: Option<&str>, value: Option<bool>) -> CapabilityFact {
        let mut f = fact(op, "table", rid);
        f.extra = Some(CapabilityExtra::Table {
            record_variable_id: None,
            temp_state: Some(crate::program::body::features::PTempState {
                kind: if value.is_some() { "known" } else { "unknown" }.to_string(),
                value,
                parameter_index: None,
            }),
            op_subtype: None,
        });
        f
    }

    fn known_temp(op: &'static str) -> CapabilityFact {
        table_fact(op, Some("t/A"), Some(true))
    }

    /// The folded flags of a routine whose ONLY reachable fact is `f`.
    fn flags_with_only(f: CapabilityFact) -> u8 {
        let mut m = HashMap::new();
        m.insert(
            "r".to_string(),
            summary("r", vec![f], vec![], Some(coverage("complete"))),
        );
        cone_store_of(&m).flags_of("r")
    }

    #[test]
    fn known_temp_non_reads_set_touches_table_but_not_the_physical_bit() {
        for op in ["rename", "lock", "modify"] {
            let flags = flags_with_only(known_temp(op));
            assert_ne!(flags & TOUCHES_TABLE, 0, "{op}: TOUCHES_TABLE must be set");
            assert_eq!(
                flags & TOUCHES_PHYSICAL_TABLE,
                0,
                "{op}: a known-temp non-read must not set TOUCHES_PHYSICAL_TABLE"
            );
        }
    }

    #[test]
    fn known_temp_read_sets_both_bits() {
        let flags = flags_with_only(known_temp("read"));
        assert_ne!(flags & TOUCHES_TABLE, 0);
        assert_ne!(
            flags & TOUCHES_PHYSICAL_TABLE,
            0,
            "a known-temp read (e.g. a temp host's CalcFields) keeps the physical bit"
        );
    }

    #[test]
    fn non_temp_fact_without_resource_id_sets_both_bits() {
        // Set BEFORE the unresolved-resource-id early return, like TOUCHES_TABLE.
        for f in [
            table_fact("modify", None, None),
            table_fact("modify", None, Some(false)),
            fact("rename", "table", None),
        ] {
            let flags = flags_with_only(f.clone());
            assert_ne!(flags & TOUCHES_TABLE, 0, "{f:?}");
            assert_ne!(flags & TOUCHES_PHYSICAL_TABLE, 0, "{f:?}");
        }
    }

    /// Derived-vs-raw parity: the store accessor + `touches_physical_table_derived`
    /// agree with the fixture-only raw oracle `touches_physical_table_of` on every
    /// shape, and both agree with the shared predicate.
    #[test]
    fn physical_table_derived_matches_raw_oracle() {
        let cases: Vec<(&str, Vec<CapabilityFact>, Vec<CapabilityFact>, &str)> = vec![
            (
                "r/temp-rename",
                vec![known_temp("rename")],
                vec![],
                "complete",
            ),
            (
                "r/temp-lock-inh",
                vec![],
                vec![known_temp("lock")],
                "complete",
            ),
            (
                "r/temp-modify",
                vec![known_temp("modify")],
                vec![],
                "complete",
            ),
            ("r/temp-read", vec![known_temp("read")], vec![], "complete"),
            (
                "r/phys-insert",
                vec![table_fact("insert", Some("t/A"), Some(false))],
                vec![],
                "complete",
            ),
            (
                "r/no-rid",
                vec![table_fact("modify", None, None)],
                vec![],
                "complete",
            ),
            (
                "r/temp-and-phys",
                vec![known_temp("modify")],
                vec![table_fact("delete", Some("t/B"), None)],
                "complete",
            ),
            (
                "r/none-complete",
                vec![fact("send", "http", None)],
                vec![],
                "complete",
            ),
            ("r/none-partial", vec![], vec![], "partial"),
            (
                "r/temp-partial",
                vec![known_temp("rename")],
                vec![],
                "partial",
            ),
        ];
        let summaries: HashMap<String, FullRoutineSummary> = cases
            .iter()
            .map(|(id, d, i, cov)| {
                (
                    id.to_string(),
                    summary(id, d.clone(), i.clone(), Some(coverage(cov))),
                )
            })
            .collect();
        let store = cone_store_of(&summaries);
        for (id, d, i, _) in &cases {
            let s = &summaries[*id];
            let raw = touches_physical_table_of(s);
            assert_eq!(
                touches_physical_table_derived(&store, s),
                raw,
                "{id}: derived vs raw"
            );
            assert_eq!(
                store.touches_physical_table(id),
                raw == EffectPresence::Yes,
                "{id}: store accessor vs raw"
            );
            let predicate = d.iter().chain(i.iter()).any(fact_sets_physical_table_bit);
            assert_eq!(predicate, raw == EffectPresence::Yes, "{id}: predicate");
        }
        // The tri-state arms: absent + partial is Unknown, absent + complete No.
        assert_eq!(
            touches_physical_table_of(&summaries["r/temp-partial"]),
            EffectPresence::Unknown
        );
        assert_eq!(
            touches_physical_table_of(&summaries["r/temp-rename"]),
            EffectPresence::No
        );
    }
}
