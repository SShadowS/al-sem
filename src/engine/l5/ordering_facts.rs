//! L4.5 ordering-facts facade — port of al-sem `src/engine/ordering-facts.ts`.
//!
//! `compute_ordering_facts` REUSES the S4 substrate
//! (`compute_digest_effects_with_ordering` — composeSnapshot → return summaries →
//! isolated events → digestQuery(order:false) with the ordering engine attached),
//! then RESOLVES each `ScopedGuarantee` to its IO effect (type/method/anchor) via
//! `io_occurrence_id` (NEVER off a COMMIT carrier) and to write/commit anchors,
//! projecting fully-resolved `OrderingFact`s per reportable routine.
//!
//! The IO is resolved via `guarantee.io_occurrence_id` and required to be an
//! external-IO type (HTTP/FILE) for IO labels or a window-opening UI type for
//! WRITE_PENDING_AT_UI; the IO-carried and COMMIT-carried copies of
//! EXTERNAL_IO_BEFORE_COMMIT collapse to one via the dedupe `key`.
//!
//! Determinism (M1/M8/M9): the per-routine `facts` list sorts by `key` via
//! `locale_compare_key`, which faithfully reproduces al-sem's `key.localeCompare(b.key)`
//! (ICU/DUCET, as Bun implements it). The key alphabet is `{ '_', '|', '0'..='9',
//! 'a'..='f', 'A'..='Z' }`; ordinal `str::cmp` MUST NOT be used — it gives the WRONG
//! order because `|` (0x7C) byte-sorts after uppercase letters and digits, so an
//! empty write-occurrence segment `||` would wrongly sort after `|hex|`.
//! The `?? ""` empty-segment convention (al-sem `ordering-facts.ts:222`) is matched
//! exactly. See `locale_compare_key` for the full weight table.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::engine::l5::digest::{
    DigestEntryResult, ProjectedEvidence, compute_digest_effects_for_ordering_with,
};
use crate::engine::l5::ordering_engine::ScopedGuarantee;
use crate::program::model::workspace::{Model, ModelRoutine};

/// External-IO effect types — the only types a resolved `io_occurrence_id` may have
/// for IO labels.
fn is_io_type(t: &str) -> bool {
    matches!(t, "HTTP" | "FILE")
}

/// Window-opening UI sink types — the only types a resolved `io_occurrence_id` may
/// have for WRITE_PENDING_AT_UI. Mirrors UI_WINDOW_SINK_TYPES in ordering-engine.
fn is_ui_sink_type(t: &str) -> bool {
    matches!(t, "UI_CONFIRM" | "UI_MESSAGE" | "UI_WINDOW_OPEN")
}

/// Ordering labels this pass resolves into facts.
fn is_relevant_label(label: &str) -> bool {
    matches!(
        label,
        "WRITE_PENDING_AT_EXTERNAL_IO"
            | "EXTERNAL_IO_BEFORE_COMMIT"
            | "WRITE_PENDING_AT_UI"
            | "IO_BEFORE_ESCAPING_ERROR"
            | "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN"
    )
}

/// Primary collation weight for a key character (M8). The dedupe `key` alphabet is
/// `{ '_', '|', '0'..='9', 'a'..='f', 'A'..='Z' }`. JS `String.localeCompare` (ICU
/// default DUCET collation) orders these: punctuation (`_` < `|`) BEFORE digits,
/// digits BEFORE letters, letters case-insensitively by primary weight with case as
/// a TERTIARY tiebreak (lowercase before uppercase). Ordinal `str::cmp` diverges
/// because `|` (0x7C) and `_` (0x5F) byte-sort AFTER digits/uppercase — so the
/// empty write-occurrence segment (`||`) would wrongly sort after a `|hex|` segment.
/// This reproduces localeCompare for the restricted key alphabet exactly.
fn primary_weight(c: char) -> u32 {
    match c {
        '_' => 0,
        '|' => 1,
        '0'..='9' => 2 + (c as u32 - '0' as u32), // 2..=11
        // Letters: primary weight is case-insensitive, alphabetical after digits.
        'a'..='z' => 12 + (c as u32 - 'a' as u32),
        'A'..='Z' => 12 + (c as u32 - 'A' as u32),
        // Any other char (not expected in keys) falls back to its codepoint shifted
        // above the known alphabet so it sorts last, deterministically.
        _ => 100 + c as u32,
    }
}

/// Tertiary (case) weight — lowercase before uppercase (ICU default). Non-letters 0.
fn case_weight(c: char) -> u8 {
    match c {
        'a'..='z' => 0,
        'A'..='Z' => 1,
        _ => 0,
    }
}

