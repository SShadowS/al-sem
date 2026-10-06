//! cli-b/b2 — PROVE ENGINE + CLI pipeline.
//!
//! Ports:
//!   - `src/prove/prove-questions.ts`   → ProveQuestion / parse_question / question_ids
//!   - `src/prove/prove-engine.ts`       → prove (tristate logic, blockers, obligations)
//!   - `src/contracts/prove.ts`          → project_prove_document (envelope kind "prove" 1.2.0)
//!   - `src/cli/prove.ts`                → format_prove_human / run_prove_pipeline
//!
//! ## Tristate semantics
//!
//! MAY-questions (may-commit, writes-table, publishes-event, reaches-ui, throws-error):
//!   yes     = ≥1 matching effect (blockers irrelevant for yes).
//!   no      = zero matches AND ALL obligations clear.
//!   unknown = zero matches + ≥1 obligation failure.
//!
//! MUST-question (commits-on-success-path):
//!   yes     = ≥1 COMMIT with conditionality "unconditional-on-success" AND obligations clear.
//!   no      = zero COMMIT effects of ANY conditionality AND obligations clear.
//!   unknown = everything else (conditional COMMIT → non-unconditional-effect-exists blocker).
//!
//! ## Blocker sort
//!   Deterministic: (kind, anchor file, anchor line, detail) ascending NUL-delimited.

use std::collections::HashMap;

use crate::engine::gate::format_json::serialize_document_value;
use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l3::program_calls::assemble_and_resolve_workspace_program;
use crate::engine::l5::conditionality::UNCONDITIONAL;
use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::digest::compute_digest_effects_cli;
use crate::engine::l5::digest_cli::{
    ChangedInput, DigestEffectFull, DigestEntryFull, FullAnchor, build_envelope_diagnostics_json,
    resolve_changed_roots, run_digest_query_full_from_entries,
};
use crate::engine::l5::snapshot::compose_snapshot;
use crate::engine::l5::transaction_spans::SeedKind;
use crate::engine::l5::unresolved_cone::unresolved_cone;

const TX_SPAN_KNOWN_WRITES: &str = "span-has-known-writes";
const TX_SPAN_NO_WRITES: &str = "span-has-no-known-writes";
const TX_UNKNOWN: &str = "unknown";

// ---------------------------------------------------------------------------
// ProveQuestion
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveQuestion {
    MayCommit,
    CommitsOnSuccessPath,
    WritesTable { table: String },
    PublishesEvent { event: String },
    ReachesUi,
    ThrowsError,
}

/// Parse a question text string into a ProveQuestion. Returns None for invalid input.
pub fn parse_question(text: &str) -> Option<ProveQuestion> {
    match text {
        "may-commit" => Some(ProveQuestion::MayCommit),
        "commits-on-success-path" => Some(ProveQuestion::CommitsOnSuccessPath),
        "reaches-ui" => Some(ProveQuestion::ReachesUi),
        "throws-error" => Some(ProveQuestion::ThrowsError),
        _ => {
            if let Some(rest) = text.strip_prefix("writes-table:") {
                if rest.is_empty() {
                    None
                } else {
                    Some(ProveQuestion::WritesTable {
                        table: rest.to_string(),
                    })
                }
            } else if let Some(rest) = text.strip_prefix("publishes-event:") {
                if rest.is_empty() {
                    None
                } else {
                    Some(ProveQuestion::PublishesEvent {
                        event: rest.to_string(),
                    })
                }
            } else {
                None
            }
        }
    }
}

/// Return all valid question id strings (for error messages).
pub fn question_ids() -> &'static [&'static str] {
    &[
        "may-commit",
        "commits-on-success-path",
        "writes-table:<table>",
        "publishes-event:<event>",
        "reaches-ui",
        "throws-error",
    ]
}

// ---------------------------------------------------------------------------
// ProveResult types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveAnswer {
    Yes,
    No,
    Unknown,
}

impl ProveAnswer {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProveAnswer::Yes => "yes",
            ProveAnswer::No => "no",
            ProveAnswer::Unknown => "unknown",
        }
    }
}

