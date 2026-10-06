//! R3a-4 — the dependency-artifact L4 PRODUCER (embedded-source path) + the
//! CONSUMER hooks. Rust port of al-sem's `src/deps/dependency-pipeline.ts`
//! (`buildAppModel` / `ingestDependencyApp` embedded-source path) +
//! `src/deps/dependency-artifact.ts` (`injectIntraAppCallEdges` /
//! `collectCitedDepEvidence` / `collectDepOrderIndex` / `isDepOrderIndexStampFresh`).
//!
//! ## What the producer does (the embedded-source path)
//!
//! ```text
//! .app bytes
//!   → iterate_embedded_source (.al entries inside the ZIP, sorted by name)
//!   → assemble_workspace_units (isolated dep L3 model, analysisRole "dependency",
//!        sourceUnitId = dep:<appGuid>:<relativePath>)
//!   → resolve (build_symbol_table → resolve_record_types → merge_extension_fields)
//!   → resolve_calls (the dep callGraph)         ← intraAppCallEdges
//!   → apply_operation_order per routine          ← depOrderIndex order data
//!   → direct_facts_for_routine per routine       ← citedOperationEvidence witnesses
//!   → compute_dep_return_summary per routine     ← depOrderIndex return summaries
//!   → project: intraAppCallEdges (own→own resolved direct/method/interface, dedup
//!        from|to first-wins, sorted), citedOperationEvidence (deduped/sorted),
//!        depOrderIndex (per-routine order entries + return summaries + freshness
//!        stamp). summaryMode gating: only "full" produces the order index.
//! ```
//!
//! It reuses the engine's OWN already-ported pipeline (L0 parser → L2 body walk +
//! operation-order + control-context → L3 resolve + call resolver → the L4 direct
//! capability extractor `direct_facts_for_routine`) over the ISOLATED dep model —
//! the producer is the engine running on the dep's embedded source, then a compact
//! projection. NO new analysis algorithm lives here.
//!
//! ## summaryMode (parity with al-sem `buildAppModel`)
//!
//! - source-bearing dep (`includesSource`) whose `.al` files parse → `"full"`
//!   (the order index is produced).
//! - source-bearing dep with NO embedded `.al` / parse-failure of all files, or a
//!   symbol-only dep → no parsed body → the order index is ABSENT (a barrier on the
//!   consumer side). The Rust producer treats "no parsed routine body" identically
//!   to al-sem's `hasAnyParsedBody === false` guard.
//!
//! ## Engine-never-throws
//!
//! A malformed `.app` / missing manifest `<App>` Id → `None` (fail-closed, no
//! entities). A `.al` file that fails to parse contributes no objects/routines —
//! never a panic. Mirrors al-sem's per-file error handling.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Cursor;

use crate::engine::deps::app_manifest::parse_app_manifest_xml;
use crate::engine::deps::app_package_zip::{app_zip_bytes, extract_navx_manifest_xml};
use crate::engine::l2::operation_order::apply_operation_order;
use crate::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
use crate::engine::l3::event_graph::{EventSymbol, build_event_graph};
use crate::engine::l3::l3_workspace::{L3Routine, L3Workspace, assemble_workspace_units, resolve};
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l3::taxonomy::{DispatchKind, Resolution};
use crate::engine::l4::capability_cone::direct_facts_for_routine;
use crate::program::attributes::{AttributeInfo, find_attribute, has_attribute};

/// Schema version for the dep order index. Mirrors al-sem
/// `DEP_ORDER_INDEX_SCHEMA_VERSION` (`src/deps/dep-order-types.ts`).
pub const DEP_ORDER_INDEX_SCHEMA_VERSION: &str = "1";

// ===========================================================================
// Embedded-source extraction (port of `iterateEmbeddedSource`).
// ===========================================================================

/// One embedded `.al` file: forward-slash relative path (the ZIP entry name) +
/// UTF-8 content. The order the producer consumes these is sorted-by-path.
#[derive(Debug, Clone)]
pub struct EmbeddedSourceFile {
    pub relative_path: String,
    pub content: String,
}

/// Extract every embedded `.al` entry from a `.app`'s raw bytes, sorted by entry
/// name (mirrors al-sem `iterateEmbeddedSourceBytes`: `Object.keys(entries).sort()`
/// over the `.al`-filtered ZIP). Never panics: an unreadable archive yields `[]`,
/// an unreadable entry is skipped, and (Task T2.2) an entry whose
/// declared/actual size exceeds [`crate::capped_io::EMBEDDED_AL_SOURCE_CAP`]
/// is skipped the same way — never an unbounded allocation.
pub fn iterate_embedded_source(app_bytes: &[u8]) -> Vec<EmbeddedSourceFile> {
    let cursor = Cursor::new(app_zip_bytes(app_bytes).into_owned());
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };

    // Collect (entry-name, index) for every `.al` entry, then sort by name so the
    // ingestion order matches al-sem's sorted keys exactly.
    let mut al_entries: Vec<(String, usize)> = Vec::new();
    for i in 0..archive.len() {
        let name = match archive.by_index(i) {
            Ok(f) => f.name().to_string(),
            Err(_) => continue,
        };
        // al-sem filter: `n.toLowerCase().endsWith(".al")`. The entry name is kept
        // verbatim (forward-slash form) as the relativePath.
        if name.to_lowercase().ends_with(".al") {
            al_entries.push((name, i));
        }
    }
    al_entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out: Vec<EmbeddedSourceFile> = Vec::new();
    for (name, idx) in al_entries {
        let file = match archive.by_index(idx) {
            Ok(f) => f,
            Err(_) => continue,
        };
        // T2.2: an entry whose declared/actual size exceeds the cap joins
        // the SAME per-entry skip this loop already applies to an unreadable
        // entry (an unreadable archive index / non-UTF-8 content) — the
        // function's established contract is "skip the one bad entry, keep
        // ingesting the rest", not "fail the whole `.app`".
        if crate::capped_io::check_declared_size(
            file.size(),
            crate::capped_io::EMBEDDED_AL_SOURCE_CAP,
        )
        .is_err()
        {
            continue;
        }
        let Ok(bytes) =
            crate::capped_io::read_capped(file, crate::capped_io::EMBEDDED_AL_SOURCE_CAP)
        else {
            continue;
        };
        // The shared `.al` decoder (lossy, BOM dropped), so this text matches
        // what the snapshot's embedded-source reader produces for the same entry.
        out.push(EmbeddedSourceFile {
            relative_path: name,
            content: crate::source_text::decode_al_source(&bytes),
        });
    }
    out
}