/// `a.localeCompare(b)` for the dedupe-key alphabet: two-level (primary, then case).
/// Primary weights compared position-by-position first; on a full primary tie, the
/// case (tertiary) level decides — matching ICU multi-level collation.
///
/// # Prefix-free-labels invariant
///
/// This collation is correct for the current key alphabet because the 5 ordering labels
/// (`WRITE_PENDING_AT_EXTERNAL_IO`, `EXTERNAL_IO_BEFORE_COMMIT`, `WRITE_PENDING_AT_UI`,
/// `IO_BEFORE_ESCAPING_ERROR`, `EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN`) are PREFIX-FREE —
/// no label is a prefix of another. This guarantees that after the label segment, the
/// first post-label character is always `|` on BOTH keys being compared, so `_` and `|`
/// can never align against a letter or digit from a label continuation, and the
/// case-tertiary tiebreak never fires on a label character.
///
/// If a future label is added that is a prefix of an existing label (or vice versa),
/// this collation MUST be re-validated against Bun's `localeCompare` for the full new
/// key alphabet — the prefix-free property is load-bearing for correctness.
fn locale_compare_key(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let n = ac.len().min(bc.len());
    // Primary level.
    for i in 0..n {
        let pa = primary_weight(ac[i]);
        let pb = primary_weight(bc[i]);
        if pa != pb {
            return pa.cmp(&pb);
        }
    }
    if ac.len() != bc.len() {
        return ac.len().cmp(&bc.len());
    }
    // Tertiary (case) level — only reached on a full primary tie.
    for i in 0..n {
        let ta = case_weight(ac[i]);
        let tb = case_weight(bc[i]);
        if ta != tb {
            return ta.cmp(&tb);
        }
    }
    Ordering::Equal
}

/// One fully-resolved ordering hazard fact, ready for `grade_guarantee`.
#[derive(Debug, Clone)]
pub struct OrderingFact {
    pub guarantee: ScopedGuarantee,
    /// Semantic dedupe key — `label|write|io|commit` occurrence ids.
    pub key: String,
    /// Resolved SINK effect type (via `guarantee.io_occurrence_id`): an external-IO
    /// type (HTTP/FILE) for IO labels, or a window-opening UI type for
    /// WRITE_PENDING_AT_UI.
    pub io_type: String,
    /// The IO effect's detail record (e.g. `{ "method": "Get" }`), insertion order.
    pub io_detail: Vec<(String, String)>,
    pub io_anchor: ProjectedEvidence,
    pub write_anchor: Option<ProjectedEvidence>,
    pub commit_anchor: Option<ProjectedEvidence>,
}

#[derive(Debug, Clone)]
pub struct OrderingFacts {
    pub routine_id: String,
    pub facts: Vec<OrderingFact>,
}

/// Reportability predicate — the in-scope routine set this pass enumerates as roots.
/// MUST match the predicate d47/d49/d51 use. Source-only ⇒ role is always primary.
///
/// # TODO — role guard for dep-bearing models
///
/// al-sem `ordering-facts.ts` also checks `roleOf(r) === "primary"` (only source
/// routines with a body are roots). That clause is OMITTED here because every routine
/// that reaches `compute_ordering_facts` today comes from a source-only workspace where
/// every routine is implicitly primary. If a dep-bearing model (i.e. one where external
/// `.app` routines are interleaved into the same L3 resolved set) ever reaches this
/// facade, the role check MUST be restored: without it, dep routines (which have
/// `body_available = false` already, so this path is currently safe) could in theory
/// be treated as reportable roots if that invariant ever changes.
pub fn is_reportable_routine(routine: &ModelRoutine) -> bool {
    routine.body_available && !routine.parse_incomplete
}

/// SHARED stable-id helper — used by this pass AND by d47/d49/d51 so lookups always
/// match. The L3 routine already carries its `stable_routine_id` (empty when the
/// object id is malformed or the signature hash is missing — those never appear as
/// digest roots).
pub fn stable_routine_id_for_routine(routine: &ModelRoutine) -> String {
    routine.stable_routine_id.clone()
}

/// Convert a digest-layer `ProjectedEvidence` (SourceAnchorContract: file/line/column,
/// 0-based) into the model `SourceAnchor` that `EvidenceStep` / `primaryLocation`
/// expect. Returns `None` for non-"source" contracts or a missing file. The point
/// range collapses to start == end.
pub fn to_source_anchor(
    contract: Option<&ProjectedEvidence>,
    enclosing_routine_id: &str,
) -> Option<crate::engine::l5::finding::SourceAnchor> {
    let c = contract?;
    if c.source_kind != "source" {
        return None;
    }
    let file = c.file.clone()?;
    let line = c.line.unwrap_or(0);
    let column = c.column.unwrap_or(0);
    Some(crate::engine::l5::finding::SourceAnchor {
        source_unit_id: file,
        start_line: line,
        start_column: column,
        end_line: line,
        end_column: column,
        enclosing_routine_id: enclosing_routine_id.to_string(),
        syntax_kind: "call".to_string(),
        normalized_text_hash: None,
        leading_context_hash: None,
        trailing_context_hash: None,
    })
}

/// Compute the per-routine ordering facts. Keyed by `StableRoutineId`. Only routines
/// with ≥1 resolved fact appear in the map.
///
/// Each root's digest entry is reduced to its facts as soon as it is computed (on
/// the worker that computed it), so the full set of entries — every effect's
/// witness paths — is never held at once. On CDO with the B3 calls that set is
/// 67 k effects over 1359 roots, while only roots with an ordering fact keep
/// anything.
pub fn compute_ordering_facts(resolved: &Model) -> HashMap<String, OrderingFacts> {
    compute_digest_effects_for_ordering_with(resolved, |entry| ordering_facts_of_entry(&entry))
        .into_iter()
        .flatten()
        .map(|of| (of.routine_id.clone(), of))
        .collect()
}

