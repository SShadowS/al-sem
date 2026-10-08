//! `project_finding` → `FindingSummary` — port of al-sem
//! `src/projection/finding-summary.ts`.
//!
//! Resolves an internal `Finding`'s `SourceAnchor.enclosing_routine_id` to display
//! names (routine + owning object) via the resolved model, and converts the 0-based
//! internal range to the 1-based SARIF/display line+column. The gate's SARIF + filter
//! layers consume the projected `FindingSummary`, exactly as al-sem does.

use std::collections::HashMap;

use crate::engine::l5::finding::{Finding, SourceAnchor};
use crate::program::model::workspace::{ModelObject, ModelRoutine};

/// `FindingLocation` (finding-summary.ts). `line`/`column` are 1-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingLocation {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub object_id: Option<String>,
    pub object_name: Option<String>,
    pub routine_id: Option<String>,
    pub routine_name: Option<String>,
}

/// `FindingSummary` (finding-summary.ts) — the compact projection every output path
/// consumes. The SARIF formatter reads `detector`, `title`, `root_cause`, `severity`,
/// `fingerprint`, and `primary_location`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingSummary {
    pub id: String,
    pub fingerprint: String,
    pub detector: String,
    pub title: String,
    pub root_cause: String,
    pub severity: String,
    pub confidence_level: String,
    pub confidence_capped_by: Option<Vec<String>>,
    pub primary_location: FindingLocation,
    pub terminal_location: Option<FindingLocation>,
    pub affected_objects: Vec<String>,
    pub affected_tables: Vec<String>,
    /// `fixHint` (finding-summary.ts) — the first `fixOptions` entry, if any.
    pub fix_hint: Option<(String, String)>,
    pub path_count: usize,
}

/// Per-model id indexes for the projection (mirror `indexFor`).
pub struct ProjectionIndex<'a> {
    pub objects_by_id: HashMap<&'a str, &'a ModelObject>,
    pub routines_by_id: HashMap<&'a str, &'a ModelRoutine>,
}

impl<'a> ProjectionIndex<'a> {
    pub fn build(objects: &'a [ModelObject], routines: &'a [ModelRoutine]) -> Self {
        ProjectionIndex {
            objects_by_id: objects.iter().map(|o| (o.id.as_str(), o)).collect(),
            routines_by_id: routines.iter().map(|r| (r.id.as_str(), r)).collect(),
        }
    }
}

/// `toLocation(anchor)` — resolve an internal anchor to a 1-based display location
/// with the owning routine + object display names.
fn to_location(anchor: &SourceAnchor, idx: &ProjectionIndex) -> FindingLocation {
    let routine = idx.routines_by_id.get(anchor.enclosing_routine_id.as_str());
    // Object-level finding convention (d64): `enclosing_routine_id` IS the
    // object's own internal id when there is no routine to anchor on (e.g. a
    // declarative page with no routines at all). Fall back to a direct object
    // lookup so `object_id`/`object_name` still resolve in the production
    // SARIF/JSON/HTML/terminal output; `routine_id`/`routine_name` correctly
    // stay `None` below (there is no routine). Behavior-preserving for every
    // routine-anchored finding: the `or_else` only runs when the routine
    // branch already missed.
    let object = routine
        .and_then(|r| idx.objects_by_id.get(r.object_id.as_str()))
        .or_else(|| idx.objects_by_id.get(anchor.enclosing_routine_id.as_str()));
    FindingLocation {
        file: anchor.source_unit_id.clone(),
        line: anchor.start_line + 1,
        column: anchor.start_column + 1,
        object_id: object.map(|o| o.id.clone()),
        object_name: object.map(|o| o.name.clone()),
        routine_id: routine.map(|r| r.id.clone()),
        routine_name: routine.map(|r| r.name.clone()),
    }
}

