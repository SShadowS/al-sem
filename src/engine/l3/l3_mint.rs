//! 1B.3b Task 3: the LAST sanctioned L3-oracle access point in the library.
//!
//! Engine-switch S1 moved this module from `program::l3_mint` into the legacy
//! engine, together with the two `mint_l3_*` wrappers that used to sit in
//! `semantic_golden.rs`: nothing under `src/program` may import the legacy engine
//! (guard: `program_has_no_legacy_engine_imports`). It is migration-only tooling
//! and is deleted with L3 (spec S9), once its regeneration callers are replaced.
//!
//! Before this task, `src/program/resolve` (the fresh resolver's GATE
//! module — `differential.rs` + `semantic_golden.rs`) depended on
//! `engine::l3`/`engine::l2` two ways: FOUR live dual-run "fresh vs L3"
//! comparison gates that ran on every CDO-gated test, and three L3-oracle
//! projections used only to MINT the committed, frozen, anonymized semantic
//! goldens. 1B.3b Task 3 deleted the dual-run gates outright — the frozen
//! goldens plus the ported fan-out applicability teeth
//! (`semantic_golden::route_applicability`) are now L3-INDEPENDENT and fully
//! replace what those gates checked — and relocated the L3-oracle
//! projections HERE, out of `src/program/resolve` entirely, so the gate
//! module itself (`differential.rs`/`semantic_golden.rs`) carries ZERO
//! `engine::l3`/`engine::l2` imports.
//!
//! # Callers
//!
//! - The dev-mint tool (`src/bin/mint-goldens.rs`, OUTSIDE `src/program/resolve`)
//!   — the only way to (re)mint the three committed goldens under
//!   `tests/goldens/semantic-edges/` from a real, CDO-licensed workspace.
//! - [`mint_l3_validated_golden`] / [`mint_l3_trigger_golden`] wrap
//!   [`project_l3`] / [`project_l3_implicit_trigger_in_scope`] respectively.
//! - `tests/program_resolve_harness.rs`'s `REGEN_TEMP_GOLDENS=1` opt-in
//!   fixture-regen path (`fixture_semantic_golden_matches_l3`), which mints a
//!   small in-repo fixture golden directly via `mint_l3_validated_golden`.
//!
//! None of these run as part of the default `cargo test --workspace` gate —
//! they are dev-only / opt-in / explicitly env-gated.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use al_syntax::IdentifierFoldExt;

use crate::program::resolve::differential::{
    CanonicalEdge, CanonicalEventRow, CanonicalKey, CanonicalSiteKey, CanonicalTarget,
    make_canonical_key, object_kind_str_to_tag,
};
use crate::program::resolve::semantic_golden::{SemanticGolden, build_golden_from_canonical};

/// **SANCTIONED L3 ORACLE USE (1B.3b: the dev-mint tool is the only caller
/// post-freeze; the in-repo fixture's `REGEN_TEMP_GOLDENS` path also still
/// calls this directly — see `tests/program_resolve_harness.rs` Test 14)**:
/// mint the Member/Interface semantic golden from the L3 oracle.
///
/// Calls [`project_l3`] over `workspace_root`, collects per-site target sets into
/// a [`SemanticGolden`] keyed by column-ignoring site key.
///
/// Empty target sets (L3 Unknown/Unresolved) are retained — they record sites
/// that L3 extracted but could not resolve, so the golden covers them.
#[must_use]
pub fn mint_l3_validated_golden(workspace_root: &Path) -> SemanticGolden {
    build_golden_from_canonical(&project_l3(workspace_root))
}

/// **SANCTIONED L3 ORACLE USE (1B.3b dev-mint tool only)**: mint the
/// ImplicitTrigger semantic golden from L3's native `PRecordOperation`-keyed
/// edges ([`project_l3_implicit_trigger_in_scope`]). Backs `cdo-trigger-anon.json`.
#[must_use]
pub fn mint_l3_trigger_golden(workspace_root: &Path) -> SemanticGolden {
    build_golden_from_canonical(&project_l3_implicit_trigger_in_scope(workspace_root))
}
use crate::program::resolve::edge::{CanonicalSpan, EdgeKind, SourcePos, callee_fp};

// ---------------------------------------------------------------------------
// L3 oracle projection
// ---------------------------------------------------------------------------