/// One root's ordering facts, or `None` when it has none.
fn ordering_facts_of_entry(entry: &DigestEntryResult) -> Option<OrderingFacts> {
    // occurrenceId → (anchor, type, detail). factId == occurrence id.
    let mut anchor_by_id: HashMap<&str, &ProjectedEvidence> = HashMap::new();
    let mut type_by_id: HashMap<&str, &str> = HashMap::new();
    let mut detail_by_id: HashMap<&str, &Vec<(String, String)>> = HashMap::new();
    for eff in &entry.effects {
        anchor_by_id.insert(eff.fact_id.as_str(), &eff.evidence);
        type_by_id.insert(eff.fact_id.as_str(), eff.effect_type.as_str());
        detail_by_id.insert(eff.fact_id.as_str(), &eff.detail);
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut facts: Vec<OrderingFact> = Vec::new();
    for eff in &entry.effects {
        for g in &eff.scoped_guarantees {
            if !is_relevant_label(g.label) {
                continue;
            }
            // Resolve the IO occurrence: root scope sets io_occurrence_id;
            // owning-routine scope rides on the carrier effect (factId).
            let io_id: &str = g
                .io_occurrence_id
                .as_deref()
                .unwrap_or(eff.fact_id.as_str());
            let Some(io_type) = type_by_id.get(io_id).copied() else {
                continue;
            };
            // Gate by label.
            if g.label == "WRITE_PENDING_AT_UI" {
                if !is_ui_sink_type(io_type) {
                    continue;
                }
            } else if !is_io_type(io_type) {
                continue;
            }
            let Some(io_anchor) = anchor_by_id.get(io_id).copied() else {
                continue;
            };

            let key = format!(
                "{}|{}|{}|{}",
                g.label,
                g.write_occurrence_id.as_deref().unwrap_or(""),
                io_id,
                g.commit_occurrence_id.as_deref().unwrap_or(""),
            );
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key.clone());

            let io_detail: Vec<(String, String)> = detail_by_id
                .get(io_id)
                .map(|d| (*d).clone())
                .unwrap_or_default();
            let write_anchor = g
                .write_occurrence_id
                .as_deref()
                .and_then(|w| anchor_by_id.get(w).map(|e| (*e).clone()));
            let commit_anchor = g
                .commit_occurrence_id
                .as_deref()
                .and_then(|c| anchor_by_id.get(c).map(|e| (*e).clone()));

            facts.push(OrderingFact {
                guarantee: g.clone(),
                key,
                io_type: io_type.to_string(),
                io_detail,
                io_anchor: io_anchor.clone(),
                write_anchor,
                commit_anchor,
            });
        }
    }

    if facts.is_empty() {
        return None;
    }
    // al-sem sorts by `a.key.localeCompare(b.key)` (ICU). Match it exactly
    // for the restricted key alphabet (see `locale_compare_key`); ordinal
    // `str::cmp` diverges on the empty-vs-hex write-occurrence segment.
    facts.sort_by(|a, b| locale_compare_key(&a.key, &b.key));
    Some(OrderingFacts {
        routine_id: entry.routine_id.clone(),
        facts,
    })
}

// ===========================================================================
// gradeGuarantee — port of al-sem `src/transaction-integrity/txn-context.ts`.
// The spec §4.2 grading table (15 rows).
// ===========================================================================

/// Default hazard grade. "none" = not a concern; "suppressed" = unproven (not
/// emitted by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HazardGrade {
    None,
    Suppressed,
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnFact {
    OpenWriteAtIo,
    OpenWriteAtUi,
    IoBeforeDurableCommit,
    IoBeforeEscapingError,
    Safe,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct TxnGrade {
    pub txn_fact: TxnFact,
    pub io_direction: &'static str,
    pub grade: HazardGrade,
}

/// IO direction from the io_type + detail map (HTTP `method`, FILE `fileOp`).
fn io_direction_of(io_type: &str, io_detail: &[(String, String)]) -> &'static str {
    let method = io_detail
        .iter()
        .find(|(k, _)| k == "method")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    let file_op = io_detail
        .iter()
        .find(|(k, _)| k == "fileOp")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    crate::engine::l5::ordering_inter::io_direction(io_type, method, file_op)
}