/// Anchor for a blocker. `source_kind` is "source" for callsite-derived blockers, but
/// can be "source" OR "symbol" for the non-unconditional-effect-exists blocker (it
/// carries the effect's evidence anchor verbatim — kept whenever sourceKind !=
/// "unavailable", mirroring TS prove-engine.ts:386).
#[derive(Debug, Clone)]
pub struct BlockerAnchor {
    pub source_kind: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProveBlocker {
    pub kind: &'static str,
    pub detail: String,
    pub anchor: Option<BlockerAnchor>,
}

#[derive(Debug, Clone)]
pub struct ProveObligations {
    pub unresolved_callsites: usize,
    pub open_world_rows: usize,
    pub analysis_gaps: usize,
    pub coverage_complete: bool,
    pub cone_truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ProveResult {
    pub answer: ProveAnswer,
    /// Indices into the effect slice for matching effects.
    pub evidence: Option<Vec<usize>>,
    pub blocked_by: Option<Vec<ProveBlocker>>,
    pub obligations: ProveObligations,
}

// ---------------------------------------------------------------------------
// Internal: gaps-in-cone intersection (mirrors digest-query.ts:729-751)
// ---------------------------------------------------------------------------

/// Count analysis gaps whose subject intersects the visited cone.
///
/// A gap subject is EITHER:
///   - a StableRoutineId (parse-incomplete / body-unavailable gaps) → DIRECT match
///     against the visited set, OR
///   - a bare app GUID (symbol-only-boundary gaps, snapshot.rs:1860-1867) → matches
///     when ANY visited routine's stable id starts with `{appGuid}:` (stable ids use
///     ":" separators, NOT "/" — see digest-query.ts:722-728's documented soundness
///     fix). WITHOUT this app-level branch every symbol-only-boundary gap is invisible,
///     so a cone entering an opaque `.app` dep reports analysisGaps=0 → a FALSE absence
///     proof. This is the exact class of bug digest-query.ts documents.
fn count_gaps_in_cone(
    gap_subjects: &[&str],
    visited_ids: &std::collections::HashSet<String>,
) -> usize {
    let mut count = 0usize;
    for subject in gap_subjects {
        // Direct routine-subject match.
        if visited_ids.contains(*subject) {
            count += 1;
            continue;
        }
        // App-level (symbol-only-boundary) match: subject is a bare app GUID and some
        // visited routine has a stable id of the form `{appGuid}:Type:{n}#hash`.
        let prefix = format!("{}:", subject);
        if visited_ids.iter().any(|v| v.starts_with(&prefix)) {
            count += 1;
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Internal: Obligation computation
// ---------------------------------------------------------------------------

fn compute_obligations(entry: &DigestEntryFull, gaps_in_cone_count: usize) -> ProveObligations {
    let mut unresolved_callsites = 0usize;
    let mut open_world_rows = 0usize;

    for item in &entry.unresolved {
        if item.open_world == Some(true) {
            open_world_rows += 1;
        } else {
            // Both "polymorphic" (without openWorld) and non-polymorphic non-openWorld
            // count as unresolved-callsite (mirrors TS prove-engine.ts lines 127-136).
            unresolved_callsites += 1;
        }
    }

    ProveObligations {
        unresolved_callsites,
        open_world_rows,
        analysis_gaps: gaps_in_cone_count,
        coverage_complete: entry.coverage_status == "complete",
        cone_truncated: entry.unresolved_traversal.truncated,
    }
}

fn obligations_clear(oblg: &ProveObligations, witness_truncated: bool) -> bool {
    oblg.unresolved_callsites == 0
        && oblg.open_world_rows == 0
        && oblg.analysis_gaps == 0
        && oblg.coverage_complete
        && !oblg.cone_truncated
        && !witness_truncated
}

fn build_obligation_blockers(
    entry: &DigestEntryFull,
    oblg: &ProveObligations,
    witness_truncated: bool,
) -> Vec<ProveBlocker> {
    let mut blockers: Vec<ProveBlocker> = Vec::new();

    // Unresolved callsites (non-openWorld first, then open-world)
    for item in &entry.unresolved {
        if item.open_world == Some(true) {
            continue;
        }
        let anchor = item.callsite_file.as_ref().map(|f| BlockerAnchor {
            source_kind: "source".to_string(),
            file: Some(f.clone()),
            line: item.callsite_line,
            column: item.callsite_column,
            excerpt: None, // callsite anchors never carry excerpt
        });
        blockers.push(ProveBlocker {
            kind: "unresolved-callsite",
            detail: format!("{} in {}", item.callee_display, item.owning_routine_display),
            anchor,
        });
    }

    // Open-world dispatch rows
    for item in &entry.unresolved {
        if item.open_world != Some(true) {
            continue;
        }
        let anchor = item.callsite_file.as_ref().map(|f| BlockerAnchor {
            source_kind: "source".to_string(),
            file: Some(f.clone()),
            line: item.callsite_line,
            column: item.callsite_column,
            excerpt: None, // callsite anchors never carry excerpt
        });
        blockers.push(ProveBlocker {
            kind: "open-world-dispatch",
            detail: format!("{} in {}", item.callee_display, item.owning_routine_display),
            anchor,
        });
    }

    if oblg.analysis_gaps > 0 {
        blockers.push(ProveBlocker {
            kind: "analysis-gap",
            detail: format!("{} analysis gap(s) in cone", oblg.analysis_gaps),
            anchor: None,
        });
    }

    if !oblg.coverage_complete {
        blockers.push(ProveBlocker {
            kind: "coverage-incomplete",
            detail: format!("coverage status: {}", entry.coverage_status),
            anchor: None,
        });
    }

    if oblg.cone_truncated {
        blockers.push(ProveBlocker {
            kind: "cone-truncated",
            detail: "transitive cone BFS was truncated".to_string(),
            anchor: None,
        });
    }

    if witness_truncated {
        blockers.push(ProveBlocker {
            kind: "witness-truncated",
            detail: "witness path reconstruction was truncated or evidence unavailable".to_string(),
            anchor: None,
        });
    }

    blockers
}

/// Deterministic blocker sort key: NUL-delimited (kind, anchor file, 10-digit line, detail).
fn blocker_sort_key(b: &ProveBlocker) -> String {
    let file = b
        .anchor
        .as_ref()
        .and_then(|a| a.file.as_deref())
        .unwrap_or("");
    let line = b.anchor.as_ref().and_then(|a| a.line).unwrap_or(0);
    let line_str = format!("{:010}", line);
    format!("{}\x00{}\x00{}\x00{}", b.kind, file, line_str, b.detail)
}

fn sort_blockers(mut blockers: Vec<ProveBlocker>) -> Vec<ProveBlocker> {
    blockers.sort_by(|a, b| {
        let ka = blocker_sort_key(a);
        let kb = blocker_sort_key(b);
        // Ordinal `.cmp()` is correct here: every key component is ASCII (kind literals,
        // normalized ws:/app: paths, zero-padded digits, ASCII details), so Rust's byte
        // ordering == TS's UTF-16 code-unit ordering. This is the engine-wide tracked
        // UTF-16-comparator item — consistent with the rest of the engine.
        ka.cmp(&kb)
    });
    blockers
}

// ---------------------------------------------------------------------------
// Effect predicate matching
// ---------------------------------------------------------------------------

const DB_WRITE_TYPES: &[&str] = &["DB_INSERT", "DB_MODIFY", "DB_DELETE"];
const UI_TYPES: &[&str] = &["UI_MESSAGE", "UI_CONFIRM", "UI_ERROR"];

/// Case-insensitive match of `query` against the resourceDisplay or the
/// trailing segment of the resourceId (after the last '/').
fn resource_name_matches(detail: &[(String, String)], query: &str) -> bool {
    let target = query.to_lowercase();

    let mut resource_display: Option<&str> = None;
    let mut resource_id: Option<&str> = None;

    for (k, v) in detail {
        if k == "resourceDisplay" {
            resource_display = Some(v.as_str());
        }
        if k == "resourceId" {
            resource_id = Some(v.as_str());
        }
    }

    if let Some(disp) = resource_display
        && disp.to_lowercase() == target
    {
        return true;
    }

    if let Some(rid) = resource_id
        && !rid.is_empty()
    {
        let trailing = match rid.rfind('/') {
            Some(sep) => &rid[sep + 1..],
            None => rid,
        };
        if trailing.to_lowercase() == target {
            return true;
        }
    }

    false
}

fn effect_matches(eff: &DigestEffectFull, question: &ProveQuestion) -> bool {
    match question {
        ProveQuestion::MayCommit => eff.effect_type == "COMMIT",
        ProveQuestion::CommitsOnSuccessPath => eff.effect_type == "COMMIT",
        ProveQuestion::WritesTable { table } => {
            if !DB_WRITE_TYPES.contains(&eff.effect_type.as_str()) {
                return false;
            }
            resource_name_matches(&eff.detail, table)
        }
        ProveQuestion::PublishesEvent { event } => {
            if eff.effect_type != "EVENT_PUBLISH" {
                return false;
            }
            resource_name_matches(&eff.detail, event)
        }
        ProveQuestion::ReachesUi => UI_TYPES.contains(&eff.effect_type.as_str()),
        ProveQuestion::ThrowsError => eff.effect_type == "ERROR_THROW",
    }
}

// ---------------------------------------------------------------------------
// MAY-question evaluation
// ---------------------------------------------------------------------------

fn evaluate_may_question(
    entry: &DigestEntryFull,
    oblg: ProveObligations,
    question: &ProveQuestion,
    witness_truncated: bool,
) -> ProveResult {
    let matching: Vec<usize> = entry
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| effect_matches(e, question))
        .map(|(i, _)| i)
        .collect();

    if !matching.is_empty() {
        return ProveResult {
            answer: ProveAnswer::Yes,
            evidence: Some(matching),
            blocked_by: None,
            obligations: oblg,
        };
    }

    if obligations_clear(&oblg, witness_truncated) {
        return ProveResult {
            answer: ProveAnswer::No,
            evidence: None,
            blocked_by: None,
            obligations: oblg,
        };
    }

    let blockers = build_obligation_blockers(entry, &oblg, witness_truncated);
    ProveResult {
        answer: ProveAnswer::Unknown,
        evidence: None,
        blocked_by: Some(sort_blockers(blockers)),
        obligations: oblg,
    }
}

// ---------------------------------------------------------------------------
// MUST-question evaluation (commits-on-success-path)
// ---------------------------------------------------------------------------

fn evaluate_must_question(
    entry: &DigestEntryFull,
    oblg: ProveObligations,
    witness_truncated: bool,
) -> ProveResult {
    let commit_indices: Vec<usize> = entry
        .effects
        .iter()
        .enumerate()
        .filter(|(_, e)| e.effect_type == "COMMIT")
        .map(|(i, _)| i)
        .collect();

    let unconditional_indices: Vec<usize> = commit_indices
        .iter()
        .copied()
        .filter(|&i| entry.effects[i].conditionality == UNCONDITIONAL)
        .collect();

    let non_unconditional_indices: Vec<usize> = commit_indices
        .iter()
        .copied()
        .filter(|&i| entry.effects[i].conditionality != UNCONDITIONAL)
        .collect();

    // YES: ≥1 unconditional COMMIT AND all obligations clear
    if !unconditional_indices.is_empty() && obligations_clear(&oblg, witness_truncated) {
        return ProveResult {
            answer: ProveAnswer::Yes,
            evidence: Some(unconditional_indices),
            blocked_by: None,
            obligations: oblg,
        };
    }

    // NO: zero COMMIT effects of ANY conditionality AND obligations clear
    if commit_indices.is_empty() && obligations_clear(&oblg, witness_truncated) {
        return ProveResult {
            answer: ProveAnswer::No,
            evidence: None,
            blocked_by: None,
            obligations: oblg,
        };
    }

    // UNKNOWN: everything else — obligation blockers + non-unconditional-effect-exists
    let mut blockers: Vec<ProveBlocker> = Vec::new();

    // Obligation blockers first
    let ob = build_obligation_blockers(entry, &oblg, witness_truncated);
    blockers.extend(ob);

    // non-unconditional-effect-exists blockers for each non-unconditional COMMIT
    for idx in &non_unconditional_indices {
        let eff = &entry.effects[*idx];
        // Anchor from effect evidence — full evidence anchor (includes excerpt),
        // kept when sourceKind !== "unavailable" (i.e. BOTH "source" AND "symbol"),
        // mirroring TS prove-engine.ts:386. A COMMIT from a `.app` symbol routine
        // (sourceKind "symbol") still carries its anchor + excerpt.
        let anchor = if eff.evidence.source_kind != "unavailable" {
            Some(BlockerAnchor {
                source_kind: eff.evidence.source_kind.to_string(),
                file: eff.evidence.file.clone(),
                line: eff.evidence.line,
                column: eff.evidence.column,
                excerpt: eff.evidence.excerpt.clone(),
            })
        } else {
            None
        };
        blockers.push(ProveBlocker {
            kind: "non-unconditional-effect-exists",
            detail: format!("COMMIT with conditionality \"{}\"", eff.conditionality),
            anchor,
        });
    }

    let evidence = if !commit_indices.is_empty() {
        Some(commit_indices)
    } else {
        None
    };

    ProveResult {
        answer: ProveAnswer::Unknown,
        evidence,
        blocked_by: Some(sort_blockers(blockers)),
        obligations: oblg,
    }
}

// ---------------------------------------------------------------------------
// prove — main tristate evaluation
// ---------------------------------------------------------------------------

pub fn prove(
    entry: &DigestEntryFull,
    gaps_in_cone_count: usize,
    question: &ProveQuestion,
) -> ProveResult {
    let oblg = compute_obligations(entry, gaps_in_cone_count);

    let witness_truncated = entry.effects.iter().any(|e| e.via_paths_truncated)
        || entry
            .entry_diagnostics
            .iter()
            .any(|d| d.kind == "evidence-unavailable");

    match question {
        ProveQuestion::CommitsOnSuccessPath => {
            evaluate_must_question(entry, oblg, witness_truncated)
        }
        _ => evaluate_may_question(entry, oblg, question, witness_truncated),
    }
}

// ---------------------------------------------------------------------------
// JSON anchor helper
// ---------------------------------------------------------------------------

fn anchor_to_json(
    source_kind: &str,
    file: Option<&str>,
    line: Option<u32>,
    column: Option<u32>,
    excerpt: Option<&str>,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("sourceKind".into(), source_kind.into());
    if let Some(f) = file {
        m.insert("file".into(), f.into());
    }
    if let Some(l) = line {
        m.insert("line".into(), l.into());
    }
    if let Some(c) = column {
        m.insert("column".into(), c.into());
    }
    if let Some(x) = excerpt {
        m.insert("excerpt".into(), x.into());
    }
    serde_json::Value::Object(m)
}

fn blocker_to_value(b: &ProveBlocker) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(ref a) = b.anchor {
        let anch = anchor_to_json(
            &a.source_kind,
            a.file.as_deref(),
            a.line,
            a.column,
            a.excerpt.as_deref(),
        );
        m.insert("anchor".into(), anch);
    }
    m.insert("detail".into(), b.detail.clone().into());
    m.insert("kind".into(), b.kind.into());
    serde_json::Value::Object(m)
}

fn obligations_to_value(o: &ProveObligations) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("analysisGaps".into(), (o.analysis_gaps as u64).into());
    m.insert("coneTruncated".into(), o.cone_truncated.into());
    m.insert("coverageComplete".into(), o.coverage_complete.into());
    m.insert("openWorldRows".into(), (o.open_world_rows as u64).into());
    m.insert(
        "unresolvedCallsites".into(),
        (o.unresolved_callsites as u64).into(),
    );
    serde_json::Value::Object(m)
}

// ---------------------------------------------------------------------------
// project_prove_document
// ---------------------------------------------------------------------------

pub struct ProveDocumentArgs<'a> {
    pub workspace_fp: &'a str,
    pub routine_stable_id: &'a str,
    pub routine_display: &'a str,
    pub object_display: &'a str,
    pub routine_anchor: Option<&'a FullAnchor>,
    pub question: &'a str,
    pub prove_result: &'a ProveResult,
    pub effects: &'a [DigestEffectFull],
    pub diagnostics_json: serde_json::Value,
    pub alsem_ver: &'a str,
    pub deterministic: bool,
}

/// Build the DocumentEnvelope<"prove", ProvePayload> as a serde_json::Value,
/// then serialize with serialize_document_value (sorted keys, null-drop, trailing \n).
pub fn project_prove_document(args: ProveDocumentArgs<'_>) -> String {
    let ProveDocumentArgs {
        workspace_fp,
        routine_stable_id,
        routine_display,
        object_display,
        routine_anchor,
        question,
        prove_result,
        effects,
        diagnostics_json,
        alsem_ver,
        deterministic,
    } = args;

    // result sub-object
    let mut result_obj = serde_json::Map::new();
    result_obj.insert("answer".into(), prove_result.answer.as_str().into());

    // evidence — only when present (and non-empty)
    if let Some(ref indices) = prove_result.evidence
        && !indices.is_empty()
    {
        // Re-use digest effect_to_value to produce the same DigestEffectContract shape.
        let ev: Vec<serde_json::Value> = indices
            .iter()
            .map(|&i| crate::engine::l5::digest_cli::effect_to_value(&effects[i]))
            .collect();
        result_obj.insert("evidence".into(), serde_json::Value::Array(ev));
    }

    // blockedBy — only when present (and non-empty)
    if let Some(ref blockers) = prove_result.blocked_by
        && !blockers.is_empty()
    {
        let bv: Vec<serde_json::Value> = blockers.iter().map(blocker_to_value).collect();
        result_obj.insert("blockedBy".into(), serde_json::Value::Array(bv));
    }

    result_obj.insert(
        "obligations".into(),
        obligations_to_value(&prove_result.obligations),
    );

    // routine ref
    let mut routine_ref = serde_json::Map::new();
    if let Some(anch) = routine_anchor {
        let anch_val = anchor_to_json(
            anch.source_kind,
            anch.file.as_deref(),
            anch.line,
            anch.column,
            anch.excerpt.as_deref(),
        );
        routine_ref.insert("anchor".into(), anch_val);
    }
    routine_ref.insert("display".into(), routine_display.into());
    routine_ref.insert("objectDisplay".into(), object_display.into());
    routine_ref.insert("stableId".into(), routine_stable_id.into());

    // payload
    let mut payload = serde_json::Map::new();
    payload.insert("question".into(), question.into());
    payload.insert("result".into(), serde_json::Value::Object(result_obj));
    payload.insert("routine".into(), serde_json::Value::Object(routine_ref));
    payload.insert("workspaceFingerprint".into(), workspace_fp.into());

    // envelope
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    let mut env = serde_json::Map::new();
    env.insert("alsemVersion".into(), alsem_ver.into());
    env.insert("deterministic".into(), deterministic.into());
    env.insert("diagnostics".into(), diagnostics_json);
    env.insert("generatedAt".into(), generated_at.into());
    env.insert("kind".into(), "prove".into());
    env.insert("payload".into(), serde_json::Value::Object(payload));
    env.insert("schemaVersion".into(), "1.2.0".into());

    serialize_document_value(serde_json::Value::Object(env))
}

// ---------------------------------------------------------------------------
// Human formatter (formatProveHuman)
// ---------------------------------------------------------------------------

pub fn format_prove_human(
    routine_display: &str,
    object_display: &str,
    question: &str,
    prove_result: &ProveResult,
    effects: &[DigestEffectFull],
) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Routine header
    let routine_header = if !object_display.is_empty() {
        format!("{}::{}", object_display, routine_display)
    } else {
        routine_display.to_string()
    };
    lines.push(format!("--- {} ---", routine_header));
    lines.push(format!("  question: {}", question));
    lines.push(format!("  answer:   {}", prove_result.answer.as_str()));