/// `projectFinding(finding, model)` — project a `Finding` into a `FindingSummary`.
/// When `actionable_anchor` is set, the primary becomes that anchor and the original
/// `primary_location` becomes the terminal (mirrors finding-summary.ts).
pub fn project_finding(finding: &Finding, idx: &ProjectionIndex) -> FindingSummary {
    let (primary, terminal) = match &finding.actionable_anchor {
        Some(actionable) => (
            to_location(actionable, idx),
            Some(to_location(&finding.primary_location, idx)),
        ),
        None => (to_location(&finding.primary_location, idx), None),
    };

    FindingSummary {
        id: finding.id.clone(),
        // al-sem: `finding.fingerprint ?? finding.id`.
        fingerprint: finding
            .fingerprint
            .clone()
            .unwrap_or_else(|| finding.id.clone()),
        detector: finding.detector.clone(),
        title: finding.title.to_string(),
        root_cause: finding.root_cause.clone(),
        severity: finding.severity.clone(),
        confidence_level: finding.confidence.level.clone(),
        confidence_capped_by: finding.confidence.capped_by.clone(),
        primary_location: primary,
        terminal_location: terminal,
        affected_objects: finding
            .affected_objects
            .iter()
            .map(|s| s.to_string())
            .collect(),
        affected_tables: finding
            .affected_tables
            .iter()
            .map(|s| s.to_string())
            .collect(),
        // al-sem: `finding.fixOptions[0]` (the first fix option, if any).
        fix_hint: finding
            .fix_options
            .first()
            .map(|f| (f.description.to_string(), f.safety.to_string())),
        // al-sem: `1 + (finding.additionalPaths?.length ?? 0)` — read through
        // `realizing_path_count`, because a cohort-bearing d1 finding no longer
        // STORES `additional_paths` (it was a retained duplicate of
        // `cohort_contexts[1..].witness`) and the accessor returns the same
        // count from the cohorts themselves.
        path_count: crate::engine::l5::finding::realizing_path_count(finding),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::d1_cohort::LoopSetId;
    use crate::engine::l5::d1_witness::WitnessSummary;
    use crate::engine::l5::finding::{
        D1CohortContext, EvidenceStep, FindingConfidence, SourceAnchor,
    };
    use std::sync::Arc;

    fn anchor() -> SourceAnchor {
        SourceAnchor {
            source_unit_id: "ws:t.al".to_string(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            enclosing_routine_id: "r".to_string(),
            syntax_kind: "x".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn step() -> EvidenceStep {
        EvidenceStep {
            routine_id: "r".to_string(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor(),
            note: "n".to_string(),
        }
    }

    fn cohort() -> D1CohortContext {
        D1CohortContext {
            severity: "high".to_string(),
            verdict: "physical".to_string(),
            depth_bucket: 1,
            uncertain: false,
            reachable_verdicts: vec!["physical".to_string()],
            loop_set: LoopSetId(0),
            loop_count: 1,
            witness: WitnessSummary {
                total_hops: 0,
                first_steps: vec![Arc::new(step())],
                omitted_hops: 0,
                last_steps: vec![],
                terminal_step: Arc::new(step()),
            },
        }
    }

    fn finding(cohorts: Option<Vec<D1CohortContext>>, extra: Option<usize>) -> Finding {
        Finding {
            id: "d1/x".to_string(),
            root_cause_key: "k".to_string(),
            detector: "d1-db-op-in-loop".to_string(),
            title: "t".into(),
            root_cause: "rc".to_string(),
            severity: "high".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: vec![],
            },
            primary_location: anchor(),
            evidence_path: vec![],
            additional_paths: extra.map(|n| vec![vec![step()]; n]),
            affected_objects: vec![],
            affected_tables: vec![],
            fix_options: vec![],
            provenance: vec![],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: cohorts,
        }
    }

    /// `pathCount` reaches the DEFAULT `analyze --format json` output and the
    /// terminal report's "+N other paths", so it is the one place d1 dropping
    /// `additional_paths` could have changed shipped bytes — and no golden covers
    /// it (the only d1 golden, `ws-d1-multi-caller`, has a single cohort and so
    /// `pathCount: 1` either way).
    ///
    /// This drives the REAL `project_finding` — the USE, not the accessor — over a
    /// finding shaped the way d1 now builds one: `additional_paths: None` with
    /// THREE cohorts. Before the change that finding carried
    /// `Some([witness, witness])` and projected `path_count: 3`; it must still.
    /// Reverting `project_finding` to `1 + additional_paths.len()` makes this
    /// answer 1.
    #[test]
    fn cohort_bearing_path_count_counts_cohorts_not_the_dropped_field() {
        let objects: Vec<ModelObject> = Vec::new();
        let routines: Vec<ModelRoutine> = Vec::new();
        let idx = ProjectionIndex::build(&objects, &routines);

        let f = finding(Some(vec![cohort(), cohort(), cohort()]), None);
        assert!(
            f.additional_paths.is_none(),
            "d1 no longer stores additional_paths — that is what this test is about"
        );
        assert_eq!(
            project_finding(&f, &idx).path_count,
            3,
            "pathCount must equal the cohort count (1 winner + 2 others)"
        );

        // A single-cohort d1 finding still projects 1 — the shape every existing
        // d1 golden carries, so this half proves the change is a no-op there.
        let one = finding(Some(vec![cohort()]), None);
        assert_eq!(project_finding(&one, &idx).path_count, 1);

        // And a NON-cohort finding still reads the stored field, unchanged.
        let plain = finding(None, Some(2));
        assert_eq!(project_finding(&plain, &idx).path_count, 3);
        let plain_none = finding(None, None);
        assert_eq!(project_finding(&plain_none, &idx).path_count, 1);
    }
}
