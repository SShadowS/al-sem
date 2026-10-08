//! D35 — Commit reachable from an event subscriber. Port of al-sem
//! `src/detectors/d35-commit-in-event-subscriber.ts`.
//!
//! event-subscriber routines, primary+body+!parseIncomplete only.
//! summaryCommits = summary None → "unknown" else may_commit(summary); skip if "no".
//! directCommit = first operation_sites kind=="commit"; isDirect = present.
//!
//! id = `d35/{routineId}/{isDirect ? directCommit.id : "transitive"}`.
//! rootCauseKey = `d35/{routineId}` (no suffix).
//! title suffix " (via callee)" when !isDirect.
//! evidence: 2 steps if direct (subscriber header + commit), 1 step if transitive.
//! severity: high (both). confidence: likely(direct) / possible(transitive).
//!
//! Within-detector sort by `a.id.cmp(&b.id)` (byte order).

use crate::engine::l5::capability_query::{EffectPresence, may_commit_derived};
use crate::engine::l5::confidence::to_confidence;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

const DETECTOR: &str = "d35-commit-in-event-subscriber";

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
        let summary_commits: &str = match ctx.summaries.get(&routine.id) {
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

        // Evidence path.
        let routine_header_anchor = anchor_of(&routine.source_anchor, routine);
        let path: Vec<EvidenceStep> = if is_direct {
            let dc = direct_commit.unwrap();
            vec![
                EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: None,
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: routine_header_anchor,
                    note: format!("[EventSubscriber] {}", routine.name),
                },
                EvidenceStep {
                    routine_id: routine.id.clone(),
                    operation_id: Some(dc.id.clone()),
                    callsite_id: None,
                    loop_id: None,
                    source_anchor: anchor_of(&dc.source_anchor, routine),
                    note: "Commit".to_string(),
                },
            ]
        } else {
            vec![EvidenceStep {
                routine_id: routine.id.clone(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: routine_header_anchor,
                // `{:?}` on summary_commits (closed set {"yes","unknown"}) reproduces
                // al-sem's literal-quoted template `== "${summaryCommits}"` byte-for-byte.
                // Do NOT simplify to `{}` — that drops the quotes and breaks parity.
                note: format!(
                    "[EventSubscriber] {} transitively commits (mayCommit(summary) == {:?})",
                    routine.name, summary_commits
                ),
            }]
        };

        let confidence_str = if is_direct { "likely" } else { "possible" };
        let confidence: FindingConfidence = to_confidence(&[], confidence_str);
        let title_suffix = if is_direct { "" } else { " (via callee)" };

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
            title: format!("Commit reachable from event subscriber{}", title_suffix).into(),
            root_cause: format!(
                "{} is an event subscriber that {} \u{2014} the publisher cannot roll back the \
                 committed state if its work later fails.",
                routine.name,
                if is_direct {
                    "calls Commit directly"
                } else {
                    "transitively reaches Commit through its callees"
                }
            ),
            severity: "high".to_string(),
            confidence,
            primary_location: anchor,
            evidence_path: path,
            additional_paths: None,
            affected_objects: vec![routine.object_id.as_str().into()],
            affected_tables: Vec::new(),
            fix_options: vec![FixOption {
                description: "Remove the Commit from the subscriber path. If durable side effects \
                              are required, schedule them outside the publisher's transaction \
                              (e.g. a job-queue entry written without Commit, processed later)."
                    .into(),
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
