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
//! severity: high(direct, yes) / medium(unknown). confidence: likely(direct) /
//! possible(yes, unknown).
//!
//! #148 — an UNKNOWN answer (no direct Commit, cone not proven commit-free) is
//! not a found Commit. It keeps the id and `possible` level, but says
//! "may be reachable", names the coverage reasons (and up to three partly
//! analysed routines) in the root cause, note and fix hint, and is capped by the
//! subscriber's uncertainty set (`CORE_SUMMARIES`). #256 — it is reported at
//! `medium` (the arm picks the severity, so a direct Commit with an unknown cone
//! stays `high`). The yes and direct arms are unchanged.
//!
//! #265 — when the gap is in the subscriber's own body, the unknown answer names
//! those call sites (`; unresolved: <callee> (line n)`, the callee rendered from
//! the structured `PCallee` as the snapshot does, source order, up to
//! three) and lists only OTHER routines under "partly analysed". "Unresolved"
//! here means a call whose callee set is not closed for this proof (the
//! call-site-local kinds: interface-open-world, ambiguous-overload,
//! external-target, member-not-found), not the `unresolved-call` uncertainty
//! kind, which is left out. A site in unreachable code is not named. The
//! subscriber stays under "partly analysed" unless every own gap is accounted
//! for: a site was named, every own edge's site was found (in its own row, and
//! reachable), and no other row with its id is opaque or parse-incomplete. Primary
//! subscribers only: a dependency subscriber's own edges are not on the
//! detector context, so it keeps the #148 text (stated limit).
//!
//! Within-detector sort by `a.id.cmp(&b.id)` (byte order).

use std::collections::{BTreeMap, BTreeSet};

use crate::engine::l4::db_effect_solver::is_callsite_local_kind;
use crate::engine::l5::capability_query::{EffectPresence, may_commit_derived};
use crate::engine::l5::confidence::{UncertaintyLite, to_confidence};
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::detectors::anchor_of;
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::full_summary::FullRoutineSummary;
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::engine::l5::snapshot::display_of_callee;
use crate::program::model::workspace::{Model, ModelRoutine};

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
    // Routine ids with an opaque or parse-incomplete row: that row's gap is not
    // a call site, so a subscriber sharing the id keeps itself listed. In
    // cross-app mode `ws.routines` already holds the dependency rows.
    let incomplete_ids: BTreeSet<&str> = ws
        .routines
        .iter()
        .filter(|r| !r.body_available || r.parse_incomplete)
        .map(|r| r.id.as_str())
        .collect();

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
        // out, which is not the same as found, so that arm says so — and, since
        // #256, at `medium`: a found Commit (direct or via callee) stays `high`.
        let (title, root_cause, path, fix_description, confidence, severity): (
            &str,
            String,
            Vec<EvidenceStep>,
            String,
            FindingConfidence,
            &str,
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
                "high",
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
                "high",
            ),
            // `summary_commits` is "unknown" here ("no" was skipped above).
            (None, _) => {
                let reasons = coverage_reasons(summary, routine, &incomplete_ids, ctx);
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
                    "medium",
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
            severity: severity.to_string(),
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
            loose_subject: None,
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

/// How many own unresolved sites, and how many partly analysed targets, the
/// message names before `+N more` (each list on its own).
const SHOWN_TARGETS: usize = 3;

/// `; {label}: ` and up to [`SHOWN_TARGETS`] of `items`, then `, +N more`;
/// nothing when `items` is empty.
fn push_capped<'a>(out: &mut String, label: &str, items: impl ExactSizeIterator<Item = &'a str>) {
    let total = items.len();
    if total == 0 {
        return;
    }
    let shown: Vec<&str> = items.take(SHOWN_TARGETS).collect();
    out.push_str(&format!("; {label}: {}", shown.join(", ")));
    if total > SHOWN_TARGETS {
        out.push_str(&format!(", +{} more", total - SHOWN_TARGETS));
    }
}