// ===========================================================================
// The dep-artifact payload types (the R3a-4 producer surface).
// ===========================================================================

/// A compact intra-app call edge — `from → to` routine-id pairs in the dep's own
/// modelInstanceId space, + the representative callsite id. Mirrors al-sem
/// `DepCallEdge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepCallEdge {
    pub from: String,
    pub to: String,
    /// Populated for source-parsed artifacts (the first callsite to this callee).
    pub callsite_id: Option<String>,
}

/// Compact source-location anchor for an operation cited as a witness by a direct
/// capability fact. Mirrors al-sem `DepOperationEvidence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepOperationEvidence {
    pub operation_id: String,
    pub source_file: String,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub display_text: String,
    pub control_context: Option<String>,
}

/// Per-op execution-order entry. Mirrors al-sem `DepOperationOrder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepOperationOrder {
    pub operation_id: String,
    pub order_id: u32,
    pub frame_id: i64,
    pub on_success_path: bool,
    pub dominates_success_return: bool,
}

/// Per-callsite execution-order entry. Mirrors al-sem `DepCallsiteOrder`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepCallsiteOrder {
    pub callsite_id: String,
    pub order_id: u32,
    pub frame_id: i64,
    pub on_success_path: bool,
    pub dominates_success_return: bool,
}

/// Compact scope frame. Mirrors al-sem `DepScopeFrame`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepScopeFrame {
    pub frame_id: i64,
    pub parent_frame_id: i64,
    pub kind: String,
    pub branch_always_terminates: Option<bool>,
    pub branch_terminates_by: Option<String>,
    pub branch_may_fall_through: Option<bool>,
}

/// Per-routine order data. Mirrors al-sem `DepRoutineOrderEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepRoutineOrderEntry {
    pub routine_id: String,
    pub scope_frames: Vec<DepScopeFrame>,
    pub operation_orders: Vec<DepOperationOrder>,
    pub callsite_orders: Vec<DepCallsiteOrder>,
}

/// Per-routine returnability summary. Mirrors al-sem `DepReturnSummaryRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepReturnSummaryRecord {
    pub routine_id: String,
    /// "true" | "false" | "unknown" (al-sem `boolean | "unknown"`).
    pub has_normal_return_path: TriBool,
    pub all_paths_error: TriBool,
    pub has_try_function_boundary: bool,
    /// "resolved" | "partial".
    pub coverage: String,
    pub commit_behavior: String,
}

/// A `boolean | "unknown"` tri-state (al-sem returnability fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriBool {
    True,
    False,
    Unknown,
}

/// Freshness stamp for the dep order index. Mirrors al-sem `DepOrderIndexStamp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepOrderIndexStamp {
    pub app_id: String,
    pub version: String,
    pub source_content_hash: String,
    pub order_index_schema_version: String,
}

/// The full dep order index. Mirrors al-sem `DepOrderIndex`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepOrderIndex {
    pub stamp: DepOrderIndexStamp,
    pub routines: Vec<DepRoutineOrderEntry>,
    pub return_summaries: Vec<DepReturnSummaryRecord>,
}

/// The dep-artifact header subset the R3a-4 hooks read. Mirrors al-sem
/// `DependencyArtifactHeader` (the freshness-relevant identity fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepArtifactHeader {
    pub app_guid: String,
    pub name: String,
    pub version: String,
    /// "app-source" | "symbol-only".
    pub source_kind: String,
    /// Filled by the orchestrator (later); empty in the producer → the freshness
    /// check treats an empty value as "not yet filled" (conservatively fresh).
    pub package_semantic_hash: String,
    /// "full" | "structural-only-*".
    pub summary_mode: String,
}

/// The R3a-4 dep-artifact L4 payload surface (the producer output). A focused
/// subset of al-sem's `DependencyArtifact` — only the L4 payloads R3a-4 adds.
#[derive(Debug, Clone)]
pub struct DependencyArtifactAbi {
    /// Internal routine ids of THIS dep's own routines (membership for the
    /// consumer's both-ends-in-model guard + the test's merged-model seed).
    pub routines_ids: Vec<String>,
    pub intra_app_call_edges: Vec<DepCallEdge>,
    pub cited_operation_evidence: Vec<DepOperationEvidence>,
    /// Absent for symbol-only / no-parsed-body / structural-only deps.
    pub dep_order_index: Option<DepOrderIndex>,
}

