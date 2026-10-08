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
//! ⟨C1 Task 3 — R6⟩ The inherited half is now **absent by default**. The analyze
//! path composes its cone under [`ConeOutput::DerivedOnly`], which never
//! materializes the per-routine `Vec<CapabilityFact>` at all (that Vec was
//! ~10.9 GB on the 8020 corpus), so a summary built there carries `None` rather
//! than an empty Vec. The old `reachable()` / `reachable_iter()` helpers are
//! GONE: they would have kept compiling and silently returned a direct-only view
//! — the exact silent-wrong-answer hazard R6 names. The one consumer that
//! genuinely needs the raw facts (the `policy` subcommand) demands
//! `substrate::RAW_INHERITED_FACTS`, and reads them through
//! [`FullRoutineSummary::inherited_raw`], which PANICS when they were not built.
//! Every other consumer reads the compact derived substrate
//! (`ctx.cone_derived`) instead.
//!
//! [`ConeOutput::DerivedOnly`]: crate::engine::l4::cone_derived::ConeOutput::DerivedOnly
//!
//! Task 2b, when it assembles these from the real pipeline, may add
//! `db_effects` / `parameter_roles` / `uncertainties` / `in_recursive_cycle` /
//! `has_unresolved_calls`. They are OMITTED here because no query helper in this
//! task reads them — `capability_query` only needs facts + coverage, and
//! `transaction_spans` only needs facts + coverage via the query helpers.

use crate::engine::l4::capability_cone::{CapabilityFact, CoverageRecord};

/// A per-routine composite: the routine's direct capability facts, OPTIONALLY its
/// raw inherited ones, and its coverage record. Consumers honour
/// `coverage.inherited_status` for the tri-state / G6 semantics.
///
/// Build with [`FullRoutineSummary::new`] — `capability_facts_inherited` is
/// deliberately private so no call site can read it without going through
/// [`inherited_raw`](Self::inherited_raw)'s absence check.
#[derive(Debug, Clone, PartialEq)]
pub struct FullRoutineSummary {
    /// The routine's INTERNAL id (matches `ModelRoutine::id`).
    pub routine_id: String,
    /// Direct capability facts emitted by this routine's body. Always present
    /// (this half was never the memory problem — it is one routine's OWN facts,
    /// not its whole reachable cone).
    pub capability_facts_direct: Vec<CapabilityFact>,
    /// Capability facts inherited from the transitive reachable closure —
    /// `None` unless the cone ran with `ConeOutput::{RawOnly, Both}`, i.e.
    /// unless `substrate::RAW_INHERITED_FACTS` was demanded. `Some(vec![])` is a
    /// materialized-but-empty cone and is NOT the same thing as `None`.
    capability_facts_inherited: Option<Vec<CapabilityFact>>,
    /// Coverage status for the direct + inherited cone. `None` ⇒ helpers treat
    /// `inherited_status` as "unknown" (al-sem `s.coverage?.inheritedStatus ??
    /// "unknown"`). Coverage is ALWAYS composed — it is not gated by the output
    /// mode (it is a handful of strings per routine, not the cone).
    pub coverage: Option<CoverageRecord>,
}

impl FullRoutineSummary {
    /// Assemble a summary. Pass `Some` for `capability_facts_inherited` only when
    /// the cone actually materialized them (`ConeOutput::{RawOnly, Both}`);
    /// `None` records "never built", which [`inherited_raw`](Self::inherited_raw)
    /// then refuses to serve.
    pub fn new(
        routine_id: String,
        capability_facts_direct: Vec<CapabilityFact>,
        capability_facts_inherited: Option<Vec<CapabilityFact>>,
        coverage: Option<CoverageRecord>,
    ) -> Self {
        Self {
            routine_id,
            capability_facts_direct,
            capability_facts_inherited,
            coverage,
        }
    }

    /// True when the raw inherited facts were materialized — i.e. when
    /// [`inherited_raw`](Self::inherited_raw) will not panic.
    ///
    /// ⟨C1 Task 3 review fix M-3⟩ `#[cfg(test)]`-only: it had zero non-test
    /// callers, and it is precisely the API a future caller could use to branch
    /// on presence and silently skip the missing half — the same silent partial
    /// answer R6's panic-on-read was designed to foreclose, reached through a
    /// different door. Tests/diagnostics that need to assert "was this bit
    /// demanded" may still use it; production code must call `inherited_raw`
    /// (or the derived-substrate queries) and let it panic.
    #[cfg(test)]
    pub fn has_inherited_raw(&self) -> bool {
        self.capability_facts_inherited.is_some()
    }

    /// The RAW inherited capability facts, in `sort_inherited` order.
    ///
    /// # Panics
    /// Panics when this summary came from a `ConeOutput::DerivedOnly`
    /// composition. That is deliberate (R6): returning an empty slice here would
    /// silently answer "this routine's whole reachable cone is empty" for every
    /// routine in the workspace. A caller that needs these facts must demand
    /// `substrate::RAW_INHERITED_FACTS` when it builds its `DetectorContext`.
    pub fn inherited_raw(&self) -> &[CapabilityFact] {
        match &self.capability_facts_inherited {
            Some(v) => v,
            None => panic!(
                "FullRoutineSummary::inherited_raw called for routine {:?}, but this context was \
                 built WITHOUT `substrate::RAW_INHERITED_FACTS` — the cone ran in \
                 `ConeOutput::DerivedOnly` and the raw inherited Vec was never materialized. \
                 Read `ctx.cone_derived` instead, or demand the bit.",
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

    /// A materialized-but-EMPTY cone is `Some(vec![])`, not absence — it must
    /// serve an empty slice rather than panic. This is the shape the `policy`
    /// path sees for a routine whose cone entry was drained by the G-18
    /// routine-id collision.
    #[test]
    fn materialized_empty_cone_is_not_absence() {
        let s = summary("r", vec![], vec![], None);
        assert!(s.has_inherited_raw());
        assert!(s.inherited_raw().is_empty());
    }

    /// ⟨R6⟩ The absent case must FAIL LOUDLY, never answer "empty cone".
    #[test]
    #[should_panic(expected = "RAW_INHERITED_FACTS")]
    fn inherited_raw_panics_when_never_materialized() {
        let s = FullRoutineSummary::new("r/x".to_string(), Vec::new(), None, None);
        assert!(!s.has_inherited_raw());
        let _ = s.inherited_raw();
    }

    /// Coverage is composed under EVERY output mode, so `inherited_status` stays
    /// meaningful on a derived-only summary (the absence arm of every tri-state
    /// helper reads it).
    #[test]
    fn inherited_status_is_independent_of_the_raw_vec() {
        use crate::engine::l5::test_support::coverage;
        let s = FullRoutineSummary::new(
            "r/x".to_string(),
            Vec::new(),
            None,
            Some(coverage("complete")),
        );
        assert_eq!(s.inherited_status(), "complete");
        let s = FullRoutineSummary::new("r/x".to_string(), Vec::new(), None, None);
        assert_eq!(s.inherited_status(), "unknown");
    }
}
