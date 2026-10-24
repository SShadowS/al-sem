//! `FullRoutineSummary` — the per-routine composite the L5 query helpers read.
//!
//! al-sem's `routine.summary` (`RoutineSummary`, `model/summary.ts`) carries the
//! capability facts AND the coverage record together. The Rust CORE
//! `RoutineSummary` (the L4 summary engine) does NOT — the capability cone
//! (`l4/capability_cone.rs`) produces `CapabilityFact[]` + `CoverageRecord`
//! SEPARATELY. So the L5 substrate re-unifies them into this composite, which the
//! `capability_query` helpers and `transaction_spans` operate on.
//!
//! Mirrors al-sem's reachable-fact semantics exactly:
//!   `reachable(s) = capabilityFactsDirect ∪ capabilityFactsInherited`
//! and the coverage tri-state honours `coverage.inheritedStatus` (None ⇒
//! "unknown", matching al-sem's `s.coverage?.inheritedStatus ?? "unknown"`).
//!
//! ⟨C1 Task 3 — R6⟩ A production summary has **no inherited half**. The det
//! composition derives the compact substrate (`ctx.cone_derived`) and keeps raw
//! inherited facts only for a `View`'s witness subjects, APART from the
//! summaries (#160 step 6 D2): policy check reads them through `View::raw`
//! (Task 3). The old `reachable()` / `reachable_iter()` helpers are GONE: they
//! would have kept compiling and silently returned a direct-only view — the
//! exact silent-wrong-answer hazard R6 names. The raw half and
//! `FullRoutineSummary::inherited_raw` (which PANICS when the half was not
//! supplied) survive under `#[cfg(test)]` only, as fixture input for the raw
//! oracles (`capability_query`) and `test_support::cone_store_of`.
//!
//! Task 2b, when it assembles these from the real pipeline, may add
//! `db_effects` / `parameter_roles` / `uncertainties` / `in_recursive_cycle` /
//! `has_unresolved_calls`. They are OMITTED here because no query helper in this
//! task reads them — `capability_query` only needs facts + coverage, and
//! `transaction_spans` only needs facts + coverage via the query helpers.

use crate::engine::l4::capability_cone::{CapabilityFact, CoverageRecord};

/// A per-routine composite: the routine's direct capability facts and its
/// coverage record (in tests, OPTIONALLY raw inherited facts as fixture input).
/// Consumers honour `coverage.inherited_status` for the tri-state / G6 semantics.
///
/// Build with [`FullRoutineSummary::new`] — the test-only
/// `capability_facts_inherited` is deliberately private so no call site can
/// read it without going through `inherited_raw`'s absence check.
#[derive(Debug, Clone, PartialEq)]
pub struct FullRoutineSummary {
    /// The routine's INTERNAL id (matches `ModelRoutine::id`).
    pub routine_id: String,
    /// Direct capability facts emitted by this routine's body. Always present
    /// (this half was never the memory problem — it is one routine's OWN facts,
    /// not its whole reachable cone).
    pub capability_facts_direct: Vec<CapabilityFact>,
    /// Test fixtures only: capability facts inherited from the transitive
    /// reachable closure, supplied as INPUT ([`Self::with_inherited`]).
    /// `Some(vec![])` is a supplied-but-empty cone and is NOT the same thing as
    /// `None` ("never supplied").
    #[cfg(test)]
    capability_facts_inherited: Option<Vec<CapabilityFact>>,
    /// Coverage status for the direct + inherited cone. `None` ⇒ helpers treat
    /// `inherited_status` as "unknown" (al-sem `s.coverage?.inheritedStatus ??
    /// "unknown"`). Coverage is ALWAYS composed — it is not gated by the output
    /// mode (it is a handful of strings per routine, not the cone).
    pub coverage: Option<CoverageRecord>,
}

impl FullRoutineSummary {
    /// Assemble a summary: no inherited half.
    pub fn new(
        routine_id: String,
        capability_facts_direct: Vec<CapabilityFact>,
        coverage: Option<CoverageRecord>,
    ) -> Self {
        Self {
            routine_id,
            capability_facts_direct,
            #[cfg(test)]
            capability_facts_inherited: None,
            coverage,
        }
    }

    /// A fixture summary carrying `capability_facts_inherited` as input.
    /// `None` records "never supplied", which [`inherited_raw`](Self::inherited_raw)
    /// then refuses to serve.
    #[cfg(test)]
    pub fn with_inherited(
        routine_id: String,
        capability_facts_direct: Vec<CapabilityFact>,
        capability_facts_inherited: Option<Vec<CapabilityFact>>,
        coverage: Option<CoverageRecord>,
    ) -> Self {
        Self {
            capability_facts_inherited,
            ..Self::new(routine_id, capability_facts_direct, coverage)
        }
    }