/// A built dependency artifact (the R3a-4 L4 subset).
#[derive(Debug, Clone)]
pub struct DependencyArtifactL4 {
    pub header: DepArtifactHeader,
    pub abi: DependencyArtifactAbi,
}

// ===========================================================================
// The PRODUCER — build_dep_artifact_l4.
// ===========================================================================

/// Build the R3a-4 dep artifact from a `.app`'s raw bytes (the embedded-source
/// PRODUCER). Returns `None` (fail-closed) when the archive is unreadable / lacks
/// a usable manifest `<App>` Id. Never panics.
///
/// `model_instance_id` is the dep model's modelInstanceId. al-sem uses
/// `dep:<artifactKey>` (a content hash that embeds the cache-version tuple +
/// devFingerprint, which is intentionally NOT reproduced here — the R3a-4 vector
/// surface is COUNT + structural, the exact id-string differential is Task 3). The
/// caller threads whatever id it wants; it appears only inside the dep's own
/// routine ids, never compared in the vectors.
pub fn build_dep_artifact_l4(
    app_bytes: &[u8],
    model_instance_id: &str,
) -> Option<DependencyArtifactL4> {
    // --- manifest identity (the dep app guid is the entity namespace) ---
    let manifest_xml = extract_navx_manifest_xml(app_bytes)?;
    let manifest = parse_app_manifest_xml(&manifest_xml);
    if manifest.error.is_some() || manifest.identity.app_guid.is_empty() {
        return None;
    }
    let app_guid = manifest.identity.app_guid.clone();
    let includes_source = manifest.includes_source;

    // --- materialize the embedded `.al` files (sorted by path) ---
    let embedded = iterate_embedded_source(app_bytes);

    // summaryMode: source-bearing + at least one parsed body → "full". A
    // symbol-only dep (no embedded source) or a source-bearing dep whose `.al`
    // files all fail to parse → no parsed body → barrier (no order index).
    // (The Rust producer does not model the resource-guard / parser-unavailable /
    // no-dep-summaries modes — those need a workspace-level orchestrator; the
    // R3a-4 corpus is source-bearing "full".)
    let summary_mode = if includes_source && !embedded.is_empty() {
        "full"
    } else {
        // No parsed body — structural-only-ish; the order index will be absent.
        "structural-only-parser-unavailable"
    };

    // --- assemble + resolve the ISOLATED dep model ---
    // Each embedded `.al` carries the al-sem source-unit id `dep:<appGuid>:<relpath>`
    // so op/callsite anchors (the cited-evidence `sourceFile`) match al-sem's
    // embedded-source path.
    let units: Vec<(String, String)> = embedded
        .iter()
        .map(|f| {
            (
                format!("dep:{app_guid}:{}", f.relative_path),
                f.content.clone(),
            )
        })
        .collect();

    let mut ws: L3Workspace = assemble_workspace_units(&units, &app_guid, model_instance_id);
    resolve(&mut ws);

    // --- run the dep call graph (intraAppCallEdges source) ---
    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let no_deps: Vec<DeclaredDependency> = Vec::new();
    let no_fetched: Vec<String> = Vec::new();
    let calls = resolve_calls(&ws, &symbols, &no_deps, &no_fetched);
    let event_graph = build_event_graph(&ws.routines, &symbols);

    // --- apply the operation-order walk per routine (order index source) ---
    // L3 assembly does NOT run apply_operation_order (it is L4-only data the
    // R3a-2 walker normally consumes inline). Run it here so each op/callsite
    // carries `order` + the routine carries `scope_frames` — exactly the data
    // al-sem reads from `r.features.scopeFrames` / `op.order` / `cs.order`.
    let mut scope_frames_by_routine: HashMap<String, Vec<DepScopeFrame>> = HashMap::new();
    for r in &mut ws.routines {
        let frames = apply_dep_operation_order(r);
        scope_frames_by_routine.insert(r.id.clone(), frames);
    }

    // own-app routine ids (membership for the intraAppCallEdges own→own filter).
    let own_routine_ids: HashSet<String> = ws
        .routines
        .iter()
        .filter(|r| r.app_guid == app_guid)
        .map(|r| r.id.clone())
        .collect();

    // ── intraAppCallEdges ───────────────────────────────────────────────────
    // own-app→own-app resolved direct / method(resolved) / interface(maybe) edges,
    // dedup by from|to (first-wins in sorted order), sort by (from, to).
    // (al-sem dependency-pipeline.ts:668-684.)
    let mut raw_edges: Vec<DepCallEdge> = Vec::new();
    for ce in &calls.edges {
        let Some(to) = &ce.to else {
            continue;
        };
        if !own_routine_ids.contains(&ce.from) || !own_routine_ids.contains(to) {
            continue;
        }
        let admit = ce.dispatch_kind == DispatchKind::Direct
            || (ce.dispatch_kind == DispatchKind::Method && ce.resolution == Resolution::Resolved)
            || (ce.dispatch_kind == DispatchKind::Interface && ce.resolution == Resolution::Maybe);
        if !admit {
            continue;
        }
        raw_edges.push(DepCallEdge {
            from: ce.from.clone(),
            to: to.clone(),
            callsite_id: Some(ce.callsite_id.clone()),
        });
    }
    // Dedup by (from, to), keeping the FIRST occurrence (al-sem `findIndex(...) === i`).
    let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
    let mut intra_app_call_edges: Vec<DepCallEdge> = Vec::new();
    for e in raw_edges {
        if seen_pairs.insert((e.from.clone(), e.to.clone())) {
            intra_app_call_edges.push(e);
        }
    }
    intra_app_call_edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));

    // ── citedOperationEvidence ──────────────────────────────────────────────
    // For each own routine's DIRECT capability facts with a witnessOperationId,
    // emit the matching operationSite / recordOperation anchor. operationSites
    // first (displayText = op.kind), recordOperations overwrite (displayText =
    // `${rv}.${op}`, controlContext from the matching operationSite).
    // (al-sem dependency-pipeline.ts:445-506.)
    let mut publisher_events_by_routine: HashMap<String, Vec<&EventSymbol>> = HashMap::new();
    for evt in &event_graph.events {
        if let Some(pr) = &evt.publisher_routine_id {
            publisher_events_by_routine
                .entry(pr.clone())
                .or_default()
                .push(evt);
        }
    }
    let empty_pub: Vec<&EventSymbol> = Vec::new();
    let mut evidence_by_id: BTreeMap<String, DepOperationEvidence> = BTreeMap::new();
    for r in &ws.routines {
        if r.app_guid != app_guid {
            continue;
        }
        let pubs = publisher_events_by_routine.get(&r.id).unwrap_or(&empty_pub);
        let (direct_facts, _status, _reasons) = direct_facts_for_routine(r, pubs);
        if direct_facts.is_empty() {
            continue;
        }
        let cited_op_ids: HashSet<String> = direct_facts
            .iter()
            .filter_map(|f| f.witness_operation_id.clone())
            .collect();
        if cited_op_ids.is_empty() {
            continue;
        }
        // operationSites first (displayText = op.kind).
        for op in &r.operation_sites {
            if !cited_op_ids.contains(&op.id) {
                continue;
            }
            let a = &op.source_anchor;
            evidence_by_id.insert(
                op.id.clone(),
                DepOperationEvidence {
                    operation_id: op.id.clone(),
                    source_file: a.source_unit_id.clone(),
                    start_line: a.start_line,
                    start_column: a.start_column,
                    end_line: a.end_line,
                    end_column: a.end_column,
                    display_text: op.kind.clone(),
                    control_context: op.control_context.clone(),
                },
            );
        }
        // recordOperations overwrite (richer displayText `${rv}.${op}`;
        // controlContext from the matching operationSite by id).
        for ro in &r.record_operations {
            if !cited_op_ids.contains(&ro.id) {
                continue;
            }
            let matching_cc = r
                .operation_sites
                .iter()
                .find(|op| op.id == ro.id)
                .and_then(|op| op.control_context.clone());
            let a = &ro.source_anchor;
            evidence_by_id.insert(
                ro.id.clone(),
                DepOperationEvidence {
                    operation_id: ro.id.clone(),
                    source_file: a.source_unit_id.clone(),
                    start_line: a.start_line,
                    start_column: a.start_column,
                    end_line: a.end_line,
                    end_column: a.end_column,
                    display_text: format!("{}.{}", ro.record_variable_name, ro.op),
                    control_context: matching_cc,
                },
            );
        }
    }
    // al-sem sorts by operationId.localeCompare; BTreeMap already yields sorted-by-key
    // (byte order). localeCompare on these hash-bearing ascii ids matches byte order
    // for the corpus; we sort explicitly to be unambiguous.
    let mut cited_operation_evidence: Vec<DepOperationEvidence> =
        evidence_by_id.into_values().collect();
    cited_operation_evidence.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));

    // ── depOrderIndex ───────────────────────────────────────────────────────
    // Only in "full" mode AND when at least one own routine has a parsed body.
    // Per summarized routine: a return summary (always), plus an order entry when
    // the routine has scope frames AND ≥1 effect-bearing op or dispatch-relevant
    // callsite. (al-sem dependency-pipeline.ts:508-616.)
    let dep_order_index = build_dep_order_index(
        &ws,
        &app_guid,
        &manifest.identity.version,
        summary_mode,
        &scope_frames_by_routine,
    );

    let mut routines_ids: Vec<String> = own_routine_ids.into_iter().collect();
    routines_ids.sort();

    Some(DependencyArtifactL4 {
        header: DepArtifactHeader {
            app_guid: app_guid.clone(),
            name: manifest.identity.name,
            version: manifest.identity.version,
            source_kind: if includes_source {
                "app-source".to_string()
            } else {
                "symbol-only".to_string()
            },
            package_semantic_hash: String::new(),
            summary_mode: summary_mode.to_string(),
        },
        abi: DependencyArtifactAbi {
            routines_ids,
            intra_app_call_edges,
            cited_operation_evidence,
            dep_order_index,
        },
    })
}