    // Evidence
    if let Some(ref indices) = prove_result.evidence
        && !indices.is_empty()
    {
        lines.push(format!("  evidence: {} effect(s)", indices.len()));
        for &i in indices {
            let eff = &effects[i];
            let ev_file = eff.evidence.file.as_deref().unwrap_or("(unavailable)");
            let ev_line = eff
                .evidence
                .line
                .map(|l| format!(":{}", l))
                .unwrap_or_default();
            // Mirror TS: `[${eff.type}] ${eff.provenance} — ${evFile}${evLine}`
            // The em-dash is U+2014. `conditionality` is already a &'static str.
            lines.push(format!(
                "    [{}] {} \u{2014} {}{}",
                eff.effect_type, eff.provenance, ev_file, ev_line
            ));
            if eff.conditionality != "unknown" {
                lines.push(format!("      conditionality: {}", eff.conditionality));
            }
        }
    }

    // Blockers
    if let Some(ref blockers) = prove_result.blocked_by
        && !blockers.is_empty()
    {
        lines.push(format!("  blockedBy: {} blocker(s)", blockers.len()));
        for b in blockers {
            // TS (prove.ts:60-63) gates on `anchor?.file !== undefined` — present
            // (even if "", which never occurs in practice), NOT on non-empty.
            let anchor_str = match b.anchor.as_ref().and_then(|a| a.file.as_ref()) {
                Some(f) => {
                    let line_str = b
                        .anchor
                        .as_ref()
                        .and_then(|a| a.line)
                        .map(|l| format!(":{}", l))
                        .unwrap_or_default();
                    format!(" @ {}{}", f, line_str)
                }
                None => String::new(),
            };
            lines.push(format!("    [{}] {}{}", b.kind, b.detail, anchor_str));
        }
    }