    /// True when the fixture supplied raw inherited facts — i.e. when
    /// [`inherited_raw`](Self::inherited_raw) will not panic.
    #[cfg(test)]
    pub fn has_inherited_raw(&self) -> bool {
        self.capability_facts_inherited.is_some()
    }

    /// The census tally of the facts this summary holds (#160 §4.1.5): the
    /// direct facts and, on a fixture, the supplied inherited ones. It returns
    /// a byte and allocation count, never the facts, so it is no door to a
    /// silently empty inherited half (M-3 above).
    pub(crate) fn census_facts_tally(&self) -> crate::engine::census::Tally {
        use crate::engine::l4::cone_census::facts_tally;
        let mut t = facts_tally(&self.capability_facts_direct);
        let i = facts_tally(self.supplied_inherited());
        t.bytes += i.bytes;
        t.allocs += i.allocs;
        t
    }

    /// The fixture's inherited facts (none when not supplied).
    #[cfg(test)]
    fn supplied_inherited(&self) -> &[CapabilityFact] {
        self.capability_facts_inherited.as_deref().unwrap_or(&[])
    }

    /// A production summary has no inherited half.
    #[cfg(not(test))]
    fn supplied_inherited(&self) -> &[CapabilityFact] {
        &[]
    }

    /// The fixture's RAW inherited capability facts, in `sort_inherited` order.
    ///
    /// # Panics
    /// Panics when the fixture supplied none. That is deliberate (R6):
    /// returning an empty slice here would silently answer "this routine's
    /// whole reachable cone is empty". Production raw facts live in the view's
    /// witness pass (`View::raw`), never on a summary.
    #[cfg(test)]
    pub fn inherited_raw(&self) -> &[CapabilityFact] {
        match &self.capability_facts_inherited {
            Some(v) => v,
            None => panic!(
                "FullRoutineSummary::inherited_raw called for routine {:?}, but no inherited \
                 facts were supplied (a production summary has none: read `ctx.cone_derived`, \
                 or a View's witness pass)",
                self.routine_id
            ),
        }
    }

    /// The inherited coverage status (`coverage.inherited_status`), or "unknown"
    /// when there is no coverage record. Mirrors al-sem
    /// `s.coverage?.inheritedStatus ?? "unknown"`. Independent of the output mode
    /// — coverage is always composed.
    pub fn inherited_status(&self) -> &str {
        self.coverage
            .as_ref()
            .map(|c| c.inherited_status.as_str())
            .unwrap_or("unknown")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::test_support::{fact, summary};

    /// A materialized cone serves its facts verbatim, in order.
    #[test]
    fn inherited_raw_serves_the_materialized_vec() {
        let s = summary(
            "r",
            vec![fact("insert", "table", Some("t/D1"))],
            vec![
                fact("modify", "table", Some("t/I1")),
                fact("commit", "transaction", None),
            ],
            None,
        );
        assert!(s.has_inherited_raw());
        let got: Vec<&str> = s.inherited_raw().iter().map(|f| f.op).collect();
        assert_eq!(got, vec!["modify", "commit"]);
    }

    /// A supplied-but-EMPTY cone is `Some(vec![])`, not absence — it must
    /// serve an empty slice rather than panic.
    #[test]
    fn materialized_empty_cone_is_not_absence() {
        let s = summary("r", vec![], vec![], None);
        assert!(s.has_inherited_raw());
        assert!(s.inherited_raw().is_empty());
    }

    /// ⟨R6⟩ The absent case must FAIL LOUDLY, never answer "empty cone".
    #[test]
    #[should_panic(expected = "no inherited facts were supplied")]
    fn inherited_raw_panics_when_never_materialized() {
        let s = FullRoutineSummary::new("r/x".to_string(), Vec::new(), None);
        assert!(!s.has_inherited_raw());
        let _ = s.inherited_raw();
    }

    /// Coverage is composed under EVERY output mode, so `inherited_status` stays
    /// meaningful on a derived-only summary (the absence arm of every tri-state
    /// helper reads it).
    #[test]
    fn inherited_status_is_independent_of_the_raw_vec() {
        use crate::engine::l5::test_support::coverage;
        let s = FullRoutineSummary::new("r/x".to_string(), Vec::new(), Some(coverage("complete")));
        assert_eq!(s.inherited_status(), "complete");
        let s = FullRoutineSummary::new("r/x".to_string(), Vec::new(), None);
        assert_eq!(s.inherited_status(), "unknown");
    }
}
