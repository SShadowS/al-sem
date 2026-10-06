//! D44 — event multi-subscriber overlap. Port of al-sem
//! `src/detectors/d44-event-multi-subscriber-overlap.ts`.
//!
//! Two findings families over an event's resolved subscribers:
//!   - WRITE/WRITE: ≥2 distinct subscribers write the SAME table (id
//!     `d44/{eventId}|{tableId}`, severity medium).
//!   - READ-AFTER-WRITE: one subscriber writes a table that a DIFFERENT subscriber
//!     reads on the same event (id `d44-rw/{eventId}|{tableId}`, severity low).
//!
//! Both set `event_kind` (via `event_kind_of`) and `cross_extension_subscribers`
//! (via `build_cross_extension_subscribers`). Output is capped per-event
//! (`D44_MAX_PER_EVENT = 32`) across BOTH families via `group_and_cap`, then sorted
//! by `compareStrings(id)`. Fingerprint computed PER-FINDING (al-sem computes it in
//! the build loop, BEFORE the cap).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l5::detector_context::DetectorContext;
use crate::engine::l5::event_flow::event_kind_of;
use crate::engine::l5::finding::{
    Evidence, EvidenceStep, Finding, FindingConfidence, FixOption, SourceAnchor,
};
use crate::engine::l5::registry::{DetectorError, DetectorOutput, DetectorStats};

use super::{anchor_of, group_and_cap};

const DETECTOR: &str = "d44-event-multi-subscriber-overlap";
const D44_MAX_PER_EVENT: usize = 32;

struct SubWrite {
    subscriber: String,
    // ⟨C1 Task 2 fix M4⟩ `decode_op_mask` already yields `&'static str`; keeping
    // this field a borrow instead of `.to_string()`-ing it avoids one
    // allocation per (subscriber, table, op) that `op_union` (below) only
    // ever borrows straight back.
    op: &'static str,
}

