//! Engine-backed custom LSP requests (T3 Task 13): `dependencyDocumentSymbol`,
//! `eventPublishersInFile`, `eventReferenceAtPosition` — the program-engine
//! replacements for legacy `src/handlers.rs`'s (deleted, T3 Task 17)
//! `dependency_document_symbol`, `event_publishers_in_file`, and
//! `event_reference_at_position`.
//!
//! `fieldProperties`/`actionProperties`/`parse_al_preview_uri` (below, in
//! their own sections) were ALREADY graph-independent in legacy — Task 15's
//! cutover pointed the dispatcher at their legacy implementations unchanged,
//! and Task 17 relocated them here verbatim (source-read/al-syntax-facade
//! logic only, never touched `graph`/`indexer`) as the last step of retiring
//! `src/handlers.rs` entirely. `telemetryStatus` has no handler function at
//! all (`server.rs` calls `crate::telemetry::status()` directly), so nothing
//! to relocate for it.
//!
//! # Wire shapes (binding — read before changing anything below)
//!
//! These are reproduced VERBATIM from the legacy implementations; the JSON a
//! client sees must not change (real clients — the AL wrapper extension —
//! depend on it).
//!
//! **`dependencyDocumentSymbol`** (`src/handlers.rs:1370-1421`):
//! ```jsonc
//! // request params (all optional; uri wins over the explicit fields when
//! // it parses — see resolve_target below)
//! { "uri"?: string, "app"?: string, "objectType"?: string, "objectName"?: string, "objectId"?: number }
//! // response: DocumentSymbol[]-shaped, one per method on the matched object
//! [{ "name": string, "detail": string, "kind": number, "tags": number[], "range": {...}, "selectionRange": {...} }]
//! ```
//! `kind` is the LSP `SymbolKind` numeric value: `24` (Event) for a
//! publisher-attributed method, `6` (Method) otherwise. `detail` is
//! `"{attributeTag} {signature}"` when the method carries a publisher/subscriber
//! attribute tag, else just `{signature}`. `range`/`selectionRange` are ALWAYS
//! zero (`{start:{0,0},end:{0,0}}`) — a dependency object's synthesized
//! `al-preview://` preview document carries no real backing text to anchor a
//! position in, exactly as legacy found (`src/handlers.rs:1580-1581`).
//!
//! **`eventPublishersInFile`** (`src/handlers.rs:1705-1752`): params `{ "uri": string }`;
//! response is the SAME `DependencyDocumentSymbol[]` shape as above, but for
//! a WORKSPACE file's own event-publisher procedures — `kind` is always `24`,
//! and `range`/`selectionRange` are REAL positions (this file has real backing
//! text), unlike the always-zero ranges above.
//!
//! **`eventReferenceAtPosition`** (`src/handlers.rs:1777-1877`): params
//! `{ "uri": string, "position": {line, character} }`; response is `null` unless
//! the cursor sits on an `[EventSubscriber(...)]` attribute's argument list, in
//! which case:
//! ```jsonc
//! {
//!   "publisherObjectType": string, "publisherObject": string, "eventName": string,
//!   "signature": string | null, "attributeKind": string | null,
//!   "appName": string | null, "appVersion": string | null
//! }
//! ```
//! The first three fields are ALWAYS populated (extracted from the attribute
//! text itself) once the cursor hit is confirmed; the last four degrade to
//! `null` independently depending on how far publisher resolution gets (dep
//! app found vs. not, method found on it vs. not) — see
//! [`event_reference_at_position`]'s doc for the exact degrade ladder.
//!
//! # Design decision: `AppSetSnapshot.apps[].abi`, not `ProgramGraph`
//!
//! The brief's own hint text points at `graph` (`ProgramGraph`'s
//! `TrustTier::SymbolOnly` nodes) for the dependency-ABI side of this work.
//! This implementation deliberately does NOT go through `graph.objects`/
//! `graph.routines` for `dependencyDocumentSymbol`/`eventReferenceAtPosition`'s
//! publisher resolution — it reads [`LspSnapshot::snap`]`.apps[i].abi:
//! Option<ParsedAppPackage>` instead. Reasons, discovered while implementing:
//!
//! 1. **Full-fidelity signatures already exist there, for free.** `AppUnit::abi`
//!    is populated (`src/snapshot/snapshot.rs:272`, `abi: Some(rd.package)`)
//!    from `crate::dependencies::load_all_apps` — the EXACT SAME `.app`-parsing
//!    pipeline `src/indexer.rs`'s `add_app_to_graph` used to build legacy's
//!    `graph.dependency_objects`. `ParsedAppPackage::objects[].methods[]`
//!    (`crate::app_package::ExternalMethod`) already carries a pre-formatted,
//!    real-parameter-name signature string (`app_package.rs::format_signature`)
//!    — byte-identical to what legacy served, for EVERY dependency, regardless
//!    of trust tier (SymbolOnly or EmbeddedSource: `abi` is set unconditionally
//!    for every resolved dependency, source availability is an orthogonal
//!    `AppUnit::source` concern).
//! 2. **The graph-node path has a REAL, structural fidelity hole.** A
//!    `RoutineNode`'s ABI-tier `abi_params: AbiParams` (`node_extract.rs`)
//!    intentionally drops each parameter's NAME (`AbiParamRetained` — "MINUS
//!    `name`/`is_temporary`" per its own doc; only `arg_dispatch` needs it, and
//!    that never needs names) — so reconstructing a signature from it would
//!    mean synthesizing placeholder parameter names for every dependency
//!    method, a real regression from legacy's real names. For an
//!    EmbeddedSource dependency the gap is worse: `RoutineNode::abi_params` is
//!    unconditionally `AbiParams::Missing` for a non-`SymbolOnly` routine (its
//!    parameter data lives in `DeclSurface`/`RoutineMeta`, which `LspSnapshot`
//!    deliberately does NOT retain for dependency files — only position data
//!    survives into `dep_decl_by_id`'s `DeclEntry`). Reaching real fidelity via
//!    the graph would require either re-parsing the dependency file on demand
//!    (for embedded-source deps only) or widening `LspSnapshot`'s stored
//!    fields — both larger changes than this task's scope, and both made
//!    unnecessary by point 1.
//! 3. **The 14-vs-18 `ObjectType`/`ObjectKind` gap the task brief flags as a
//!    carry-forward simply does not apply to this data source.**
//!    `ExternalObject::object_type` is typed as `crate::types::ObjectType` —
//!    legacy's OWN 14-variant enum — because `app_package.rs::push_objects`
//!    (which builds `ParsedAppPackage`) is the SAME parser legacy always used.
//!    Its own object-collection code only ever constructs the 14 known
//!    variants (`push_objects`'s explicit per-field calls), so an object of a
//!    kind legacy's `ObjectType` cannot name (`ReportExtension`/`Entitlement`/
//!    `Profile`/`Other`) is simply never present in `ParsedAppPackage::objects`
//!    at all — this handler inherits legacy's exact visible set as a natural
//!    consequence of reusing legacy's own object collector, not as a
//!    deliberately-mirrored compromise. Widening that visible set is a
//!    NEW_BETTER opportunity for a future task that extends `app_package.rs`'s
//!    `SymbolReference`/`push_objects` (out of scope here — that module isn't
//!    T3-owned).
//!
//! `event_publishers_in_file` (workspace-file publishers) is unaffected by any
//! of this — it reads `ParsedFileEntry.file` (the real, retained IR for every
//! workspace file) directly, giving full-fidelity real-parameter signatures
//! with zero re-parsing.
//!
//! # Other known deltas from legacy (flagged, not silently absorbed)
//!
//! - **`object_id`-based lookup (NEW_BETTER).** Legacy's
//!   `DependencyDocumentSymbolParams::object_id` field is parsed but never
//!   read (`#[allow(dead_code)]`) — `resolve_dependency_object` is
//!   name-keyed only, so a numbered dependency object could never resolve via
//!   that field. This implementation DOES consult `object_id` when present,
//!   but ONLY as a fallback AFTER a name match is tried first and misses (see
//!   [`find_external_object`]'s doc — an earlier draft let `object_id`
//!   shadow a matching name, which a review caught as NOT actually strictly
//!   additive: a stale/mismatched id could have resolved a different object
//!   than legacy for the same request). With name-first ordering, the claim
//!   holds by construction: it can only find MORE matches than legacy, never
//!   fewer or DIFFERENT ones, so it cannot regress Task 14's differential.
//! - **No disk I/O.** Legacy's `event_reference_at_position` re-reads the
//!   file from disk (`std::fs::read_to_string`) on every call. This
//!   implementation reads `LspSnapshot::parsed`'s already-in-memory text —
//!   consistent with the rest of the (unsaved-edit-aware) engine surface, and
//!   immune to a stale-on-disk vs. live-editor-buffer race legacy was exposed
//!   to.
//! - **Dependency-only publisher resolution scope, mirrored exactly.**
//!   Legacy's `find_dependency_object_by_type_name` only ever scans
//!   `graph.dependency_objects` — a publisher declared in the user's OWN
//!   workspace source can never resolve via `eventReferenceAtPosition`. This
//!   implementation mirrors that scope precisely (only `snap.snap.apps[1..]`,
//!   never the workspace unit at index 0) for exact behavioural parity;
//!   extending it to also resolve a workspace-local publisher is a further
//!   NEW_BETTER opportunity, not implemented here.
//! - **`ObjectType::Database` → `Table` arg-0 normalization, mirrored
//!   locally.** Legacy's text-based attribute parser
//!   (`src/handlers.rs::parse_event_subscriber_args`) treats a literal
//!   `ObjectType::Database` first argument as an alias for `ObjectType::Table`
//!   before doing the lookup (and reflects the NORMALIZED value back in the
//!   response). `crate::program::resolve::event::parse_event_subscriber_ir` —
//!   the engine's shared, whole-program event-attribute parser used by
//!   `node_extract`/`index.rs` for real edge resolution — does NOT perform
//!   this normalization. This module applies the SAME normalization locally
//!   (see [`extract_subscriber_display`]) purely for wire-parity with
//!   legacy's `eventReferenceAtPosition` response; it does NOT touch the
//!   shared resolver. Whether real `[EventSubscriber(ObjectType::Database,
//!   ...)]` source exists anywhere and is mishandled by the whole-program
//!   event-flow resolver itself is a separate, unverified question worth a
//!   follow-up — flagged, not fixed, here (a north-star-metric-affecting
//!   change needs its own measurement).