/// Apply the operation-order walk to a routine's op/callsite records (populating
/// `order`) and return the routine's projected scope frames. al-sem's
/// `routine-indexer.ts` runs this during indexing; the Rust L3 assembly does not,
/// so the producer runs it here over the routine's L3-carried features.
///
/// Returns the `DepScopeFrame` projection of the routine's frame table.
fn apply_dep_operation_order(r: &mut L3Routine) -> Vec<DepScopeFrame> {
    // Reconstruct a minimal PFeatures the order walker reads: statement_tree +
    // op/callsite records. The walker mutates `order` on each op/callsite and
    // returns the scope-frame table.
    use crate::engine::l2::features::PFeatures;

    let mut features = PFeatures {
        loops: Vec::new(),
        operation_sites: r.operation_sites.clone(),
        record_operations: Vec::new(),
        call_sites: r.call_sites.clone(),
        field_accesses: Vec::new(),
        record_variables: Vec::new(),
        nesting_depth: 0,
        has_branching: false,
        unreachable_statements: Vec::new(),
        identifier_references: Vec::new(),
        variables: Vec::new(),
        var_assignments: Vec::new(),
        condition_references: Vec::new(),
        statement_tree: r.statement_tree.clone(),
        scope_frames: Vec::new(),
    };

    // attr_names_lc — the order walker's TryFunction gate (a TryFunction body
    // produces no order/frames). Mirror al-sem's attribute-name lowercasing.
    let attr_names_lc: Vec<String> = r
        .attributes_parsed
        .iter()
        .map(|a| a.name.to_lowercase())
        .collect();
    apply_operation_order(&mut features, &attr_names_lc);

    // Write the `order` fields back onto the routine's op/callsite records.
    r.operation_sites = features.operation_sites;
    r.call_sites = features.call_sites;

    features
        .scope_frames
        .into_iter()
        .map(|f| DepScopeFrame {
            frame_id: f.frame_id,
            parent_frame_id: f.parent_frame_id,
            kind: f.kind,
            branch_always_terminates: f.branch_always_terminates,
            branch_terminates_by: f.branch_terminates_by,
            branch_may_fall_through: f.branch_may_fall_through,
        })
        .collect()
}

