//! `pickActionableAnchor` — port of al-sem `src/projection/actionable-anchor.ts`.
//!
//! Pick the first evidence step whose owning routine is in the PRIMARY app. Walk
//! forward through `evidence_path`; return `None` when `primary_location` is
//! already primary (the caller leaves `actionable_anchor` unset, signalling "use
//! primaryLocation as-is").
//!
//! ## Role resolution (source-only note)
//! al-sem's `roleOf(r)` reads `r.analysisRole` (absent ⇒ "primary"). The Rust
//! `ModelRoutine` carries no `analysisRole` field — in the SOURCE-ONLY pipeline
//! every routine is "primary", exactly as `registry::run_detectors` documents.
//! To keep the port faithful AND testable, the caller supplies a
//! `role_by_routine` map (the SAME map `run_detectors` builds: routine id →
//! "primary" | "dependency"). A routine ABSENT from the map, or whose role is
//! not "primary", is treated as non-primary — mirroring al-sem's
//! `r !== undefined && roleOf(r) === "primary"` (an unknown routine is
//! non-primary). In source-only runs every routine maps to "primary", so this
//! always returns `None`; the oracle injects "dependency" roles to exercise the
//! dep-anchored path.

use std::collections::HashMap;

use crate::engine::l5::finding::{Finding, SourceAnchor};

/// Pick the primary-app actionable anchor when `primary_location` is in a
/// dependency. Mirrors al-sem `pickActionableAnchor`.
///
/// `role_by_routine` maps an internal routine id to its analysis role
/// ("primary" | "dependency"); a routine absent from the map is non-primary.
pub fn pick_actionable_anchor(
    finding: &Finding,
    role_by_routine: &HashMap<&str, &str>,
) -> Option<SourceAnchor> {
    let is_primary_routine =
        |id: &str| -> bool { role_by_routine.get(id).copied() == Some("primary") };

    // primaryLocation.enclosingRoutineId is the terminal routine — if it's
    // primary, no anchor needed.
    if is_primary_routine(&finding.primary_location.enclosing_routine_id) {
        return None;
    }

    // Walk the evidence path; return the first step that belongs to a primary routine.
    // Read through `evidence_path_of` rather than the field: a cohort-bearing d1
    // finding does not STORE its path (see that function's doc), and this runs
    // inside `assemble_cohort_findings` on exactly such a finding.
    for step in crate::engine::l5::finding::evidence_path_of(finding).iter() {
        if is_primary_routine(&step.routine_id) {
            return Some(step.source_anchor.clone());
        }
    }
    None
}