use anyhow::{Context, Result};
use lsp_types::Position;
use serde::{Deserialize, Serialize};

use crate::app_package::{ExternalMethodKind, ExternalObject};
use crate::lsp::encoding::PositionEncoding;
use crate::lsp::handlers::{origin_to_range, resolve_virtual_path};
use crate::lsp::snapshot::LspSnapshot;
use crate::program::resolve::event::{PublisherKind, is_event_publisher};
use crate::protocol::uri_to_path;
use crate::snapshot::{AppSetSnapshot, AppUnit};
use crate::types::ObjectType;
use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, AttributeIr, Ir};

// ---------------------------------------------------------------------------
// Shared response shapes (dependencyDocumentSymbol + eventPublishersInFile)
// ---------------------------------------------------------------------------

/// One synthesized `DocumentSymbol` entry — the SAME shape legacy's private
/// `DependencyDocumentSymbol` used (`src/handlers.rs:1387-1398`), reused
/// verbatim by both `dependencyDocumentSymbol` and `eventPublishersInFile`.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DependencyDocumentSymbol {
    pub name: String,
    pub detail: String,
    /// LSP `SymbolKind` value: `24` (Event) or `6` (Method).
    pub kind: u32,
    /// LSP `SymbolTag` values — always empty today (mirrors legacy).
    pub tags: Vec<u32>,
    pub range: DependencyRange,
    pub selection_range: DependencyRange,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DependencyRange {
    pub start: DependencyPosition,
    pub end: DependencyPosition,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DependencyPosition {
    pub line: u32,
    pub character: u32,
}

const ZERO_DEP_RANGE: DependencyRange = DependencyRange {
    start: DependencyPosition {
        line: 0,
        character: 0,
    },
    end: DependencyPosition {
        line: 0,
        character: 0,
    },
};

fn lsp_range_to_dep_range(r: lsp_types::Range) -> DependencyRange {
    DependencyRange {
        start: DependencyPosition {
            line: r.start.line,
            character: r.start.character,
        },
        end: DependencyPosition {
            line: r.end.line,
            character: r.end.character,
        },
    }
}

fn external_kind_to_lsp_kind(kind: ExternalMethodKind) -> u32 {
    match kind {
        ExternalMethodKind::IntegrationEvent
        | ExternalMethodKind::BusinessEvent
        | ExternalMethodKind::InternalEvent => 24,
        ExternalMethodKind::EventSubscriber | ExternalMethodKind::Procedure => 6,
    }
}

// ---------------------------------------------------------------------------
// dependencyDocumentSymbol
// ---------------------------------------------------------------------------

/// Request params — mirrors legacy's `DependencyDocumentSymbolParams`
/// (`src/handlers.rs:1370-1385`) field-for-field (camelCase wire names).
///
/// `Clone` (multi-root routing, `server.rs`'s
/// `dispatch_dependency_document_symbol`): this request's `uri` can be a
/// root-agnostic synthetic scheme with no per-root discriminator, so a
/// multi-root session may need to probe more than one configured root's
/// snapshot with the SAME params before finding a non-empty answer.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyDocumentSymbolParams {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub object_type: Option<String>,
    #[serde(default)]
    pub object_name: Option<String>,
    /// Unlike legacy (where this field is parsed but never consulted — see
    /// this module's doc), a numeric id here IS used to resolve the target
    /// object — but only as a FALLBACK when `object_name` is absent or fails
    /// to match (see [`find_external_object`]'s doc for why name always
    /// takes priority).
    #[serde(default)]
    pub object_id: Option<i64>,
}

