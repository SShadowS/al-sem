//! Core call-hierarchy handlers on the program-engine `LspSnapshot` (T3 Task
//! 11) — `prepare`/`incoming`/`outgoing`, the engine-backed replacements for
//! `src/handlers.rs`'s `prepare_call_hierarchy`/`incoming_calls`/
//! `outgoing_calls` (cut over at Task 15).
//!
//! # The two NON-NEGOTIABLE live-span rules (audit §6.1 + Task-10 finding)
//!
//! 1. A TARGET routine's `range`/`selectionRange` NEVER comes from a stored
//!    `Route::Witness::SourceSpan` (or any other baked-in `ClassifiedEdge`
//!    span) — always re-derived LIVE from [`LspSnapshot::decl_and_text`] at
//!    query time. A stored witness span is byte-extent-dependent on the
//!    TARGET file's CURRENT body (`docs/superpowers/specs/
//!    2026-07-12-t3-def-surface-audit.md` §3.4) and goes stale the instant
//!    that file's body is edited, even though rung 1 never re-resolves the
//!    UNRELATED caller edge that references it.
//! 2. An `EdgeKind::EventFlow` edge's `SiteId`/`site.span` is NEVER served
//!    either — `event_edges` is Arc-cloned FORWARD, unchanged, across every
//!    rung-1 apply (by design — see `updater.rs`'s module doc), so its
//!    cached span goes stale the instant ANYTHING above the publisher's own
//!    declaration in its file shifts lines. Every EventFlow-derived position
//!    (fromRanges AND the publisher/subscriber items themselves) is
//!    re-derived from [`LspSnapshot::decl_and_text`] instead.
//!
//! # Position encoding
//!
//! Every byte-native span crosses the LSP boundary through exactly one of
//! [`origin_to_range`]/[`canonical_span_to_range`], both routed through a
//! per-file [`LineTable`] built from the CURRENT snapshot's text — never a
//! hand-rolled column computation elsewhere in this module.

use std::collections::HashMap;

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Uri,
};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::lsp::encoding::{ColOut, PositionEncoding};
use crate::lsp::snapshot::{DeclView, LspSnapshot};
use crate::program::resolve::edge::{AbiRoutineKey, EdgeKind, Route, RouteTarget};
use crate::program::resolve::full::ClassifiedEdge;
use crate::program::{AppRef, ObjectNodeId, ProgramGraph, RoutineNodeId};
use crate::protocol::{path_to_uri, uri_to_path};

/// `item.data` payload — a serde round-trip of the content-addressed id.
/// Additive `Serialize`/`Deserialize` derives on `RoutineNodeId` and its
/// component types (`src/program/node.rs`) make this possible with zero
/// behavior change to any resolution path (see that file's own doc on the
/// serde "remote" mirror it uses for the foreign `al_syntax::ir::ObjectKind`
/// type).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemData {
    pub node: RoutineNodeId,
}

/// Path-segment encode set for the synthetic dependency-source/ABI-symbol
/// URIs this module mints (see [`dep_source_uri`]/[`abi_symbol_uri`]) — a
/// deliberately conservative set (space, the path separators, and the
/// percent sign itself) since no real client parses these authoritatively
/// yet; a future content-provider task can re-derive the exact identity
/// from the decoded segments regardless of which characters ended up
/// percent-escaped.
const SYNTH_URI_SEGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'/').add(b'?').add(b'#').add(b'%');

const ZERO_RANGE: Range = Range {
    start: Position {
        line: 0,
        character: 0,
    },
    end: Position {
        line: 0,
        character: 0,
    },
};

// ---------------------------------------------------------------------------
// prepare
// ---------------------------------------------------------------------------

/// `textDocument/prepareCallHierarchy`. `uri`/`line`/`character` are the
/// raw wire values (Task 15 deserializes `CallHierarchyPrepareParams` and
/// unpacks them); `character` is in the negotiated `enc`, converted to the
/// engine's native UTF-8 byte column via a per-file [`LineTable`] before
/// [`LspSnapshot::decl_at`] ever sees it.
///
/// Returns `None` for: an unparsable/non-`file://` `uri`, a `uri` outside
/// `snap.workspace_root` (dependency-file-originated call hierarchy is
/// explicitly out of v1 scope — design doc §12), or a position that hits no
/// routine (`decl_at` returns `None`).
#[must_use]
pub fn prepare(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    uri: &str,
    line: u32,
    character: u32,
) -> Option<Vec<CallHierarchyItem>> {
    let virtual_path = resolve_virtual_path(snap, uri)?;
    let entry = snap.parsed.get(&virtual_path)?;
    let table = entry.line_table();
    let byte_col = table.col_in(line, character, enc);

    let decl = snap.decl_at(&virtual_path, line, byte_col)?;
    let view = DeclView::from_entry(decl);
    let item = build_item(snap, enc, view, table, decl_uri(snap, view), None);
    Some(vec![item])
}

/// Turn an inbound `textDocument` URI into the `virtual_path` key
/// `decls_by_file`/`parsed` use. Two normalization mismatches to reconcile,
/// neither of which the other side of this lookup applies:
///
/// - [`uri_to_path`] unconditionally lowercases the WHOLE path on Windows
///   (`protocol::normalize_path`, matching how Windows filesystems are
///   themselves case-insensitive); [`LspSnapshot::workspace_root`] is
///   stored normalized the SAME way so `strip_prefix` succeeds structurally.
/// - But `virtual_path` keys are extracted CASE-PRESERVING from disk
///   (`snapshot::provider::walk_al_source`'s `path.strip_prefix(root)`, no
///   normalization) — so the lowercased relative path FROM the URI can
///   legitimately differ in case from the real key even after `strip_prefix`
///   succeeds (e.g. a real `Codeunit1.al` vs. a lowercased `codeunit1.al`
///   candidate). Try the exact (fast, common: Unix, or an all-lowercase
///   workspace) key first; fall back to a case-insensitive scan of
///   `snap.parsed`'s keys — never silently return the wrong file.
pub(crate) fn resolve_virtual_path(snap: &LspSnapshot, uri: &str) -> Option<String> {
    let parsed_uri: Uri = uri.parse().ok()?;
    let path = uri_to_path(&parsed_uri)?;
    let rel = path.strip_prefix(snap.workspace_root.as_path()).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");

    if snap.parsed.contains_key(&rel_str) {
        return Some(rel_str);
    }
    snap.parsed
        .keys()
        .find(|k| k.eq_ignore_ascii_case(&rel_str))
        .cloned()
}

// ---------------------------------------------------------------------------
// incoming
// ---------------------------------------------------------------------------

