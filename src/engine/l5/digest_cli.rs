//! cli-b/b1 — the DIGEST CLI command: `alsem digest <ws>`.
//!
//! Ports:
//!   - `src/digest/changed-roots.ts`      → resolve_changed_roots
//!   - `src/digest/digest-query.ts`       → digest_query_full (order:false path)
//!   - `src/contracts/digest.ts`          → project_digest_document + serialize
//!   - `src/cli/digest.ts`                → format_digest_human + run_digest
//!
//! The full-document path adds:
//!   - conditionality (per-path + effect-level)
//!   - transactionContext for COMMIT effects
//!   - unresolved-cone BFS
//!   - guarantees / factId / scopedGuarantees (from ordering engine)
//!   - project_digest_document → JSON envelope (sorted, null-drop, 2-space, trailing \n)
//!   - formatDigestHuman → human text

use std::collections::HashMap;

use crate::engine::gate::format_json::serialize_document_value;
use crate::engine::gate::model_instance_id::compute_gate_model_instance_id;
use crate::engine::gate::run::compute_analyzer_diagnostics;
use crate::engine::l3::l3_workspace::L3Resolved;
use crate::engine::l3::program_calls::assemble_and_resolve_workspace_program;
use crate::engine::l5::conditionality::EffectConditionality;
use crate::engine::l5::detector_context::build_detector_context;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::diff_parser::{DiffFileKind, parse_unified_diff};
use crate::engine::l5::digest::{DigestEntryResult, compute_digest_effects_cli};
use crate::engine::l5::snapshot::{CapabilitySnapshot, compose_snapshot};
use crate::engine::l5::transaction_spans::SeedKind;
use crate::engine::l5::unresolved_cone::{
    UnresolvedConeItem, UnresolvedTraversal, unresolved_cone,
};

// ---------------------------------------------------------------------------
// DEFAULT_DETECTOR_NAMES — same as cli_b_snapshot_differential (the 34-detector set).
// Shared by the digest AND prove CLI pipelines (the next detector added must land
// here so BOTH stay in sync — see build_envelope_diagnostics_json below).
// ---------------------------------------------------------------------------

pub const DEFAULT_DETECTOR_NAMES: &[&str] = &[
    "d1-db-op-in-loop",
    "d2-event-fanout-in-loop",
    "d3-missing-setloadfields",
    "d4-repeated-lookup-in-loop",
    "d5-set-based-opportunity",
    "d7-recursive-event-expansion",
    "d8-commit-in-transaction",
    "d9-transaction-span-summary",
    "d10-self-modifying-loop",
    "d11-modify-without-get",
    "d12-dead-integration-event",
    "d13-cross-app-internal-call",
    "d14-dead-routine",
    "d16-blob-in-loop",
    "d17-non-setbased-on-large-table",
    "d18-event-subscriber-heavy",
    "d19-flowfield-in-loop",
    "d20-unbounded-result-set",
    "d21-temp-table-misuse",
    "d22-deprecated-api-use",
    "d29-onaftergetrecord-heavy",
    "d32-internal-event-publisher",
    "d33-event-without-subscribers",
    "d34-page-source-heavy",
    "d35-implicit-transaction-scope",
    "d36-redundant-calcfields",
    "d37-record-passed-by-value",
    "d38-page-trigger-heavy",
    "d39-codeunit-instantiation-in-loop",
    "d41-unindexed-filter",
    "d42-locktable-late",
    "d43-event-ishandled-skip",
    "d44-event-recursive-publish",
    "d45-text-encoding-mismatch",
];

// ---------------------------------------------------------------------------
// Shared envelope-diagnostics projection
// ---------------------------------------------------------------------------

/// Build the envelope `diagnostics` JSON array (the analyzer's per-stage diagnostics
/// over the DEFAULT_DETECTOR_NAMES set). Shared by the digest AND prove CLI pipelines
/// so they cannot desync. Each entry is `{code: "DIAG-<stage>", message, severity}`.
pub fn build_envelope_diagnostics_json(
    workspace: &std::path::Path,
    resolved: &L3Resolved,
) -> serde_json::Value {
    let default_detectors: Vec<_> = registered_detectors()
        .into_iter()
        .filter(|d| DEFAULT_DETECTOR_NAMES.contains(&d.name.as_str()))
        .collect();
    let diag_vec = compute_analyzer_diagnostics(workspace, resolved, &default_detectors);
    let arr: Vec<serde_json::Value> = diag_vec
        .iter()
        .map(|d| {
            let mut m = serde_json::Map::new();
            m.insert("code".into(), format!("DIAG-{}", d.stage).into());
            m.insert("message".into(), d.message.clone().into());
            m.insert("severity".into(), d.severity.clone().into());
            serde_json::Value::Object(m)
        })
        .collect();
    serde_json::Value::Array(arr)
}

// ---------------------------------------------------------------------------
// Changed-root resolution
// ---------------------------------------------------------------------------

/// Strip URI scheme prefix from a snapshot source path.
fn strip_scheme(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("ws:") {
        return rest.replace('\\', "/");
    }
    if let Some(rest) = p.strip_prefix("app:")
        && let Some(i) = rest.find(':')
    {
        return rest[i + 1..].replace('\\', "/");
    }
    p.replace('\\', "/")
}

/// Normalize an input path for comparison: strip scheme, forward-slash, lowercase.
fn normalize_input(p: &str) -> String {
    strip_scheme(p).to_lowercase()
}

// ---------------------------------------------------------------------------
// --changed alias auto-detection (port of cli/digest.ts autoDetectChanged) (#12)
// ---------------------------------------------------------------------------

/// The category a `--changed <value>` resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedAutoDetect {
    /// Existing file path → treat as --diff.
    Diff(String),
    /// Comma-list with any `.al` entry → treat as --changed-files.
    Files(Vec<String>),
    /// Else → treat as --changed-routines.
    Routines(Vec<String>),
}