/// `dependencyDocumentSymbol` — see the module doc for the exact wire shape
/// and the design rationale for reading `snap.snap.apps[].abi` rather than
/// `snap.graph`. Returns an empty `Vec` (never an error) on no match, mirroring
/// legacy exactly.
#[must_use]
pub fn dependency_document_symbol(
    snap: &LspSnapshot,
    params: DependencyDocumentSymbolParams,
) -> Vec<DependencyDocumentSymbol> {
    // The URI wins over the explicit fields whenever it parses — mirrors
    // legacy's `resolve_dependency_object` match exactly (`src/handlers.rs:
    // 1428-1439`): a `Some(uri)` that FAILS to parse still falls through to
    // the explicit fields below, it does not short-circuit to "no match".
    let from_uri = params.uri.as_deref().and_then(parse_al_preview_uri);

    let (app, otype, name): (Option<String>, Option<ObjectType>, String) = match from_uri {
        Some((app, otype, name)) => (Some(app), Some(otype), name),
        None => {
            let otype = params
                .object_type
                .as_deref()
                .and_then(|s| ObjectType::try_from(s).ok());
            (
                params.app.clone(),
                otype,
                params.object_name.clone().unwrap_or_default(),
            )
        }
    };

    let Some(otype) = otype else {
        return Vec::new();
    };

    let Some(obj) =
        resolve_external_object(&snap.snap, app.as_deref(), otype, &name, params.object_id)
    else {
        return Vec::new();
    };

    build_dependency_symbols(obj)
}

/// Resolve a dependency object: app-scoped exact match first (when `app` is
/// given and non-empty), then an any-app fallback scan — mirrors legacy's
/// `resolve_dependency_object` two-tier lookup exactly
/// (`src/handlers.rs:1444-1449`). Always excludes `snap.apps[0]` (the
/// workspace unit — `AppSetSnapshot::apps`'s own doc: "index 0 is always the
/// workspace app"), matching legacy's `dependency_objects` index, which is
/// populated ONLY from `.app` dependencies, never the workspace.
fn resolve_external_object<'a>(
    snap: &'a AppSetSnapshot,
    app: Option<&str>,
    ty: ObjectType,
    name: &str,
    object_id: Option<i64>,
) -> Option<&'a ExternalObject> {
    if name.is_empty() && object_id.is_none() {
        return None;
    }

    if let Some(app_name) = app.filter(|s| !s.is_empty())
        && let Some(unit) = snap
            .apps
            .iter()
            .skip(1)
            .find(|u| u.id.name.eq_ignore_ascii_case(app_name))
        && let Some(obj) = find_external_object(unit, ty, name, object_id)
    {
        return Some(obj);
    }

    snap.apps
        .iter()
        .skip(1)
        .find_map(|unit| find_external_object(unit, ty, name, object_id))
}

/// NAME first, exactly as legacy's `get_dependency_object`/
/// `find_dependency_object_by_type_name` always resolved (name-keyed only —
/// `object_id` didn't exist as a lookup key at all); `object_id` is consulted
/// ONLY as a fallback when the name lookup misses (or no name was given).
/// This ordering (fixed T3 Task 13 review fix-wave; the original draft let a
/// present `object_id` shadow a matching name, so a stale/mismatched id could
/// resolve a DIFFERENT object than legacy would for the exact same request —
/// not actually "strictly additive") makes the additive claim true BY
/// CONSTRUCTION: an `object_id` can only ever widen a result that name
/// resolution alone would have missed, never override one it would have hit.
fn find_external_object<'a>(
    unit: &'a AppUnit,
    ty: ObjectType,
    name: &str,
    object_id: Option<i64>,
) -> Option<&'a ExternalObject> {
    let abi = unit.abi.as_ref()?;

    if !name.is_empty()
        && let Some(obj) = abi
            .objects
            .iter()
            .find(|o| o.object_type == ty && o.name.eq_fold_identifier(name))
    {
        return Some(obj);
    }

    let id = object_id?;
    abi.objects
        .iter()
        .find(|o| o.object_type == ty && o.id == id)
}