/// Project the L3 resolver's output over `workspace_root` into
/// [`CanonicalEdge`]s — the oracle side of the (now dev-only) differential.
///
/// # Algorithm
/// 1. Assemble + resolve the workspace with the default L3 pipeline (mirrors
///    `aldump --l3-call-graph-stats`).
/// 2. Build two lookup maps:
///    - `routine_by_id`: internal routine id → `&L3Routine`
///    - `callsite_by_id`: internal callsite id → `&PCallSite`
/// 3. For each `CallEdge` emitted by `resolve_calls`:
///    - Look up the `from` routine → build [`CanonicalKey`] via
///      [`make_canonical_key`] (same helper as the fresh side).
///    - Look up the callsite → read `PAnchor` (0-based line/col — same basis
///      as the fresh side's `byte_to_pos`) → build [`CanonicalSpan`].
///    - Compute `callee_fp` via [`callee_fp`] on `PCallSite::callee_text`
///      (same hash as `stub.rs`).
///    - If `to` is `Some` → look up the callee routine → build one
///      [`CanonicalTarget`] with `kind` from [`object_kind_str_to_tag`].
///    - If `to` is `None` → empty targets set (same as fresh Unresolved).
/// 4. Sort the result by the natural `CanonicalEdge` `Ord` for determinism.
///
/// Returns an empty `Vec` when the workspace is unsound / unparseable
/// (fail-closed, never panics).
///
/// # PAnchor coordinate base
/// `PAnchor.start_line` / `start_column` are **0-based** (the IR walk fills
/// them from tree-sitter row/utf-16-col directly, both 0-based).  The fresh
/// side's `byte_to_pos` is also 0-based.  No conversion is needed, but note
/// that the fresh side records **byte columns** while L3 records **UTF-16
/// columns** — they agree on ASCII-only source; non-ASCII may differ by one
/// column in the matcher.
#[must_use]
pub fn project_l3(workspace_root: &Path) -> Vec<CanonicalEdge> {
    use crate::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
    use crate::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
    use crate::engine::l3::symbol_table::SymbolTable;

    let Some(resolved) = assemble_and_resolve_workspace_default(workspace_root) else {
        return Vec::new();
    };
    let ws = &resolved.workspace;

    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let no_deps: Vec<DeclaredDependency> = Vec::new();
    let no_fetched: Vec<String> = Vec::new();
    let resolved_calls = resolve_calls(ws, &symbols, &no_deps, &no_fetched);

    // Build lookup: internal routine id → &L3Routine.
    let routine_by_id: HashMap<&str, &crate::engine::l3::l3_workspace::L3Routine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();

    // Build lookup: internal callsite id → &PCallSite.
    // PCallSite.id is the internal callsite id (`{routine_id}/cs{N}`).
    let mut callsite_by_id: HashMap<&str, &crate::engine::l2::features::PCallSite> = HashMap::new();
    for routine in &ws.routines {
        for cs in &routine.call_sites {
            callsite_by_id.insert(cs.id.as_str(), cs);
        }
    }

    let mut edges: Vec<CanonicalEdge> = resolved_calls
        .edges
        .iter()
        .filter_map(|edge| {
            // Resolve the `from` routine.
            let from_r = routine_by_id.get(edge.from.as_str())?;
            let from = make_canonical_key(
                from_r.app_guid.clone(),
                from_r.object_type.to_ascii_lowercase(),
                format!("{}", from_r.object_number),
                from_r.name.fold_identifier(),
            );

            // Resolve the callsite → span + callee fingerprint.
            let cs = callsite_by_id.get(edge.callsite_id.as_str())?;
            let a = &cs.source_anchor;
            // PAnchor line/col are 0-based (from tree-sitter row + utf-16 col).
            // L3 workspace anchors carry `source_unit_id = "ws:<rel-posix-path>"`.
            // Strip the "ws:" prefix so the canonical span unit matches the
            // fresh side's `virtual_path` (a plain relative POSIX path).
            let unit_str = a
                .source_unit_id
                .strip_prefix("ws:")
                .unwrap_or(&a.source_unit_id)
                .to_string();
            let span = CanonicalSpan {
                unit: unit_str,
                start: SourcePos {
                    line: a.start_line,
                    col: a.start_column,
                },
                end: SourcePos {
                    line: a.end_line,
                    col: a.end_column,
                },
            };
            let fp = callee_fp(&cs.callee_text);

            // The callsite's logical caller == the `from` routine on the L3 model.
            let site = CanonicalSiteKey {
                caller: from.clone(),
                span,
                callee_fp: fp,
            };

            // Build the target set.  L3 `to == None` → unresolved → empty set.
            let targets: BTreeSet<CanonicalTarget> = if let Some(to_id) = &edge.to {
                if let Some(to_r) = routine_by_id.get(to_id.as_str()) {
                    let mut set = BTreeSet::new();
                    set.insert(CanonicalTarget {
                        kind: object_kind_str_to_tag(&to_r.object_type.to_ascii_lowercase()),
                        app: Some(to_r.app_guid.clone()),
                        object_lc: format!("{}", to_r.object_number),
                        routine_lc: Some(to_r.name.fold_identifier()),
                    });
                    set
                } else {
                    // `to` id present but not in the workspace index — treat as
                    // unresolved (should not happen in a sound workspace).
                    BTreeSet::new()
                }
            } else {
                BTreeSet::new()
            };

            Some(CanonicalEdge {
                from,
                site,
                kind: EdgeKind::Call,
                targets,
            })
        })
        .collect();

    edges.sort();
    edges
}

