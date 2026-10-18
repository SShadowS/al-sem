//! D35 — Commit reachable from an event subscriber. Port of al-sem
//! `src/detectors/d35-commit-in-event-subscriber.ts`.
//!
//! event-subscriber routines, primary+body+!parseIncomplete only.
//! summaryCommits = summary None → "unknown" else may_commit(summary); skip if "no".
//! directCommit = first operation_sites kind=="commit"; isDirect = present.
//!
//! id = `d35/{routineId}/{isDirect ? directCommit.id : "transitive"}`.
//! rootCauseKey = `d35/{routineId}` (no suffix).
//! Three arms: direct (a Commit in the body), yes (a Commit proven in the cone,
//! title suffix " (via callee)") and unknown (below, its own title).
//! evidence: 2 steps if direct (subscriber header + commit), 1 step otherwise.
//! severity: high (all). confidence: likely(direct) / possible(yes, unknown).
//!
//! #148 — an UNKNOWN answer (no direct Commit, cone not proven commit-free) is
//! not a found Commit. It keeps the id, severity and `possible` level, but says
//! "may be reachable", names the coverage reasons (and up to three partly
//! analysed routines) in the root cause, note and fix hint, and is capped by the
//! subscriber's uncertainty set (`CORE_SUMMARIES`). The yes and direct arms are
//! unchanged.
//!
//! Within-detector sort by `a.id.cmp(&b.id)` (byte order).

use std::collections::BTreeSet;

use crate::engine::l5::capability_query::{EffectPresence, may_commit_derived};
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

pub(crate) const DETECTOR: &str = "d35-commit-in-event-subscriber";

pub fn detect_d35(
    resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let ws = &resolved.workspace;
    let fp_index = &ctx.fingerprint_index;
    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates_considered = 0usize;
    let mut skipped_commit_free_subscriber = 0u64;

    for routine in &ws.routines {
        // roleOf(routine) === "primary": source-only ⇒ every routine is primary.
        if routine.kind != "event-subscriber" {
            continue;
        }
        if !routine.body_available {
            continue;
        }
        if routine.parse_incomplete {
            continue;
        }
        candidates_considered += 1;

        // summaryCommits = summary None → "unknown" else may_commit(summary).
        let summary = ctx.summaries.get(&routine.id);
        let summary_commits: &str = match summary {
            None => "unknown",
            Some(s) => match may_commit_derived(&ctx.cone_derived, s) {
                EffectPresence::Yes => "yes",
                EffectPresence::No => "no",
                EffectPresence::Unknown => "unknown",
            },
        };

        if summary_commits == "no" {
            skipped_commit_free_subscriber += 1;
            continue;
        }

        // Find first direct commit operation in this routine's body.
        let direct_commit = routine.operation_sites.iter().find(|s| s.kind == "commit");
        let is_direct = direct_commit.is_some();

        let anchor = match direct_commit {
            Some(os) => anchor_of(&os.source_anchor, routine),
            None => anchor_of(&routine.source_anchor, routine),
        };

        let header_step = |note: String| EvidenceStep {
            routine_id: routine.id.clone(),
            operation_id: None,
            callsite_id: None,
            loop_id: None,
            source_anchor: anchor_of(&routine.source_anchor, routine),
            note,
        };
        const REMOVE_FIX: &str = "Remove the Commit from the subscriber path. If durable side \
             effects are required, schedule them outside the publisher's transaction (e.g. a \
             job-queue entry written without Commit, processed later).";

        // One arm per answer. A direct Commit wins, then a proven (`yes`) one in
        // the cone. #148: otherwise the answer is UNKNOWN — a commit is not ruled
        // out, which is not the same as found, so that arm says so.
        let (title, root_cause, path, fix_description, confidence): (
            &str,
            String,
            Vec<EvidenceStep>,
            String,
            FindingConfidence,
        ) = match (direct_commit, summary_commits) {
            (Some(dc), _) => (
                "Commit reachable from event subscriber",
                format!(
                    "{} is an event subscriber that calls Commit directly \u{2014} the publisher \
                     cannot roll back the committed state if its work later fails.",
                    routine.name
                ),
                vec![
                    header_step(format!("[EventSubscriber] {}", routine.name)),
                    EvidenceStep {
                        routine_id: routine.id.clone(),
                        operation_id: Some(dc.id.clone()),
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor_of(&dc.source_anchor, routine),
                        note: "Commit".to_string(),
                    },
                ],
                REMOVE_FIX.to_string(),
                to_confidence(&[], "likely"),
            ),
            (None, "yes") => (
                "Commit reachable from event subscriber (via callee)",
                format!(
                    "{} is an event subscriber that transitively reaches Commit through its \
                     callees \u{2014} the publisher cannot roll back the committed state if its \
                     work later fails.",
                    routine.name
                ),
                vec![header_step(format!(
                    "[EventSubscriber] {} transitively commits (mayCommit(summary) == \"yes\")",
                    routine.name
                ))],
                REMOVE_FIX.to_string(),
                to_confidence(&[], "possible"),
            ),
            // `summary_commits` is "unknown" here ("no" was skipped above).
            (None, _) => {
                let reasons = coverage_reasons(summary, ctx);
                // Capped by what kept the cone incomplete — the subscriber's own
                // uncertainty set (CORE_SUMMARIES). It may be empty: a cone can be
                // partial for a reason the summarize step does not record.
                let lites: Vec<UncertaintyLite> = ctx
                    .uncertainty_view()
                    .values_of(&routine.id)
                    .map(UncertaintyLite::of)
                    .collect();
                (
                    "Commit may be reachable from event subscriber (incomplete call coverage)",
                    format!(
                        "{} is an event subscriber whose call coverage is incomplete \
                         ({reasons}); a Commit cannot be ruled out \u{2014} the publisher could \
                         not roll back committed state if one exists.",
                        routine.name
                    ),
                    vec![header_step(format!(
                        "[EventSubscriber] {} may commit (mayCommit(summary) == \"unknown\"; \
                         incomplete coverage: {reasons})",
                        routine.name
                    ))],
                    format!(
                        "Check what could not be analysed ({reasons}); if a Commit is reachable, \
                         move it out of the subscriber or defer it."
                    ),
                    to_confidence(&lites, "possible"),
                )
            }
        };

        let id = format!(
            "d35/{}/{}",
            routine.id,
            if is_direct {
                direct_commit.map(|dc| dc.id.as_str()).unwrap_or("x")
            } else {
                "transitive"
            }
        );
        let root_cause_key = format!("d35/{}", routine.id);

        let mut finding = Finding {
            id,
            root_cause_key,
            detector: DETECTOR.to_string(),
            title: title.into(),
            root_cause,
            severity: "high".to_string(),
            confidence,
            primary_location: anchor,
            evidence_path: path,
            additional_paths: None,
            affected_objects: vec![routine.object_id.as_str().into()],
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description: fix_description.into(),
                safety: "medium".into(),
            }],
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        };
        finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
        findings.push(finding);
    }

    findings.sort_by(|a, b| a.id.cmp(&b.id));

    let emitted = findings.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates_considered, emitted);
    stats.add_skip("commitFreeSubscriber", skipped_commit_free_subscriber);
    Ok(DetectorOutput::no_diag(findings, stats))
}