    // Obligations summary
    let o = &prove_result.obligations;
    let mut parts: Vec<String> = Vec::new();

    if o.unresolved_callsites > 0 {
        parts.push(format!("unresolved={}", o.unresolved_callsites));
    }
    if o.open_world_rows > 0 {
        parts.push(format!("openWorld={}", o.open_world_rows));
    }
    if o.analysis_gaps > 0 {
        parts.push(format!("gaps={}", o.analysis_gaps));
    }
    if !o.coverage_complete {
        parts.push("coverage=incomplete".to_string());
    }
    if o.cone_truncated {
        parts.push("cone=truncated".to_string());
    }

    if parts.is_empty() {
        lines.push("  obligations: all clear".to_string());
    } else {
        lines.push(format!("  obligations: {}", parts.join(", ")));
    }

    lines.push(String::new());
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Transaction context (mirrors digest_cli's compute_tx_context_map)
// ---------------------------------------------------------------------------

fn compute_tx_context_map(resolved: &L3Resolved) -> HashMap<String, &'static str> {
    // ⟨C1 Task 3 — R2/R3 caller classification⟩ DERIVED-ONLY: plain `ALL`, no
    // `RAW_INHERITED_FACTS`. Same shape as `digest_cli::compute_tx_context_map` —
    // only `ctx.transaction_spans` is read, and spans are derived-substrate-backed
    // since Task 2.
    let ctx = build_detector_context(resolved, crate::engine::l5::registry::substrate::ALL);
    let mut map: HashMap<String, &'static str> = HashMap::new();
    for span in &ctx.transaction_spans {
        if span.seed_kind != SeedKind::ExplicitCommit {
            continue;
        }
        let tc: &'static str = if !span.coverage_complete {
            TX_UNKNOWN
        } else if !span.writes_tables.is_empty() {
            TX_SPAN_KNOWN_WRITES
        } else {
            TX_SPAN_NO_WRITES
        };
        map.insert(span.commit_operation_id.clone(), tc);
    }
    map
}