/// Why the commit answer is unknown, for the message: the subscriber's coverage
/// reasons, deduplicated, sorted and joined (or a fallback naming what is
/// missing), then `; unresolved: …` naming the subscriber's own call-site-local
/// gaps (sites of `routine`'s own row) in source order, then
/// `; partly analysed: …` naming the cone's other partially analysed routines,
/// sorted; each list capped at [`SHOWN_TARGETS`]
/// with `+N more`. The subscriber itself is left out of "partly analysed" only
/// when its own sites account for its whole gap (module doc, #265).
fn coverage_reasons(
    summary: Option<&FullRoutineSummary>,
    routine: &ModelRoutine,
    incomplete_ids: &BTreeSet<&str>,
    ctx: &DetectorContext,
) -> String {
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
    // Own sites, one per site id, in source order.
    let mut sites: BTreeMap<(u32, u32, &str), String> = BTreeMap::new();
    let mut every_site_found = true;
    let own_edges = ctx.uncertainty_edges_by_from.get(&routine.id);
    for u in own_edges.into_iter().flatten() {
        if !is_callsite_local_kind(&u.kind) {
            continue;
        }
        // This row's own site: rows sharing the id (e.g. `#if` alternatives) share
        // the edge list and site ids, and `call_site_by_id` keeps only the last.
        let cs = u
            .callsite_id
            .as_deref()
            .and_then(|id| routine.call_sites.iter().find(|cs| cs.id == id))
            // Dead code is not the gap to point at (the cone's facts skip it).
            .filter(|cs| cs.control_context.as_deref() != Some("unreachable"));
        let Some(cs) = cs else {
            every_site_found = false;
            continue;
        };
        let a = &cs.source_anchor;
        // The structured callee, as the snapshot shows it (the raw `callee_text`
        // may span lines and hold comments), on one line: its receiver is still
        // raw source, so whitespace runs become one space.
        sites
            .entry((a.start_line, a.start_column, cs.id.as_str()))
            .or_insert_with(|| {
                let callee = display_of_callee(&cs.callee);
                let callee = callee.split_whitespace().collect::<Vec<_>>().join(" ");
                format!("{callee} (line {})", a.start_line + 1)
            });
    }
    let drop_self =
        !sites.is_empty() && every_site_found && !incomplete_ids.contains(routine.id.as_str());
    // A target is a routine id; show its routine when the model has it.
    let targets: BTreeSet<String> = coverage
        .unknown_targets
        .iter()
        .filter(|t| !(drop_self && **t == routine.id))
        .map(|t| match ctx.routine_by_id.get(t.as_str()) {
            Some(r) => format!("{} ({} {})", r.name, r.object_type, r.object_number),
            None => t.clone(),
        })
        .collect();
    push_capped(&mut out, "unresolved", sites.values().map(String::as_str));
    push_capped(
        &mut out,
        "partly analysed",
        targets.iter().map(String::as_str),
    );
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::engine::l4::combined_graph::Uncertainty as CgUncertainty;
    use crate::engine::l4::summary::Uncertainty;
    use crate::engine::l5::test_support::{
        call_site, coverage, minimal_ctx, op_commit_routine, routine, summary,
    };
    use crate::program::body::features::{PCallSite, PCallee};
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
        let owned = minimal_ctx(&routines, HashMap::new(), summaries);
        let ctx = owned.ctx();
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
        // #256: an unknown answer is not a found Commit.
        assert_eq!(f.severity, "medium");
    }

    /// #256: a DIRECT Commit stays `high`/`likely` even when the cone answer is
    /// unknown (no summary here) — severity follows the arm, not `summary_commits`.
    #[test]
    fn direct_commit_with_unknown_cone_stays_high() {
        let routines = vec![op_commit_routine("sub", "event-subscriber", &["op1"])];
        let owned = minimal_ctx(&routines, HashMap::new(), HashMap::new());
        let ctx = owned.ctx();
        let out = detect_d35(&model_of(&routines), &ctx).unwrap().findings;
        assert_eq!(out.len(), 1, "expected one d35 finding, got {out:#?}");
        let f = &out[0];
        assert_eq!(&*f.title, "Commit reachable from event subscriber");
        assert_eq!(f.id, "d35/sub/op1");
        assert_eq!(f.severity, "high");
        assert_eq!(f.confidence, to_confidence(&[], "likely"));
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

    /// A `sub` call site `id` with `callee_text` at 0-based `(line, column)`.
    fn site_at(id: &str, callee_text: &str, line: u32, column: u32) -> PCallSite {
        let mut cs = call_site(id, callee_text, vec![]);
        cs.source_anchor.start_line = line;
        cs.source_anchor.start_column = column;
        cs
    }

    /// A combined-graph uncertainty edge of `kind` at call site `cs`.
    fn own_edge(kind: &str, cs: &str) -> CgUncertainty {
        CgUncertainty {
            kind: kind.to_string(),
            callsite_id: Some(cs.to_string()),
            operation_id: None,
            routine_id: None,
            interface_name: None,
        }
    }

    /// Run d35 on `routines` (the subscriber `sub` first) with `sub`'s summary
    /// `s` and `sub`'s own uncertainty edges `edges`; return its one finding.
    fn finding_with_edges(
        routines: &[ModelRoutine],
        s: FullRoutineSummary,
        edges: Vec<CgUncertainty>,
    ) -> Finding {
        let mut owned = minimal_ctx(
            routines,
            HashMap::new(),
            HashMap::from([("sub".to_string(), s)]),
        );
        owned
            .art_mut()
            .uncertainty_edges_by_from
            .insert("sub".to_string(), edges);
        let ctx = owned.ctx();
        let mut out = detect_d35(&model_of(routines), &ctx).unwrap().findings;
        assert_eq!(out.len(), 1, "expected one d35 finding, got {out:#?}");
        out.remove(0)
    }

    /// #265 A5: the subscriber's own site is named (its structured callee,
    /// line 1-based) and the subscriber leaves "partly analysed".
    #[test]
    fn own_site_named_and_self_dropped() {
        // CR3: the site prints its structured callee, not the raw source text,
        // which here spans lines and holds a comment.
        let text = "Strategy // pick\n    .NextKm";
        assert!(
            text.contains('\n'),
            "precondition: a multi-line callee text"
        );
        let mut cs = site_at("cs1", text, 10, 11);
        cs.callee = PCallee::Member {
            receiver: "Strategy".to_string(),
            method: "NextKm".to_string(),
        };
        let mut sub = routine("sub", "event-subscriber");
        sub.call_sites = vec![cs];
        let routines = vec![sub, routine("helper", "procedure")];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub", "helper"]),
            vec![own_edge("interface-open-world", "cs1")],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; unresolved: Strategy.NextKm (line 11); partly analysed: \
             helper (Codeunit 1)",
        );
    }

    /// #265 A4: own sites in (line, column) order, one per site (an edge
    /// repeated adjacent and non-adjacent prints once; two sites with equal
    /// text both count), the `unresolved-call` kind left out, three shown.
    #[test]
    fn own_sites_source_order_dedup_and_more() {
        let mut sub = routine("sub", "event-subscriber");
        sub.call_sites = vec![
            site_at("cs-b", "Beta", 5, 0),
            site_at("cs-a1", "Alpha", 9, 0),
            site_at("cs-a2", "Alpha", 2, 4),
            site_at("cs-z", "Zed", 7, 8),
            site_at("cs-y", "Yak", 7, 2),
            site_at("cs-u", "Unres", 1, 0),
        ];
        let routines = vec![sub];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub"]),
            vec![
                own_edge("interface-open-world", "cs-a2"),
                own_edge("interface-open-world", "cs-a2"),
                own_edge("ambiguous-overload", "cs-b"),
                own_edge("external-target", "cs-z"),
                own_edge("interface-open-world", "cs-a2"),
                own_edge("member-not-found", "cs-a1"),
                own_edge("interface-open-world", "cs-y"),
                own_edge("unresolved-call", "cs-u"),
            ],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; unresolved: Alpha (line 3), Beta (line 6), Yak (line 8), \
             +2 more",
        );
    }

    /// #265 A6: no own site found, so the subscriber stays under "partly analysed".
    #[test]
    fn own_gap_without_site_keeps_self() {
        let routines = vec![routine("sub", "event-subscriber")];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub"]),
            vec![],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; partly analysed: sub (Codeunit 1)",
        );
    }

    /// #265 A6b: one own edge's site is found, one is missing: the found site is
    /// named AND the subscriber stays under "partly analysed".
    #[test]
    fn own_gap_partly_located_keeps_self() {
        let mut sub = routine("sub", "event-subscriber");
        sub.call_sites = vec![site_at("cs1", "X", 4, 0)];
        let routines = vec![sub];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub"]),
            vec![
                own_edge("interface-open-world", "cs1"),
                own_edge("interface-open-world", "cs-missing"),
            ],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; unresolved: X (line 5); partly analysed: sub (Codeunit 1)",
        );
    }

    /// #265 A6c: another row with the subscriber's id is opaque, or parse-
    /// incomplete: its gap is not a call site, so the subscriber stays under
    /// "partly analysed" though its own site is named. The sibling is not a d35
    /// candidate, so the run still has one finding.
    #[test]
    fn own_site_with_incomplete_sibling_keeps_self() {
        for (label, opaque) in [("opaque", true), ("parse-incomplete", false)] {
            let mut sub = routine("sub", "event-subscriber");
            sub.call_sites = vec![site_at("cs1", "X", 4, 0)];
            let mut sibling = routine("sub", "event-subscriber");
            if opaque {
                sibling.body_available = false;
            } else {
                sibling.parse_incomplete = true;
            }
            let routines = vec![sub, sibling];
            let f = finding_with_edges(
                &routines,
                partial(&["interface-open-world"], &["sub"]),
                vec![own_edge("interface-open-world", "cs1")],
            );
            assert_eq!(f.id, "d35/sub/transitive", "{label}");
            assert_unknown_with(
                &f,
                "interface-open-world; unresolved: X (line 5); partly analysed: sub (Codeunit 1)",
            );
        }
    }

    /// #265 FC3 / N1: the structured callee's receiver is raw source and may span
    /// lines; the rendered site is printed on one line.
    #[test]
    fn multi_line_receiver_prints_on_one_line() {
        let receiver = "GetStrategy(\n    1)";
        assert!(
            receiver.contains('\n'),
            "precondition: a multi-line receiver"
        );
        let mut cs = site_at("cs1", "GetStrategy(\n    1).NextKm", 4, 0);
        cs.callee = PCallee::Member {
            receiver: receiver.to_string(),
            method: "NextKm".to_string(),
        };
        let mut sub = routine("sub", "event-subscriber");
        sub.call_sites = vec![cs];
        let routines = vec![sub];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub"]),
            vec![own_edge("interface-open-world", "cs1")],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; unresolved: GetStrategy( 1).NextKm (line 5)",
        );
    }

    /// #265 CR4: a site in unreachable code is not named (the cone's own fact
    /// pass skips it); it counts as not found, so the subscriber stays listed.
    #[test]
    fn unreachable_own_site_is_not_named() {
        let mut dead = site_at("cs-dead", "Dead", 6, 0);
        dead.control_context = Some("unreachable".to_string());
        let mut sub = routine("sub", "event-subscriber");
        sub.call_sites = vec![site_at("cs1", "X", 4, 0), dead];
        let routines = vec![sub];
        let f = finding_with_edges(
            &routines,
            partial(&["interface-open-world"], &["sub"]),
            vec![
                own_edge("interface-open-world", "cs1"),
                own_edge("interface-open-world", "cs-dead"),
            ],
        );
        assert_unknown_with(
            &f,
            "interface-open-world; unresolved: X (line 5); partly analysed: sub (Codeunit 1)",
        );
    }

    /// #265 CR1: two rows share the subscriber's id (e.g. `#if` alternatives), so
    /// their own edges are merged under one id. Each finding names only a site
    /// from its OWN row; an edge whose site is in the other row only counts as
    /// not found, so the subscriber stays listed.
    #[test]
    fn same_id_rows_name_only_their_own_sites() {
        let run = |a_sites: Vec<PCallSite>, b_sites: Vec<PCallSite>, edges: &[&str]| {
            let mut a = routine("sub", "event-subscriber");
            a.call_sites = a_sites;
            let mut b = routine("sub", "event-subscriber");
            b.call_sites = b_sites;
            let routines = vec![a, b];
            let mut owned = minimal_ctx(
                &routines,
                HashMap::new(),
                HashMap::from([(
                    "sub".to_string(),
                    partial(&["interface-open-world"], &["sub"]),
                )]),
            );
            owned.art_mut().uncertainty_edges_by_from.insert(
                "sub".to_string(),
                edges
                    .iter()
                    .map(|cs| own_edge("interface-open-world", cs))
                    .collect(),
            );
            let ctx = owned.ctx();
            let out = detect_d35(&model_of(&routines), &ctx).unwrap().findings;
            out.into_iter()
                .map(|f| f.fix_options[0].description.to_string())
                .collect::<Vec<_>>()
        };
        let fix = |reasons: &str| {
            format!(
                "Check what could not be analysed ({reasons}); if a Commit is reachable, move \
                 it out of the subscriber or defer it."
            )
        };
        // The rows share the site id `cs1` at different lines.
        assert_eq!(
            run(
                vec![site_at("cs1", "A", 2, 0)],
                vec![site_at("cs1", "B", 7, 0)],
                &["cs1"],
            ),
            vec![
                fix("interface-open-world; unresolved: A (line 3)"),
                fix("interface-open-world; unresolved: B (line 8)"),
            ],
            "shared site id"
        );
        // Each row has a site the other lacks.
        assert_eq!(
            run(
                vec![site_at("cs-a", "A", 2, 0)],
                vec![site_at("cs-b", "B", 7, 0)],
                &["cs-a", "cs-b"],
            ),
            vec![
                fix(
                    "interface-open-world; unresolved: A (line 3); partly analysed: sub (Codeunit 1)"
                ),
                fix(
                    "interface-open-world; unresolved: B (line 8); partly analysed: sub (Codeunit 1)"
                ),
            ],
            "distinct site ids"
        );
    }

    /// The unknown arm's confidence is the subscriber's own uncertainty set,
    /// passed through `to_confidence` — `cappedBy` AND the per-uncertainty evidence.
    #[test]
    fn unknown_confidence_reads_the_subscriber_uncertainties() {
        let routines = vec![routine("sub", "event-subscriber")];
        let mut owned = minimal_ctx(&routines, HashMap::new(), HashMap::new());
        owned.art_mut().set_uncertainties_for_test(
            "sub",
            vec![Uncertainty {
                kind: "unresolved-call".to_string(),
                callsite_id: Some("cs1".to_string()),
                operation_id: None,
                routine_id: None,
                interface_name: None,
            }],
        );
        let ctx = owned.ctx();
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