/// How many partly analysed targets the message names before `+N more`.
const SHOWN_TARGETS: usize = 3;

/// Why the commit answer is unknown, for the message: the subscriber's coverage
/// reasons, deduplicated, sorted and joined (or a fallback naming what is
/// missing), then `; partly analysed: …` naming up to [`SHOWN_TARGETS`] of the cone's
/// partially analysed routines, sorted, with `+N more` for the rest.
fn coverage_reasons(summary: Option<&FullRoutineSummary>, ctx: &DetectorContext) -> String {
    let Some(summary) = summary else {
        return "no effect summary".to_string();
    };
    let Some(coverage) = &summary.coverage else {
        return "coverage unknown".to_string();
    };
    let reasons: BTreeSet<&str> = coverage.reasons.iter().map(String::as_str).collect();
    let mut out = if reasons.is_empty() {
        "incomplete coverage".to_string()
    } else {
        reasons.into_iter().collect::<Vec<_>>().join(", ")
    };
    // A target is a routine id; show its routine when the model has it.
    let targets: BTreeSet<String> = coverage
        .unknown_targets
        .iter()
        .map(|t| match ctx.routine_by_id.get(t.as_str()) {
            Some(r) => format!("{} ({} {})", r.name, r.object_type, r.object_number),
            None => t.clone(),
        })
        .collect();
    if !targets.is_empty() {
        let shown: Vec<&str> = targets
            .iter()
            .take(SHOWN_TARGETS)
            .map(String::as_str)
            .collect();
        out.push_str("; partly analysed: ");
        out.push_str(&shown.join(", "));
        if targets.len() > SHOWN_TARGETS {
            out.push_str(&format!(", +{} more", targets.len() - SHOWN_TARGETS));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::summary::Uncertainty;
    use crate::engine::l5::test_support::{coverage, minimal_ctx, routine, summary};
    use crate::program::model::workspace::{Model, ModelEntities, ModelRoutine};

    fn model_of(routines: &[ModelRoutine]) -> Model {
        Model {
            workspace: ModelEntities {
                objects: vec![],
                tables: vec![],
                routines: routines.to_vec(),
            },
            root_classifications: vec![],
            primary_app: None,
            infra_diagnostics: Vec::new(),
            workspace_diagnostics: Vec::new(),
            dependency_diagnostics: Vec::new(),
            calls: Default::default(),
            events: Default::default(),
        }
    }

    /// Run d35 on one subscriber `sub` with the given summaries; return its one finding.
    fn one_finding(summaries: HashMap<String, FullRoutineSummary>) -> Finding {
        one_finding_with(&[], summaries)
    }

    /// As [`one_finding`], with `extra` (non-subscriber) routines in the model.
    fn one_finding_with(
        extra: &[ModelRoutine],
        summaries: HashMap<String, FullRoutineSummary>,
    ) -> Finding {
        let mut routines = vec![routine("sub", "event-subscriber")];
        routines.extend_from_slice(extra);
        let ctx = minimal_ctx(&routines, HashMap::new(), summaries);
        let mut out = detect_d35(&model_of(&routines), &ctx).unwrap().findings;
        assert_eq!(out.len(), 1, "expected one d35 finding, got {out:#?}");
        out.remove(0)
    }

    fn assert_unknown_with(f: &Finding, reasons: &str) {
        assert_eq!(
            &*f.title,
            "Commit may be reachable from event subscriber (incomplete call coverage)"
        );
        assert_eq!(
            f.root_cause,
            format!(
                "sub is an event subscriber whose call coverage is incomplete ({reasons}); a \
                 Commit cannot be ruled out \u{2014} the publisher could not roll back \
                 committed state if one exists."
            )
        );
        assert_eq!(
            f.evidence_path[0].note,
            format!(
                "[EventSubscriber] sub may commit (mayCommit(summary) == \"unknown\"; \
                 incomplete coverage: {reasons})"
            )
        );
        assert_eq!(
            &*f.fix_options[0].description,
            format!(
                "Check what could not be analysed ({reasons}); if a Commit is reachable, move \
                 it out of the subscriber or defer it."
            )
            .as_str()
        );
        assert_eq!(f.id, "d35/sub/transitive");
        assert_eq!(f.severity, "high");
    }

    #[test]
    fn no_summary_is_unknown_with_no_effect_summary() {
        let f = one_finding(HashMap::new());
        assert_unknown_with(&f, "no effect summary");
        assert_eq!(f.confidence, to_confidence(&[], "possible"));
    }

    #[test]
    fn coverage_none_is_unknown_with_coverage_unknown() {
        let s = summary("sub", vec![], vec![], None);
        let f = one_finding(HashMap::from([("sub".to_string(), s)]));
        assert_unknown_with(&f, "coverage unknown");
    }

    /// A partial coverage record with the given reasons and unknown targets.
    fn partial(reasons: &[&str], targets: &[&str]) -> FullRoutineSummary {
        let mut cov = coverage("partial");
        cov.reasons = reasons.iter().map(|s| s.to_string()).collect();
        cov.unknown_targets = targets.iter().map(|s| s.to_string()).collect();
        summary("sub", vec![], vec![], Some(cov))
    }

    #[test]
    fn reasons_are_deduplicated_and_sorted() {
        let s = partial(
            &["opaque-call", "object-run-unresolved", "opaque-call"],
            &[],
        );
        let f = one_finding(HashMap::from([("sub".to_string(), s)]));
        assert_unknown_with(&f, "object-run-unresolved, opaque-call");
    }

    #[test]
    fn empty_reasons_is_incomplete_coverage() {
        let s = partial(&[], &[]);
        let f = one_finding(HashMap::from([("sub".to_string(), s)]));
        assert_unknown_with(&f, "incomplete coverage");
    }

    /// Up to three partly analysed targets, sorted by what is shown (a known
    /// routine's display, else its id), then `+N more`.
    #[test]
    fn unknown_targets_show_three_then_more() {
        let extra = vec![routine("helper", "procedure")];
        let s = partial(
            &["opaque-call"],
            &["zz-id", "helper", "aa-id", "mm-id", "qq-id"],
        );
        let f = one_finding_with(&extra, HashMap::from([("sub".to_string(), s)]));
        assert_unknown_with(
            &f,
            "opaque-call; partly analysed: aa-id, helper (Codeunit 1), mm-id, +2 more",
        );
    }

    /// The unknown arm's confidence is the subscriber's own uncertainty set,
    /// passed through `to_confidence` — `cappedBy` AND the per-uncertainty evidence.
    #[test]
    fn unknown_confidence_reads_the_subscriber_uncertainties() {
        let routines = vec![routine("sub", "event-subscriber")];
        let mut ctx = minimal_ctx(&routines, HashMap::new(), HashMap::new());
        ctx.set_uncertainties_for_test(
            "sub",
            vec![Uncertainty {
                kind: "unresolved-call".to_string(),
                callsite_id: Some("cs1".to_string()),
                operation_id: None,
                routine_id: None,
                interface_name: None,
            }],
        );
        let out = detect_d35(&model_of(&routines), &ctx).unwrap().findings;
        assert_eq!(out.len(), 1);
        let c = &out[0].confidence;
        assert_eq!(c.level, "possible");
        assert_eq!(c.capped_by, Some(vec!["unresolved-call".to_string()]));
        assert_eq!(c.evidence.len(), 1);
        assert_eq!(
            c.evidence[0].note.as_deref(),
            Some("unresolved-call at cs1")
        );
    }
}