/// Build the dep order index over the resolved dep model (al-sem
/// dependency-pipeline.ts:508-616). Returns `None` when not "full" mode, when no
/// own routine has a parsed body, or when there is no useful order data.
fn build_dep_order_index(
    ws: &L3Workspace,
    app_guid: &str,
    version: &str,
    summary_mode: &str,
    scope_frames_by_routine: &HashMap<String, Vec<DepScopeFrame>>,
) -> Option<DepOrderIndex> {
    if summary_mode != "full" {
        return None;
    }
    let has_any_parsed_body = ws
        .routines
        .iter()
        .any(|r| r.app_guid == app_guid && r.body_available);
    if !has_any_parsed_body {
        return None;
    }

    let mut routine_entries: Vec<DepRoutineOrderEntry> = Vec::new();
    let mut return_summary_list: Vec<DepReturnSummaryRecord> = Vec::new();

    for r in &ws.routines {
        if r.app_guid != app_guid {
            continue;
        }
        // FAITHFUL gate (al-sem `if (r.summary === undefined) continue`): `runSummaries`
        // assigns a summary to EVERY non-leaf dep-own routine — body-available OR not
        // (a bodyless routine's `computeRoutineReturnSummary` yields unknown/partial).
        // Dep-own routines start with `summary === undefined` so they are all non-leaves
        // → all summarized → none skipped here. (The Rust dep model summarizes the same
        // set; there is no per-routine `summary` field, so "is summarized" ≡ "is an
        // own routine in the resolved dep model", which the appGuid filter above already
        // established. NOT gated on `body_available` — on a bodyless own routine al-sem
        // still emits a return summary, and gating on body would drop it.)

        // Return summary (for ALL summarized own routines — incl. bodyless, which
        // `compute_dep_return_summary` reduces to unknown/partial).
        return_summary_list.push(compute_dep_return_summary(r));

        // Order entry: only for routines with scope frames + ops/callsites. A bodyless
        // routine has no scope frames → it contributes a return summary but no order
        // entry (matches al-sem's `r.features?.scopeFrames` empty-skip below).
        let scope_frames = scope_frames_by_routine
            .get(&r.id)
            .cloned()
            .unwrap_or_default();
        if scope_frames.is_empty() {
            continue;
        }

        let mut op_orders: Vec<DepOperationOrder> = Vec::new();
        for op in &r.operation_sites {
            if let Some(o) = &op.order {
                op_orders.push(DepOperationOrder {
                    operation_id: op.id.clone(),
                    order_id: o.order_id,
                    frame_id: o.frame_id,
                    on_success_path: o.on_success_path,
                    dominates_success_return: o.dominates_success_return,
                });
            }
        }
        let mut cs_orders: Vec<DepCallsiteOrder> = Vec::new();
        for cs in &r.call_sites {
            if let Some(o) = &cs.order {
                cs_orders.push(DepCallsiteOrder {
                    callsite_id: cs.id.clone(),
                    order_id: o.order_id,
                    frame_id: o.frame_id,
                    on_success_path: o.on_success_path,
                    dominates_success_return: o.dominates_success_return,
                });
            }
        }

        // Only include routines with at least one effect-bearing op or callsite.
        if op_orders.is_empty() && cs_orders.is_empty() {
            continue;
        }

        routine_entries.push(DepRoutineOrderEntry {
            routine_id: r.id.clone(),
            scope_frames,
            operation_orders: op_orders,
            callsite_orders: cs_orders,
        });
    }

    if routine_entries.is_empty() && return_summary_list.is_empty() {
        return None;
    }

    return_summary_list.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));
    routine_entries.sort_by(|a, b| a.routine_id.cmp(&b.routine_id));

    // Stamp: sourceContentHash empty (orchestrator fills packageSemanticHash later;
    // the freshness check treats empty as conservatively fresh). app_id/version
    // come from the dep identity (al-sem `dependency-pipeline.ts:605-610`).
    Some(DepOrderIndex {
        stamp: DepOrderIndexStamp {
            app_id: app_guid.to_string(),
            version: version.to_string(),
            source_content_hash: String::new(),
            order_index_schema_version: DEP_ORDER_INDEX_SCHEMA_VERSION.to_string(),
        },
        routines: routine_entries,
        return_summaries: return_summary_list,
    })
}

