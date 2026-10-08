//! D45 — event transitive table exposure. Port of al-sem
//! `src/detectors/d45-event-transitive-table-exposure.ts`.
//!
//! For each primary event PUBLISHER — and, since engine-switch S8.4, each
//! DEPENDENCY publisher with a primary routine in its chain, anchored on that
//! routine — walk the transitive subscriber chain
//! (`collect_relay_subscribers` — bridges event-graph dispatches + call-graph relays)
//! and surface every table written by a chain subscriber. One finding per
//! (publisher, table); id `d45/{publisher}|{table}`, severity info.
//!
//! Builds evidence as `EvidenceStep[]` from the WRITER SUBSCRIBER set (NOT a raw
//! ChainNode dump — d45 uses `collect_relay_subscribers`'s depth map, not
//! `walk_event_chain`'s tree). Sets `event_kind` (publisher's first event) and
//! `cross_extension_subscribers` (same first event). Output capped per-publisher
//! (`D45_MAX_PER_PUBLISHER = 16`) via `group_and_cap`, then sorted by
//! `compareStrings(id)`. Fingerprint computed PER-FINDING (BEFORE the cap).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::engine::l5::capability_query::reachable_coverage;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::event_flow::{RelayWalkOptions, collect_relay_subscribers, event_kind_of};
use crate::engine::l5::finding::{Evidence, EvidenceStep, Finding, FindingConfidence, FixOption};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};
use crate::program::model::workspace::Model;

use super::{anchor_of, group_and_cap};

const DETECTOR: &str = "d45-event-transitive-table-exposure";
const D45_MAX_DEPTH: usize = 4;
const D45_MAX_NODES: usize = 256;
const D45_MAX_PER_PUBLISHER: usize = 16;