pub fn detect_d44(
    _resolved: &L3Resolved,
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

    // anchor lookup helper.
    let anchor_for = |routine_id: &str, fallback: &SourceAnchor| -> SourceAnchor {
        match ctx.routine_by_id.get(routine_id) {
            Some(r) => anchor_of(&r.source_anchor, r),
            None => fallback.clone(),
        }
    };

    // --- WRITE/WRITE: (event, table) → subscriber writes ---------------------
    // key = `${eventId}|${tableId}`. BTreeMap → sorted-key iteration (deterministic;
    // the final id sort makes insertion order irrelevant either way).
    let mut grouped: BTreeMap<String, Vec<SubWrite>> = BTreeMap::new();
    for (event_id, subs) in &ix.subscribers_by_event {
        for sub in subs {
            let Some(r) = ctx.routine_by_id.get(sub.as_str()).copied() else {
                continue;
            };
            let Some(summary) = ctx.summaries.get(&r.id) else {
                continue;
            };
            // ⟨C1 Task 2⟩ The same set of (table, op) pairs the per-fact
            // `find_capabilities(table ∧ write ∧ resource_id ∧ ¬known-temp)`
            // scan USED to produce (that helper and the raw Vec it scanned are
            // both retired as of Task 3), read off the folded cone row.
            // `unique_subs` and `op_union` below are both `BTreeSet`s, so
            // collapsing duplicate facts on the SAME (table, op) into one entry
            // is invisible.
            for (table_id, ops) in ctx
                .cone_derived
                .physical_table_write_ops_of(&summary.routine_id)
            {
                let entry = grouped.entry(format!("{event_id}|{table_id}")).or_default();
                for op in ops {
                    entry.push(SubWrite {
                        subscriber: sub.clone(),
                        op,
                    });
                }
            }
        }
    }

    for (key, writes) in &grouped {
        let unique_subs: BTreeSet<&str> = writes.iter().map(|w| w.subscriber.as_str()).collect();
        if unique_subs.len() < 2 {
            continue;
        }
        candidates += 1;
        let (event_id, table_id) = split_once_pipe(key);
        let sub_list: Vec<&str> = unique_subs.iter().copied().collect();
        let op_union: BTreeSet<&str> = writes.iter().map(|w| w.op).collect();
        let op_union: Vec<&str> = op_union.into_iter().collect();
        let Some(first_id) = anchor_subscriber(&sub_list, ctx) else {
            continue;
        };
        let Some(first) = ctx.routine_by_id.get(first_id).copied() else {
            continue;
        };
        let first_anchor = anchor_of(&first.source_anchor, first);

        let root_cause_key = format!("d44/{event_id}|{table_id}");
        let evidence: Vec<EvidenceStep> = sub_list
            .iter()
            .map(|sub| EvidenceStep {
                routine_id: (*sub).to_string(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_for(sub, &first_anchor),
                note: format!("writes table {table_id}"),
            })
            .collect();
        let cross_ext = cross_ext_by_event
            .get(event_id)
            .filter(|v| !v.is_empty())
            .cloned();

        let mut finding = Finding {
            id: root_cause_key.clone(),
            root_cause_key: root_cause_key.clone(),
            detector: DETECTOR.to_string(),
            title: "Multiple event subscribers write the same table".into(),
            root_cause: format!(
                "{} subscribers of event {event_id} write table {table_id} (ops: {})",
                sub_list.len(),
                op_union.join(", ")
            ),
            severity: "medium".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: Vec::new(),
            },
            primary_location: first_anchor.clone(),
            evidence_path: evidence,
            additional_paths: None,
            affected_objects: Vec::new(),
            affected_tables: vec![table_id.to_string().into()],
            fix_options: vec![FixOption {
                description: "Coordinate the writes (single subscriber, or merge intent) to avoid lost-update / ordering surprises.".into(),
                safety: "medium".into(),
            }],
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: event_kind_by_id.get(event_id).map(|s| s.to_string()),
            cross_extension_subscribers: cross_ext,
            cohort_contexts: None,
        };
        finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
        findings.push(finding);
    }

    // --- READ-AFTER-WRITE ----------------------------------------------------
    let mut writers_by_event_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut readers_by_event_table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (event_id, subs) in &ix.subscribers_by_event {
        for sub in subs {
            let Some(r) = ctx.routine_by_id.get(sub.as_str()).copied() else {
                continue;
            };
            let Some(summary) = ctx.summaries.get(&r.id) else {
                continue;
            };
            // ⟨C1 Task 2⟩ Both sides are subscriber SETS per (event, table), so
            // only the distinct table ids matter — the folded row's write / read
            // id-sets are exactly those.
            for table_id in ctx
                .cone_derived
                .writes_physical_tables_of(&summary.routine_id)
            {
                writers_by_event_table
                    .entry(format!("{event_id}|{table_id}"))
                    .or_default()
                    .insert(sub.clone());
            }
            for table_id in ctx
                .cone_derived
                .physical_table_reads_of(&summary.routine_id)
            {
                readers_by_event_table
                    .entry(format!("{event_id}|{table_id}"))
                    .or_default()
                    .insert(sub.clone());
            }
        }
    }

    let empty_set: BTreeSet<String> = BTreeSet::new();
    for (key, writers) in &writers_by_event_table {
        let readers = readers_by_event_table.get(key).unwrap_or(&empty_set);
        let distinct_readers: Vec<&str> = readers
            .iter()
            .filter(|rd| !writers.contains(rd.as_str()))
            .map(|s| s.as_str())
            .collect();
        if distinct_readers.is_empty() {
            continue;
        }
        let (event_id, table_id) = split_once_pipe(key);
        let writer_list: Vec<&str> = writers.iter().map(|s| s.as_str()).collect();
        let reader_list: Vec<&str> = distinct_readers; // already sorted (BTreeSet iter + filter)
        let Some(first_id) = anchor_subscriber(&writer_list, ctx) else {
            continue;
        };
        let Some(first) = ctx.routine_by_id.get(first_id).copied() else {
            continue;
        };
        let first_anchor = anchor_of(&first.source_anchor, first);

        let root_cause_key = format!("d44-rw/{event_id}|{table_id}");
        let mut evidence: Vec<EvidenceStep> = Vec::new();
        for sub in &writer_list {
            evidence.push(EvidenceStep {
                routine_id: (*sub).to_string(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_for(sub, &first_anchor),
                note: format!("writes {table_id}"),
            });
        }
        for sub in &reader_list {
            evidence.push(EvidenceStep {
                routine_id: (*sub).to_string(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor_for(sub, &first_anchor),
                note: format!("reads {table_id}"),
            });
        }
        let cross_ext = cross_ext_by_event
            .get(event_id)
            .filter(|v| !v.is_empty())
            .cloned();

        let mut finding = Finding {
            id: root_cause_key.clone(),
            root_cause_key: root_cause_key.clone(),
            detector: DETECTOR.to_string(),
            title: "Event subscriber reads a table that another subscriber writes".into(),
            root_cause: format!(
                "On event {event_id}, subscribers {{{}}} write {table_id}; subscribers {{{}}} read {table_id}. AL subscriber order is undefined — reads may see pre- or post-mutation state.",
                writer_list.join(", "),
                reader_list.join(", ")
            ),
            severity: "low".to_string(),
            confidence: FindingConfidence {
                level: "likely".to_string(),
                capped_by: None,
                evidence: Vec::new(),
            },
            primary_location: first_anchor.clone(),
            evidence_path: evidence,
            additional_paths: None,
            affected_objects: Vec::new(),
            affected_tables: vec![table_id.to_string().into()],
            fix_options: vec![FixOption {
                description: "Make subscriber ordering explicit, or move the read into the writing subscriber.".into(),
                safety: "medium".into(),
            }],
            provenance: vec![Evidence {
                source: "tree-sitter",
                note: None,
            }],
            actionable_anchor: None,
            fingerprint: None,
            event_kind: event_kind_by_id.get(event_id).map(|s| s.to_string()),
            cross_extension_subscribers: cross_ext,
            cohort_contexts: None,
        };
        finding.fingerprint = Some(fp_index.fingerprint_of(&finding));
        findings.push(finding);
    }

    // Apply the per-event output cap across BOTH families.
    let (kept, truncated) = group_and_cap(
        findings,
        |f| {
            // ^d44(?:-rw)?\/([^|]+)
            let rest = f
                .root_cause_key
                .strip_prefix("d44-rw/")
                .or_else(|| f.root_cause_key.strip_prefix("d44/"));
            match rest {
                Some(r) => r.split('|').next().unwrap_or(&f.root_cause_key).to_string(),
                None => f.root_cause_key.clone(),
            }
        },
        D44_MAX_PER_EVENT,
    );

    let mut kept = kept;
    kept.sort_by(|a, b| a.id.cmp(&b.id));
    let emitted = kept.len();
    let mut stats = DetectorStats::new(DETECTOR, candidates, emitted);
    // Was computed and silently discarded — surface it so a capped event's
    // dropped-finding count is visible instead of vanishing. `add_skip` only
    // inserts when > 0, so this is additive: byte-identical output whenever no
    // event exceeds `D44_MAX_PER_EVENT`.
    stats.add_skip("outputCapped", truncated as u64);
    Ok(DetectorOutput {
        findings: kept,
        stats,
        diagnostics: vec![],
        d1_cohort_index: None,
    })
}

/// Split a `${a}|${b}` key into (a, b) at the FIRST pipe (al-sem `key.split("|", 2)`).
fn split_once_pipe(key: &str) -> (&str, &str) {
    match key.split_once('|') {
        Some((a, b)) => (a, b),
        None => (key, ""),
    }
}

/// The subscriber a finding anchors on: the first in id order that is not a
/// dependency routine, else the first. In cross-app mode an event's subscribers
/// include dependency routines (engine-switch S7.4); anchoring on one put the
/// finding in the dependency, where the scope filter dropped it (CDO's
/// OnRegisterManualSetup pairs: Base App, Core and System App subscribers sort
/// before the workspace's).
fn anchor_subscriber<'a>(subs: &[&'a str], ctx: &DetectorContext) -> Option<&'a str> {
    subs.iter()
        .copied()
        .find(|s| !ctx.dep_routine_ids.contains(*s))
        .or_else(|| subs.first().copied())
}