// ---------------------------------------------------------------------------
// ProveRunResult
// ---------------------------------------------------------------------------

pub struct ProveRunResult {
    pub json_text: String,
    pub human_text: String,
    /// Exit code: 0 = answered, 2 = routine not resolved (dummy-doc)
    pub exit_code: u8,
}

/// Run the full prove pipeline for a workspace + routine selector + question text.
///
/// `maxPaths` is hardcoded to 3 (prove.ts:124) inside the digest substrate — there is
/// no caller-tunable knob, so no parameter is exposed.
///
/// Returns:
///   Ok(result) with exit_code 0 (answered) or 2 (routine not resolved → dummy-doc).
///   Err(message) for workspace/pipeline errors → emit to stderr, exit 1.
pub fn run_prove_pipeline(
    workspace: &std::path::Path,
    routine_selector: &str,
    question_text: &str,
    alsem_ver: &str,
    deterministic: bool,
) -> Result<ProveRunResult, String> {
    use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;

    // Assemble workspace
    let model_id = compute_gate_model_instance_id(workspace)
        .ok_or_else(|| "prove: could not compute modelInstanceId".to_string())?;
    // Engine-switch S6.1: the program-backed model (program calls and events).
    let resolved = assemble_and_resolve_workspace_program(workspace, &model_id, false)
        .ok_or_else(|| "prove: workspace did not resolve".to_string())?;

    // Compose snapshot
    let snap = compose_snapshot(&resolved);
    let workspace_fp =
        crate::engine::l5::snapshot_full::workspace_fingerprint_of(workspace, alsem_ver);

    // Build envelope diagnostics (same 34-detector set, shared with digest).
    let diagnostics_json = build_envelope_diagnostics_json(workspace, &resolved);

    // Resolve the routine selector → root IDs
    let changed_input = ChangedInput {
        files: None,
        routines: Some(vec![routine_selector.to_string()]),
        diff_text: None,
    };
    let changed_roots = resolve_changed_roots(&snap, &changed_input);

    // EXIT CODE 2: routine not resolved (zero roots → dummy-doc)
    if changed_roots.roots.is_empty() {
        // Dummy ProveResult: answer=unknown, zeroed obligations (coverageComplete=false)
        let dummy_oblg = ProveObligations {
            unresolved_callsites: 0,
            open_world_rows: 0,
            analysis_gaps: 0,
            coverage_complete: false,
            cone_truncated: false,
        };
        let dummy_result = ProveResult {
            answer: ProveAnswer::Unknown,
            evidence: None,
            blocked_by: None,
            obligations: dummy_oblg,
        };

        let json_text = project_prove_document(ProveDocumentArgs {
            workspace_fp: &workspace_fp,
            routine_stable_id: routine_selector,
            routine_display: routine_selector,
            object_display: "",
            routine_anchor: None,
            question: question_text,
            prove_result: &dummy_result,
            effects: &[],
            diagnostics_json,
            alsem_ver,
            deterministic,
        });

        let human_text =
            format_prove_human(routine_selector, "", question_text, &dummy_result, &[]);

        return Ok(ProveRunResult {
            json_text,
            human_text,
            exit_code: 2,
        });
    }

    // Use first root (prove always passes a single routine selector)
    let root_id = &changed_roots.roots[0];

    // Compute transaction context map
    let tx_ctx_map = compute_tx_context_map(&resolved);

    // Compute S4 ordering effects for all routines
    let all_s4_entries = compute_digest_effects_cli(&snap, &resolved);

    // Run full digest query for this single root
    let query_full = run_digest_query_full_from_entries(
        &snap,
        std::slice::from_ref(root_id),
        &all_s4_entries,
        &tx_ctx_map,
    );

    if query_full.entries.is_empty() {
        return Err("prove: internal error — entry missing after resolution".to_string());
    }

    let entry = &query_full.entries[0];

    // Compute gaps_in_cone count: re-run unresolved_cone to get the visited_ids,
    // then count snap.analysis_gaps whose subject intersects the cone.
    let cone_result = unresolved_cone(&snap, root_id);
    let gap_subjects: Vec<&str> = snap
        .analysis_gaps
        .iter()
        .map(|g| g.subject.as_str())
        .collect();
    let gaps_in_cone_count = count_gaps_in_cone(&gap_subjects, &cone_result.visited_ids);

    // Parse question (already validated at CLI layer, but re-check cleanly)
    let parsed_question = parse_question(question_text)
        .ok_or_else(|| format!("prove: unknown question '{}'", question_text))?;

    // Run tristate evaluation
    let prove_result = prove(entry, gaps_in_cone_count, &parsed_question);

    // Project document
    let json_text = project_prove_document(ProveDocumentArgs {
        workspace_fp: &workspace_fp,
        routine_stable_id: root_id,
        routine_display: &entry.routine_display,
        object_display: &entry.object_display,
        routine_anchor: entry.routine_anchor.as_ref(),
        question: question_text,
        prove_result: &prove_result,
        effects: &entry.effects,
        diagnostics_json,
        alsem_ver,
        deterministic,
    });

    let human_text = format_prove_human(
        &entry.routine_display,
        &entry.object_display,
        question_text,
        &prove_result,
        &entry.effects,
    );

    Ok(ProveRunResult {
        json_text,
        human_text,
        exit_code: 0,
    })
}