/// `autoDetectChanged` (cli/digest.ts). A non-existent path that is not a `.al`
/// comma-list falls through to a routine selector (graceful miscategorization,
/// NOT an OS error). `exists` is injected so this stays a pure, testable function.
pub fn auto_detect_changed_with(value: &str, exists: impl Fn(&str) -> bool) -> ChangedAutoDetect {
    if exists(value) {
        return ChangedAutoDetect::Diff(value.to_string());
    }
    let parts: Vec<String> = value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.iter().any(|p| p.to_lowercase().ends_with(".al")) {
        ChangedAutoDetect::Files(parts)
    } else {
        ChangedAutoDetect::Routines(parts)
    }
}

/// Convenience wrapper that probes the real filesystem.
pub fn auto_detect_changed(value: &str) -> ChangedAutoDetect {
    auto_detect_changed_with(value, |p| std::path::Path::new(p).exists())
}

// ---------------------------------------------------------------------------
// Routine selector resolution (port of fingerprint-query.ts resolveSelector +
// indexes.ts normalizeDisplayKey / displayToStableIds). 5-form cascade. (#6)
//
// The index-builder and resolver live in `selector_index` — shared with
// fingerprint_query so the two CLIs cannot re-diverge on selector semantics
// or ordering (see that module's docs).
// ---------------------------------------------------------------------------

use crate::engine::l5::selector_index::{build_selector_indexes, resolve_selector};

/// Changed-roots diagnostics (mirrors ChangedRootsDiagnostic).
#[derive(Debug, Clone)]
pub enum ChangedRootsDiagnostic {
    FileUnmatched {
        file: String,
    },
    SelectorUnmatched {
        selector: String,
    },
    SelectorAmbiguous {
        selector: String,
        candidates: Vec<String>,
    },
    DiffFileUnmatched {
        file: String,
    },
    HunkOutsideRoutines {
        file: String,
        start_line: i32,
        end_line: i32,
    },
    DiffFileDeleted {
        file: String,
    },
    DiffParseError {
        detail: String,
    },
}

pub struct ChangedRootsResult {
    pub roots: Vec<String>,
    pub diagnostics: Vec<ChangedRootsDiagnostic>,
}

pub struct ChangedInput {
    pub files: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub diff_text: Option<String>,
}

/// Build the file → routines index from operationIndex + callsiteIndex.
#[allow(clippy::type_complexity)] // documented parallel index maps; a struct adds no clarity
fn build_routine_file_index(
    snap: &CapabilitySnapshot,
) -> (
    HashMap<String, Vec<String>>,        // file → routines
    HashMap<String, Vec<(String, u32)>>, // file → [(routineId, startLine)]
) {
    let mut by_file: HashMap<String, Vec<String>> = HashMap::new();
    let mut line_entries: HashMap<String, Vec<(String, u32)>> = HashMap::new();

    let mut add = |source_file: &str, routine: &str, start_line: u32| {
        let norm = normalize_input(source_file);
        let rset = by_file.entry(norm.clone()).or_default();
        if !rset.contains(&routine.to_string()) {
            rset.push(routine.to_string());
        }
        line_entries
            .entry(norm)
            .or_default()
            .push((routine.to_string(), start_line));
    };

    for op in &snap.operation_index {
        add(&op.source_file, &op.routine, op.start_line);
    }
    for cs in &snap.callsite_index {
        add(&cs.source_file, &cs.routine, cs.start_line);
    }

    // Sort line_entries by startLine for each file
    for entries in line_entries.values_mut() {
        entries.sort_by_key(|(_, l)| *l);
    }

    (by_file, line_entries)
}

pub fn resolve_changed_roots(
    snap: &CapabilitySnapshot,
    input: &ChangedInput,
) -> ChangedRootsResult {
    let mut roots: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut diagnostics: Vec<ChangedRootsDiagnostic> = Vec::new();

    let (by_file, line_entries) = build_routine_file_index(snap);

    // Selector indexes (5-form resolveSelector cascade).
    let selector_idx = build_selector_indexes(snap);

    // 1. File-based matching
    for file in input.files.iter().flatten() {
        let norm = normalize_input(file);
        match by_file.get(&norm) {
            Some(rset) if !rset.is_empty() => {
                for r in rset {
                    roots.insert(r.clone());
                }
            }
            _ => {
                diagnostics.push(ChangedRootsDiagnostic::FileUnmatched { file: file.clone() });
            }
        }
    }

    // 2. Routine selector matching (port of resolveSelector — 5-form cascade).
    for selector in input.routines.iter().flatten() {
        let (matches, _matched_form) = resolve_selector(selector, &selector_idx);
        if matches.is_empty() {
            diagnostics.push(ChangedRootsDiagnostic::SelectorUnmatched {
                selector: selector.clone(),
            });
        } else if matches.len() >= 2 {
            // candidates = matches.map(id => routineDisplayById.get(id) ?? id)
            // — deterministic (matches preserve identity-table / bucket order).
            let candidates: Vec<String> = matches
                .iter()
                .map(|id| {
                    selector_idx
                        .routine_display_by_id
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| id.clone())
                })
                .collect();
            diagnostics.push(ChangedRootsDiagnostic::SelectorAmbiguous {
                selector: selector.clone(),
                candidates,
            });
        } else {
            roots.insert(matches.into_iter().next().unwrap());
        }
    }

    // 3. Unified diff matching
    if let Some(ref diff_text) = input.diff_text
        && !diff_text.trim().is_empty()
    {
        let parsed = parse_unified_diff(diff_text);
        for err in &parsed.errors {
            diagnostics.push(ChangedRootsDiagnostic::DiffParseError {
                detail: err.clone(),
            });
        }
        for diff_file in &parsed.files {
            if diff_file.kind == DiffFileKind::Deleted {
                diagnostics.push(ChangedRootsDiagnostic::DiffFileDeleted {
                    file: diff_file.path.clone(),
                });
                continue;
            }
            let norm_path = normalize_input(&diff_file.path);
            let rset = by_file.get(&norm_path);
            if rset.map(|v| v.is_empty()).unwrap_or(true) {
                diagnostics.push(ChangedRootsDiagnostic::DiffFileUnmatched {
                    file: diff_file.path.clone(),
                });
                continue;
            }
            let rset = rset.unwrap();
            let entries = line_entries
                .get(&norm_path)
                .map(|v| v.as_slice())
                .unwrap_or(&[]);

            if diff_file.hunks.is_empty() {
                // Rename with no content — include all routines
                for r in rset {
                    roots.insert(r.clone());
                }
                continue;
            }

            for hunk in &diff_file.hunks {
                // 1-based [newStart, newStart+newCount) → 0-based [newStart-1, newStart+newCount-1)
                let hunk_start = (hunk.new_start - 1).max(0) as u32;
                let hunk_end = (hunk.new_start - 1 + hunk.new_count.max(0)) as u32;

                let mut matched: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for (rid, line) in entries {
                    if *line >= hunk_start && *line < hunk_end {
                        matched.insert(rid.clone());
                    }
                }
                if matched.is_empty() {
                    // endLine = newStart + max(newCount - 1, 0). When newCount == 0
                    // (pure deletion hunk) TS yields newStart, NOT newStart - 1 (#14).
                    diagnostics.push(ChangedRootsDiagnostic::HunkOutsideRoutines {
                        file: diff_file.path.clone(),
                        start_line: hunk.new_start,
                        end_line: hunk.new_start + (hunk.new_count - 1).max(0),
                    });
                } else {
                    for r in matched {
                        roots.insert(r);
                    }
                }
            }
        }
    }

    // Sort + dedup
    let mut sorted: Vec<String> = roots.into_iter().collect();
    sorted.sort();

    ChangedRootsResult {
        roots: sorted,
        diagnostics,
    }
}