// ---------------------------------------------------------------------------
// L3 ImplicitTrigger oracle projection
// ---------------------------------------------------------------------------

/// Project the L3 resolver's `DispatchKind::ImplicitTrigger` edges for the workspace.
///
/// The L3 `build_implicit_trigger_edges` uses `op.id` (a `PRecordOperation` id)
/// as `callsite_id`.  To recover the call site's source span and callee-text
/// fingerprint this function builds a reverse map `op.id → PCallSite` via
/// `PCallSite.operation_id`.
///
/// SANCTIONED L3 USE (1B.3b Task 3): the dev-mint tool
/// (`src/bin/mint-goldens.rs`) and `semantic_golden::mint_l3_trigger_golden`
/// call this to freeze the ImplicitTrigger committed golden
/// (`cdo-trigger-anon.json`). No live dual-run gate calls it anymore — the
/// old `run_implicit_trigger_harness` (`differential.rs`) was deleted in
/// 1B.3b Task 3; its coverage is now the frozen trigger baseline + the
/// `tests/fixtures/implicit-trigger` fixture + the ported applicability teeth.
#[must_use]
pub fn project_l3_implicit_trigger_in_scope(workspace_root: &Path) -> Vec<CanonicalEdge> {
    use crate::engine::l3::call_resolver::{DeclaredDependency, resolve_calls};
    use crate::engine::l3::l3_workspace::{
        L3RecordOperation, assemble_and_resolve_workspace_default,
    };
    use crate::engine::l3::symbol_table::SymbolTable;
    use crate::engine::l3::taxonomy::DispatchKind;

    let Some(resolved) = assemble_and_resolve_workspace_default(workspace_root) else {
        return Vec::new();
    };
    let ws = &resolved.workspace;

    let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let no_deps: Vec<DeclaredDependency> = Vec::new();
    let no_fetched: Vec<String> = Vec::new();
    let resolved_calls = resolve_calls(ws, &symbols, &no_deps, &no_fetched);

    let routine_by_id: HashMap<&str, &crate::engine::l3::l3_workspace::L3Routine> =
        ws.routines.iter().map(|r| (r.id.as_str(), r)).collect();

    // L3 ImplicitTrigger edges use PRecordOperation.id as callsite_id.
    // Build a direct op.id → &L3RecordOperation map (NOT via PCallSite.operation_id,
    // which is a separate numbering namespace: "{routine}/op{op_count+i}").
    let mut op_by_id: HashMap<&str, &L3RecordOperation> = HashMap::new();
    for routine in &ws.routines {
        for op in &routine.record_operations {
            op_by_id.insert(op.id.as_str(), op);
        }
    }

    let mut edges: Vec<CanonicalEdge> = resolved_calls
        .edges
        .iter()
        .filter(|edge| matches!(edge.dispatch_kind, DispatchKind::ImplicitTrigger))
        .filter_map(|edge| {
            let from_r = routine_by_id.get(edge.from.as_str())?;
            let from = make_canonical_key(
                from_r.app_guid.clone(),
                from_r.object_type.to_ascii_lowercase(),
                format!("{}", from_r.object_number),
                from_r.name.fold_identifier(),
            );

            // ImplicitTrigger edges use PRecordOperation.id as callsite_id;
            // look up the record op directly for its source_anchor and callee text.
            let op = op_by_id.get(edge.callsite_id.as_str())?;
            let a = &op.source_anchor;
            let unit_str = a
                .source_unit_id
                .strip_prefix("ws:")
                .unwrap_or(&a.source_unit_id)
                .to_string();
            let span = CanonicalSpan {
                unit: unit_str,
                start: SourcePos {
                    line: a.start_line,
                    col: a.start_column,
                },
                end: SourcePos {
                    line: a.end_line,
                    col: a.end_column,
                },
            };
            // callee_fp must match the fresh side: fresh uses the raw Member expression
            // text (e.g. "Rec.Insert"); L3RecordOperation stores the receiver name and
            // op in the same original case → produce the same lowercased fingerprint.
            let callee_text = format!("{}.{}", op.record_variable_name, op.op);
            let fp = callee_fp(&callee_text);
            let site = CanonicalSiteKey {
                caller: from.clone(),
                span,
                callee_fp: fp,
            };

            let targets: BTreeSet<CanonicalTarget> = if let Some(to_id) = &edge.to {
                if let Some(to_r) = routine_by_id.get(to_id.as_str()) {
                    let mut set = BTreeSet::new();
                    set.insert(CanonicalTarget {
                        kind: object_kind_str_to_tag(&to_r.object_type.to_ascii_lowercase()),
                        app: Some(to_r.app_guid.clone()),
                        object_lc: format!("{}", to_r.object_number),
                        routine_lc: Some(to_r.name.fold_identifier()),
                    });
                    set
                } else {
                    BTreeSet::new()
                }
            } else {
                BTreeSet::new()
            };

            Some(CanonicalEdge {
                from,
                site,
                kind: EdgeKind::ImplicitTrigger,
                targets,
            })
        })
        .collect();

    edges.sort();
    edges
}