/// Compute a dep routine's returnability summary (port of al-sem
/// `computeRoutineReturnSummary`, `src/engine/return-summary.ts`). Structural walk
/// over the CFN statement tree, with the TryFunction / no-body / no-tree barriers.
pub fn compute_dep_return_summary(r: &L3Routine) -> DepReturnSummaryRecord {
    let has_try_function = has_attribute(&r.attributes_parsed, "TryFunction");
    let commit_behavior = parse_commit_behavior(&r.attributes_parsed);

    // Symbol-only / no body → unknown / partial.
    if !r.body_available {
        return DepReturnSummaryRecord {
            routine_id: r.id.clone(),
            has_normal_return_path: TriBool::Unknown,
            all_paths_error: TriBool::Unknown,
            has_try_function_boundary: has_try_function,
            coverage: "partial".to_string(),
            commit_behavior,
        };
    }

    // TryFunction → unknown / partial (errors caught internally).
    if has_try_function {
        return DepReturnSummaryRecord {
            routine_id: r.id.clone(),
            has_normal_return_path: TriBool::Unknown,
            all_paths_error: TriBool::Unknown,
            has_try_function_boundary: true,
            coverage: "partial".to_string(),
            commit_behavior,
        };
    }

    // Body available but no statement tree → fall-off-end (normal), partial.
    let Some(tree) = &r.statement_tree else {
        return DepReturnSummaryRecord {
            routine_id: r.id.clone(),
            has_normal_return_path: TriBool::True,
            all_paths_error: TriBool::False,
            has_try_function_boundary: false,
            coverage: "partial".to_string(),
            commit_behavior,
        };
    };

    let reach = walk_subtree(tree);
    DepReturnSummaryRecord {
        routine_id: r.id.clone(),
        has_normal_return_path: TriBool::from(reach.has_normal),
        all_paths_error: TriBool::from(reach.all_error),
        has_try_function_boundary: false,
        coverage: "resolved".to_string(),
        commit_behavior,
    }
}

impl From<bool> for TriBool {
    fn from(b: bool) -> Self {
        if b { TriBool::True } else { TriBool::False }
    }
}

struct SubtreeReach {
    has_normal: bool,
    all_error: bool,
}

/// Walk a CFN subtree for normal-return reachability / all-paths-error (port of
/// al-sem `walkSubtree`, `return-summary.ts`).
fn walk_subtree(node: &crate::engine::l2::features::PCFNNode) -> SubtreeReach {
    match node.kind.as_str() {
        "error" => SubtreeReach {
            has_normal: false,
            all_error: true,
        },
        "exit" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },
        "op" | "call" | "other" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },
        "block" => {
            let children = node.children.as_deref().unwrap_or(&[]);
            if children.is_empty() {
                return SubtreeReach {
                    has_normal: true,
                    all_error: false,
                };
            }
            for child in children {
                if walk_subtree(child).all_error {
                    return SubtreeReach {
                        has_normal: false,
                        all_error: true,
                    };
                }
            }
            SubtreeReach {
                has_normal: true,
                all_error: false,
            }
        }
        "if" => {
            let then_children = node.children.as_deref().unwrap_or(&[]);
            let else_children = node.else_children.as_deref().unwrap_or(&[]);
            let then_reach = if then_children.is_empty() {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            } else {
                walk_subtree_list(then_children)
            };
            let else_reach = if else_children.is_empty() {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            } else {
                walk_subtree_list(else_children)
            };
            SubtreeReach {
                has_normal: then_reach.has_normal || else_reach.has_normal,
                all_error: then_reach.all_error && else_reach.all_error,
            }
        }
        "case" => {
            let branches = node.children.as_deref().unwrap_or(&[]);
            if branches.is_empty() {
                return SubtreeReach {
                    has_normal: true,
                    all_error: false,
                };
            }
            let has_else_branch = !node.else_children.as_deref().unwrap_or(&[]).is_empty();
            let mut all_branches_error = true;
            let mut some_normal = false;
            for branch in branches {
                let reach = walk_subtree(branch);
                if reach.has_normal {
                    some_normal = true;
                }
                if !reach.all_error {
                    all_branches_error = false;
                }
            }
            if has_else_branch {
                let else_reach = walk_subtree_list(node.else_children.as_deref().unwrap_or(&[]));
                if else_reach.has_normal {
                    some_normal = true;
                }
                if !else_reach.all_error {
                    all_branches_error = false;
                }
            } else {
                some_normal = true;
                all_branches_error = false;
            }
            SubtreeReach {
                has_normal: some_normal,
                all_error: all_branches_error,
            }
        }
        "case-branch" => walk_subtree_list(node.children.as_deref().unwrap_or(&[])),
        "while" | "for" | "foreach" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },
        "repeat" => {
            let body = node.children.as_deref().unwrap_or(&[]);
            if walk_subtree_list(body).all_error {
                SubtreeReach {
                    has_normal: false,
                    all_error: true,
                }
            } else {
                SubtreeReach {
                    has_normal: true,
                    all_error: false,
                }
            }
        }
        "try" => SubtreeReach {
            has_normal: true,
            all_error: false,
        },
        _ => SubtreeReach {
            has_normal: true,
            all_error: false,
        },
    }
}