// ---------------------------------------------------------------------------
// Transaction context computation
// ---------------------------------------------------------------------------

pub type TransactionContext = &'static str;
const TX_SPAN_KNOWN_WRITES: TransactionContext = "span-has-known-writes";
const TX_SPAN_NO_WRITES: TransactionContext = "span-has-no-known-writes";
const TX_UNKNOWN: TransactionContext = "unknown";

/// Compute the transaction-context map: `commit_operation_id → TransactionContext`.
/// Uses the full detector-context substrate (combined graph → summaries → spans).
/// Mirrors TS: `computeTransactionSpans` + `transactionContextByOperationId` build.
fn compute_tx_context_map(resolved: &L3Resolved) -> HashMap<String, TransactionContext> {
    // ⟨C1 Task 3 — R2/R3 caller classification⟩ DERIVED-ONLY: plain `ALL`, no
    // `RAW_INHERITED_FACTS`. This path reads ONLY `ctx.transaction_spans`, and
    // `compute_transaction_spans` sources its `writes_tables` / `publishes_events`
    // from `ctx.cone_derived` (Task 2) and its completeness from
    // `reachable_coverage` — never from a fact Vec.
    let ctx = build_detector_context(resolved, crate::engine::l5::registry::substrate::ALL);
    let mut map: HashMap<String, TransactionContext> = HashMap::new();
    for span in &ctx.transaction_spans {
        if span.seed_kind != SeedKind::ExplicitCommit {
            continue;
        }
        let tc: TransactionContext = if !span.coverage_complete {
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
// Full digest query — extends digest_query with conditionality, tx-context,
// unresolved-cone, and guarantees.
// ---------------------------------------------------------------------------

pub struct DigestEntryFull {
    pub routine_id: String,
    pub routine_display: String,
    pub object_display: String,
    pub routine_anchor: Option<FullAnchor>,
    pub coverage_status: String,
    pub coverage_reasons: Vec<String>,
    pub effects: Vec<DigestEffectFull>,
    pub unresolved: Vec<UnresolvedConeItem>,
    pub unresolved_traversal: UnresolvedTraversal,
    pub entry_diagnostics: Vec<EntryDiagnostic>,
}

pub struct DigestEffectFull {
    pub effect_type: String,
    pub detail: Vec<(String, String)>,
    pub provenance: &'static str,
    pub evidence: FullAnchor,
    pub via_paths: Vec<Vec<crate::engine::l5::digest::ProjectedHop>>,
    pub via_paths_truncated: bool,
    pub conditionality: EffectConditionality,
    pub transaction_context: TransactionContext,
    pub guarantees: Vec<String>,
    pub fact_id: String,
    pub scoped_guarantees: Vec<crate::engine::l5::ordering_engine::ScopedGuarantee>,
}

pub struct FullAnchor {
    pub source_kind: &'static str, // "source" | "unavailable"
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub excerpt: Option<String>,
}

pub struct EntryDiagnostic {
    pub kind: &'static str,
    pub effect_type: Option<String>,
    pub fact_subject: Option<String>,
}

/// Query-level diagnostics (mirrors `DigestQueryDiagnostic` in digest-query.ts).
/// These feed BOTH the envelope diagnostics channel AND rootsRequested (#5).
#[derive(Debug, Clone)]
pub enum DigestQueryDiagnostic {
    RootNotInSnapshot { routine_id: String },
    NoCoverageRecord { routine_id: String },
}

/// Result of the full digest query — entries plus query-level diagnostics.
pub struct DigestQueryFullResult {
    pub entries: Vec<DigestEntryFull>,
    pub query_diagnostics: Vec<DigestQueryDiagnostic>,
}

fn split_object_routine(display: &str) -> (&str, &str) {
    if let Some(idx) = display.rfind("::") {
        (&display[..idx], &display[idx + 2..])
    } else {
        ("", display)
    }
}

/// The real entry point: given resolved workspace + a list of roots (their stableIds),
/// produce DigestEntryFull for each root (order:false path).
pub fn run_digest_query_full_from_entries(
    snap: &CapabilitySnapshot,
    roots: &[String],
    base_entries: &[DigestEntryResult],
    tx_ctx_map: &HashMap<String, TransactionContext>,
) -> DigestQueryFullResult {
    let mut query_diagnostics: Vec<DigestQueryDiagnostic> = Vec::new();

    // Coverage map
    let mut coverage_by_id: HashMap<&str, (&str, Vec<&str>)> = HashMap::new();
    for rec in &snap.coverage {
        let reasons: Vec<&str> = rec.reasons.iter().map(|s| s.as_str()).collect();
        coverage_by_id.insert(
            rec.subject.as_str(),
            (rec.inherited_status.as_str(), reasons),
        );
    }

    // Identities
    let mut display_by_id: HashMap<&str, &str> = HashMap::new();
    for i in 0..snap.identities.stable_ids.len() {
        let id = snap
            .identities
            .stable_ids
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        let nm = snap
            .identities
            .display_names
            .get(i)
            .map(|s| s.as_str())
            .unwrap_or("");
        if !id.is_empty() {
            display_by_id.insert(id, nm);
        }
    }

    // Root anchor map
    let mut anchor_by_rid: HashMap<&str, FullAnchor> = HashMap::new();
    for slot in &snap.root_classifications {
        if let Some(ref sa) = slot.source_anchor
            && !sa.source_unit_id.is_empty()
        {
            anchor_by_rid.insert(
                slot.routine_id.as_str(),
                FullAnchor {
                    source_kind: "source",
                    file: Some(sa.source_unit_id.clone()),
                    line: Some(sa.range.start_line),
                    column: Some(sa.range.start_column),
                    excerpt: None,
                },
            );
        }
    }
    for op in &snap.operation_index {
        if !anchor_by_rid.contains_key(op.routine.as_str()) {
            anchor_by_rid.insert(
                op.routine.as_str(),
                FullAnchor {
                    source_kind: "source",
                    file: Some(op.source_file.clone()),
                    line: Some(op.start_line),
                    column: Some(op.start_column),
                    excerpt: None,
                },
            );
        }
    }
    for cs in &snap.callsite_index {
        if !anchor_by_rid.contains_key(cs.routine.as_str()) {
            anchor_by_rid.insert(
                cs.routine.as_str(),
                FullAnchor {
                    source_kind: "source",
                    file: Some(cs.source_file.clone()),
                    line: Some(cs.start_line),
                    column: Some(cs.start_column),
                    excerpt: None,
                },
            );
        }
    }

    // Build index of base entries by routineId
    let base_by_rid: HashMap<&str, &DigestEntryResult> = base_entries
        .iter()
        .map(|e| (e.routine_id.as_str(), e))
        .collect();

    let mut out: Vec<DigestEntryFull> = Vec::new();

    for rid in roots {
        // Verify the routine exists in the snapshot. Missing display → emit
        // `root-not-in-snapshot` and skip (no entry). Mirrors digest-query.ts.
        let Some(display_full) = display_by_id.get(rid.as_str()).copied() else {
            query_diagnostics.push(DigestQueryDiagnostic::RootNotInSnapshot {
                routine_id: rid.clone(),
            });
            continue;
        };
        let (obj_display, rtn_display) = split_object_routine(display_full);

        // Coverage. Missing record → emit `no-coverage-record` but STILL build the
        // entry with status "unknown" (TS: covRec === undefined still produces an entry).
        let (cov_status, cov_reasons) = match coverage_by_id.get(rid.as_str()) {
            Some((s, r)) => (*s, r.iter().map(|x| x.to_string()).collect::<Vec<_>>()),
            None => {
                query_diagnostics.push(DigestQueryDiagnostic::NoCoverageRecord {
                    routine_id: rid.clone(),
                });
                ("unknown", Vec::new())
            }
        };

        // Routine anchor
        let routine_anchor = anchor_by_rid.remove(rid.as_str()).map(|a| FullAnchor {
            source_kind: a.source_kind,
            file: a.file,
            line: a.line,
            column: a.column,
            excerpt: a.excerpt,
        });

        // Get base effects (from S4 ordering)
        let base_entry = base_by_rid.get(rid.as_str());

        let mut full_effects: Vec<DigestEffectFull> = Vec::new();
        let mut entry_diagnostics: Vec<EntryDiagnostic> = Vec::new();

        if let Some(entry) = base_entry {
            for eff in &entry.effects {
                // Conditionality — the per-path value computed in digest_query and
                // stored on the effect (#8 / #17). No recomputation here.
                let cond = eff.conditionality;

                // Transaction context for COMMIT
                let tx_ctx = if eff.effect_type == "COMMIT" {
                    if let Some(op_id) = &eff.evidence_operation_id {
                        tx_ctx_map
                            .get(op_id.as_str())
                            .copied()
                            .unwrap_or(TX_UNKNOWN)
                    } else {
                        TX_UNKNOWN
                    }
                } else {
                    TX_UNKNOWN
                };

                // Evidence anchor
                let evidence = FullAnchor {
                    source_kind: eff.evidence.source_kind,
                    file: eff.evidence.file.clone(),
                    line: eff.evidence.line,
                    column: eff.evidence.column,
                    excerpt: eff.evidence.excerpt.clone(),
                };

                // evidence-unavailable diagnostic
                if eff.evidence.source_kind == "unavailable" {
                    entry_diagnostics.push(EntryDiagnostic {
                        kind: "evidence-unavailable",
                        effect_type: Some(eff.effect_type.clone()),
                        fact_subject: Some(eff.fact_subject.clone()),
                    });
                }

                // Guarantees: unique labels from scopedGuarantees in first-occurrence order.
                // Mirrors TS guaranteesByIndex dedup: intra + root merged, duplicates suppressed
                // (a label may appear twice in scopedGuarantees with different scopes but only
                // once in the plain guarantees[] backward-compat array).
                let guarantees: Vec<String> = {
                    let mut seen = std::collections::HashSet::new();
                    eff.scoped_guarantees
                        .iter()
                        .filter_map(|sg| {
                            if seen.insert(sg.label) {
                                Some(sg.label.to_string())
                            } else {
                                None
                            }
                        })
                        .collect()
                };

                full_effects.push(DigestEffectFull {
                    effect_type: eff.effect_type.clone(),
                    detail: eff.detail.clone(),
                    provenance: eff.provenance,
                    evidence,
                    via_paths: eff.via_paths.clone(),
                    via_paths_truncated: eff.via_paths_truncated,
                    conditionality: cond,
                    transaction_context: tx_ctx,
                    guarantees,
                    fact_id: eff.fact_id.clone(),
                    scoped_guarantees: eff.scoped_guarantees.clone(),
                });
            }
        }

        // Unresolved cone. (gapsInCone — TS digestQuery computes it onto the internal
        // DigestEntryResult for the prove engine, but it is NOT part of the digest JSON
        // contract (contracts/digest.ts omits it) and the CLI does not expose queryEntries,
        // so it is intentionally not computed here (#20).)
        let cone = unresolved_cone(snap, rid);

        out.push(DigestEntryFull {
            routine_id: rid.clone(),
            routine_display: rtn_display.to_string(),
            object_display: obj_display.to_string(),
            routine_anchor,
            coverage_status: cov_status.to_string(),
            coverage_reasons: cov_reasons,
            effects: full_effects,
            unresolved: cone.items,
            unresolved_traversal: cone.traversal,
            entry_diagnostics,
        });
    }

    // Sort by routineId (already sorted from resolve_changed_roots, but be safe)
    out.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    DigestQueryFullResult {
        entries: out,
        query_diagnostics,
    }
}

// ---------------------------------------------------------------------------
// JSON document projection
// ---------------------------------------------------------------------------

pub fn anchor_to_value(a: &FullAnchor) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("sourceKind".into(), a.source_kind.into());
    if let Some(ref f) = a.file {
        m.insert("file".into(), f.clone().into());
    }
    if let Some(l) = a.line {
        m.insert("line".into(), l.into());
    }
    if let Some(c) = a.column {
        m.insert("column".into(), c.into());
    }
    if let Some(ref x) = a.excerpt {
        m.insert("excerpt".into(), x.clone().into());
    }
    serde_json::Value::Object(m)
}

fn hop_to_value(hop: &crate::engine::l5::digest::ProjectedHop) -> serde_json::Value {
    crate::engine::l5::digest::hop_to_json_value(&hop.inner)
}

fn scoped_guarantee_to_value(
    sg: &crate::engine::l5::ordering_engine::ScopedGuarantee,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("label".into(), sg.label.into());
    m.insert("scope".into(), sg.scope.into());
    if let Some(ref v) = sg.write_occurrence_id {
        m.insert("writeOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.commit_occurrence_id {
        m.insert("commitOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.io_occurrence_id {
        m.insert("ioOccurrenceId".into(), v.clone().into());
    }
    if let Some(ref v) = sg.return_occurrence_id {
        m.insert("returnOccurrenceId".into(), v.clone().into());
    }
    // supportingEdgeIds — only emit when non-empty (mirrors TS contract: optional field).
    if !sg.supporting_edge_ids.is_empty() {
        let eids: Vec<serde_json::Value> = sg
            .supporting_edge_ids
            .iter()
            .map(|s| s.clone().into())
            .collect();
        m.insert("supportingEdgeIds".into(), serde_json::Value::Array(eids));
    }
    if let Some(ce) = sg.commit_effectiveness {
        m.insert("commitEffectiveness".into(), ce.into());
    }
    m.insert("interveningBoundary".into(), sg.intervening_boundary.into());
    m.insert("validForRefutation".into(), sg.valid_for_refutation.into());
    serde_json::Value::Object(m)
}

pub fn effect_to_value(eff: &DigestEffectFull) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), eff.effect_type.clone().into());

    // detail: insertion-order map
    let mut detail_obj = serde_json::Map::new();
    for (k, v) in &eff.detail {
        detail_obj.insert(k.clone(), v.clone().into());
    }
    m.insert("detail".into(), serde_json::Value::Object(detail_obj));

    m.insert("provenance".into(), eff.provenance.into());
    m.insert("evidence".into(), anchor_to_value(&eff.evidence));

    // viaPaths
    let via: Vec<serde_json::Value> = eff
        .via_paths
        .iter()
        .map(|path| {
            let hops: Vec<serde_json::Value> = path.iter().map(hop_to_value).collect();
            serde_json::Value::Array(hops)
        })
        .collect();
    m.insert("viaPaths".into(), serde_json::Value::Array(via));
    m.insert("viaPathsTruncated".into(), eff.via_paths_truncated.into());
    m.insert("confidence".into(), "static".into());
    m.insert("conditionality".into(), eff.conditionality.into());
    m.insert("transactionContext".into(), eff.transaction_context.into());
    let guarantees: Vec<serde_json::Value> =
        eff.guarantees.iter().map(|g| g.clone().into()).collect();
    m.insert("guarantees".into(), serde_json::Value::Array(guarantees));
    m.insert("factId".into(), eff.fact_id.clone().into());
    let sg: Vec<serde_json::Value> = eff
        .scoped_guarantees
        .iter()
        .map(scoped_guarantee_to_value)
        .collect();
    m.insert("scopedGuarantees".into(), serde_json::Value::Array(sg));

    serde_json::Value::Object(m)
}

fn unresolved_item_to_value(item: &UnresolvedConeItem) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    if let Some(ref f) = item.callsite_file {
        // Build callsiteAnchor
        let mut anch = serde_json::Map::new();
        anch.insert("sourceKind".into(), "source".into());
        anch.insert("file".into(), f.clone().into());
        if let Some(l) = item.callsite_line {
            anch.insert("line".into(), l.into());
        }
        if let Some(c) = item.callsite_column {
            anch.insert("column".into(), c.into());
        }
        m.insert("callsiteAnchor".into(), serde_json::Value::Object(anch));
    }
    m.insert("calleeDisplay".into(), item.callee_display.clone().into());
    m.insert("status".into(), item.status.clone().into());
    if let Some(ref cands) = item.candidates {
        let cv: Vec<serde_json::Value> = cands.iter().map(|c| c.clone().into()).collect();
        m.insert("candidates".into(), serde_json::Value::Array(cv));
    }
    if item.open_world == Some(true) {
        m.insert("openWorld".into(), true.into());
    }
    m.insert("owningRoutine".into(), item.owning_routine.clone().into());
    m.insert(
        "owningRoutineDisplay".into(),
        item.owning_routine_display.clone().into(),
    );
    serde_json::Value::Object(m)
}