// ---------------------------------------------------------------------------
// L3 EventFlow oracle projection
// ---------------------------------------------------------------------------

/// SANCTIONED L3 USE (1B.3b Task 3): project L3's RESOLVED EventFlow
/// publisher→subscriber pairs for `workspace_root`, keyed by the SAME
/// `CanonicalKey` shape as `differential::project_fresh_event_rows`. Only
/// `engine::l3::event_graph::project_event_graph` edges with
/// `resolution == "resolved"` contribute a row; a row is skipped (fail-closed)
/// when the publisher's routine id is unknown or either endpoint's
/// `stable_routine_id` does not resolve to an `L3Routine` in this workspace.
///
/// Used ONLY by the dev-mint tool to freeze `cdo-event-anon.json`. No live
/// dual-run gate calls it anymore — the old `run_event_flow_gate`
/// (`differential.rs`) was deleted in 1B.3b Task 3; its coverage is now the
/// frozen event baseline + the `tests/fixtures/events` fixture
/// (`event_fixture_two_stage_join`) + the ported event-route teeth.
#[must_use]
pub fn project_l3_event_rows(workspace_root: &Path) -> Vec<CanonicalEventRow> {
    use crate::engine::l3::event_graph::{build_event_graph, project_event_graph};
    use crate::engine::l3::l3_workspace::assemble_and_resolve_workspace_default;
    use crate::engine::l3::symbol_table::SymbolTable;

    let Some(resolved) = assemble_and_resolve_workspace_default(workspace_root) else {
        return Vec::new();
    };
    let ws = &resolved.workspace;
    let l3_symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
    let l3_eg = build_event_graph(&ws.routines, &l3_symbols);
    let l3_proj = project_event_graph(&l3_eg);

    let routine_by_stable_id: HashMap<&str, &crate::engine::l3::l3_workspace::L3Routine> = ws
        .routines
        .iter()
        .map(|r| (r.stable_routine_id.as_str(), r))
        .collect();
    let sym_by_id: HashMap<&str, &crate::engine::l3::event_graph::PEventSymbol> =
        l3_proj.events.iter().map(|s| (s.id.as_str(), s)).collect();

    let key_of = |r: &crate::engine::l3::l3_workspace::L3Routine| -> CanonicalKey {
        make_canonical_key(
            r.app_guid.clone(),
            r.object_type.to_ascii_lowercase(),
            format!("{}", r.object_number),
            r.name.fold_identifier(),
        )
    };

    let mut rows: Vec<CanonicalEventRow> = l3_proj
        .edges
        .iter()
        .filter(|edge| edge.resolution == "resolved")
        .filter_map(|edge| {
            let sym = sym_by_id.get(edge.event_id.as_str())?;
            let pub_stable = sym.publisher_routine_id.as_deref()?;
            let pub_r = routine_by_stable_id.get(pub_stable)?;
            let sub_r = routine_by_stable_id.get(edge.subscriber_routine_id.as_str())?;
            Some(CanonicalEventRow {
                publisher: key_of(pub_r),
                event_name_lc: sym.event_name.fold_identifier(),
                subscriber: key_of(sub_r),
                publisher_arity: Some(sym.parameters.len()),
            })
        })
        .collect();
    rows.sort();
    rows
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_l3_yields_spanned_canonical_edges_on_cdo() {
        let Some(ws) = std::env::var_os("CDO_WS")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let edges = project_l3(&ws);
        assert!(
            edges.len() > 1000,
            "L3 should project many edges, got {}",
            edges.len()
        );
        // Every projected site carries a real span (non-zero end).
        assert!(
            edges
                .iter()
                .all(|e| e.site.span.end.line >= e.site.span.start.line)
        );
    }
}