fn build_dependency_symbols(obj: &ExternalObject) -> Vec<DependencyDocumentSymbol> {
    obj.methods
        .iter()
        .map(|m| {
            let kind = external_kind_to_lsp_kind(m.kind);
            let tag = m.kind.tag();
            let detail = if tag.is_empty() {
                m.signature.clone()
            } else {
                format!("{tag} {}", m.signature)
            };
            DependencyDocumentSymbol {
                name: m.name.clone(),
                detail,
                kind,
                tags: Vec::new(),
                range: ZERO_DEP_RANGE,
                selection_range: ZERO_DEP_RANGE,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// eventPublishersInFile
// ---------------------------------------------------------------------------

/// Request params — mirrors legacy's `EventPublishersInFileParams`
/// (`src/handlers.rs:1705-1709`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPublishersInFileParams {
    pub uri: String,
}

/// `eventPublishersInFile` — event-publisher procedures ([`IntegrationEvent`]/
/// [`BusinessEvent`]/[`InternalEvent`]) declared in the workspace file at
/// `uri`, read directly from `snap.parsed`'s retained `AlFile` IR (no
/// re-parsing, no disk I/O). Returns an empty `Vec` for a URI outside the
/// workspace, or one this snapshot has no parse for — mirroring legacy's
/// fail-closed-to-empty behaviour.
#[must_use]
pub fn event_publishers_in_file(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    uri: &str,
) -> Vec<DependencyDocumentSymbol> {
    let Some(virtual_path) = resolve_virtual_path(snap, uri) else {
        return Vec::new();
    };
    let Some(entry) = snap.parsed.get(&virtual_path) else {
        return Vec::new();
    };
    let table = entry.line_table();

    let mut out = Vec::new();
    for obj in &entry.file.objects {
        for routine in &obj.routines {
            let Some(kind) = is_event_publisher(routine) else {
                continue;
            };
            // `is_event_publisher` only ever classifies a REAL source
            // attribute (`integrationevent`/`businessevent`/`internalevent`)
            // — `PublisherKind::Platform` is exclusively synthesized later,
            // by `program::build::inject_platform_event_publishers`, and can
            // never be this function's return value.
            let tag = match kind {
                PublisherKind::Integration => "[IntegrationEvent]",
                PublisherKind::Business => "[BusinessEvent]",
                PublisherKind::Internal => "[InternalEvent]",
                PublisherKind::Platform => continue,
            };
            let signature = crate::analysis::signature_ir(&entry.text, routine);
            out.push(DependencyDocumentSymbol {
                name: routine.name.clone(),
                detail: format!("{tag} {signature}"),
                kind: 24,
                tags: Vec::new(),
                range: lsp_range_to_dep_range(origin_to_range(&routine.origin, table, enc)),
                selection_range: lsp_range_to_dep_range(origin_to_range(
                    &routine.name_origin,
                    table,
                    enc,
                )),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// eventReferenceAtPosition
// ---------------------------------------------------------------------------

/// Request params — mirrors legacy's `EventReferenceAtPositionParams`
/// (`src/handlers.rs:1777-1781`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventReferenceAtPositionParams {
    pub uri: String,
    pub position: Position,
}

/// The resolved (or partially-resolved) event reference — mirrors legacy's
/// `EventReferenceMatch` (`src/handlers.rs:1784-1797`) field-for-field.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EventReferenceMatch {
    pub publisher_object_type: String,
    pub publisher_object: String,
    pub event_name: String,
    pub signature: Option<String>,
    pub attribute_kind: Option<String>,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
}

/// `eventReferenceAtPosition` — see the module doc for the wire shape and the
/// degrade ladder. Returns `None` unless `pos` sits within an
/// `[EventSubscriber(...)]` attribute's argument list on a routine in the
/// WORKSPACE file at `uri`.
#[must_use]
pub fn event_reference_at_position(
    snap: &LspSnapshot,
    enc: PositionEncoding,
    uri: &str,
    pos: Position,
) -> Option<EventReferenceMatch> {
    let virtual_path = resolve_virtual_path(snap, uri)?;
    let entry = snap.parsed.get(&virtual_path)?;
    let table = entry.line_table();
    let byte_col = table.col_in(pos.line, pos.character, enc);
    let target = (pos.line, byte_col);

    let (object_type, object_name, event_name) =
        find_event_subscriber_display_at(&entry.file, &entry.text, target)?;

    let otype = ObjectType::try_from(object_type.as_str()).ok();
    let found = otype.and_then(|ty| {
        snap.snap.apps.iter().skip(1).find_map(|unit| {
            let abi = unit.abi.as_ref()?;
            let obj = abi
                .objects
                .iter()
                .find(|o| o.object_type == ty && o.name.eq_fold_identifier(&object_name))?;
            Some((unit, obj))
        })
    });

    let (signature, attribute_kind, app_name, app_version) = match found {
        Some((unit, obj)) => match obj
            .methods
            .iter()
            .find(|m| m.name.eq_fold_identifier(&event_name))
        {
            Some(m) => {
                let tag = m.kind.tag();
                let attribute_kind = if tag.is_empty() {
                    None
                } else {
                    Some(tag.to_string())
                };
                (
                    Some(m.signature.clone()),
                    attribute_kind,
                    Some(unit.id.name.clone()),
                    Some(unit.id.version.clone()),
                )
            }
            None => (
                None,
                None,
                Some(unit.id.name.clone()),
                Some(unit.id.version.clone()),
            ),
        },
        None => (None, None, None, None),
    };

    Some(EventReferenceMatch {
        publisher_object_type: object_type,
        publisher_object: object_name,
        event_name,
        signature,
        attribute_kind,
        app_name,
        app_version,
    })
}

/// Scan every routine's `[EventSubscriber(...)]` attribute (any object, in
/// document order) for one whose argument-list span contains `pos`
/// (`(line, utf8_byte_col)`, inclusive both ends). Legacy's own inclusive
/// window (`cursor_offset >= after_open && cursor_offset <= close_idx`,
/// `src/handlers.rs:1969-1974`) is `[after_open, close_idx]` — `after_open`
/// sits just PAST the attribute's own `(`, `close_idx` is the position of
/// its matching `)` — i.e. the PARENTHESIZED ARGUMENT LIST ONLY; it does NOT
/// cover `[EventSubscriber(` or the trailing `)]` at all (an earlier draft of
/// this doc claimed the opposite — a review fix). This engine version's
/// span, `[first_arg.origin.start, last_arg.origin.end]`, is a strict SUBSET
/// of legacy's window (it additionally excludes any leading whitespace
/// between `(` and the first arg, and any trailing whitespace between the
/// last arg and `)`) — the IR's args are already split correctly by the
/// grammar (handling nested parens/quotes/comments legacy's own text scanner
/// had to hand-roll), so no comma-splitting is needed here; the whitespace-only
/// narrowing is inconsequential in practice — a real hover/click always lands
/// ON an argument's own text, never in the surrounding whitespace.
fn find_event_subscriber_display_at(
    file: &AlFile,
    source: &str,
    pos: (u32, u32),
) -> Option<(String, String, String)> {
    for obj in &file.objects {
        for routine in &obj.routines {
            for attr in &routine.attributes_parsed {
                if !attr.name.eq_ignore_ascii_case("eventsubscriber") || attr.args.len() < 3 {
                    continue;
                }
                let first = &file.ir.expr(attr.args[0]).origin;
                let last = &file
                    .ir
                    .expr(*attr.args.last().expect("len checked >= 3 above"))
                    .origin;
                let start = (first.start.row, first.start.column);
                let end = (last.end.row, last.end.column);
                if pos >= start && pos <= end {
                    return extract_subscriber_display(source, &file.ir, attr);
                }
            }
        }
    }
    None
}

/// Extract the raw, ORIGINAL-CASE (object_type, object_name, event_name)
/// display triple from an `[EventSubscriber(...)]` attribute's first three
/// args, by slicing each arg's own source span — deliberately NOT
/// `crate::program::resolve::event::parse_event_subscriber_ir` (which
/// lowercases every field for case-insensitive dispatch matching; legacy's
/// response preserves the casing exactly as written in source, e.g.
/// `"Approvals Mgmt."` not `"approvals mgmt."` — see the module doc's
/// "Other known deltas" note on why the two parsers deliberately diverge
/// here). Mirrors legacy's `parse_event_subscriber_args`
/// (`src/handlers.rs:2063-2107`) field-for-field, including its
/// `ObjectType::Database` → `Table` arg-0 normalization AND its fail-closed
/// `None` when arg 0 carries no `::` qualifier at all (`p0.split("::")
/// .nth(1)` — a `None` there means legacy's own attribute match fails
/// outright, `src/handlers.rs:2074-2077`; a malformed arg 0 like a bare
/// `Codeunit` identifier with no `ObjectType::` prefix must NEVER be
/// misread as a literal object-type NAME "Codeunit" — a review fix: an
/// earlier draft's `.unwrap_or(a0)` fell open here instead).
fn extract_subscriber_display(
    source: &str,
    ir: &Ir,
    attr: &AttributeIr,
) -> Option<(String, String, String)> {
    let a0 = &source[ir.expr(attr.args[0]).origin.byte.clone()];
    let a1 = &source[ir.expr(attr.args[1]).origin.byte.clone()];
    let a2 = &source[ir.expr(attr.args[2]).origin.byte.clone()];

    // Fail-closed (mirrors legacy exactly): no `::` in arg 0 means this
    // isn't a recognizable `ObjectType::X` qualifier at all — never guess.
    let raw_type = a0.split("::").nth(1)?.trim();
    let object_type = if raw_type.eq_ignore_ascii_case("Database") {
        "Table".to_string()
    } else {
        raw_type.to_string()
    };

    let raw_name = a1.split("::").last().unwrap_or(a1).trim();
    let object_name = raw_name.trim_matches('"').to_string();

    let event_name = a2.trim().trim_matches('\'').to_string();

    if object_name.is_empty() || event_name.is_empty() {
        return None;
    }

    Some((object_type, object_name, event_name))
}

// ---------------------------------------------------------------------------
// fieldProperties / actionProperties (T3 Task 17: relocated verbatim from
// legacy src/handlers.rs, which is deleted this task — graph-independent,
// pure source-read + al-syntax facade lookup, never touched graph/indexer;
// Task 15's cutover already dispatched the server here unchanged, so this is
// a pure move, no behavior change).
// ---------------------------------------------------------------------------

/// Parameters for al-call-hierarchy/fieldProperties and al-call-hierarchy/actionProperties.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolPropertiesParams {
    uri: String,
    /// For fieldProperties
    #[serde(default)]
    field_name: String,
    /// For actionProperties
    #[serde(default)]
    action_name: String,
}

/// Generic response: all declared properties as key-value pairs.
/// Keys are human-readable property names (e.g., "Caption", "CalcFormula").
/// Only properties explicitly declared in source are included.
#[derive(Debug, Serialize, Default)]
pub struct SymbolPropertiesResult {
    /// For fields: the field ID number
    #[serde(skip_serializing_if = "Option::is_none")]
    field_id: Option<u32>,
    /// All declared properties from source (key = property name, value = property value)
    properties: Vec<PropertyEntry>,
}

/// A single property entry preserving declaration order
#[derive(Debug, Serialize)]
pub struct PropertyEntry {
    name: String,
    value: String,
}

/// Extract all properties for a table field, via the owned `al-syntax` facade.
pub fn field_properties(params: SymbolPropertiesParams) -> Result<SymbolPropertiesResult> {
    let source = read_source_from_uri(&params.uri)?;
    Ok(al_syntax::lookup_symbol_properties(
        &source,
        al_syntax::SymbolDeclKind::Field,
        &params.field_name,
    )
    .map(to_symbol_properties_result)
    .unwrap_or_default())
}

/// Extract all properties for a page action, via the owned `al-syntax` facade.
pub fn action_properties(params: SymbolPropertiesParams) -> Result<SymbolPropertiesResult> {
    let source = read_source_from_uri(&params.uri)?;
    Ok(al_syntax::lookup_symbol_properties(
        &source,
        al_syntax::SymbolDeclKind::Action,
        &params.action_name,
    )
    .map(to_symbol_properties_result)
    .unwrap_or_default())
}

/// Read an AL file's source from a `file:` URI (no parsing — al-syntax owns that).
fn read_source_from_uri(uri_str: &str) -> Result<String> {
    let uri: lsp_types::Uri = uri_str.parse().context("Invalid URI")?;
    let path = uri_to_path(&uri).ok_or_else(|| anyhow::anyhow!("Invalid file URI"))?;
    crate::source_text::read_al_source(&path)
        .with_context(|| format!("Failed to read {}", path.display()))
}

/// Map the al-syntax facade result into the LSP response shape.
fn to_symbol_properties_result(p: al_syntax::SymbolProperties) -> SymbolPropertiesResult {
    SymbolPropertiesResult {
        field_id: p.field_id,
        properties: p
            .properties
            .into_iter()
            .map(|e| PropertyEntry {
                name: e.name,
                value: e.value,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// al-preview:// URI parsing (T3 Task 17: relocated verbatim from legacy
// src/handlers.rs, which is deleted this task). Used by THIS module's own
// `dependency_document_symbol` above (the `from_uri` branch) and by
// `src/lsp/handlers.rs`'s `abi_symbol_uri` conformance test, which mints
// URIs in this same scheme for `RouteTarget::AbiSymbol`.
// ---------------------------------------------------------------------------

/// Parse an `al-preview:/allang/{App}/{Type}/{Id}/{Name}.dal` URI into its parts.
/// Returns (app_name, object_type, object_name). Tolerates URL-encoded segments
/// and unusual scheme separators.
///
/// `pub(crate)`: `src/lsp/handlers.rs`'s `abi_symbol_uri` mints URIs in this
/// SAME `al-preview://` object-level layout for the fresh engine's
/// `RouteTarget::AbiSymbol` fallback item — its own conformance test calls
/// this parser directly to prove the emitted URI actually round-trips
/// through the ONE real consumer this scheme has today, rather than merely
/// resembling it by eye.
pub(crate) fn parse_al_preview_uri(uri: &str) -> Option<(String, ObjectType, String)> {
    // Strip scheme and any number of leading slashes.
    let rest = uri.strip_prefix("al-preview:")?;
    let rest = rest.trim_start_matches('/');

    // Expect "allang/<App>/<Type>/<Id>/<Name>.dal" — but the App name and the
    // object Name can themselves contain '/', so a naive split is wrong.
    // Heuristic: locate the ".dal" suffix and walk segments from there.
    let trimmed = rest.strip_suffix(".dal").unwrap_or(rest);
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() < 5 {
        return None;
    }

    // Layout: ["allang", <App pieces...>, <Type>, <Id>, <Name pieces...>]
    // Walk from the right: last segment(s) = Name, segment before Id = Type,
    // before that = Id, everything between "allang" and Type = App.
    //
    // The simplest robust approach: the *type* segment must match a known
    // ObjectType (case-insensitive). Scan segments from index 1 onward and
    // pick the first that parses as ObjectType — that anchors the layout.
    let mut type_idx = None;
    for (i, seg) in segments.iter().enumerate().skip(1) {
        let decoded = urldecode(seg);
        if ObjectType::try_from(decoded.as_str()).is_ok() {
            type_idx = Some(i);
            break;
        }
    }
    let type_idx = type_idx?;
    if type_idx + 2 > segments.len() - 1 {
        // need Id and at least one name segment after Type
        return None;
    }

    let object_type: ObjectType =
        ObjectType::try_from(urldecode(segments[type_idx]).as_str()).ok()?;
    let app_parts: Vec<String> = segments[1..type_idx].iter().map(|s| urldecode(s)).collect();
    let app = app_parts.join("/");

    // Skip Id segment, take rest as Name (may contain slashes if Microsoft ever does that).
    let name_parts: Vec<String> = segments[type_idx + 2..]
        .iter()
        .map(|s| urldecode(s))
        .collect();
    let mut name = name_parts.join("/");
    // The original Name segment may also have included the trailing ".dal";
    // we already stripped it once from the whole URI, but if it landed inside
    // the name segment due to splitting, strip again.
    if let Some(stripped) = name.strip_suffix(".dal") {
        name = stripped.to_string();
    }

    Some((app, object_type, name))
}

/// Minimal URL-decoder for the percent-encoded segments AL LSP may emit.
/// Avoids pulling in another crate.
fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_package::{AppMetadata, ExternalMethod, ParsedAppPackage};
    use crate::program::abi_ingest::AbiCache;
    use crate::program::resolve::full::ProgramContext;
    use crate::program::{assemble_program_graph, build_dep_layer};
    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::embedded::SourceFile;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::{AppId, AppSetSnapshot, Provenance, TrustTier, World, parse_snapshot};
    use std::collections::HashSet;

    const WS_SRC: &str = r#"codeunit 50100 "H13WsCu"
{
    [IntegrationEvent(false, false)]
    procedure OnAfterThing(Value: Integer): Boolean
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"H13DepCu", 'OnAfterDepEvent', '', false, false)]
    local procedure HandleDepEvent()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"H13DepCu", 'NoSuchEvent', '', false, false)]
    local procedure HandleMissingEvent()
    begin
    end;

    [EventSubscriber(ObjectType::Codeunit, Codeunit::"NoSuchDep", 'Whatever', '', false, false)]
    local procedure HandleUnknownDep()
    begin
    end;

    [EventSubscriber(Codeunit, Codeunit::"H13DepCu", 'MalformedArg0', '', false, false)]
    local procedure HandleMalformedArg0()
    begin
    end;

    procedure PlainProcedure()
    begin
    end;
}
"#;

    /// Hand-assembles a two-app `LspSnapshot` in-memory (no disk `.app` zip
    /// needed) — mirrors `src/lsp/handlers.rs`'s own `two_app_snapshot` test
    /// helper (Task 11), with the dependency unit's `abi` field populated
    /// with a hand-built `ParsedAppPackage` (this task's data source — see
    /// the module doc).
    fn two_app_snapshot() -> LspSnapshot {
        let ws_id = AppId {
            guid: String::new(),
            name: "H13Ws".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        };
        let dep_id = AppId {
            guid: String::new(),
            name: "H13Dep".into(),
            publisher: "Test".into(),
            version: "2.1.0.0".into(),
        };

        let mut ws_unit = AppUnit {
            id: ws_id.clone(),
            provenance: Provenance {
                app: ws_id.clone(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: vec![SourceFile {
                    virtual_path: "Ws.al".to_string(),
                    text: WS_SRC.into(),
                }]
                .into(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        };
        ws_unit.declared_deps = vec![crate::dependencies::AppDependency {
            app_id: String::new(),
            name: dep_id.name.clone(),
            publisher: dep_id.publisher.clone(),
            version: dep_id.version.clone(),
        }];

        let dep_package = ParsedAppPackage {
            metadata: AppMetadata {
                app_id: String::new(),
                name: dep_id.name.clone(),
                publisher: dep_id.publisher.clone(),
                version: dep_id.version.clone(),
                runtime: String::new(),
                platform: String::new(),
                application: String::new(),
                dependencies: vec![],
                internals_visible_to: vec![],
            },
            objects: vec![ExternalObject {
                name: "H13DepCu".to_string(),
                object_type: ObjectType::Codeunit,
                id: 60100,
                methods: vec![
                    ExternalMethod {
                        name: "OnAfterDepEvent".to_string(),
                        kind: ExternalMethodKind::IntegrationEvent,
                        signature: "procedure OnAfterDepEvent(Sender: Codeunit \"H13DepCu\")"
                            .to_string(),
                        is_local: false,
                    },
                    ExternalMethod {
                        name: "DoWork".to_string(),
                        kind: ExternalMethodKind::Procedure,
                        signature: "procedure DoWork(var Rec: Record \"Customer\")".to_string(),
                        is_local: false,
                    },
                    ExternalMethod {
                        name: "Helper".to_string(),
                        kind: ExternalMethodKind::Procedure,
                        signature: "local procedure Helper()".to_string(),
                        is_local: true,
                    },
                ],
            }],
        };

        let dep_unit = AppUnit {
            id: dep_id.clone(),
            provenance: Provenance {
                app: dep_id.clone(),
                tier: TrustTier::SymbolOnly,
                content_hash: String::new(),
            },
            source: None,
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: Some(std::sync::Arc::new(dep_package)),
            app_path: None,
            app_stamp: None,
        };

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

    // ── dependencyDocumentSymbol ───────────────────────────────────────────

    #[test]
    fn dependency_document_symbol_by_explicit_fields() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: Some("H13Dep".to_string()),
                object_type: Some("Codeunit".to_string()),
                object_name: Some("H13DepCu".to_string()),
                object_id: None,
            },
        );
        assert_eq!(result.len(), 3, "{result:#?}");

        let event = result
            .iter()
            .find(|s| s.name == "OnAfterDepEvent")
            .expect("OnAfterDepEvent symbol");
        assert_eq!(event.kind, 24, "event publisher must be SymbolKind::Event");
        assert_eq!(
            event.detail,
            "[IntegrationEvent] procedure OnAfterDepEvent(Sender: Codeunit \"H13DepCu\")"
        );
        assert_eq!(event.range.start.line, 0);
        assert_eq!(event.range.end.line, 0);

        let proc = result
            .iter()
            .find(|s| s.name == "DoWork")
            .expect("DoWork symbol");
        assert_eq!(proc.kind, 6, "plain procedure must be SymbolKind::Method");
        assert_eq!(
            proc.detail, "procedure DoWork(var Rec: Record \"Customer\")",
            "a non-publisher method's detail has no attribute tag prefix"
        );

        let helper = result
            .iter()
            .find(|s| s.name == "Helper")
            .expect("Helper symbol");
        assert_eq!(helper.kind, 6);
        assert_eq!(helper.detail, "local procedure Helper()");
    }

    #[test]
    fn dependency_document_symbol_by_al_preview_uri() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: Some("al-preview:/allang/H13Dep/Codeunit/60100/H13DepCu.dal".to_string()),
                app: None,
                object_type: None,
                object_name: None,
                object_id: None,
            },
        );
        assert_eq!(result.len(), 3, "{result:#?}");
    }

    #[test]
    fn dependency_document_symbol_falls_back_to_any_app_when_app_name_is_wrong() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: Some("Not The Real App Name".to_string()),
                object_type: Some("Codeunit".to_string()),
                object_name: Some("H13DepCu".to_string()),
                object_id: None,
            },
        );
        assert_eq!(
            result.len(),
            3,
            "an app-name mismatch must still fall back to the any-app scan, \
             mirroring legacy's resolve_dependency_object"
        );
    }

    #[test]
    fn dependency_document_symbol_resolves_by_numeric_object_id_new_better() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("Codeunit".to_string()),
                object_name: None,
                object_id: Some(60100),
            },
        );
        assert_eq!(
            result.len(),
            3,
            "a numeric object_id must resolve even with no object_name — a \
             legacy-can-never-do-this NEW_BETTER improvement"
        );
    }

    // ── review fix-wave: object_id must never shadow a matching name ──────

    #[test]
    fn dependency_document_symbol_name_wins_over_a_conflicting_object_id() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("Codeunit".to_string()),
                object_name: Some("H13DepCu".to_string()),
                // Deliberately WRONG id for H13DepCu (whose real id is
                // 60100) — the NAME match must still win, exactly as legacy
                // (which never even reads object_id) would resolve.
                object_id: Some(99999),
            },
        );
        assert_eq!(
            result.len(),
            3,
            "a correct name match must win over a conflicting/stale \
             object_id, not be shadowed by it — {result:#?}"
        );
    }

    #[test]
    fn dependency_document_symbol_falls_back_to_object_id_when_name_misses() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("Codeunit".to_string()),
                object_name: Some("Bogus Name".to_string()),
                object_id: Some(60100),
            },
        );
        assert_eq!(
            result.len(),
            3,
            "a name miss must fall back to a valid object_id — the additive \
             win, not the default path — {result:#?}"
        );
    }

    #[test]
    fn dependency_document_symbol_empty_when_both_name_and_object_id_miss() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("Codeunit".to_string()),
                object_name: Some("Does Not Exist".to_string()),
                object_id: Some(99999),
            },
        );
        assert!(result.is_empty());
    }

    #[test]
    fn dependency_document_symbol_empty_on_no_match() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("Codeunit".to_string()),
                object_name: Some("Does Not Exist".to_string()),
                object_id: None,
            },
        );
        assert!(result.is_empty());
    }

    #[test]
    fn dependency_document_symbol_empty_on_unparseable_object_type() {
        let snap = two_app_snapshot();
        let result = dependency_document_symbol(
            &snap,
            DependencyDocumentSymbolParams {
                uri: None,
                app: None,
                object_type: Some("NotARealType".to_string()),
                object_name: Some("H13DepCu".to_string()),
                object_id: None,
            },
        );
        assert!(result.is_empty());
    }

    // ── eventPublishersInFile ───────────────────────────────────────────────

    #[test]
    fn event_publishers_in_file_finds_only_the_publisher_procedure() {
        let snap = two_app_snapshot();
        let uri = crate::protocol::path_to_uri(&snap.workspace_root.join("Ws.al"));
        let result = event_publishers_in_file(&snap, PositionEncoding::Utf16, uri.as_str());

        assert_eq!(result.len(), 1, "{result:#?}");
        let publisher = &result[0];
        assert_eq!(publisher.name, "OnAfterThing");
        assert_eq!(publisher.kind, 24);
        assert_eq!(
            publisher.detail,
            "[IntegrationEvent] procedure OnAfterThing(Value: Integer): Boolean"
        );
        assert_ne!(
            publisher.range, ZERO_DEP_RANGE,
            "a workspace file's publisher must get a REAL, non-zero range"
        );
    }

    #[test]
    fn event_publishers_in_file_empty_for_unknown_uri() {
        let snap = two_app_snapshot();
        let result = event_publishers_in_file(
            &snap,
            PositionEncoding::Utf16,
            "file:///nowhere/NoSuchFile.al",
        );
        assert!(result.is_empty());
    }

    // ── eventReferenceAtPosition ────────────────────────────────────────────

    fn ws_uri(snap: &LspSnapshot) -> lsp_types::Uri {
        crate::protocol::path_to_uri(&snap.workspace_root.join("Ws.al"))
    }

    /// Locate the byte offset of `needle` in `WS_SRC` and convert it to an
    /// LSP `Position` (UTF-16, matching the fixture's ASCII-only content so
    /// byte and UTF-16 columns coincide).
    fn position_at(needle: &str) -> Position {
        let idx = WS_SRC.find(needle).expect("needle must be present");
        let prefix = &WS_SRC[..idx];
        let line = prefix.matches('\n').count() as u32;
        let col = match prefix.rfind('\n') {
            Some(nl) => prefix.len() - nl - 1,
            None => prefix.len(),
        };
        Position {
            line,
            character: col as u32,
        }
    }

    #[test]
    fn event_reference_at_position_resolves_a_known_publisher_and_event() {
        let snap = two_app_snapshot();
        let uri = ws_uri(&snap);
        let pos = position_at("'OnAfterDepEvent'");

        let result = event_reference_at_position(&snap, PositionEncoding::Utf16, uri.as_str(), pos)
            .expect("cursor is inside the EventSubscriber attribute's arg list");

        assert_eq!(result.publisher_object_type, "Codeunit");
        assert_eq!(result.publisher_object, "H13DepCu");
        assert_eq!(result.event_name, "OnAfterDepEvent");
        assert_eq!(
            result.signature.as_deref(),
            Some("procedure OnAfterDepEvent(Sender: Codeunit \"H13DepCu\")")
        );
        assert_eq!(result.attribute_kind.as_deref(), Some("[IntegrationEvent]"));
        assert_eq!(result.app_name.as_deref(), Some("H13Dep"));
        assert_eq!(result.app_version.as_deref(), Some("2.1.0.0"));
    }

    #[test]
    fn event_reference_at_position_degrades_when_event_name_not_found_on_a_resolved_publisher() {
        let snap = two_app_snapshot();
        let uri = ws_uri(&snap);
        let pos = position_at("'NoSuchEvent'");

        let result = event_reference_at_position(&snap, PositionEncoding::Utf16, uri.as_str(), pos)
            .expect("cursor still hits a real EventSubscriber attribute");

        assert_eq!(result.publisher_object, "H13DepCu");
        assert_eq!(result.event_name, "NoSuchEvent");
        assert_eq!(
            result.signature, None,
            "the publisher app resolves but has no matching method"
        );
        assert_eq!(result.attribute_kind, None);
        assert_eq!(
            result.app_name.as_deref(),
            Some("H13Dep"),
            "app identity must still be reported even when the method isn't found"
        );
        assert_eq!(result.app_version.as_deref(), Some("2.1.0.0"));
    }

    #[test]
    fn event_reference_at_position_degrades_fully_when_the_publisher_app_is_unresolvable() {
        let snap = two_app_snapshot();
        let uri = ws_uri(&snap);
        let pos = position_at("'Whatever'");

        let result = event_reference_at_position(&snap, PositionEncoding::Utf16, uri.as_str(), pos)
            .expect("cursor still hits a real EventSubscriber attribute");

        assert_eq!(result.publisher_object, "NoSuchDep");
        assert_eq!(result.signature, None);
        assert_eq!(result.attribute_kind, None);
        assert_eq!(result.app_name, None);
        assert_eq!(result.app_version, None);
    }

    #[test]
    fn event_reference_at_position_none_when_cursor_is_outside_any_attribute() {
        let snap = two_app_snapshot();
        let uri = ws_uri(&snap);
        let pos = position_at("PlainProcedure");

        assert!(
            event_reference_at_position(&snap, PositionEncoding::Utf16, uri.as_str(), pos)
                .is_none()
        );
    }

    // ── review fix-wave: a malformed arg 0 (no `::`) must fail closed ──────

    #[test]
    fn event_reference_at_position_none_on_malformed_arg0_without_double_colon() {
        let snap = two_app_snapshot();
        let uri = ws_uri(&snap);
        let pos = position_at("'MalformedArg0'");

        assert!(
            event_reference_at_position(&snap, PositionEncoding::Utf16, uri.as_str(), pos)
                .is_none(),
            "a bare `Codeunit` arg0 (no ObjectType:: qualifier) must fail \
             closed to None, mirroring legacy's own parse_event_subscriber_args \
             None-on-missing-\"::\" behaviour — never guess a nonsense object type"
        );
    }

    #[test]
    fn database_alias_normalizes_to_table_in_arg0_extraction() {
        // Isolated unit test for the ObjectType::Database -> Table
        // normalization (see the module doc's "Other known deltas" note) —
        // real source using this alias is rare/unverified, so this is
        // exercised directly rather than via a full attribute fixture.
        let src = "[EventSubscriber(ObjectType::Database, Database::\"Some Table\", 'OnAfterInsertEvent', '', false, false)]";
        let file = al_syntax::parse(&format!(
            "codeunit 1 \"X\" {{ {src}\n    procedure P()\n    begin\n    end; }}"
        ));
        let attr = &file.objects[0].routines[0].attributes_parsed[0];
        let (object_type, object_name, event_name) =
            extract_subscriber_display(&file_source(src), &file.ir, attr).expect("must parse");
        assert_eq!(object_type, "Table");
        assert_eq!(object_name, "Some Table");
        assert_eq!(event_name, "OnAfterInsertEvent");
    }

    /// Reconstructs the exact source text `two_app_snapshot`'s parse used for
    /// the isolated `database_alias_normalizes_to_table_in_arg0_extraction`
    /// fixture above (byte-identical to what was fed to `al_syntax::parse`).
    fn file_source(attr_src: &str) -> String {
        format!("codeunit 1 \"X\" {{ {attr_src}\n    procedure P()\n    begin\n    end; }}")
    }
}