fn walk_subtree_list(nodes: &[crate::engine::l2::features::PCFNNode]) -> SubtreeReach {
    if nodes.is_empty() {
        return SubtreeReach {
            has_normal: true,
            all_error: false,
        };
    }
    for node in nodes {
        if walk_subtree(node).all_error {
            return SubtreeReach {
                has_normal: false,
                all_error: true,
            };
        }
    }
    SubtreeReach {
        has_normal: true,
        all_error: false,
    }
}

/// Parse `[CommitBehavior(CommitBehavior::Ignore|Error)]` → "ignore"|"error", else
/// "normal" (port of al-sem `parseCommitBehavior`, both native + ABI arg shapes).
fn parse_commit_behavior(attrs: &[AttributeInfo]) -> String {
    let Some(attr) = find_attribute(attrs, "CommitBehavior") else {
        return "normal".to_string();
    };
    let arg0 = attr.args.first();
    // Native shape: qualified_enum_value → member. ABI shape: bare value/text.
    let member = arg0
        .and_then(|a| a.member.clone())
        .or_else(|| arg0.and_then(|a| a.value.clone()))
        .or_else(|| arg0.map(|a| a.text.clone()));
    match member.map(|m| m.to_lowercase()).as_deref() {
        Some("ignore") => "ignore".to_string(),
        Some("error") => "error".to_string(),
        _ => "normal".to_string(),
    }
}

// ===========================================================================
// The CONSUMER hooks.
// ===========================================================================

/// One synthetic direct-call typed edge injected by `inject_intra_app_call_edges`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectedTypedEdge {
    pub kind: String,
    pub from: String,
    pub to: String,
    pub callsite_id: String,
    /// Always "synthetic" (the dep routines have no real source location).
    pub syntax_kind: String,
}

/// The consumer-side model fields the R3a-4 hooks populate. A focused stand-in for
/// al-sem's `SemanticModel` (only the R3a-4-consumed fields), so the cross-app L4
/// path (R3a-5) can read the injected edges + collected evidence/order data.
#[derive(Debug, Clone, Default)]
pub struct ConsumerModel {
    /// Routine-id membership (the both-ends-in-model injection guard).
    pub routine_ids: HashSet<String>,
    /// Injected synthetic direct-call typed edges (the cone substrate addition).
    pub injected_typed_edges: Vec<InjectedTypedEdge>,
    /// Collected cited dep operation evidence (deduped/sorted).
    pub cited_dep_operation_evidence: Vec<DepOperationEvidence>,
    /// Collected dep routine order entries (keyed by routineId, first-wins).
    pub dep_routine_order_entries: BTreeMap<String, DepRoutineOrderEntry>,
    /// Collected dep return summaries (keyed by routineId, first-wins).
    pub dep_return_summaries: BTreeMap<String, DepReturnSummaryRecord>,
}

impl ConsumerModel {
    /// A merged model whose routine membership is the given id set (the workspace +
    /// dep merge; here the test seeds it with the dep's own routines).
    pub fn with_routine_ids(ids: Vec<String>) -> Self {
        ConsumerModel {
            routine_ids: ids.into_iter().collect(),
            ..Default::default()
        }
    }
}