fn entry_to_value(entry: &DigestEntryFull) -> serde_json::Value {
    let mut m = serde_json::Map::new();

    // coverage
    let mut cov = serde_json::Map::new();
    cov.insert("status".into(), entry.coverage_status.clone().into());
    let reasons: Vec<serde_json::Value> = entry
        .coverage_reasons
        .iter()
        .map(|r| r.clone().into())
        .collect();
    cov.insert("reasons".into(), serde_json::Value::Array(reasons));
    m.insert("coverage".into(), serde_json::Value::Object(cov));

    // effects
    let effects: Vec<serde_json::Value> = entry.effects.iter().map(effect_to_value).collect();
    m.insert("effects".into(), serde_json::Value::Array(effects));

    // entryDiagnostics
    let ed: Vec<serde_json::Value> = entry
        .entry_diagnostics
        .iter()
        .map(|d| {
            let mut dm = serde_json::Map::new();
            dm.insert("kind".into(), d.kind.into());
            if let Some(ref et) = d.effect_type {
                dm.insert("effectType".into(), et.clone().into());
            }
            if let Some(ref fs) = d.fact_subject {
                dm.insert("factSubject".into(), fs.clone().into());
            }
            serde_json::Value::Object(dm)
        })
        .collect();
    m.insert("entryDiagnostics".into(), serde_json::Value::Array(ed));

    // routine
    let mut routine_m = serde_json::Map::new();
    routine_m.insert("stableId".into(), entry.routine_id.clone().into());
    routine_m.insert("display".into(), entry.routine_display.clone().into());
    routine_m.insert("objectDisplay".into(), entry.object_display.clone().into());
    if let Some(ref a) = entry.routine_anchor {
        routine_m.insert("anchor".into(), anchor_to_value(a));
    }
    m.insert("routine".into(), serde_json::Value::Object(routine_m));

    // unresolved
    let ur: Vec<serde_json::Value> = entry
        .unresolved
        .iter()
        .map(unresolved_item_to_value)
        .collect();
    m.insert("unresolved".into(), serde_json::Value::Array(ur));

    // unresolvedTraversal
    let mut trav = serde_json::Map::new();
    trav.insert(
        "maxDepth".into(),
        (entry.unresolved_traversal.max_depth as u64).into(),
    );
    trav.insert(
        "truncated".into(),
        entry.unresolved_traversal.truncated.into(),
    );
    trav.insert(
        "visitedRoutines".into(),
        (entry.unresolved_traversal.visited_routines as u64).into(),
    );
    m.insert(
        "unresolvedTraversal".into(),
        serde_json::Value::Object(trav),
    );

    serde_json::Value::Object(m)
}