// ── fieldProperties / actionProperties (T3 Task 17: relocated from legacy) ──

#[cfg(test)]
mod symbol_properties_tests {
    use super::*;

    #[test]
    fn test_field_properties_extraction() {
        let source = r#"
table 50000 "TEST Customer"
{
    fields
    {
        field(1; "No."; Code[20])
        {
            Caption = 'No.';
            DataClassification = CustomerContent;
        }

        field(11; Balance; Decimal)
        {
            Caption = 'Balance';
            Editable = false;
            FieldClass = FlowField;
            CalcFormula = sum("Cust. Ledger Entry".Amount where("Customer No." = field("No.")));
        }

        field(20; "Payment Terms Code"; Code[10])
        {
            Caption = 'Payment Terms Code';
            DataClassification = CustomerContent;
            TableRelation = "Payment Terms";
        }
    }
}
"#;

        // Helper to find a property by name in the result
        fn prop(result: &SymbolPropertiesResult, name: &str) -> Option<String> {
            result
                .properties
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.value.clone())
        }
        let lookup = |target: &str| {
            to_symbol_properties_result(
                al_syntax::lookup_symbol_properties(
                    source,
                    al_syntax::SymbolDeclKind::Field,
                    target,
                )
                .unwrap(),
            )
        };

        // Test Balance field (FlowField with CalcFormula)
        let result = lookup("balance");
        assert_eq!(result.field_id, Some(11));
        assert_eq!(prop(&result, "Caption").as_deref(), Some("'Balance'"));
        assert_eq!(prop(&result, "Editable").as_deref(), Some("false"));
        assert_eq!(prop(&result, "FieldClass").as_deref(), Some("FlowField"));
        assert!(prop(&result, "CalcFormula").is_some());
        assert!(
            prop(&result, "CalcFormula")
                .unwrap()
                .contains("Cust. Ledger Entry")
        );