/// Inject each artifact's `intraAppCallEdges` into the model as synthetic
/// `direct-call` typed edges — but ONLY when both ends are present in the merged
/// model (the membership guard). Mirrors al-sem `injectIntraAppCallEdges`
/// (`dependency-artifact.ts:281-320`). Engine-never-throws: artifacts without
/// intra-app edges are a no-op.
pub fn inject_intra_app_call_edges(model: &mut ConsumerModel, artifacts: &[DependencyArtifactL4]) {
    let mut extra: Vec<InjectedTypedEdge> = Vec::new();
    for artifact in artifacts {
        for edge in &artifact.abi.intra_app_call_edges {
            if !model.routine_ids.contains(&edge.from) || !model.routine_ids.contains(&edge.to) {
                continue;
            }
            // Prefer the stored real callsite id (source-parsed artifacts); fall
            // back to the legacy synthetic `${from}📦${to}` id when absent.
            let callsite_id = edge
                .callsite_id
                .clone()
                .unwrap_or_else(|| format!("{}\u{1f4e6}{}", edge.from, edge.to));
            extra.push(InjectedTypedEdge {
                kind: "direct-call".to_string(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                callsite_id,
                syntax_kind: "synthetic".to_string(),
            });
        }
    }
    model.injected_typed_edges.extend(extra);
}

/// Collect cited operation evidence from all artifacts into the model, dedup by
/// operationId (first-wins) + sort. Mirrors al-sem `collectCitedDepEvidence`.
pub fn collect_cited_dep_evidence(model: &mut ConsumerModel, artifacts: &[DependencyArtifactL4]) {
    let mut by_id: BTreeMap<String, DepOperationEvidence> = BTreeMap::new();
    for artifact in artifacts {
        for rec in &artifact.abi.cited_operation_evidence {
            by_id
                .entry(rec.operation_id.clone())
                .or_insert_with(|| rec.clone());
        }
    }
    if by_id.is_empty() {
        return;
    }
    let mut sorted: Vec<DepOperationEvidence> = by_id.into_values().collect();
    sorted.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    model.cited_dep_operation_evidence = sorted;
}

/// Collect dep order index data from all artifacts whose stamp is FRESH (the
/// freshness barrier). Stale / absent / schema-mismatched → skipped. Mirrors
/// al-sem `collectDepOrderIndex`.
pub fn collect_dep_order_index(model: &mut ConsumerModel, artifacts: &[DependencyArtifactL4]) {
    let mut order_entries: BTreeMap<String, DepRoutineOrderEntry> = BTreeMap::new();
    let mut return_summaries: BTreeMap<String, DepReturnSummaryRecord> = BTreeMap::new();

    for artifact in artifacts {
        let Some(idx) = &artifact.abi.dep_order_index else {
            continue;
        };
        // Freshness barrier (§J8). Stale → skip the artifact entirely.
        if !is_dep_order_index_stamp_fresh(&idx.stamp, &artifact.header) {
            continue;
        }
        for entry in &idx.routines {
            order_entries
                .entry(entry.routine_id.clone())
                .or_insert_with(|| entry.clone());
        }
        for rs in &idx.return_summaries {
            return_summaries
                .entry(rs.routine_id.clone())
                .or_insert_with(|| rs.clone());
        }
    }

    if !order_entries.is_empty() {
        model.dep_routine_order_entries = order_entries;
    }
    if !return_summaries.is_empty() {
        model.dep_return_summaries = return_summaries;
    }
}

/// Verify a dep order index stamp against an artifact header identity. FRESH iff
/// ALL of: appId matches, version matches, schema version matches, AND the
/// sourceContentHash matches the header's packageSemanticHash (OR the
/// packageSemanticHash is empty — "not yet filled", conservatively fresh).
/// Mirrors al-sem `isDepOrderIndexStampFresh` exactly. A false return MUST cause
/// the caller to treat the order index as ABSENT (a barrier).
pub fn is_dep_order_index_stamp_fresh(
    stamp: &DepOrderIndexStamp,
    header: &DepArtifactHeader,
) -> bool {
    if stamp.app_id != header.app_guid {
        return false;
    }
    if stamp.version != header.version {
        return false;
    }
    if stamp.order_index_schema_version != DEP_ORDER_INDEX_SCHEMA_VERSION {
        return false;
    }
    // sourceContentHash check: empty packageSemanticHash → conservatively fresh.
    if !header.package_semantic_hash.is_empty()
        && stamp.source_content_hash != header.package_semantic_hash
    {
        return false;
    }
    true
}

#[cfg(test)]
mod capped_io_tests {
    use super::*;

    /// Task T2.2: `iterate_embedded_source`'s established contract is
    /// "skip the one bad entry, keep ingesting the rest" (an unreadable
    /// archive index is already skipped this way). An entry over
    /// [`crate::capped_io::EMBEDDED_AL_SOURCE_CAP`] must join that SAME
    /// per-entry skip — never abort the whole `.app`, never panic — while a
    /// well-under-cap sibling entry in the same archive still comes through.
    #[test]
    fn oversized_al_entry_is_skipped_others_still_ingested() {
        use std::io::Write as _;

        let mut buf = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            writer.start_file("Small.Codeunit.al", opts).unwrap();
            writer.write_all(b"codeunit 1 Small { }").unwrap();

            writer.start_file("Bomb.Codeunit.al", opts).unwrap();
            const CHUNK: usize = 1024 * 1024;
            let chunk = vec![0u8; CHUNK];
            let mut remaining = crate::capped_io::EMBEDDED_AL_SOURCE_CAP as usize + 1024;
            while remaining > 0 {
                let n = remaining.min(CHUNK);
                writer.write_all(&chunk[..n]).unwrap();
                remaining -= n;
            }

            writer.finish().unwrap();
        }

        let files = iterate_embedded_source(&buf.into_inner());
        assert_eq!(
            files.len(),
            1,
            "the oversized entry must be skipped, the small one kept"
        );
        assert_eq!(files[0].relative_path, "Small.Codeunit.al");
        assert_eq!(files[0].content, "codeunit 1 Small { }");
    }

    /// The source of a Ready-to-Run package lives in its nested app; the
    /// wrapper's own zip holds only `.dll` files and manifests.
    #[test]
    fn iterate_embedded_source_reads_the_nested_app_of_a_ready_to_run_package() {
        use crate::engine::deps::app_package_zip::test_apps;
        let app = test_apps::build_app(&[(
            "src/SalesPost.Codeunit.al",
            b"codeunit 80 \"Sales-Post\" { }",
        )]);
        let files = iterate_embedded_source(&test_apps::wrap_ready_to_run(&app));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, "src/SalesPost.Codeunit.al");
        assert_eq!(files[0].content, "codeunit 80 \"Sales-Post\" { }");
    }
}