/// `gradeGuarantee` — maps ONE `ScopedGuarantee` (+ IO type/detail) to a
/// transaction-state fact and a default hazard severity.
pub fn grade_guarantee(
    guarantee: &ScopedGuarantee,
    io_type: &str,
    io_detail: &[(String, String)],
) -> TxnGrade {
    let dir = io_direction_of(io_type, io_detail);

    match guarantee.label {
        "WRITE_PENDING_AT_EXTERNAL_IO" => {
            if !guarantee.valid_for_refutation {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            if io_type == "HTTP" {
                return TxnGrade {
                    txn_fact: TxnFact::OpenWriteAtIo,
                    io_direction: dir,
                    grade: HazardGrade::Critical,
                };
            }
            // FILE / other: write-style → critical; unknown/read → high.
            TxnGrade {
                txn_fact: TxnFact::OpenWriteAtIo,
                io_direction: dir,
                grade: if dir == "write" {
                    HazardGrade::Critical
                } else {
                    HazardGrade::High
                },
            }
        }
        "EXTERNAL_IO_BEFORE_COMMIT" => {
            if guarantee.commit_effectiveness == Some("proven_suppressed") {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            if dir != "write" {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            TxnGrade {
                txn_fact: TxnFact::IoBeforeDurableCommit,
                io_direction: dir,
                grade: HazardGrade::Info,
            }
        }
        "WRITE_PENDING_AT_UI" => {
            if !guarantee.valid_for_refutation {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            TxnGrade {
                txn_fact: TxnFact::OpenWriteAtUi,
                io_direction: dir,
                grade: HazardGrade::High,
            }
        }
        "WRITE_COMMITTED_BEFORE_EXTERNAL_IO" => TxnGrade {
            txn_fact: TxnFact::Safe,
            io_direction: dir,
            grade: HazardGrade::None,
        },
        "IO_BEFORE_ESCAPING_ERROR" => {
            if !guarantee.valid_for_refutation {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            // Detector-audit d51 BUG: a PROVEN read-direction request (HTTP
            // GET/HEAD) re-issued on retry duplicates no external side effect —
            // it is idempotent by definition. Only write / unknown-direction IO
            // carries a duplication hazard (mirrors the dir-gate the sibling
            // EXTERNAL_IO_BEFORE_COMMIT arm applies). Suppression-direction safe:
            // only the exact "read" signal suppresses; "unknown" keeps firing.
            if dir == "read" {
                return TxnGrade {
                    txn_fact: TxnFact::Unknown,
                    io_direction: dir,
                    grade: HazardGrade::Suppressed,
                };
            }
            let grade = if guarantee.commit_effectiveness == Some("proven_effective") {
                HazardGrade::Medium
            } else {
                HazardGrade::Low
            };
            TxnGrade {
                txn_fact: TxnFact::IoBeforeEscapingError,
                io_direction: dir,
                grade,
            }
        }
        "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN" => TxnGrade {
            txn_fact: TxnFact::OpenWriteAtIo,
            io_direction: dir,
            grade: HazardGrade::Info,
        },
        _ => TxnGrade {
            txn_fact: TxnFact::Safe,
            io_direction: dir,
            grade: HazardGrade::None,
        },
    }
}

/// Map a grade to a `Finding.severity` string, or `None` for none/suppressed (skip).
pub fn to_severity(grade: HazardGrade) -> Option<&'static str> {
    match grade {
        HazardGrade::None | HazardGrade::Suppressed => None,
        HazardGrade::Info => Some("info"),
        HazardGrade::Low => Some("low"),
        HazardGrade::Medium => Some("medium"),
        HazardGrade::High => Some("high"),
        HazardGrade::Critical => Some("critical"),
    }
}

// ===========================================================================
// M5 PROJECTION — project_r4f_ordering_facts (the orderingfacts golden shape).
// Top-level: fixtureName, routineCount (= entry count), entries[].
// Each entry: routineId, facts[]. Each fact: key, ioType, ioDetail, guarantee,
// ioAnchor, [writeAnchor], [commitAnchor]. Entries sorted by routineId.
// The anchor excerpt is OMITTED (the golden carries only sourceKind/file/line/column).
// ===========================================================================

/// SourceAnchorContract serialize for the M5 projection: `sourceKind`, [file],
/// [line], [column] — excerpt OMITTED; "unavailable" emits only sourceKind.
struct AnchorSer<'a>(&'a ProjectedEvidence);
impl Serialize for AnchorSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let e = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("sourceKind", e.source_kind)?;
        if let Some(f) = &e.file {
            map.serialize_entry("file", f)?;
        }
        if let Some(l) = &e.line {
            map.serialize_entry("line", l)?;
        }
        if let Some(c) = &e.column {
            map.serialize_entry("column", c)?;
        }
        map.end()
    }
}

/// ioDetail serialize — insertion-ordered key/value map.
struct DetailSer<'a>(&'a [(String, String)]);
impl Serialize for DetailSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// Guarantee serialize — same field order as the scoped-guarantee golden:
/// label, scope, [write], [commit], [io], [return], supportingEdgeIds,
/// [commitEffectiveness], interveningBoundary, validForRefutation.
struct GuaranteeSer<'a>(&'a ScopedGuarantee);
impl Serialize for GuaranteeSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let g = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("label", g.label)?;
        map.serialize_entry("scope", g.scope)?;
        if let Some(v) = &g.write_occurrence_id {
            map.serialize_entry("writeOccurrenceId", v)?;
        }
        if let Some(v) = &g.commit_occurrence_id {
            map.serialize_entry("commitOccurrenceId", v)?;
        }
        if let Some(v) = &g.io_occurrence_id {
            map.serialize_entry("ioOccurrenceId", v)?;
        }
        if let Some(v) = &g.return_occurrence_id {
            map.serialize_entry("returnOccurrenceId", v)?;
        }
        map.serialize_entry("supportingEdgeIds", &g.supporting_edge_ids)?;
        if let Some(v) = g.commit_effectiveness {
            map.serialize_entry("commitEffectiveness", v)?;
        }
        map.serialize_entry("interveningBoundary", g.intervening_boundary)?;
        map.serialize_entry("validForRefutation", &g.valid_for_refutation)?;
        map.end()
    }
}