        // Test Payment Terms Code field (with TableRelation)
        let result = lookup("payment terms code");
        assert_eq!(result.field_id, Some(20));
        assert!(prop(&result, "TableRelation").is_some());
        assert!(
            prop(&result, "TableRelation")
                .unwrap()
                .contains("Payment Terms")
        );

        // Test No. field (basic field)
        let result = lookup("no.");
        assert_eq!(result.field_id, Some(1));
        assert_eq!(prop(&result, "Caption").as_deref(), Some("'No.'"));
        assert_eq!(
            prop(&result, "DataClassification").as_deref(),
            Some("CustomerContent")
        );
        assert!(prop(&result, "FieldClass").is_none());
        assert!(prop(&result, "CalcFormula").is_none());
    }

    #[test]
    fn test_action_properties_extraction() {
        let source = r#"
page 50001 "TEST Customer Card"
{
    PageType = Card;
    SourceTable = "TEST Customer";

    actions
    {
        area(Navigation)
        {
            action(LedgerEntries)
            {
                ApplicationArea = All;
                Caption = 'Ledger E&ntries';
                Image = CustomerLedger;
                RunObject = page "Customer Ledger Entries";
                RunPageLink = "Customer No." = field("No.");
                RunPageView = sorting("Customer No.");
                ShortcutKey = 'Ctrl+F7';
                ToolTip = 'View the history of transactions for the customer.';
            }

            action(CheckCreditLimit)
            {
                ApplicationArea = All;
                Caption = 'Check Credit Limit';
                Image = Check;
                ToolTip = 'Check if the customer has exceeded their credit limit.';

                trigger OnAction()
                begin
                end;
            }
        }
    }
}
"#;

        // Helper to find a property by name
        fn prop(result: &SymbolPropertiesResult, name: &str) -> Option<String> {
            result
                .properties
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.value.clone())
        }
        let lookup = |target: &str| {
            to_symbol_properties_result(
                al_syntax::lookup_symbol_properties(
                    source,
                    al_syntax::SymbolDeclKind::Action,
                    target,
                )
                .unwrap(),
            )
        };

        // Test LedgerEntries action (with RunObject)
        let result = lookup("ledgerentries");
        assert_eq!(
            prop(&result, "Caption").as_deref(),
            Some("'Ledger E&ntries'")
        );
        assert_eq!(prop(&result, "Image").as_deref(), Some("CustomerLedger"));
        assert!(prop(&result, "RunObject").is_some());
        assert!(
            prop(&result, "RunObject")
                .unwrap()
                .contains("Customer Ledger Entries")
        );
        assert!(prop(&result, "RunPageLink").is_some());
        assert!(prop(&result, "RunPageView").is_some());
        assert_eq!(prop(&result, "ShortcutKey").as_deref(), Some("'Ctrl+F7'"));
        assert!(prop(&result, "ToolTip").is_some());
        assert!(
            prop(&result, "ToolTip")
                .unwrap()
                .contains("history of transactions")
        );

        // Test CheckCreditLimit action (no RunObject, has trigger)
        let result = lookup("checkcreditlimit");
        assert_eq!(
            prop(&result, "Caption").as_deref(),
            Some("'Check Credit Limit'")
        );
        assert_eq!(prop(&result, "Image").as_deref(), Some("Check"));
        assert!(prop(&result, "RunObject").is_none());
        assert!(prop(&result, "ToolTip").is_some());
    }
}