fn changed_roots_diagnostic_to_value(d: &ChangedRootsDiagnostic) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    match d {
        ChangedRootsDiagnostic::FileUnmatched { file } => {
            m.insert("kind".into(), "file-unmatched".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::SelectorUnmatched { selector } => {
            m.insert("kind".into(), "selector-unmatched".into());
            m.insert("selector".into(), selector.clone().into());
        }
        ChangedRootsDiagnostic::SelectorAmbiguous {
            selector,
            candidates,
        } => {
            m.insert("kind".into(), "selector-ambiguous".into());
            m.insert("selector".into(), selector.clone().into());
            let cv: Vec<serde_json::Value> = candidates.iter().map(|c| c.clone().into()).collect();
            m.insert("candidates".into(), serde_json::Value::Array(cv));
        }
        ChangedRootsDiagnostic::DiffFileUnmatched { file } => {
            m.insert("kind".into(), "diff-file-unmatched".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::HunkOutsideRoutines {
            file,
            start_line,
            end_line,
        } => {
            m.insert("kind".into(), "hunk-outside-routines".into());
            m.insert("file".into(), file.clone().into());
            m.insert("startLine".into(), (*start_line as u64).into());
            m.insert("endLine".into(), (*end_line as u64).into());
        }
        ChangedRootsDiagnostic::DiffFileDeleted { file } => {
            m.insert("kind".into(), "diff-file-deleted".into());
            m.insert("file".into(), file.clone().into());
        }
        ChangedRootsDiagnostic::DiffParseError { detail } => {
            m.insert("kind".into(), "diff-parse-error".into());
            m.insert("detail".into(), detail.clone().into());
        }
    }
    serde_json::Value::Object(m)
}

/// Project a query-level diagnostic into the envelope `{code, severity, message}`
/// shape. Mirrors `projectQueryDiagnostics` in contracts/digest.ts.
fn project_query_diagnostic(d: &DigestQueryDiagnostic) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    match d {
        DigestQueryDiagnostic::RootNotInSnapshot { routine_id } => {
            m.insert("code".into(), "DIAG-digest-root-not-in-snapshot".into());
            m.insert("severity".into(), "warning".into());
            m.insert(
                "message".into(),
                format!("Root routine '{routine_id}' not found in snapshot").into(),
            );
        }
        DigestQueryDiagnostic::NoCoverageRecord { routine_id } => {
            m.insert("code".into(), "DIAG-digest-no-coverage-record".into());
            m.insert("severity".into(), "warning".into());
            m.insert(
                "message".into(),
                format!("No coverage record for routine '{routine_id}'").into(),
            );
        }
    }
    serde_json::Value::Object(m)
}

/// Build the DocumentEnvelope<"digest", DigestPayload> as a serde_json::Value,
/// then serialize with serialize_document_value (sorted keys, null-drop, trailing \n).
#[allow(clippy::too_many_arguments)] // document-envelope fields; grouping would obscure
pub fn project_digest_document(
    workspace_fp: &str,
    changed_input: &ChangedInputContract,
    changed_roots_result: &ChangedRootsResult,
    entries: &[DigestEntryFull],
    query_diagnostics: &[DigestQueryDiagnostic],
    diagnostics_json: serde_json::Value, // pre-built array from analyzer
    alsem_ver: &str,
    deterministic: bool,
) -> String {
    // payload.summary
    // rootsRequested = roots.length + queryResult.diagnostics.length (contracts/digest.ts:443).
    let roots_requested = changed_roots_result.roots.len() + query_diagnostics.len();
    let roots_resolved = entries.len();
    let total_effects: usize = entries.iter().map(|e| e.effects.len()).sum();
    let total_unresolved: usize = entries.iter().map(|e| e.unresolved.len()).sum();

    let mut summary = serde_json::Map::new();
    summary.insert("rootsRequested".into(), (roots_requested as u64).into());
    summary.insert("rootsResolved".into(), (roots_resolved as u64).into());
    summary.insert("totalEffects".into(), (total_effects as u64).into());
    summary.insert("totalUnresolved".into(), (total_unresolved as u64).into());

    // changed
    let mut changed = serde_json::Map::new();
    changed.insert("diffProvided".into(), changed_input.diff_provided.into());
    if let Some(ref files) = changed_input.files {
        let fv: Vec<serde_json::Value> = files.iter().map(|f| f.clone().into()).collect();
        changed.insert("files".into(), serde_json::Value::Array(fv));
    }
    if let Some(ref routines) = changed_input.routines {
        let rv: Vec<serde_json::Value> = routines.iter().map(|r| r.clone().into()).collect();
        changed.insert("routines".into(), serde_json::Value::Array(rv));
    }

    // entries
    let entries_val: Vec<serde_json::Value> = entries.iter().map(entry_to_value).collect();

    // changedRootsDiagnostics
    let crd: Vec<serde_json::Value> = changed_roots_result
        .diagnostics
        .iter()
        .map(changed_roots_diagnostic_to_value)
        .collect();

    let mut payload = serde_json::Map::new();
    payload.insert("changed".into(), serde_json::Value::Object(changed));
    payload.insert(
        "changedRootsDiagnostics".into(),
        serde_json::Value::Array(crd),
    );
    payload.insert("entries".into(), serde_json::Value::Array(entries_val));
    payload.insert("summary".into(), serde_json::Value::Object(summary));
    payload.insert(
        "workspaceFingerprint".into(),
        workspace_fp.to_string().into(),
    );

    // Envelope
    // makeEnvelope: deterministic ? pinned epoch : live ISO-8601 (#18 — was a dead
    // both-branches-identical block). The shared gate helper drives both cases.
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    // Envelope diagnostics = [...analyzerDiagnostics, ...projectQueryDiagnostics] (#5).
    let diagnostics_with_query = {
        let mut arr = match diagnostics_json {
            serde_json::Value::Array(a) => a,
            other => vec![other],
        };
        for qd in query_diagnostics {
            arr.push(project_query_diagnostic(qd));
        }
        serde_json::Value::Array(arr)
    };

    let mut env = serde_json::Map::new();
    env.insert("alsemVersion".into(), alsem_ver.into());
    env.insert("deterministic".into(), deterministic.into());
    env.insert("diagnostics".into(), diagnostics_with_query);
    env.insert("generatedAt".into(), generated_at.into());
    env.insert("kind".into(), "digest".into());
    env.insert("payload".into(), serde_json::Value::Object(payload));
    env.insert("schemaVersion".into(), "1.3.0".into());

    serialize_document_value(serde_json::Value::Object(env))
}

pub struct ChangedInputContract {
    pub files: Option<Vec<String>>,
    pub routines: Option<Vec<String>>,
    pub diff_provided: bool,
}

// ---------------------------------------------------------------------------
// Human formatter (formatDigestHuman)
// ---------------------------------------------------------------------------

pub fn format_digest_human(entries: &[DigestEntryFull]) -> String {
    if entries.is_empty() {
        return "digest: no changed roots resolved\n".to_string();
    }
    let mut lines: Vec<String> = Vec::new();
    for entry in entries {
        let display = if !entry.object_display.is_empty() {
            format!("{}::{}", entry.object_display, entry.routine_display)
        } else {
            entry.routine_display.clone()
        };
        lines.push(format!("--- {} ---", display));
        lines.push(format!("  coverage: {}", entry.coverage_status));
        if !entry.coverage_reasons.is_empty() {
            lines.push(format!("  reasons: {}", entry.coverage_reasons.join(", ")));
        }
        if entry.effects.is_empty() {
            lines.push("  (no effects)".to_string());
        }
        for eff in &entry.effects {
            let prov_tag = if eff.provenance == "direct" {
                "direct"
            } else {
                "transitive"
            };
            let ev_file = eff.evidence.file.as_deref().unwrap_or("(unavailable)");
            let ev_line = eff
                .evidence
                .line
                .map(|l| format!(":{}", l))
                .unwrap_or_default();
            let excerpt = eff
                .evidence
                .excerpt
                .as_deref()
                .map(|e| format!(" \"{}\"", e))
                .unwrap_or_default();
            lines.push(format!("  [{}] {}", eff.effect_type, prov_tag));
            lines.push(format!("    evidence: {}{}{}", ev_file, ev_line, excerpt));
            let detail_entries: Vec<String> = eff
                .detail
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            if !detail_entries.is_empty() {
                lines.push(format!("    detail: {}", detail_entries.join(", ")));
            }
            if !eff.via_paths.is_empty() {
                let truncated_str = if eff.via_paths_truncated {
                    " (truncated)"
                } else {
                    ""
                };
                lines.push(format!(
                    "    via: {} path(s){}",
                    eff.via_paths.len(),
                    truncated_str
                ));
            }
        }
        if !entry.unresolved.is_empty() {
            lines.push(format!(
                "  unresolved: {} callsite(s)",
                entry.unresolved.len()
            ));
            if entry.unresolved_traversal.truncated {
                lines.push("    (traversal truncated)".to_string());
            }
        }
        if !entry.entry_diagnostics.is_empty() {
            for d in &entry.entry_diagnostics {
                let et_part = d
                    .effect_type
                    .as_deref()
                    .map(|et| format!(" ({})", et))
                    .unwrap_or_default();
                lines.push(format!("  diagnostic: {}{}", d.kind, et_part));
            }
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Top-level run_digest (used by the CLI command)
// ---------------------------------------------------------------------------

/// Result from run_digest_pipeline.
pub struct DigestRunResult {
    pub json_text: String,
    pub human_text: String,
    pub exit_code: u8, // 0 = OK, 2 = zero roots
}

/// Run the full digest pipeline for a workspace.
///
/// `changed_files` are workspace-relative paths (e.g. "src/Foo.al").
/// `diff_text` is a unified diff string (for --diff).
/// `alsem_ver` is the version string (e.g. "cli-b-v1").
pub fn run_digest_pipeline(
    workspace: &std::path::Path,
    changed_files: Option<Vec<String>>,
    changed_routines: Option<Vec<String>>,
    diff_text: Option<String>,
    alsem_ver: &str,
    deterministic: bool,
    _max_paths_override: Option<usize>,
) -> Result<DigestRunResult, String> {
    // Assemble workspace
    let model_id = compute_gate_model_instance_id(workspace)
        .ok_or_else(|| "digest: could not compute modelInstanceId".to_string())?;
    // Engine-switch S6.2: the program-backed model (program calls and events).
    let resolved = assemble_and_resolve_workspace_program(workspace, &model_id, false)
        .ok_or_else(|| "digest: workspace did not resolve".to_string())?;

    // Compose snapshot ONCE. The workspace fingerprint is computed directly via the
    // shared helper (no second full-snapshot composition just to fish it out) (#19).
    let snap = compose_snapshot(&resolved);
    let workspace_fp =
        crate::engine::l5::snapshot_full::workspace_fingerprint_of(workspace, alsem_ver);

    // Resolve changed roots
    let changed_input = ChangedInput {
        files: changed_files.clone(),
        routines: changed_routines.clone(),
        diff_text: diff_text.clone(),
    };

    let has_input = changed_files
        .as_ref()
        .map(|f| !f.is_empty())
        .unwrap_or(false)
        || changed_routines
            .as_ref()
            .map(|r| !r.is_empty())
            .unwrap_or(false)
        || diff_text
            .as_ref()
            .map(|d| !d.trim().is_empty())
            .unwrap_or(false);

    if !has_input {
        return Err(
            "digest: at least one of changedFiles, changedRoutines, or diffText is required"
                .to_string(),
        );
    }

    let changed_roots = resolve_changed_roots(&snap, &changed_input);

    // Compute transaction context map (for COMMIT effects).
    // Mirrors TS pipeline: computeTransactionSpans → transactionContextByOperationId.
    let tx_ctx_map = compute_tx_context_map(&resolved);

    // Compute S4 ordering effects for ALL reportable roots (then filter to roots we care about).
    // Use compute_digest_effects_cli which matches TS runDigestPipeline (no routineReturnSummaries).
    let all_s4_entries = compute_digest_effects_cli(&snap, &resolved);

    // Run full digest query for the resolved roots
    let query_full = run_digest_query_full_from_entries(
        &snap,
        &changed_roots.roots,
        &all_s4_entries,
        &tx_ctx_map,
    );
    let entries = query_full.entries;
    let query_diagnostics = query_full.query_diagnostics;

    let exit_code: u8 = if changed_roots.roots.is_empty() { 2 } else { 0 };

    // Build envelope diagnostics (same 34-detector set, shared with prove).
    let diagnostics_json = build_envelope_diagnostics_json(workspace, &resolved);

    let changed_input_contract = ChangedInputContract {
        files: changed_files,
        routines: changed_routines,
        diff_provided: diff_text.is_some(),
    };

    // JSON
    let json_text = project_digest_document(
        &workspace_fp,
        &changed_input_contract,
        &changed_roots,
        &entries,
        &query_diagnostics,
        diagnostics_json,
        alsem_ver,
        deterministic,
    );

    // Human
    let human_text = format_digest_human(&entries);

    Ok(DigestRunResult {
        json_text,
        human_text,
        exit_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The #6 resolveSelector-cascade + normalize_display_key unit tests now live
    // in `selector_index::tests` (shared implementation, shared tests).

    // --- #12 autoDetectChanged --------------------------------------------

    #[test]
    fn auto_detect_existing_path_is_diff() {
        let r = auto_detect_changed_with("some.patch", |_| true);
        assert_eq!(r, ChangedAutoDetect::Diff("some.patch".to_string()));
    }

    #[test]
    fn auto_detect_al_comma_list_is_files() {
        let r = auto_detect_changed_with("src/A.al, src/B.al", |_| false);
        assert_eq!(
            r,
            ChangedAutoDetect::Files(vec!["src/A.al".into(), "src/B.al".into()])
        );
    }

    #[test]
    fn auto_detect_nonexistent_nonal_is_routines() {
        // A non-existent `--changed bad.patch` → NOT a file, not `.al` → routine
        // selector ["bad.patch"] (graceful miscategorization, not an OS error) (#12).
        let r = auto_detect_changed_with("bad.patch", |_| false);
        assert_eq!(r, ChangedAutoDetect::Routines(vec!["bad.patch".into()]));
    }

    #[test]
    fn auto_detect_routine_names() {
        let r = auto_detect_changed_with("MyCodeunit::DoWork, Other", |_| false);
        assert_eq!(
            r,
            ChangedAutoDetect::Routines(vec!["MyCodeunit::DoWork".into(), "Other".into()])
        );
    }
}