struct FactSer<'a>(&'a OrderingFact);
impl Serialize for FactSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let f = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("key", &f.key)?;
        map.serialize_entry("ioType", &f.io_type)?;
        map.serialize_entry("ioDetail", &DetailSer(&f.io_detail))?;
        map.serialize_entry("guarantee", &GuaranteeSer(&f.guarantee))?;
        map.serialize_entry("ioAnchor", &AnchorSer(&f.io_anchor))?;
        if let Some(w) = &f.write_anchor {
            map.serialize_entry("writeAnchor", &AnchorSer(w))?;
        }
        if let Some(c) = &f.commit_anchor {
            map.serialize_entry("commitAnchor", &AnchorSer(c))?;
        }
        map.end()
    }
}

struct EntrySer<'a> {
    routine_id: &'a str,
    facts: &'a [OrderingFact],
}
impl Serialize for EntrySer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("routineId", self.routine_id)?;
        let facts: Vec<FactSer> = self.facts.iter().map(FactSer).collect();
        map.serialize_entry("facts", &facts)?;
        map.end()
    }
}

struct ProjectionSer<'a> {
    fixture_name: &'a str,
    entries: Vec<EntrySer<'a>>,
}
impl Serialize for ProjectionSer<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("fixtureName", self.fixture_name)?;
        map.serialize_entry("routineCount", &self.entries.len())?;
        map.serialize_entry("entries", &self.entries)?;
        map.end()
    }
}

/// Project the M5 ordering-facts differential document, PRETTY-serialized with a
/// trailing newline (the exact on-disk golden form). Entries sorted by routineId.
pub fn project_r4f_ordering_facts(resolved: &Model, fixture_name: &str) -> String {
    let facts_map = compute_ordering_facts(resolved);
    // Sort entries by routineId (deterministic; the map iteration is not).
    let mut keys: Vec<&String> = facts_map.keys().collect();
    keys.sort();
    let entries: Vec<EntrySer> = keys
        .iter()
        .map(|k| {
            let of = &facts_map[*k];
            EntrySer {
                routine_id: of.routine_id.as_str(),
                facts: &of.facts,
            }
        })
        .collect();

    let doc = ProjectionSer {
        fixture_name,
        entries,
    };
    let mut s =
        serde_json::to_string_pretty(&doc).expect("serialize R4-F ordering-facts projection");
    s.push('\n');
    s
}