/// `callHierarchy/incomingCalls`. Groups `snap.incoming[data.node]`'s
/// `EdgeRef`s by `edge.from` (the caller/publisher routine) — one
/// `CallHierarchyIncomingCall` per DISTINCT caller, carrying every from-range
/// that caller has to the target (a deliberate improvement over the legacy
/// per-call-site-ungrouped shape — see this module's own report for the
/// exact legacy behavior this diverges from).
///
/// A stale `data.node` (not in `snap.incoming` at all, OR present but its
/// grouped caller's own decl has since vanished) degrades to an empty
/// result for that entry — fail-closed, never a guess or a panic.
#[must_use]
pub fn incoming(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    data: &ItemData,
) -> Vec<CallHierarchyIncomingCall> {
    let Some(refs) = snap.incoming.get(&data.node) else {
        return Vec::new();
    };

    // One pass: resolve every EdgeRef exactly once, grouping by caller.
    // (Previously this re-filtered ALL refs per distinct caller — O(refs²)
    // with a string-hashed map lookup per pair; see the 2026-07-14
    // improvement-hunt F1 finding.)
    let mut groups: HashMap<RoutineNodeId, (bool, Vec<&ClassifiedEdge>)> = HashMap::new();
    for r in refs {
        let ce = snap.edge(r);
        let entry = groups
            .entry(ce.edge.from.clone())
            .or_insert_with(|| (false, Vec::new()));
        entry.0 |= ce.edge.kind == EdgeKind::EventFlow;
        entry.1.push(ce);
    }

    let mut callers: Vec<RoutineNodeId> = groups.keys().cloned().collect();
    callers.sort();

    let mut out = Vec::new();
    for caller_id in callers {
        let (has_event_flow, edges) = &groups[&caller_id];
        let Some((decl, table)) = snap.decl_and_line_table(&caller_id) else {
            // The caller's own decl vanished from the current snapshot —
            // fail closed by dropping this group rather than guessing at a
            // position for an item we can no longer locate.
            continue;
        };
        // Snapshot-scoped cache (`docs/OUTSTANDING.md`'s "Snapshot-scoped
        // LineTable cache" item): for a workspace caller `table` is the SAME
        // `LineTable` every other distinct caller sharing this file, and
        // every OTHER handler querying this file this generation, already
        // built — never re-scanned per call. A dependency caller's is the
        // tier's `LineIndex`.

        let mut from_ranges: Vec<Range> = Vec::new();
        for ce in edges {
            let range = if ce.edge.kind == EdgeKind::EventFlow {
                // Rule 2: an EventFlow edge's own site span is stale-prone;
                // re-derive from the PUBLISHER's (== this caller's) fresh
                // name_origin instead.
                origin_to_range(decl.name_origin, table, enc)
            } else {
                canonical_span_to_range(&ce.edge.site.span, table, enc)
            };
            from_ranges.push(range);
        }
        from_ranges.sort_by_key(range_sort_key);
        from_ranges.dedup();

        let tag = (*has_event_flow).then_some("[EventPublisher]");
        let item = build_item(snap, enc, decl, table, decl_uri(snap, decl), tag);

        out.push(CallHierarchyIncomingCall {
            from: item,
            from_ranges,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// outgoing
// ---------------------------------------------------------------------------

/// `callHierarchy/outgoingCalls`. Iterates `data.node`'s OWN file bucket in
/// `snap.edges_by_file` filtered `edge.from == data.node` (Call/Run/
/// ImplicitTrigger edges), plus `snap.event_edges` filtered the same way
/// (this routine as an event PUBLISHER — subscribers surface as outgoing
/// targets, the design doc's "natural direction" decision). One
/// `CallHierarchyOutgoingCall` PER qualifying route (never grouped by
/// target — see this module's own report for why this mirrors the legacy
/// per-call-site cardinality rather than `incoming`'s deliberate grouping).
///
/// Outgoing route taxonomy (spec §5 / task brief, binding):
/// - `RouteTarget::Routine(id)` → a real item via [`LspSnapshot::decl_and_text`]
///   (workspace OR dependency-with-embedded-source — both give REAL spans).
/// - `RouteTarget::AbiSymbol{key}` → a zero-range item at a synthesized URI
///   (see [`abi_symbol_item`]), matching the legacy external-def fallback's
///   SHAPE (identity-bearing detail + an `external`-flagged `data` blob),
///   deliberately NOT its exact behavior (legacy reused the CALLER's own
///   file/range as a stand-in; this synthesizes an honest zero-range instead).
/// - `RouteTarget::Builtin`/`RouteTarget::Unresolved` → no item (honest
///   dynamic/empty/unknown edges all decline identically — legacy showed
///   nothing for these either).
///
/// `data.node` not found in `snap.decl_by_id` (stale ItemData, OR a
/// dependency-declared routine — outgoing queries only ever originate from
/// a `prepare`-returned workspace item, so this is the SAME "stale" fail
/// path) → empty `Vec`, never a panic.
#[must_use]
pub fn outgoing(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    data: &ItemData,
) -> Vec<CallHierarchyOutgoingCall> {
    let Some(caller_decl) = snap.decl_by_id.get(&data.node) else {
        return Vec::new();
    };
    let Some(caller_entry) = snap.parsed.get(&caller_decl.virtual_path) else {
        return Vec::new();
    };
    let caller_table = caller_entry.line_table();

    let mut out = Vec::new();

    if let Some(edges) = snap.edges_by_file.get(&caller_decl.virtual_path) {
        for ce in edges.iter().filter(|ce| ce.edge.from == data.node) {
            let from_ranges = vec![canonical_span_to_range(
                &ce.edge.site.span,
                caller_table,
                enc,
            )];
            push_route_items(snap, enc, &ce.edge.routes, &from_ranges, &mut out);
        }
    }

    for ce in snap
        .event_edges
        .iter()
        .filter(|ce| ce.edge.from == data.node)
    {
        // Rule 2: re-derive from THIS routine's (the publisher's) own fresh
        // name_origin — never `ce.edge.site.span`.
        let from_ranges = vec![origin_to_range(&caller_decl.name_origin, caller_table, enc)];
        push_route_items(snap, enc, &ce.edge.routes, &from_ranges, &mut out);
    }

    out
}

/// Emit one `CallHierarchyOutgoingCall` per route in `routes` that resolves
/// to a real or ABI-boundary target, sharing the same `from_ranges` (they
/// are all candidates for the SAME call/event site).
fn push_route_items(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    routes: &[Route],
    from_ranges: &[Range],
    out: &mut Vec<CallHierarchyOutgoingCall>,
) {
    for route in routes {
        let item = match &route.target {
            RouteTarget::Routine(rid) => match snap.decl_and_line_table(rid) {
                Some((decl, table)) => {
                    build_item(snap, enc, decl, table, decl_uri(snap, decl), None)
                }
                // Structurally shouldn't happen (a `Routine(id)` route is
                // only ever constructed when the SAME body_map lookup this
                // snapshot's decl indexes were built from just succeeded —
                // see `LspSnapshot::dep_meta`'s doc) — fail closed by
                // skipping rather than guessing.
                None => continue,
            },
            RouteTarget::AbiSymbol { key } => abi_symbol_item(snap, key),
            RouteTarget::Builtin(_) | RouteTarget::Unresolved => continue,
        };
        out.push(CallHierarchyOutgoingCall {
            to: item,
            from_ranges: from_ranges.to_vec(),
        });
    }
}

/// The `RouteTarget::AbiSymbol` fallback item — mirrors legacy's
/// external-definition-found shape (`src/handlers.rs:348-363`: a detail
/// string naming the source app, plus `data: {"external": true, "app":
/// ..}`), but with a HONEST zero-range at a synthesized `al-preview://`
/// URI (reusing the SAME scheme prefix `dependency_document_symbol`'s
/// existing object-level virtual-document convention uses) instead of
/// legacy's choice to reuse the CALLER's own file/range as a stand-in — see
/// this module's own doc / the task report for the exact legacy shapes this
/// deliberately diverges from.
fn abi_symbol_item(snap: &LspSnapshot, key: &AbiRoutineKey) -> CallHierarchyItem {
    let app_name = snap
        .graph
        .apps
        .try_resolve(key.app)
        .map(|id| id.name.as_str())
        .unwrap_or("external");
    let object_display = if key.object_number != 0 {
        format!("{} {}", key.object_type, key.object_number)
    } else {
        format!("{} {}", key.object_type, key.object_name_lc)
    };
    let detail = format!("{object_display}.{} (from {app_name})", key.routine_name_lc);
    let uri = abi_symbol_uri(key, app_name);

    CallHierarchyItem {
        name: key.routine_name_lc.clone(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail: Some(detail),
        uri,
        range: ZERO_RANGE,
        selection_range: ZERO_RANGE,
        data: Some(serde_json::json!({
            "external": true,
            "app": app_name,
        })),
    }
}

// ---------------------------------------------------------------------------
// Shared item construction
// ---------------------------------------------------------------------------

/// Build one `CallHierarchyItem` for `decl`, whose position ALWAYS comes
/// from `decl.origin`/`decl.name_origin` (never a stored edge span — rule 1)
/// converted through `table` (built from the file's CURRENT text) in the
/// negotiated `enc`.
fn build_item(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    decl: DeclView<'_>,
    table: &dyn ColOut,
    uri: Uri,
    tag: Option<&str>,
) -> CallHierarchyItem {
    let object_name = object_name_for(&snap.graph, &decl.id.object).unwrap_or("Unknown");
    let mut detail = format!("{object_name}.{}", decl.name);
    if let Some(t) = tag {
        detail.push(' ');
        detail.push_str(t);
    }

    CallHierarchyItem {
        name: decl.name.to_string(),
        kind: symbol_kind_for(snap, decl.id),
        tags: None,
        detail: Some(detail),
        uri,
        range: origin_to_range(decl.origin, table, enc),
        selection_range: origin_to_range(decl.name_origin, table, enc),
        data: Some(
            serde_json::to_value(ItemData {
                node: decl.id.clone(),
            })
            .expect(
                "ItemData must serialize — RoutineNodeId's component types all derive Serialize",
            ),
        ),
    }
}

/// The URI for `decl`: a real `file://` path for a workspace decl, or a
/// synthesized `al-dep-source://` virtual-document URI for a dependency
/// decl (no real on-disk `.al` file exists for embedded-source dependency
/// text — see `src/snapshot/embedded.rs`'s doc: it is extracted straight
/// from the `.app` zip into memory, never materialized to disk).
fn decl_uri(snap: &LspSnapshot, decl: DeclView<'_>) -> Uri {
    if is_dep_app(snap, decl.id.object.app) {
        dep_source_uri(snap, decl.id.object.app, decl.virtual_path)
    } else {
        path_to_uri(&snap.workspace_root.join(decl.virtual_path))
    }
}

fn is_dep_app(snap: &LspSnapshot, app: AppRef) -> bool {
    snap.graph.apps.find(&snap.snap.workspace_app) != Some(app)
}

/// Synthesize a virtual URI for a dependency-source `DeclEntry` — no real
/// on-disk file backs one (see [`decl_uri`]'s doc). A future task (the
/// "custom" LSP-methods wave, per the arc's task list) is expected to wire
/// a `TextDocumentContentProvider`-style consumer for this scheme; until
/// then, an editor that navigates here has nowhere real to render the
/// content — a known, documented limitation, not a silent wrong answer
/// (the position data itself is real and correct).
fn dep_source_uri(snap: &LspSnapshot, app: AppRef, virtual_path: &str) -> Uri {
    let app_name = snap
        .graph
        .apps
        .try_resolve(app)
        .map(|id| id.name.as_str())
        .unwrap_or("dependency");
    let encoded_app = utf8_percent_encode(app_name, SYNTH_URI_SEGMENT).to_string();
    let encoded_path = virtual_path
        .split('/')
        .map(|seg| utf8_percent_encode(seg, SYNTH_URI_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    format!("al-dep-source:///{encoded_app}/{encoded_path}")
        .parse()
        .unwrap_or_else(|_| {
            "al-dep-source:///unknown"
                .parse()
                .expect("static URI parses")
        })
}

/// Synthesize an `al-preview://` URI for an ABI-boundary (`SymbolOnly`, no
/// embedded source) target — the SAME scheme AND OBJECT-LEVEL LAYOUT
/// `crate::lsp::custom::{dependency_document_symbol, parse_al_preview_uri}`
/// already establish (originally legacy `src/handlers.rs`'s scheme; relocated
/// verbatim to `lsp::custom` at T3 Task 17's legacy deletion): exactly
/// `al-preview:///allang/<App>/<Type>/<Id>/<Name>.dal`, 5 path segments
/// after the scheme (App, Type, Id, Name — `parse_al_preview_uri` anchors
/// on `Type` parsing as a known `crate::types::ObjectType`, so segment
/// ORDER and COUNT both matter, not merely the scheme prefix; an earlier
/// version of this function emitted only 3 segments, `allang/App/
/// ObjectDisplay/Routine`, which the parser structurally rejects —
/// caught in T3 Task 11 review, fixed here).
///
/// Object-level granularity only — no per-routine position exists at this
/// trust tier at all — so this navigates to the dep OBJECT's synthesized
/// preview document, the SAME target a `dependencyDocumentSymbol` click on
/// this object already resolves to; the caller (one specific routine
/// within that object) still gets a zero-range item, unable to jump
/// directly to the routine's own line — a known, documented limitation,
/// not a silently wrong answer.
///
/// A NUMBERED dependency object's [`AbiRoutineKey`] carries no raw-cased
/// display NAME at all (`resolver::make_routine_route` sets
/// `object_name_lc` to `String::new()` for an `ObjKey::Id`-keyed object —
/// only the number survives) — the `Name` segment falls back to the
/// object number's own decimal text in that case, so the URI still
/// round-trips STRUCTURALLY through `parse_al_preview_uri` (which never
/// validates `Name` against real data, just splits it out as a bare
/// string), even though it isn't a real lookupable object name. An
/// `object_type` value the legacy-derived `ObjectType` enum never modeled
/// (e.g. `ReportExtension`) fails `parse_al_preview_uri`'s type-anchor scan
/// entirely — a PRE-EXISTING schema gap inherited from legacy, not something
/// this conformance fix closes.
fn abi_symbol_uri(key: &AbiRoutineKey, app_name: &str) -> Uri {
    let encode = |s: &str| utf8_percent_encode(s, SYNTH_URI_SEGMENT).to_string();
    let id_segment = key.object_number.to_string();
    let name_segment = if key.object_name_lc.is_empty() {
        key.object_number.to_string()
    } else {
        key.object_name_lc.clone()
    };
    format!(
        "al-preview:///allang/{}/{}/{}/{}.dal",
        encode(app_name),
        encode(&key.object_type),
        encode(&id_segment),
        encode(&name_segment)
    )
    .parse()
    .unwrap_or_else(|_| "al-preview:///unknown".parse().expect("static URI parses"))
}

pub(crate) fn object_name_for<'g>(
    graph: &'g ProgramGraph,
    obj_id: &ObjectNodeId,
) -> Option<&'g str> {
    graph
        .objects
        .binary_search_by(|probe| probe.id.cmp(obj_id))
        .ok()
        .map(|i| graph.objects[i].name.as_str())
}

/// Best-effort `SymbolKind` classification (`FUNCTION` vs. `EVENT`) via a
/// `graph.routines` lookup — not required for the audit's live-span
/// guarantees, but cheap and mirrors legacy's `DefinitionKind::Trigger`/
/// `EventSubscriber` → `SymbolKind::EVENT` mapping reasonably closely.
fn symbol_kind_for(snap: &LspSnapshot, id: &RoutineNodeId) -> SymbolKind {
    let node = snap
        .graph
        .routines
        .binary_search_by(|probe| probe.id.cmp(id))
        .ok()
        .map(|i| &snap.graph.routines[i]);
    match node {
        Some(n) if n.is_trigger || !n.event_subscribers.is_empty() => SymbolKind::EVENT,
        _ => SymbolKind::FUNCTION,
    }
}

// ---------------------------------------------------------------------------
// Position-encoding conversion — the ONE place a byte-native span crosses
// into an LSP `Range` (see this module's own doc).
// ---------------------------------------------------------------------------

pub(crate) fn origin_to_range(
    origin: &al_syntax::ir::Origin,
    table: &dyn ColOut,
    enc: PositionEncoding,
) -> Range {
    Range {
        start: Position {
            line: origin.start.row,
            character: table.col_out(origin.start.row, origin.start.column, enc),
        },
        end: Position {
            line: origin.end.row,
            character: table.col_out(origin.end.row, origin.end.column, enc),
        },
    }
}

fn canonical_span_to_range(
    span: &crate::program::resolve::edge::CanonicalSpan,
    table: &dyn ColOut,
    enc: PositionEncoding,
) -> Range {
    Range {
        start: Position {
            line: span.start.line,
            character: table.col_out(span.start.line, span.start.col, enc),
        },
        end: Position {
            line: span.end.line,
            character: table.col_out(span.end.line, span.end.col, enc),
        },
    }
}

fn range_sort_key(r: &Range) -> (u32, u32, u32, u32) {
    (r.start.line, r.start.character, r.end.line, r.end.character)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::snapshot::DeclEntry;
    use crate::lsp::updater::{ChangeEvent, Rung, Updater};

    /// The fixture workspace exercised by every test in this module: a
    /// resolved call (`Alpha.DoWork` → `Beta.Process`), a genuine same-object
    /// overload ambiguity (`Beta.Bar(Integer)`/`Beta.Bar(Text)`, dispatched
    /// with a `Variant`-typed argument so arg-type dispatch cannot break the
    /// tie — confirmed empirically against the real resolver before writing
    /// these tests), a global builtin call (`Message`), a cross-file event
    /// publisher/subscriber pair (`Beta.OnAfterProcess`/`Gamma.
    /// HandleAfterProcess`), a SECOND caller of `Beta.Process`
    /// (`Gamma.Standalone`, for the incoming-grouped-by-caller test), and a
    /// non-ASCII identifier (`Løbenr`) for the position-encoding tests.
    const ALPHA_SRC: &str = r#"codeunit 50100 "Alpha"
{
    procedure DoWork()
    var
        Beta: Codeunit "Beta";
        V: Variant;
    begin
        Beta.Process();
        Beta.Bar(V);
        Message('hi');
    end;

    procedure Løbenr()
    begin
    end;
}
"#;

    const BETA_SRC: &str = r#"codeunit 50101 "Beta"
{
    procedure Process()
    begin
    end;

    procedure Bar(X: Integer)
    begin
    end;

    procedure Bar(X: Text)
    begin
    end;

    [IntegrationEvent(false, false)]
    procedure OnAfterProcess()
    begin
    end;
}
"#;

    const GAMMA_SRC: &str = r#"codeunit 50102 "Gamma"
{
    var
        Beta: Codeunit "Beta";

    procedure Standalone()
    begin
        Beta.Process();
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Beta", 'OnAfterProcess', '', false, false)]
    local procedure HandleAfterProcess()
    begin
    end;
}
"#;

    fn write_fixture_workspace(dir: &std::path::Path) {
        std::fs::write(
            dir.join("app.json"),
            r#"{
    "id": "66666666-0000-0000-0000-000000000011",
    "name": "Task11 Handlers Fixture",
    "publisher": "probe",
    "version": "1.0.0.0"
}"#,
        )
        .expect("write app.json");
        std::fs::write(dir.join("Alpha.al"), ALPHA_SRC).expect("write Alpha.al");
        std::fs::write(dir.join("Beta.al"), BETA_SRC).expect("write Beta.al");
        std::fs::write(dir.join("Gamma.al"), GAMMA_SRC).expect("write Gamma.al");
    }

    fn fixture_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fixture_workspace(dir.path());
        dir
    }

    fn uri_string(dir: &std::path::Path, file: &str) -> String {
        path_to_uri(&dir.join(file)).as_str().to_string()
    }

    fn item_data_of(decl: &DeclEntry) -> ItemData {
        ItemData {
            node: decl.id.clone(),
        }
    }

    // ── prepare: name hit / body-fallback hit / none ───────────────────────

    #[test]
    fn prepare_hits_name_then_falls_back_to_body_then_returns_none() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");

        let lobenr = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "Løbenr")
            .expect("Løbenr decl");

        let items = prepare(
            &snap,
            PositionEncoding::Utf8,
            &uri,
            lobenr.name_origin.start.row,
            lobenr.name_origin.start.column,
        )
        .expect("name-position hit");
        assert_eq!(items.len(), 1);
        let data: ItemData = serde_json::from_value(items[0].data.clone().unwrap()).unwrap();
        assert_eq!(data.node, lobenr.id);

        assert!(
            (lobenr.origin.start.row, lobenr.origin.start.column)
                < (
                    lobenr.name_origin.start.row,
                    lobenr.name_origin.start.column
                ),
            "fixture assumption: origin must start before name_origin"
        );
        let items = prepare(
            &snap,
            PositionEncoding::Utf8,
            &uri,
            lobenr.origin.start.row,
            lobenr.origin.start.column,
        )
        .expect("whole-decl-position hit");
        let data: ItemData = serde_json::from_value(items[0].data.clone().unwrap()).unwrap();
        assert_eq!(data.node, lobenr.id);

        // Whitespace outside any routine (the `{` line, before the first
        // `procedure` keyword) and far past EOF must both return `None`.
        assert!(prepare(&snap, PositionEncoding::Utf8, &uri, 1, 0).is_none());
        assert!(prepare(&snap, PositionEncoding::Utf8, &uri, 9_999, 0).is_none());
    }

    // ── prepare: utf-16 vs utf-8 column difference on a non-ASCII name ─────

    #[test]
    fn prepare_selection_range_differs_between_utf8_and_utf16_for_non_ascii_name() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let uri = uri_string(dir.path(), "Alpha.al");
        let entry = snap.parsed.get("Alpha.al").expect("Alpha.al parsed entry");
        let table = entry.line_table();

        let lobenr = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "Løbenr")
            .expect("Løbenr decl");

        let items_utf8 = prepare(
            &snap,
            PositionEncoding::Utf8,
            &uri,
            lobenr.name_origin.start.row,
            lobenr.name_origin.start.column,
        )
        .expect("utf8 hit");

        let utf16_char = table.col_out(
            lobenr.name_origin.start.row,
            lobenr.name_origin.start.column,
            PositionEncoding::Utf16,
        );
        let items_utf16 = prepare(
            &snap,
            PositionEncoding::Utf16,
            &uri,
            lobenr.name_origin.start.row,
            utf16_char,
        )
        .expect("utf16 hit");

        let d8: ItemData = serde_json::from_value(items_utf8[0].data.clone().unwrap()).unwrap();
        let d16: ItemData = serde_json::from_value(items_utf16[0].data.clone().unwrap()).unwrap();
        assert_eq!(d8.node, lobenr.id, "utf8 query must hit Løbenr");
        assert_eq!(d16.node, lobenr.id, "utf16 query must hit Løbenr");

        // "Løbenr" contains "ø" (2 UTF-8 bytes, 1 UTF-16 unit) — the UTF-8
        // byte-column end must be numerically LARGER than the UTF-16
        // unit-column end for the identical underlying name span.
        assert!(
            items_utf8[0].selection_range.end.character
                > items_utf16[0].selection_range.end.character,
            "utf8 end={} utf16 end={}",
            items_utf8[0].selection_range.end.character,
            items_utf16[0].selection_range.end.character
        );
    }

    // ── incoming: cross-file callee grouped by caller ──────────────────────

    #[test]
    fn incoming_groups_by_caller_with_correct_from_ranges() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let process_decl = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("Beta.Process decl");
        let data = item_data_of(process_decl);

        let calls = incoming(&snap, PositionEncoding::Utf16, &data);
        assert_eq!(calls.len(), 2, "two distinct callers; got {calls:#?}");

        let names: std::collections::BTreeSet<String> =
            calls.iter().map(|c| c.from.name.clone()).collect();
        assert_eq!(
            names,
            ["DoWork", "Standalone"]
                .into_iter()
                .map(String::from)
                .collect()
        );

        for call in &calls {
            assert_eq!(
                call.from_ranges.len(),
                1,
                "each caller has exactly one call site in this fixture; got {call:#?}"
            );
        }

        // The DoWork caller's from_range must land on the actual
        // `Beta.Process();` line in Alpha.al (not some other statement).
        let dowork_call = calls.iter().find(|c| c.from.name == "DoWork").unwrap();
        let expected_line = ALPHA_SRC
            .lines()
            .position(|l| l.contains("Beta.Process()"))
            .expect("fixture must contain the call site") as u32;
        assert_eq!(dowork_call.from_ranges[0].start.line, expected_line);
    }

    // ── incoming: subscriber's incoming lists the publisher ────────────────

    #[test]
    fn incoming_on_subscriber_includes_the_publisher() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let sub_decl = snap.decls_by_file["Gamma.al"]
            .iter()
            .find(|d| d.name == "HandleAfterProcess")
            .expect("Gamma.HandleAfterProcess decl");
        let data = item_data_of(sub_decl);

        let calls = incoming(&snap, PositionEncoding::Utf16, &data);
        assert_eq!(calls.len(), 1, "{calls:#?}");
        assert_eq!(calls[0].from.name, "OnAfterProcess");
        assert!(
            calls[0]
                .from
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("EventPublisher"),
            "{:?}",
            calls[0].from.detail
        );

        let pub_decl = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "OnAfterProcess")
            .expect("Beta.OnAfterProcess decl");
        let table = snap.parsed["Beta.al"].line_table();
        let expected_range = origin_to_range(&pub_decl.name_origin, table, PositionEncoding::Utf16);
        assert_eq!(calls[0].from_ranges, vec![expected_range]);
    }

    // ── outgoing: one resolved + one ambiguous (2 candidates) + one builtin ─

    #[test]
    fn outgoing_resolved_ambiguous_and_builtin() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let dowork = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("Alpha.DoWork decl");
        let data = item_data_of(dowork);

        let calls = outgoing(&snap, PositionEncoding::Utf16, &data);
        assert_eq!(
            calls.len(),
            3,
            "1 resolved (Process) + 2 ambiguous candidates (Bar); builtin absent. Got {calls:#?}"
        );

        let names: Vec<String> = calls.iter().map(|c| c.to.name.clone()).collect();
        assert_eq!(names.iter().filter(|n| n.as_str() == "Process").count(), 1);
        assert_eq!(names.iter().filter(|n| n.as_str() == "Bar").count(), 2);
        assert!(
            !names.iter().any(|n| n.eq_ignore_ascii_case("message")),
            "the builtin call must produce NO outgoing item; got {names:?}"
        );

        // The 2 Bar candidates must be genuinely distinct targets.
        let bar_ids: std::collections::HashSet<_> = calls
            .iter()
            .filter(|c| c.to.name == "Bar")
            .map(|c| {
                let d: ItemData = serde_json::from_value(c.to.data.clone().unwrap()).unwrap();
                d.node
            })
            .collect();
        assert_eq!(bar_ids.len(), 2, "the 2 Bar candidates must be distinct");
    }

    // ── stale ItemData → empty, never a panic ──────────────────────────────

    #[test]
    fn stale_item_data_returns_empty_for_both_handlers() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");
        let dowork = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .unwrap();
        let mut bogus_id = dowork.id.clone();
        bogus_id.name_lc = "does_not_exist_xyz".into();
        let data = ItemData { node: bogus_id };

        assert!(incoming(&snap, PositionEncoding::Utf16, &data).is_empty());
        assert!(outgoing(&snap, PositionEncoding::Utf16, &data).is_empty());
    }

    // ── AUDIT §6.1 (non-negotiable): live target span, never a stale witness ─

    #[test]
    fn outgoing_target_span_is_re_derived_live_after_a_rung1_edit_to_the_target_file() {
        let dir = fixture_dir();
        let (base, parsed) =
            LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
        let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

        // Body-only (comment-prefix) edit to Beta.al — the TARGET file, never
        // itself saved from the caller's perspective — pushing every
        // declaration down 5 lines. Rung-1 eligible: no signature changes.
        let edited_beta = format!("// pad\n// pad\n// pad\n// pad\n// pad\n{BETA_SRC}");
        std::fs::write(dir.path().join("Beta.al"), &edited_beta).expect("rewrite Beta.al");

        let (new_snap, rung) = updater
            .apply_batch(&base, &[ChangeEvent::FileSaved(dir.path().join("Beta.al"))])
            .expect("apply_batch");
        assert_eq!(
            rung,
            Rung::One,
            "a comment-only prefix edit must stay rung 1"
        );

        // Ground truth: an INDEPENDENT fresh batch build of the EDITED
        // workspace on disk.
        let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
        let fresh_process = fresh.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("fresh Process decl");
        let base_process = base.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("base Process decl");
        assert_ne!(
            base_process.origin.start.row, fresh_process.origin.start.row,
            "the edit must actually shift Process()'s line — otherwise this \
             test would pass trivially even with a stale-witness bug"
        );

        let dowork = new_snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("Alpha.DoWork decl (unedited file)");
        let calls = outgoing(
            &new_snap,
            PositionEncoding::Utf16,
            &ItemData {
                node: dowork.id.clone(),
            },
        );
        let process_call = calls
            .iter()
            .find(|c| c.to.name == "Process")
            .expect("Process outgoing call");

        let fresh_table = fresh.parsed["Beta.al"].line_table();
        let expected_range =
            origin_to_range(&fresh_process.origin, fresh_table, PositionEncoding::Utf16);
        let expected_selection = origin_to_range(
            &fresh_process.name_origin,
            fresh_table,
            PositionEncoding::Utf16,
        );

        assert_eq!(
            process_call.to.range, expected_range,
            "the target's range must match the FRESH parse position, never a \
             stale witness span baked in when Alpha's edge was last resolved"
        );
        assert_eq!(process_call.to.selection_range, expected_selection);
    }

    // ── Task-10 finding: EventFlow's site span ALSO must never be trusted ──

    #[test]
    fn incoming_on_subscriber_uses_fresh_publisher_position_after_rung1_edit_above_it() {
        let dir = fixture_dir();
        let (base, parsed) =
            LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
        let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

        // Same padding trick, this time on the file containing the PUBLISHER
        // (Beta.al) — shifts OnAfterProcess's line while staying rung-1
        // eligible.
        let edited_beta = format!("// pad\n// pad\n// pad\n{BETA_SRC}");
        std::fs::write(dir.path().join("Beta.al"), &edited_beta).expect("rewrite Beta.al");

        let (new_snap, rung) = updater
            .apply_batch(&base, &[ChangeEvent::FileSaved(dir.path().join("Beta.al"))])
            .expect("apply_batch");
        assert_eq!(
            rung,
            Rung::One,
            "a comment-only prefix edit must stay rung 1"
        );

        let fresh = LspSnapshot::build_full(dir.path()).expect("fresh build_full");
        let fresh_pub = fresh.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "OnAfterProcess")
            .expect("fresh OnAfterProcess decl");
        let base_pub = base.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "OnAfterProcess")
            .expect("base OnAfterProcess decl");
        assert_ne!(
            base_pub.origin.start.row, fresh_pub.origin.start.row,
            "the edit must actually shift OnAfterProcess's line"
        );

        let sub_decl = new_snap.decls_by_file["Gamma.al"]
            .iter()
            .find(|d| d.name == "HandleAfterProcess")
            .expect("Gamma.HandleAfterProcess decl");
        let calls = incoming(
            &new_snap,
            PositionEncoding::Utf16,
            &ItemData {
                node: sub_decl.id.clone(),
            },
        );
        assert_eq!(calls.len(), 1, "{calls:#?}");

        let fresh_table = fresh.parsed["Beta.al"].line_table();
        let expected_from_range =
            origin_to_range(&fresh_pub.name_origin, fresh_table, PositionEncoding::Utf16);
        let expected_item_range =
            origin_to_range(&fresh_pub.origin, fresh_table, PositionEncoding::Utf16);

        assert_eq!(
            calls[0].from_ranges,
            vec![expected_from_range],
            "an EventFlow edge's stale `site.span` (Arc-cloned forward, \
             unchanged, across rung 1) must NEVER be served — this must be \
             the publisher's FRESH name_origin"
        );
        assert_eq!(calls[0].from.range, expected_item_range);
    }

    // ── dependency-source targets get REAL spans (design doc §5) ───────────

    /// Hand-assembles a two-app (workspace + embedded-source dependency)
    /// `LspSnapshot` in-memory, mirroring `program::build`'s own
    /// `assemble_program_graph_matches_build_program_graph_field_by_field`
    /// layer-split fixture pattern — no disk `.app` zip needed since
    /// `LspSnapshot::from_context` (widened to `pub(crate)` for exactly this
    /// purpose) accepts an already-built `ProgramContext` directly.
    fn two_app_snapshot(ws_src: &str, dep_src: &str) -> LspSnapshot {
        use crate::program::abi_ingest::AbiCache;
        use crate::program::resolve::full::ProgramContext;
        use crate::program::{assemble_program_graph, build_dep_layer};
        use crate::snapshot::compilation::CompilationContext;
        use crate::snapshot::embedded::SourceFile;
        use crate::snapshot::provider::SourceRoot;
        use crate::snapshot::{
            AppId, AppSetSnapshot, AppUnit, Provenance, TrustTier, World, parse_snapshot,
        };
        use std::collections::HashSet;

        let ws_id = AppId {
            guid: String::new(),
            name: "H11Ws".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let dep_id = AppId {
            guid: String::new(),
            name: "H11Dep".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };

        let make_unit = |id: &AppId, tier: TrustTier, name: &str, src: &str| AppUnit {
            id: id.clone(),
            provenance: Provenance {
                app: id.clone(),
                tier,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: vec![SourceFile {
                    virtual_path: name.to_string(),
                    text: src.into(),
                }]
                .into(),
                tier,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        };

        let mut ws_unit = make_unit(&ws_id, TrustTier::Workspace, "Ws.al", ws_src);
        // The workspace app must DECLARE a dependency on the dep app for
        // cross-app object references to resolve at all (mirrors
        // `program::build`'s own layer-split fixture) — otherwise the dep
        // app never enters the workspace app's topology closure and
        // `Codeunit "H11DepCu"` resolves to nothing.
        ws_unit.declared_deps = vec![crate::dependencies::AppDependency {
            app_id: String::new(),
            name: dep_id.name.clone(),
            publisher: dep_id.publisher.clone(),
            version: dep_id.version.clone(),
        }];
        let dep_unit = make_unit(&dep_id, TrustTier::EmbeddedSource, "Dep.al", dep_src);

        let snap = AppSetSnapshot {
            apps: vec![ws_unit, dep_unit],
            workspace_app: ws_id.clone(),
            world: World::Closed,
        };

        let cache = AbiCache::new();
        let parsed = parse_snapshot(&snap);
        let dep_layer = build_dep_layer(&snap, &cache, &parsed);
        let ws_parsed_unit = parsed
            .iter()
            .find(|u| u.app == snap.workspace_app)
            .expect("ws unit must have parsed");
        let graph = assemble_program_graph(&dep_layer, ws_parsed_unit, &snap);
        let primary_app_ref = graph
            .apps
            .find(&snap.workspace_app)
            .expect("ws app interned");
        let ws_file_set: HashSet<String> = ws_parsed_unit
            .files
            .iter()
            .map(|f| f.virtual_path.clone())
            .collect();

        let ctx = ProgramContext {
            snap,
            graph,
            parsed,
            primary_app_ref,
            ws_file_set,
            dep_layer,
            // Test fixture: assembled by hand, stands for the LSP build.
            profile: crate::program::profile::BuildProfile::LIGHT,
        };
        LspSnapshot::from_context(ctx, std::path::Path::new("/workspace")).0
    }

    #[test]
    fn outgoing_to_embedded_source_dependency_gets_a_real_span() {
        let ws_src = r#"codeunit 50100 "H11WsCu"
{
    procedure CallDep()
    var
        D: Codeunit "H11DepCu";
    begin
        D.Bar();
    end;
}
"#;
        let dep_src = r#"codeunit 60100 "H11DepCu"
{
    procedure Bar()
    begin
    end;
}
"#;
        let snap = two_app_snapshot(ws_src, dep_src);

        let caller = snap.decls_by_file["Ws.al"]
            .iter()
            .find(|d| d.name == "CallDep")
            .expect("Ws.CallDep decl");
        let calls = outgoing(
            &snap,
            PositionEncoding::Utf16,
            &ItemData {
                node: caller.id.clone(),
            },
        );
        assert_eq!(calls.len(), 1, "{calls:#?}");
        let to = &calls[0].to;
        assert_eq!(to.name, "Bar");
        assert_ne!(
            to.range, ZERO_RANGE,
            "a dependency with embedded source must get a REAL span, never \
             zero-range — legacy could never do this at all"
        );
        assert!(
            to.uri.as_str().starts_with("al-dep-source:///"),
            "{}",
            to.uri.as_str()
        );

        // Cross-check against the dep source's OWN known layout: `procedure
        // Bar()` is line 2 (0-based) of `dep_src`.
        assert_eq!(to.selection_range.start.line, 2);
    }

    /// S10.1: a dependency decl's columns come from the tier's text-free
    /// `LineIndex`. For every dependency decl's `origin` and `name_origin`
    /// ends, both encodings, they equal what a `LineTable` over the original
    /// text gives — on source where UTF-16 and UTF-8 columns differ.
    #[test]
    fn dependency_decl_columns_equal_the_line_table_over_the_text() {
        let ws_src =
            "codeunit 50100 \"H11WsCu\"\n{\n    procedure CallDep()\n    begin\n    end;\n}\n";
        let dep_src = "codeunit 60100 \"Bøgeløn Æble\"\n{\n    /* ø🚀 */ procedure \"Ændr Løn\"()\n    begin\n    end;\n\n    procedure Plain()\n    begin\n    end;\n}\n";
        let snap = two_app_snapshot(ws_src, dep_src);
        let table = crate::lsp::encoding::LineTable::new(dep_src);
        let mut points = 0;
        let mut wide = 0;
        for id in snap.dep_meta.keys() {
            let (decl, cols) = snap.decl_and_line_table(id).expect("dependency decl");
            for o in [decl.origin, decl.name_origin] {
                for p in [o.start, o.end] {
                    for enc in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
                        assert_eq!(
                            cols.col_out(p.row, p.column, enc),
                            table.col_out(p.row, p.column, enc),
                            "{} at {p:?} {enc:?}",
                            decl.name
                        );
                        points += 1;
                    }
                    let utf8 = table.col_out(p.row, p.column, PositionEncoding::Utf8);
                    let utf16 = table.col_out(p.row, p.column, PositionEncoding::Utf16);
                    wide += usize::from(utf8 != utf16);
                }
            }
        }
        assert!(points >= 16, "precondition: two dependency decls, {points}");
        assert!(
            wide > 0,
            "precondition: some column differs between encodings"
        );
    }

    // ── T3 Task 11 review fix-wave: abi_symbol_uri must conform to ─────────
    // ── legacy's OWN `parse_al_preview_uri`, not merely resemble its scheme ─

    #[test]
    fn abi_symbol_uri_conforms_to_legacys_parse_al_preview_uri() {
        use crate::program::resolve::edge::{AbiEventKind, AbiRoutineKind};
        use crate::types::ObjectType;

        // Numbered object: no raw display name is ever carried in
        // `AbiRoutineKey` for an `ObjKey::Id`-keyed object (see
        // `resolver::make_routine_route`) — the Name segment must fall back
        // to the object number's own text so the URI still round-trips.
        let key_numbered = AbiRoutineKey {
            app: AppRef(0),
            object_type: "codeunit".to_string(),
            object_number: 50100,
            object_name_lc: String::new(),
            routine_name_lc: "bar".to_string(),
            params_count: 0,
            param_type_fp: 0,
            routine_kind: AbiRoutineKind::Procedure,
            event_kind: AbiEventKind::None,
        };
        let uri = abi_symbol_uri(&key_numbered, "Some Dep App");
        let (app, otype, name) = crate::lsp::custom::parse_al_preview_uri(uri.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "emitted URI must parse via legacy's OWN al-preview parser; got {}",
                    uri.as_str()
                )
            });
        assert_eq!(app, "Some Dep App");
        assert_eq!(otype, ObjectType::Codeunit);
        assert_eq!(
            name, "50100",
            "no raw display name exists for a numbered ABI object; the id \
             is used as a placeholder Name segment"
        );

        // Id-less (name-keyed) object: the real display name IS available
        // and must survive the round trip.
        let key_named = AbiRoutineKey {
            object_number: 0,
            object_name_lc: "my ext object".to_string(),
            ..key_numbered.clone()
        };
        let uri2 = abi_symbol_uri(&key_named, "Some Dep App");
        let (_, _, name2) = crate::lsp::custom::parse_al_preview_uri(uri2.as_str())
            .expect("id-less object URI must also parse");
        assert_eq!(name2, "my ext object");
    }

    // ── URI case-insensitivity (Windows filesystems are case-insensitive) ──

    #[cfg(windows)]
    #[test]
    fn prepare_resolves_virtual_path_case_insensitively_on_windows() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let mismatched_uri = path_to_uri(&dir.path().join("aLpHa.AL"));
        let dowork = snap.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("Alpha.DoWork decl");

        let items = prepare(
            &snap,
            PositionEncoding::Utf8,
            mismatched_uri.as_str(),
            dowork.name_origin.start.row,
            dowork.name_origin.start.column,
        )
        .expect("case-insensitive virtual_path match must still hit");
        assert_eq!(items[0].name, "DoWork");
    }

    // ── snapshot-scoped LineTable cache (docs/OUTSTANDING.md item) ─────────

    /// Reuse test: two `incoming` calls against the SAME snapshot generation
    /// for the SAME target must return identical results — the cache must
    /// be purely an internal memoization, invisible to callers.
    #[test]
    fn incoming_repeated_query_against_same_snapshot_returns_identical_results() {
        let dir = fixture_dir();
        let snap = LspSnapshot::build_full(dir.path()).expect("build_full");

        let process_decl = snap.decls_by_file["Beta.al"]
            .iter()
            .find(|d| d.name == "Process")
            .expect("Beta.Process decl");
        let data = item_data_of(process_decl);

        let first = incoming(&snap, PositionEncoding::Utf16, &data);
        let second = incoming(&snap, PositionEncoding::Utf16, &data);

        assert_eq!(first.len(), 2, "{first:#?}");
        assert_eq!(
            first.len(),
            second.len(),
            "repeated query against the same snapshot must return the same \
             number of incoming calls"
        );
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.from.name, b.from.name);
            assert_eq!(a.from.uri, b.from.uri);
            assert_eq!(a.from.range, b.from.range);
            assert_eq!(a.from.selection_range, b.from.selection_range);
            assert_eq!(a.from_ranges, b.from_ranges);
        }
    }

    /// Belt-and-suspenders on top of
    /// `outgoing_target_span_is_re_derived_live_after_a_rung1_edit_to_the_target_file`/
    /// `incoming_on_subscriber_uses_fresh_publisher_position_after_rung1_edit_above_it`
    /// (which prove the NEW generation sees the fresh post-edit position):
    /// this test keeps the OLD, pre-edit `LspSnapshot` alive across a rung-1
    /// apply and re-queries IT — proving the new generation's rebuilt cache
    /// (and rebuilt text) never leaks backward into an old, still-referenced
    /// snapshot. With the `ParsedFileEntry`-embedded `OnceLock` cache design,
    /// this holds by construction (a rung-1 edit builds a brand-new
    /// `Arc<ParsedFileEntry>` for the touched file rather than mutating the
    /// old one in place) — this test locks that property in explicitly.
    #[test]
    fn old_snapshot_generation_is_unaffected_by_a_later_rung1_edit_and_swap() {
        let dir = fixture_dir();
        let (base, parsed) =
            LspSnapshot::build_full_with_parsed(dir.path()).expect("build_full_with_parsed");
        let mut updater = Updater::new(dir.path().to_path_buf(), parsed);

        let dowork = base.decls_by_file["Alpha.al"]
            .iter()
            .find(|d| d.name == "DoWork")
            .expect("Alpha.DoWork decl")
            .id
            .clone();

        // Warm the OLD snapshot's cache BEFORE the edit — the interesting
        // case is a cache that was already populated, not one that happens
        // to be empty.
        let pre_edit_calls = outgoing(
            &base,
            PositionEncoding::Utf16,
            &ItemData {
                node: dowork.clone(),
            },
        );
        let pre_edit_process = pre_edit_calls
            .iter()
            .find(|c| c.to.name == "Process")
            .expect("Process outgoing call")
            .to
            .range;

        // Body-only edit to Beta.al (the TARGET file) — pushes every
        // declaration down, staying rung-1 eligible (mirrors the existing
        // live-span audit tests above).
        let edited_beta = format!("// pad\n// pad\n// pad\n// pad\n// pad\n{BETA_SRC}");
        std::fs::write(dir.path().join("Beta.al"), &edited_beta).expect("rewrite Beta.al");
        let (new_snap, rung) = updater
            .apply_batch(&base, &[ChangeEvent::FileSaved(dir.path().join("Beta.al"))])
            .expect("apply_batch");
        assert_eq!(rung, Rung::One);

        // The NEW snapshot must see the shifted position (sanity — already
        // covered by the audit tests, re-asserted here to anchor the
        // contrast below).
        let post_edit_calls = outgoing(
            &new_snap,
            PositionEncoding::Utf16,
            &ItemData {
                node: dowork.clone(),
            },
        );
        let post_edit_process = post_edit_calls
            .iter()
            .find(|c| c.to.name == "Process")
            .expect("Process outgoing call")
            .to
            .range;
        assert_ne!(
            pre_edit_process, post_edit_process,
            "fixture assumption: the edit must actually shift Process()'s line"
        );

        // The OLD snapshot, queried AGAIN after the swap, must report the
        // EXACT SAME (pre-edit) position it did before — its cache and text
        // must not have been mutated by the later rung-1 apply.
        let re_query_calls = outgoing(&base, PositionEncoding::Utf16, &ItemData { node: dowork });
        let re_query_process = re_query_calls
            .iter()
            .find(|c| c.to.name == "Process")
            .expect("Process outgoing call")
            .to
            .range;
        assert_eq!(
            re_query_process, pre_edit_process,
            "an OLD, still-alive snapshot must report the SAME position on \
             re-query after a LATER generation's rung-1 edit+swap — the new \
             generation's fresh ParsedFileEntry/cache must never leak \
             backward into an old one"
        );
    }
}