// ── al-preview:// URI parsing (T3 Task 17: relocated from legacy) ──────────

#[cfg(test)]
mod parse_al_preview_uri_tests {
    use super::*;

    #[test]
    fn parse_uri_basic() {
        let uri = "al-preview:/allang/Base Application/Codeunit/1535/Approvals Mgmt..dal";
        let (app, ty, name) = parse_al_preview_uri(uri).expect("parse");
        assert_eq!(app, "Base Application");
        assert_eq!(ty, ObjectType::Codeunit);
        assert_eq!(name, "Approvals Mgmt.");
    }

    #[test]
    fn parse_uri_with_percent_encoding() {
        let uri = "al-preview:/allang/Base%20Application/Codeunit/1535/Approvals%20Mgmt..dal";
        let (app, ty, name) = parse_al_preview_uri(uri).expect("parse");
        assert_eq!(app, "Base Application");
        assert_eq!(ty, ObjectType::Codeunit);
        assert_eq!(name, "Approvals Mgmt.");
    }

    #[test]
    fn parse_uri_multi_slash() {
        let uri = "al-preview:///allang/Base Application/Codeunit/1535/Approvals Mgmt..dal";
        let (_, ty, name) = parse_al_preview_uri(uri).expect("parse");
        assert_eq!(ty, ObjectType::Codeunit);
        assert_eq!(name, "Approvals Mgmt.");
    }
}