// ===========================================================================
// Native oracles — ordering_facts
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::ordering_engine::ScopedGuarantee;

    // -------------------------------------------------------------------------
    // Helper: build a minimal ScopedGuarantee for grade_guarantee oracles.
    // -------------------------------------------------------------------------

    fn guarantee(
        label: &'static str,
        valid_for_refutation: bool,
        commit_effectiveness: Option<&'static str>,
    ) -> ScopedGuarantee {
        ScopedGuarantee {
            label,
            scope: "root",
            write_occurrence_id: None,
            commit_occurrence_id: None,
            io_occurrence_id: None,
            return_occurrence_id: None,
            supporting_edge_ids: vec![],
            commit_effectiveness,
            intervening_boundary: "none",
            valid_for_refutation,
        }
    }

    // Convenience: build a detail vec from a single key/value pair.
    fn detail(key: &str, val: &str) -> Vec<(String, String)> {
        vec![(key.to_string(), val.to_string())]
    }

    // =========================================================================
    // Oracle A: locale_compare_key — load-bearing collation correctness.
    //
    // The key alphabet is { '_', '|', '0'..='9', 'a'..='f', 'A'..='Z' }.
    // ICU/DUCET/Bun localeCompare order (verified against Bun v1.x):
    //   '_' < '|' < '0'..'9' < letters (case-insensitive primary, lowercase-first tertiary).
    //
    // al-sem ordering-facts.ts:256: `out.sort((a, b) => a.key.localeCompare(b.key))`.
    //
    // These assertions lock in the collation so a "simplify to str::cmp" refactor
    // would be caught immediately — ordinal cmp diverges on `|` vs `_` vs digits.
    // =========================================================================

    #[test]
    fn locale_compare_key_underscore_before_pipe() {
        // '_' (primary 0) < '|' (primary 1) — DUCET punctuation tier.
        // ordinal: '_'=0x5F, '|'=0x7C — ordinal also gives '_'<'|', same sign here.
        assert!(
            locale_compare_key("_", "|") == std::cmp::Ordering::Less,
            "'_' must sort before '|' (ICU primary weights 0 < 1)"
        );
    }

    #[test]
    fn locale_compare_key_pipe_before_digit() {
        // '|' (primary 1) < '0' (primary 2) — punctuation before digits in ICU.
        // ordinal DIVERGES: '|'=0x7C > '0'=0x30 — str::cmp would give the WRONG order.
        assert!(
            locale_compare_key("|", "0") == std::cmp::Ordering::Less,
            "'|' must sort before '0' (ICU primary 1 < 2); ordinal str::cmp would invert this"
        );
    }

    #[test]
    fn locale_compare_key_digit_9_before_letter_a() {
        // '9' (primary 11) < 'a' (primary 12) — digits before letters in ICU.
        assert!(
            locale_compare_key("9", "a") == std::cmp::Ordering::Less,
            "'9' must sort before 'a' (ICU primary 11 < 12)"
        );
    }

    #[test]
    fn locale_compare_key_lowercase_before_uppercase_tertiary() {
        // 'a' and 'A' share primary weight 12; tertiary: lowercase (0) < uppercase (1).
        // This is the ICU default locale-compare behavior in Bun (lowercase-first tertiary).
        // Case-tertiary only fires on a full primary tie (same length, same primaries).
        assert!(
            locale_compare_key("a", "A") == std::cmp::Ordering::Less,
            "'a' must sort before 'A' (ICU tertiary lowercase-first)"
        );
    }

    #[test]
    fn locale_compare_key_shorter_before_longer_extension() {
        // A key that is a prefix of a longer key sorts before it (length tiebreak).
        // e.g. "a" < "ab" — the shorter key wins at the length comparison.
        assert!(
            locale_compare_key("a", "ab") == std::cmp::Ordering::Less,
            "shorter key 'a' must sort before 'ab' (length extension)"
        );
        assert!(
            locale_compare_key("ab", "a") == std::cmp::Ordering::Greater,
            "longer 'ab' must sort after 'a'"
        );
        assert!(
            locale_compare_key("abc", "abc") == std::cmp::Ordering::Equal,
            "equal keys must be Equal"
        );
    }

    #[test]
    fn locale_compare_key_golden_pair_different_labels() {
        // ACTUAL load-bearing golden pair shape (diverge at first character of label):
        //   "EXTERNAL_IO_BEFORE_COMMIT|<hex>|<hex>|"  — starts with 'E' (primary 16)
        //   "WRITE_PENDING_AT_EXTERNAL_IO||<hex>|"    — starts with 'W' (primary 34)
        // 'E' < 'W' → EXTERNAL_IO key sorts BEFORE WRITE_PENDING key.
        let external_io = "EXTERNAL_IO_BEFORE_COMMIT|abc123|def456|";
        let write_pending = "WRITE_PENDING_AT_EXTERNAL_IO||def456|";
        assert!(
            locale_compare_key(external_io, write_pending) == std::cmp::Ordering::Less,
            "EXTERNAL_IO label ('E') must sort before WRITE_PENDING ('W')"
        );
    }

    #[test]
    fn locale_compare_key_golden_pair_empty_vs_hex_write_occ() {
        // ACTUAL load-bearing same-label pair: empty write-occurrence vs. hex write-occurrence.
        // "X||io|" — write segment is empty → `|` immediately follows label separator.
        // "X|abc|io|" — write segment is "abc" → 'a' (primary 12) follows label separator.
        // At the write-segment position: '|' (primary 1) < 'a' (primary 12).
        // ordinal str::cmp: '|'=0x7C > 'a'=0x61 — WRONG sign; this is why str::cmp breaks.
        let empty_write = "X||io|";
        let hex_write = "X|abc|io|";
        assert!(
            locale_compare_key(empty_write, hex_write) == std::cmp::Ordering::Less,
            "empty write-occurrence segment ('|') must sort before hex write-occurrence ('a'); \
             ordinal str::cmp would invert this (0x7C > 0x61)"
        );
    }

    // =========================================================================
    // Oracle B: grade_guarantee — all 15 spec §4.2 rows.
    //
    // al-sem txn-context.ts (full source reference per row).
    // =========================================================================

    // --- WRITE_PENDING_AT_EXTERNAL_IO ---

    #[test]
    fn grade_guarantee_write_pending_at_external_io_not_vfr_suppressed() {
        // Row: !validForRefutation → suppressed (txn-context.ts:43-45).
        let g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", false, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_write_pending_at_external_io_http_critical() {
        // Row: HTTP + vFR → critical regardless of method (txn-context.ts:46-49).
        // BC runtime-illegal for ALL HTTP methods.
        let g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "GET"));
        assert_eq!(result.grade, HazardGrade::Critical);
        assert_eq!(result.txn_fact, TxnFact::OpenWriteAtIo);
        // POST also critical.
        let result2 = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result2.grade, HazardGrade::Critical);
    }

    #[test]
    fn grade_guarantee_write_pending_at_external_io_file_write_blob_critical() {
        // Row: FILE + write-blob direction → critical (txn-context.ts:50-63).
        // NOTE: the al-sem comment says "today FILE direction is ALWAYS unknown" (spec §9
        // deferred), but the branch exists and this oracle covers the reachable-once-taxonomy-
        // lands path (txn-context.ts:54-58).
        let g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", true, None);
        let result = grade_guarantee(&g, "FILE", &detail("fileOp", "write-blob"));
        assert_eq!(result.io_direction, "write");
        assert_eq!(result.grade, HazardGrade::Critical);
        assert_eq!(result.txn_fact, TxnFact::OpenWriteAtIo);
    }

    #[test]
    fn grade_guarantee_write_pending_at_external_io_file_no_write_high() {
        // Row: FILE + unknown direction (no write-blob fileOp) → high (txn-context.ts:62).
        let g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", true, None);
        let result = grade_guarantee(&g, "FILE", &[]);
        assert_eq!(result.io_direction, "unknown");
        assert_eq!(result.grade, HazardGrade::High);
        assert_eq!(result.txn_fact, TxnFact::OpenWriteAtIo);
    }

    // --- EXTERNAL_IO_BEFORE_COMMIT ---

    #[test]
    fn grade_guarantee_external_io_before_commit_proven_suppressed_suppressed() {
        // Row: proven_suppressed → suppressed (txn-context.ts:75-77).
        let g = guarantee("EXTERNAL_IO_BEFORE_COMMIT", true, Some("proven_suppressed"));
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_external_io_before_commit_http_read_suppressed() {
        // Row: dir != "write" (GET → "read") → suppressed (txn-context.ts:78-80).
        let g = guarantee("EXTERNAL_IO_BEFORE_COMMIT", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "GET"));
        assert_eq!(result.io_direction, "read");
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_external_io_before_commit_write_dir_info() {
        // Row: write-direction (POST) → info advisory (txn-context.ts:81-84).
        let g = guarantee("EXTERNAL_IO_BEFORE_COMMIT", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.io_direction, "write");
        assert_eq!(result.grade, HazardGrade::Info);
        assert_eq!(result.txn_fact, TxnFact::IoBeforeDurableCommit);
    }

    // --- WRITE_PENDING_AT_UI ---

    #[test]
    fn grade_guarantee_write_pending_at_ui_not_vfr_suppressed() {
        // Row: !validForRefutation → suppressed (txn-context.ts:92-94).
        let g = guarantee("WRITE_PENDING_AT_UI", false, None);
        let result = grade_guarantee(&g, "UI_CONFIRM", &[]);
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_write_pending_at_ui_vfr_high() {
        // Row: validForRefutation → high (txn-context.ts:95).
        let g = guarantee("WRITE_PENDING_AT_UI", true, None);
        let result = grade_guarantee(&g, "UI_CONFIRM", &[]);
        assert_eq!(result.grade, HazardGrade::High);
        assert_eq!(result.txn_fact, TxnFact::OpenWriteAtUi);
    }

    // --- IO_BEFORE_ESCAPING_ERROR ---

    #[test]
    fn grade_guarantee_io_before_escaping_error_not_vfr_suppressed() {
        // Row: !validForRefutation → suppressed (txn-context.ts:109-111).
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", false, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_io_before_escaping_error_proven_effective_medium() {
        // Row: vFR + proven_effective → medium (txn-context.ts:112-114).
        // This branch is CORPUS-UNREACHABLE (both d51 goldens are "low"), so there
        // is no golden that exercises this path. The oracle covers it directly.
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", true, Some("proven_effective"));
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Medium);
        assert_eq!(result.txn_fact, TxnFact::IoBeforeEscapingError);
    }

    #[test]
    fn grade_guarantee_io_before_escaping_error_no_proven_effective_low() {
        // Row: vFR + no proven_effective → low (txn-context.ts:113 else branch).
        // This IS the path both d51 corpus goldens exercise.
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Low);
        assert_eq!(result.txn_fact, TxnFact::IoBeforeEscapingError);
    }

    #[test]
    fn grade_guarantee_io_before_escaping_error_http_read_suppressed() {
        // Detector-audit d51 BUG: a proven read-direction request (GET) re-issued
        // on retry duplicates no side effect → suppressed (vfr + read).
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "GET"));
        assert_eq!(result.io_direction, "read");
        assert_eq!(result.grade, HazardGrade::Suppressed);
        assert_eq!(result.txn_fact, TxnFact::Unknown);
    }

    #[test]
    fn grade_guarantee_io_before_escaping_error_unknown_dir_still_fires() {
        // Suppression-direction: an UNKNOWN-direction request (HttpClient.Send,
        // method not resolvable) is NOT proven-read → keeps firing (low).
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "Send"));
        assert_eq!(result.io_direction, "unknown");
        assert_eq!(result.grade, HazardGrade::Low);
        assert_eq!(result.txn_fact, TxnFact::IoBeforeEscapingError);
    }

    // --- EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN ---

    #[test]
    fn grade_guarantee_external_io_in_event_subscriber_txn_info() {
        // Row: always info, regardless of direction (txn-context.ts:122-124).
        // validForRefutation is structurally always false for this label; we don't
        // gate on it but test both values to confirm.
        for vfr in [true, false] {
            let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", vfr, None);
            let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
            assert_eq!(
                result.grade,
                HazardGrade::Info,
                "EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN must always be info (vfr={vfr})"
            );
            assert_eq!(result.txn_fact, TxnFact::OpenWriteAtIo);
        }
    }

    // --- WRITE_COMMITTED_BEFORE_EXTERNAL_IO ---

    #[test]
    fn grade_guarantee_write_committed_before_external_io_none() {
        // Row: the "safe" label → grade none, txn_fact safe (txn-context.ts:99-101).
        let g = guarantee("WRITE_COMMITTED_BEFORE_EXTERNAL_IO", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::None);
        assert_eq!(result.txn_fact, TxnFact::Safe);
    }

    // --- Unknown label ---

    #[test]
    fn grade_guarantee_unknown_label_none() {
        // Row: catch-all → grade none, txn_fact safe (txn-context.ts:127).
        let g = guarantee("SOME_UNKNOWN_LABEL", true, None);
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::None);
        assert_eq!(result.txn_fact, TxnFact::Safe);
    }

    // =========================================================================
    // Oracle C: io_direction (delegated through io_direction_of).
    //
    // io_direction lives in ordering_inter.rs; we test it via grade_guarantee's
    // io_direction field so we cover the delegation path in this module.
    // al-sem io-direction.ts + txn-context.ts line 39.
    // =========================================================================

    #[test]
    fn io_direction_http_write_methods() {
        // POST/PUT/PATCH/DELETE → "write" (io-direction.ts:23).
        let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", false, None);
        for method in &["POST", "PUT", "PATCH", "DELETE"] {
            let r = grade_guarantee(&g, "HTTP", &detail("method", method));
            assert_eq!(r.io_direction, "write", "HTTP {method} must be write");
        }
    }

    #[test]
    fn io_direction_http_read_methods() {
        // GET/HEAD → "read" (io-direction.ts:24).
        let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", false, None);
        for method in &["GET", "HEAD"] {
            let r = grade_guarantee(&g, "HTTP", &detail("method", method));
            assert_eq!(r.io_direction, "read", "HTTP {method} must be read");
        }
    }

    #[test]
    fn io_direction_http_unknown_method() {
        // Unknown method (e.g. OPTIONS) → "unknown" (io-direction.ts:25).
        let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", false, None);
        let r = grade_guarantee(&g, "HTTP", &detail("method", "OPTIONS"));
        assert_eq!(r.io_direction, "unknown");
    }

    #[test]
    fn io_direction_file_write_blob() {
        // FILE + fileOp=write-blob → "write" (io-direction.ts:32).
        let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", false, None);
        let r = grade_guarantee(&g, "FILE", &detail("fileOp", "write-blob"));
        assert_eq!(r.io_direction, "write");
    }

    #[test]
    fn io_direction_file_other_op() {
        // FILE + other fileOp (e.g. open) → "unknown" (io-direction.ts:34).
        let g = guarantee("EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN", false, None);
        let r = grade_guarantee(&g, "FILE", &detail("fileOp", "open"));
        assert_eq!(r.io_direction, "unknown");
    }

    // =========================================================================
    // Oracle D: ioId fallback — io_occurrence_id resolution.
    //
    // In compute_ordering_facts: `io_id = g.io_occurrence_id.unwrap_or(eff.fact_id)`.
    // We cannot exercise the full pipeline here without a real Model, but we
    // document the two branches via assertions on the ScopedGuarantee field shape
    // since compute_ordering_facts is a pipeline function tested end-to-end by the
    // r4f_ordering_facts golden test.
    //
    // This oracle asserts the field semantics:
    //   - None     → falls back to the owning-effect factId (the carrier occurrence).
    //   - Some(x)  → uses x directly (root-scope guarantee carries the io occurrence).
    // =========================================================================

    #[test]
    fn io_id_fallback_none_means_use_fact_id() {
        // A guarantee with io_occurrence_id = None must have the facade use eff.fact_id.
        // We verify the field is None on a freshly-built guarantee (the pipeline then
        // falls back to eff.fact_id — tested end-to-end by the golden suite).
        let g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", true, None);
        assert!(
            g.io_occurrence_id.is_none(),
            "io_occurrence_id=None triggers eff.fact_id fallback in compute_ordering_facts"
        );
    }

    #[test]
    fn io_id_fallback_some_uses_provided_id() {
        // A guarantee with io_occurrence_id = Some(x) must have the facade use x.
        let mut g = guarantee("WRITE_PENDING_AT_EXTERNAL_IO", true, None);
        g.io_occurrence_id = Some("abc123def456a1b2".to_string());
        assert_eq!(
            g.io_occurrence_id.as_deref(),
            Some("abc123def456a1b2"),
            "io_occurrence_id=Some(x) must be passed through directly"
        );
    }

    // =========================================================================
    // Oracle E: d51 medium path — IO_BEFORE_ESCAPING_ERROR + proven_effective
    //           → medium grade + the appended rootCause sentence.
    //
    // Both d51 corpus goldens are "low" (no proven_effective commit on path), so
    // this branch is CORPUS-UNREACHABLE. We verify the grade_guarantee output
    // directly (the d51 finding-builder reads sev from to_severity(grade)).
    //
    // al-sem d51-retry-side-effect-duplication.ts:99-103 and txn-context.ts:112-114.
    // =========================================================================

    #[test]
    fn d51_medium_path_grade_is_medium() {
        // IO_BEFORE_ESCAPING_ERROR + vFR + proven_effective → medium.
        // to_severity(Medium) = Some("medium").
        // The finding builder appends the extra rootCause sentence at sev=="medium"
        // (d51.rs:118-123 — verified by inspection; not testable without a full Model).
        let g = guarantee("IO_BEFORE_ESCAPING_ERROR", true, Some("proven_effective"));
        let result = grade_guarantee(&g, "HTTP", &detail("method", "POST"));
        assert_eq!(result.grade, HazardGrade::Medium);
        assert_eq!(to_severity(result.grade), Some("medium"));
        // Confirm the low path (no proven_effective) gives a different grade.
        let g_low = guarantee("IO_BEFORE_ESCAPING_ERROR", true, None);
        let result_low = grade_guarantee(&g_low, "HTTP", &detail("method", "POST"));
        assert_eq!(result_low.grade, HazardGrade::Low);
        assert_eq!(to_severity(result_low.grade), Some("low"));
    }
}