pub fn detect_d45(
    _resolved: &Model,
    ctx: &DetectorContext,
) -> Result<DetectorOutput, DetectorError> {
    let fp_index = &ctx.fingerprint_index;
    let ix = &ctx.event_flow_indexes;

    let mut findings: Vec<Finding> = Vec::new();
    let mut candidates = 0usize;

    // eventKind per internal eventId.
    let mut event_kind_by_id: HashMap<&str, &'static str> = HashMap::new();
    for ev in &ctx.event_graph.events {
        event_kind_by_id.insert(ev.id.as_str(), event_kind_of(&ev.event_kind));
    }
    // Cross-extension subscriber lookup per event — shared, built once in ctx.
    let cross_ext_by_event = &ctx.cross_extension_subscribers;

    // events_by_publisher keys iterate in byte order (BTreeMap) — deterministic.
    for publisher in ix.events_by_publisher.keys() {
        let Some(pub_routine) = ctx.routine_by_id.get(publisher.as_str()).copied() else {
            continue;
        };
        let Some(pub_summary) = ctx.summaries.get(&pub_routine.id) else {
            continue;
        };
        let publisher_is_primary = ix.primary_routines.contains(publisher);
        let pub_writes: BTreeSet<String> = ctx
            .cone_derived
            .writes_physical_tables_of(&pub_summary.routine_id)
            .into_iter()
            .collect();
        let pub_cov = reachable_coverage(pub_summary, None).to_string();

        // Walk the full subscriber chain (N hops via event + call graph).
        let subscribers_by_depth = collect_relay_subscribers(
            publisher,
            ix,
            &ctx.graph.edges_by_from,
            &RelayWalkOptions {
                max_depth: D45_MAX_DEPTH,
                max_nodes: D45_MAX_NODES,
            },
        );

        // A dependency publisher is a root only when a PRIMARY routine is in its
        // subscriber chain (engine-switch S8.4, owner decision 2026-10-06): the
        // workspace then takes part in that event. The finding anchors on the first
        // such primary subscriber, the code the user owns. Single-app: every routine
        // is primary, so nothing changes.
        let primary_anchor = if publisher_is_primary {
            None
        } else {
            match subscribers_by_depth
                .keys()
                .find(|s| ix.primary_routines.contains(*s))
                .and_then(|s| ctx.routine_by_id.get(s.as_str()).copied())
            {
                Some(r) => Some(r),
                None => continue,
            }
        };

        // Aggregate subscriber-induced writes; track worst coverage + writer sets.
        let mut writer_subs_by_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut sub_cov_worst: &str = "complete";

        for sub in subscribers_by_depth.keys() {
            let Some(r) = ctx.routine_by_id.get(sub.as_str()).copied() else {
                sub_cov_worst = "unknown";
                continue;
            };
            let Some(summary) = ctx.summaries.get(&r.id) else {
                sub_cov_worst = "unknown";
                continue;
            };
            let status = reachable_coverage(summary, None);
            if status == "unknown" {
                sub_cov_worst = "unknown";
            } else if status == "partial" && sub_cov_worst != "unknown" {
                sub_cov_worst = "partial";
            }
            for t in ctx
                .cone_derived
                .writes_physical_tables_of(&summary.routine_id)
            {
                writer_subs_by_table
                    .entry(t)
                    .or_default()
                    .insert(sub.clone());
            }
        }

        // publisher's first event (sorted) → eventKind + crossExt.
        let publisher_events: Vec<&String> = ix
            .events_by_publisher
            .get(publisher)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        // events_by_publisher value lists are already sorted (BTreeMap freeze).
        let first_event = publisher_events.first().copied();
        let publisher_event_kind = first_event
            .and_then(|e| event_kind_by_id.get(e.as_str()))
            .map(|s| s.to_string());

        for (table, writer_set) in &writer_subs_by_table {
            // For a dependency publisher, only a table a PRIMARY subscriber writes is
            // the workspace's exposure; another dependency's subscriber writing its
            // own tables is not the workspace's to act on (S8.4: CDO 323 findings
            // before, most of them Continia Core's own tables).
            if primary_anchor.is_some()
                && !writer_set.iter().any(|w| ix.primary_routines.contains(w))
            {
                continue;
            }
            candidates += 1;
            let publisher_also_writes = if pub_writes.contains(table) {
                "yes"
            } else if pub_cov == "complete" {
                "no"
            } else {
                "unknown"
            };
            let writer_subs: Vec<&str> = writer_set.iter().map(|s| s.as_str()).collect();
            // direct = at least one writer is at chain depth <= 1.
            let mut coverage_reach = "transitive";
            for sub in &writer_subs {
                if let Some(&d) = subscribers_by_depth.get(*sub)
                    && d <= 1
                {
                    coverage_reach = "direct";
                    break;
                }
            }

            let root_cause_key = format!("d45/{publisher}|{table}");
            let evidence: Vec<EvidenceStep> = writer_subs
                .iter()
                .map(|sub| {
                    let anchor = match ctx.routine_by_id.get(*sub) {
                        Some(r) => anchor_of(&r.source_anchor, r),
                        None => anchor_of(&pub_routine.source_anchor, pub_routine),
                    };
                    EvidenceStep {
                        routine_id: (*sub).to_string(),
                        operation_id: None,
                        callsite_id: None,
                        loop_id: None,
                        source_anchor: anchor,
                        note: format!("subscriber writes {table}"),
                    }
                })
                .collect();
            let cross_ext = first_event
                .and_then(|e| cross_ext_by_event.get(e))
                .filter(|v| !v.is_empty())
                .cloned();

            let mut finding = Finding {
                id: root_cause_key.clone(),
                root_cause_key: root_cause_key.clone(),
                detector: DETECTOR.to_string(),
                title: "Event subscribers expose table transitively from publisher".into(),
                root_cause: format!(
                    "Publisher {publisher} dispatches to {} subscriber(s) that write table {table}; reach={coverage_reach}; publisherAlsoWrites={publisher_also_writes}; subscriberCoverage={sub_cov_worst}",
                    writer_subs.len()
                ),
                severity: "info".to_string(),
                confidence: FindingConfidence {
                    level: "likely".to_string(),
                    capped_by: None,
                    evidence: Vec::new(),
                },
                primary_location: match primary_anchor {
                    Some(r) => anchor_of(&r.source_anchor, r),
                    None => anchor_of(&pub_routine.source_anchor, pub_routine),
                },
                evidence_path: evidence,
                additional_paths: None,
                affected_objects: Vec::new(),
                affected_tables: vec![table.as_str().into()],
                fix_options: vec![FixOption {
                    description: format!(
                        "Treat {table} as part of this event's effect surface for permission/transaction reasoning."
                    ).into(),
                    safety: "high".into(),
                }],
                provenance: vec![Evidence {
                    source: "tree-sitter",
                    note: None,
                }],
                actionable_anchor: None,
                fingerprint: None,
                event_kind: publisher_event_kind.clone(),
                cross_extension_subscribers: cross_ext,
                cohort_contexts: None,
            };
            finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
            findings.push(finding);
        }
    }

    // Apply the per-publisher cap.
    let (kept, truncated) = group_and_cap(
        findings,
        |f| {
            // ^d45\/([^|]+)
            let rest = f.root_cause_key.strip_prefix("d45/");
            match rest {
                Some(r) => r.split('|').next().unwrap_or(&f.root_cause_key).to_string(),
                None => f.root_cause_key.clone(),
            }
        },
        D45_MAX_PER_PUBLISHER,
    );

    let mut kept = kept;
    kept.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = kept.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates, emitted);
    // Was computed and silently discarded — surface it so a capped publisher's
    // dropped-finding count is visible instead of vanishing. `add_skip` only
    // inserts when > 0, so this is additive: byte-identical output whenever no
    // publisher exceeds `D45_MAX_PER_PUBLISHER`.
    stats.add_skip("outputCapped", truncated as u64);
    Ok(DetectorOutput {
        findings: kept,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}