// ===========================================================================
// Native oracles — dep-anchored finding picks the primary anchor; all-primary
// finding returns None.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::finding::{EvidenceStep, FindingConfidence};

    fn anchor(unit: &str, routine: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: unit.to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            enclosing_routine_id: routine.to_string(),
            syntax_kind: "x".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn step(unit: &str, routine: &str) -> EvidenceStep {
        EvidenceStep {
            routine_id: routine.to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(unit, routine),
            note: "hop".to_string(),
        }
    }

    fn finding(primary_routine: &str, path: Vec<EvidenceStep>) -> Finding {
        Finding {
            id: "d1/x".to_string(),
            root_cause_key: "k".to_string(),
            detector: "d1".to_string(),
            title: "t".into(),
            root_cause: "rc".to_string(),
            severity: "high".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: vec![],
            },
            primary_location: anchor("ws:term.al", primary_routine),
            evidence_path: path,
            additional_paths: None,
            affected_objects: vec![],
            affected_tables: vec![],
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        }
    }

    #[test]
    fn dep_anchored_finding_picks_first_primary_step() {
        // Terminal routine r_dep is a dependency; the path passes through a
        // dependency hop then a primary caller r_pri. The anchor should be r_pri's.
        let f = finding(
            "r_dep",
            vec![
                step("ws:dep2.al", "r_dep2"),
                step("ws:pri.al", "r_pri"),
                step("ws:pri2.al", "r_pri2"),
            ],
        );
        let mut roles: HashMap<&str, &str> = HashMap::new();
        roles.insert("r_dep", "dependency");
        roles.insert("r_dep2", "dependency");
        roles.insert("r_pri", "primary");
        roles.insert("r_pri2", "primary");

        let got = pick_actionable_anchor(&f, &roles);
        let got = got.expect("a primary anchor should be picked");
        // First primary step in path order is r_pri.
        assert_eq!(got.source_unit_id, "ws:pri.al");
        assert_eq!(got.enclosing_routine_id, "r_pri");
    }

    #[test]
    fn all_primary_finding_returns_none() {
        // primaryLocation already primary → no anchor needed.
        let f = finding("r_pri", vec![step("ws:pri.al", "r_pri")]);
        let mut roles: HashMap<&str, &str> = HashMap::new();
        roles.insert("r_pri", "primary");
        assert_eq!(pick_actionable_anchor(&f, &roles), None);
    }

    #[test]
    fn dep_anchored_with_no_primary_step_returns_none() {
        // Terminal AND every path step are dependencies → no primary anchor exists.
        let f = finding("r_dep", vec![step("ws:dep2.al", "r_dep2")]);
        let mut roles: HashMap<&str, &str> = HashMap::new();
        roles.insert("r_dep", "dependency");
        roles.insert("r_dep2", "dependency");
        assert_eq!(pick_actionable_anchor(&f, &roles), None);
    }

    /// The cohort-bearing case: a d1 finding carries an EMPTY `evidence_path`
    /// and derives its path from `cohort_contexts[0].witness`, so this walk has
    /// to read through `evidence_path_of` or it silently anchors nothing.
    ///
    /// `pick_actionable_anchor` is called from inside `assemble_cohort_findings`
    /// on exactly such a finding, so this is the USE, not just the helper: the
    /// finding below is shaped the way d1 builds one (empty field, populated
    /// cohort witness) and the expected anchor exists ONLY in the witness.
    /// Reverting the walk to `&finding.evidence_path` makes it return `None`.
    #[test]
    fn cohort_bearing_finding_anchors_through_the_witness() {
        use crate::engine::l5::d1_witness::WitnessSummary;
        use crate::engine::l5::finding::D1CohortContext;
        use std::sync::Arc;

        let mut f = finding("r_dep", vec![]);
        f.cohort_contexts = Some(vec![D1CohortContext {
            severity: "high".to_string(),
            verdict: "physical".to_string(),
            depth_bucket: 1,
            uncertain: false,
            reachable_verdicts: vec!["physical".to_string()],
            loop_set: crate::engine::l5::d1_cohort::LoopSetId(0),
            loop_count: 1,
            witness: WitnessSummary {
                total_hops: 1,
                first_steps: vec![Arc::new(step("ws:dep2.al", "r_dep2"))],
                omitted_hops: 0,
                last_steps: vec![Arc::new(step("ws:pri.al", "r_pri"))],
                terminal_step: Arc::new(step("ws:term.al", "r_dep")),
            },
        }]);

        let mut roles: HashMap<&str, &str> = HashMap::new();
        roles.insert("r_dep", "dependency");
        roles.insert("r_dep2", "dependency");
        roles.insert("r_pri", "primary");

        assert!(
            f.evidence_path.is_empty(),
            "d1 does not store the path — that is the point of this test"
        );
        let got = pick_actionable_anchor(&f, &roles)
            .expect("the primary step lives in the cohort witness");
        assert_eq!(got.source_unit_id, "ws:pri.al");
        assert_eq!(got.enclosing_routine_id, "r_pri");
    }

    #[test]
    fn unknown_routine_is_non_primary() {
        // A routine absent from the role map is non-primary (al-sem
        // `r !== undefined` guard). Terminal unknown, path has one unknown then a
        // primary → picks the primary.
        let f = finding(
            "r_unknown",
            vec![step("ws:u.al", "r_other_unknown"), step("ws:p.al", "r_pri")],
        );
        let mut roles: HashMap<&str, &str> = HashMap::new();
        roles.insert("r_pri", "primary");
        let got = pick_actionable_anchor(&f, &roles).expect("primary picked");
        assert_eq!(got.enclosing_routine_id, "r_pri");
    }
}