// ---------------------------------------------------------------------------
// Native oracles for corpus-invisible blocker types
//
// The prove corpus (18 entries) exercises: unresolved-callsite,
// non-unconditional-effect-exists, coverage-incomplete. The following types
// are NOT in any golden but ARE in the TS source — we verify them unit-wise:
//   - open-world-dispatch  (item.openWorld == true → separate blocker kind)
//   - analysis-gap         (gapsInCone.length > 0)
//   - cone-truncated       (entry.unresolvedTraversal.truncated == true)
//   - witness-truncated    (any effect.viaPathsTruncated OR evidence-unavailable diag)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::digest_cli::{
        DigestEffectFull, DigestEntryFull, EntryDiagnostic, FullAnchor,
    };
    use crate::engine::l5::unresolved_cone::{UnresolvedConeItem, UnresolvedTraversal};

    /// Build a minimal DigestEntryFull for oracle tests.
    fn minimal_entry(
        coverage_status: &str,
        unresolved: Vec<UnresolvedConeItem>,
        effects: Vec<DigestEffectFull>,
        entry_diagnostics: Vec<EntryDiagnostic>,
        cone_truncated: bool,
    ) -> DigestEntryFull {
        DigestEntryFull {
            routine_id: "test-routine".to_string(),
            routine_display: "TestRoutine".to_string(),
            object_display: "".to_string(),
            routine_anchor: None,
            coverage_status: coverage_status.to_string(),
            coverage_reasons: Vec::new(),
            effects,
            unresolved,
            unresolved_traversal: UnresolvedTraversal {
                truncated: cone_truncated,
                max_depth: 64,
                visited_routines: 1,
            },
            entry_diagnostics,
        }
    }

    fn unresolved_item(open_world: Option<bool>) -> UnresolvedConeItem {
        UnresolvedConeItem {
            callsite_file: Some("ws:src/Test.al".to_string()),
            callsite_line: Some(10),
            callsite_column: Some(8),
            callee_display: "SomeCallee".to_string(),
            status: if open_world == Some(true) {
                "open-world".to_string()
            } else {
                "unresolved".to_string()
            },
            candidates: None,
            open_world,
            owning_routine: "test-routine".to_string(),
            owning_routine_display: "TestRoutine".to_string(),
        }
    }

    // -- open-world-dispatch oracle ------------------------------------------

    /// An unresolved item with openWorld=true produces an open-world-dispatch
    /// blocker (not an unresolved-callsite blocker).
    #[test]
    fn open_world_dispatch_blocker_kind() {
        let entry = minimal_entry(
            "complete",
            vec![unresolved_item(Some(true))],
            vec![],
            vec![],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "open-world-dispatch");
        // obligations: openWorldRows=1, unresolvedCallsites=0
        assert_eq!(result.obligations.open_world_rows, 1);
        assert_eq!(result.obligations.unresolved_callsites, 0);
    }

    /// Verify that open-world rows do NOT contribute to unresolvedCallsites.
    #[test]
    fn open_world_counted_separately_from_unresolved() {
        let entry = minimal_entry(
            "complete",
            vec![
                unresolved_item(None),       // → unresolvedCallsites
                unresolved_item(Some(true)), // → openWorldRows
            ],
            vec![],
            vec![],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.obligations.unresolved_callsites, 1);
        assert_eq!(result.obligations.open_world_rows, 1);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        // Two blockers: one unresolved-callsite, one open-world-dispatch
        assert_eq!(blockers.len(), 2);
        let kinds: Vec<&str> = blockers.iter().map(|b| b.kind).collect();
        assert!(kinds.contains(&"unresolved-callsite"));
        assert!(kinds.contains(&"open-world-dispatch"));
    }

    // -- analysis-gap oracle ------------------------------------------------

    /// Gaps in cone produce an analysis-gap blocker with count in the detail.
    #[test]
    fn analysis_gap_blocker_kind() {
        let entry = minimal_entry("complete", vec![], vec![], vec![], false);
        let result = prove(&entry, 3, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "analysis-gap");
        assert!(
            blockers[0].detail.contains("3"),
            "detail should mention gap count: {}",
            blockers[0].detail
        );
        assert_eq!(result.obligations.analysis_gaps, 3);
    }

    // -- cone-truncated oracle ----------------------------------------------

    /// cone_truncated=true produces a cone-truncated blocker.
    #[test]
    fn cone_truncated_blocker_kind() {
        let entry = minimal_entry("complete", vec![], vec![], vec![], true);
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].kind, "cone-truncated");
        assert!(result.obligations.cone_truncated);
    }

    // -- witness-truncated oracle -------------------------------------------

    /// An effect with viaPathsTruncated=true → witness-truncated blocker
    /// (even though there is NO matching effect, so it's unknown, not yes).
    #[test]
    fn witness_truncated_via_paths_truncated() {
        use crate::engine::l5::conditionality::UNKNOWN;
        let effect = DigestEffectFull {
            effect_type: "EVENT_PUBLISH".to_string(),
            detail: vec![],
            provenance: "direct",
            evidence: FullAnchor {
                source_kind: "source",
                file: Some("ws:src/Test.al".to_string()),
                line: Some(5),
                column: Some(4),
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: true, // ← the key flag
            conditionality: UNKNOWN,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "abcdef1234567890".to_string(),
            scoped_guarantees: vec![],
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let wt = blockers.iter().find(|b| b.kind == "witness-truncated");
        assert!(
            wt.is_some(),
            "expected witness-truncated blocker; got: {:?}",
            blockers.iter().map(|b| b.kind).collect::<Vec<_>>()
        );
    }

    /// An entry diagnostic with kind "evidence-unavailable" also sets
    /// witness_truncated and produces a witness-truncated blocker.
    #[test]
    fn witness_truncated_via_evidence_unavailable_diagnostic() {
        let entry = minimal_entry(
            "complete",
            vec![],
            vec![],
            vec![EntryDiagnostic {
                kind: "evidence-unavailable",
                effect_type: Some("COMMIT".to_string()),
                fact_subject: Some("some-fact".to_string()),
            }],
            false,
        );
        let result = prove(&entry, 0, &ProveQuestion::MayCommit);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let wt = blockers.iter().find(|b| b.kind == "witness-truncated");
        assert!(
            wt.is_some(),
            "expected witness-truncated blocker; got: {:?}",
            blockers.iter().map(|b| b.kind).collect::<Vec<_>>()
        );
    }

    // -- Blocker sort oracle ------------------------------------------------

    /// Blockers are sorted deterministically by the FULL key tier:
    /// (kind, anchor file, anchor line, detail). This drives all four tiers:
    ///   - kind:   "analysis-gap" < "open-world-dispatch" < "unresolved-callsite"
    ///   - file:   same file for the two unresolved-callsite rows
    ///   - line:   5 before 20
    ///   - detail: "A in TestRoutine" < "Z in TestRoutine"
    #[test]
    fn blockers_are_deterministically_sorted() {
        // Two unresolved-callsite items in reverse callee/line order, one open-world
        // item, plus an analysis gap (count > 0). The expected sorted output exercises
        // every tier of the key.
        let entry = minimal_entry(
            "complete",
            vec![
                {
                    let mut item = unresolved_item(None);
                    item.callee_display = "Z".to_string();
                    item.callsite_line = Some(20);
                    item
                },
                {
                    let mut item = unresolved_item(None);
                    item.callee_display = "A".to_string();
                    item.callsite_line = Some(5);
                    item
                },
                {
                    let mut item = unresolved_item(Some(true));
                    item.callee_display = "OW".to_string();
                    item.callsite_line = Some(9);
                    item
                },
            ],
            vec![],
            vec![],
            false,
        );
        // gaps_in_cone_count = 2 → one analysis-gap blocker
        let result = prove(&entry, 2, &ProveQuestion::MayCommit);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);

        // Expected canonical order: analysis-gap < open-world-dispatch < unresolved-callsite,
        // and within unresolved-callsite (same file) by line then detail.
        let seq: Vec<(&str, Option<u32>, &str)> = blockers
            .iter()
            .map(|b| {
                (
                    b.kind,
                    b.anchor.as_ref().and_then(|a| a.line),
                    b.detail.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seq,
            vec![
                ("analysis-gap", None, "2 analysis gap(s) in cone"),
                ("open-world-dispatch", Some(9), "OW in TestRoutine"),
                ("unresolved-callsite", Some(5), "A in TestRoutine"),
                ("unresolved-callsite", Some(20), "Z in TestRoutine"),
            ],
            "blockers must be sorted by (kind, file, line, detail) tiers"
        );
    }

    // -- #1 symbol-evidence anchor survives oracle --------------------------

    /// A non-unconditional COMMIT whose evidence has sourceKind "symbol" (a `.app`
    /// dependency routine) STILL produces a blocker with the FULL anchor (incl. excerpt),
    /// because the TS gate is `sourceKind !== "unavailable"` (BOTH source AND symbol),
    /// not `== "source"`. Corpus-invisible — the prove corpus only has "source" commits.
    #[test]
    fn non_unconditional_symbol_evidence_keeps_anchor() {
        use crate::engine::l5::conditionality::LOOP_BODY;
        let effect = DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "transitive",
            evidence: FullAnchor {
                source_kind: "symbol", // ← from a .app symbol routine
                file: Some("app:dep/src/Sym.al".to_string()),
                line: Some(42),
                column: Some(8),
                excerpt: Some("Commit()".to_string()),
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: LOOP_BODY, // non-unconditional
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "1234567890abcdef".to_string(),
            scoped_guarantees: vec![],
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::CommitsOnSuccessPath);
        assert_eq!(result.answer, ProveAnswer::Unknown);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let nu = blockers
            .iter()
            .find(|b| b.kind == "non-unconditional-effect-exists")
            .expect("expected non-unconditional-effect-exists blocker");
        let anchor = nu
            .anchor
            .as_ref()
            .expect("symbol-evidence anchor must survive (sourceKind != unavailable)");
        assert_eq!(anchor.source_kind, "symbol");
        assert_eq!(anchor.file.as_deref(), Some("app:dep/src/Sym.al"));
        assert_eq!(anchor.line, Some(42));
        assert_eq!(
            anchor.excerpt.as_deref(),
            Some("Commit()"),
            "the full evidence anchor (incl. excerpt) must be kept"
        );
    }

    /// Counter-case: an "unavailable" evidence anchor on a non-unconditional COMMIT
    /// drops the blocker anchor (the only sourceKind that drops it).
    #[test]
    fn non_unconditional_unavailable_evidence_drops_anchor() {
        use crate::engine::l5::conditionality::LOOP_BODY;
        let effect = DigestEffectFull {
            effect_type: "COMMIT".to_string(),
            detail: vec![],
            provenance: "transitive",
            evidence: FullAnchor {
                source_kind: "unavailable",
                file: None,
                line: None,
                column: None,
                excerpt: None,
            },
            via_paths: vec![],
            via_paths_truncated: false,
            conditionality: LOOP_BODY,
            transaction_context: "unknown",
            guarantees: vec![],
            fact_id: "fedcba0987654321".to_string(),
            scoped_guarantees: vec![],
        };
        let entry = minimal_entry("complete", vec![], vec![effect], vec![], false);
        let result = prove(&entry, 0, &ProveQuestion::CommitsOnSuccessPath);
        let blockers = result.blocked_by.as_deref().unwrap_or(&[]);
        let nu = blockers
            .iter()
            .find(|b| b.kind == "non-unconditional-effect-exists")
            .expect("expected non-unconditional-effect-exists blocker");
        assert!(
            nu.anchor.is_none(),
            "unavailable evidence must drop the blocker anchor"
        );
    }

    // -- #2 gaps-in-cone app-level prefix oracle ----------------------------

    /// The SOUNDNESS fix: a symbol-only-boundary gap whose subject is a bare app GUID
    /// is counted when ANY visited routine's stable id starts with `{appGuid}:`. A
    /// direct-match-only filter (the pre-fix bug) would report 0 → a false absence proof.
    #[test]
    fn gaps_in_cone_matches_app_level_prefix() {
        use std::collections::HashSet;
        // Visited routine inside an opaque dep app GUID "11112222...".
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert("11112222-0000-0000-0000-00000000aaaa:Codeunit:50000#deadbeef".to_string());
        visited.insert("33334444-0000-0000-0000-00000000bbbb:Codeunit:60000#cafef00d".to_string());

        // Gap subjects: one bare app GUID (symbol-only-boundary) that matches the prefix,
        // one bare app GUID that does NOT match any visited routine.
        let gap_subjects = vec![
            "11112222-0000-0000-0000-00000000aaaa", // matches via `{guid}:` prefix
            "99998888-0000-0000-0000-00000000cccc", // no visited routine in this app
        ];
        let count = count_gaps_in_cone(&gap_subjects, &visited);
        assert_eq!(
            count, 1,
            "exactly the app-level-matching gap is in cone (the prefix branch must fire)"
        );
    }

    /// Direct routine-subject gaps still match (the non-app branch).
    #[test]
    fn gaps_in_cone_matches_direct_routine_subject() {
        use std::collections::HashSet;
        let mut visited: HashSet<String> = HashSet::new();
        let rid = "aaaa1111-0000-0000-0000-00000000dddd:Codeunit:70000#beadfeed".to_string();
        visited.insert(rid.clone());

        // A parse-incomplete gap whose subject IS a visited StableRoutineId.
        let gap_subjects = vec![rid.as_str()];
        let count = count_gaps_in_cone(&gap_subjects, &visited);
        assert_eq!(count, 1, "direct routine-subject gap must match");
    }

    /// Negative: a gap subject neither directly present nor an app-prefix of any
    /// visited routine is NOT counted.
    #[test]
    fn gaps_in_cone_excludes_unrelated_subject() {
        use std::collections::HashSet;
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert("aaaa1111-0000-0000-0000-00000000dddd:Codeunit:70000#beadfeed".to_string());

        let gap_subjects = vec!["zzzz9999-0000-0000-0000-00000000eeee"];
        let count = count_gaps_in_cone(&gap_subjects, &visited);
        assert_eq!(count, 0, "unrelated gap subject must not be counted");
    }
}
