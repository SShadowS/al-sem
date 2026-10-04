//! Cached ABI ingestion: parse SymbolOnly dep .app packages into graph nodes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::app_package::open_app_zip;
use crate::engine::deps::symbol_reference::{
    AbiEventKind as SrAbiEventKind, AbiRoutine, AbiTable, SymbolReferenceAbi,
    parse_symbol_reference,
};
use crate::engine::l3::al_attributes::{AttributeInfo, bool_arg, find_attribute};
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::{
    AbiParamRetained, AbiParams, Access, FieldNode, ObjectNode, RoutineNode,
};
use crate::program::resolve::edge::{AbiEventKind, AbiRoutineKind};
use crate::program::resolve::event::PublisherKind;
use crate::program::sig_fp::{fnv1a, write_len_prefixed};
use crate::snapshot::{AppUnit, TrustTier};
use al_syntax::IdentifierFoldExt;
use al_syntax::ir::ObjectKind;

// ---------------------------------------------------------------------------
// Fold one parameter's canonical discriminator tuple
// ---------------------------------------------------------------------------

/// Fold one parameter's canonical discriminator tuple — `type_text` (the
/// bare-fallback SOURCE-SHAPED text) + `subtype_id` + `subtype_raw_name` +
/// `subtype_tag` (Task 2 round-2 addendum: "outer kind + subtype id + raw
/// subtype name + a degradation tag") — into `buf` as four length-delimited
/// fields. See [`AbiParameter::subtype_id`]'s doc for why this is necessary
/// even though `type_text` alone often already carries full fidelity: on the
/// FAIL-CLOSED DECLINE shapes (Id-only Subtype; a Subtype Name containing a
/// `"`), `type_text` degrades to the bare outer keyword ALONE — two
/// genuinely different declarations can share that degraded text — so the
/// raw discriminator fields are folded in ADDITIONALLY (never as a
/// substitute for `type_text`) to keep them from silently colliding.
fn fold_param_discriminator(
    buf: &mut String,
    p: &crate::engine::deps::symbol_reference::AbiParameter,
) {
    write_len_prefixed(buf, &p.type_text.fold_identifier());
    write_len_prefixed(
        buf,
        &p.subtype_id.map(|id| id.to_string()).unwrap_or_default(),
    );
    write_len_prefixed(
        buf,
        &p.subtype_raw_name
            .as_deref()
            .unwrap_or("")
            .fold_identifier(),
    );
    write_len_prefixed(buf, p.subtype_tag.as_str());
}

/// The ABI overload dedup fingerprint (`RoutineNodeId::sig_fp`) — a
/// length-delimited fold of every parameter's canonical discriminator tuple
/// (Task 2 round-2 addendum), using the project's STABLE fingerprint
/// primitive ([`fnv1a`] — never `DefaultHasher`/a process-random hasher,
/// which would make `sig_fp` non-reproducible across runs and silently break
/// every consumer that persists or compares it within one run).
///
/// Prior to Task 2 this folded ONLY `type_text.to_ascii_lowercase()`
/// `|`-joined — degrading a parameter's type to its OUTER keyword alone
/// (never a `Subtype`), so two genuinely DIFFERENT same-arity ABI overloads
/// differing only by an object-typed parameter's Subtype (`Get(X: Codeunit
/// A)` vs `Get(X: Codeunit B)`) silently fingerprint-collided. Two
/// parameters now fingerprint identically ONLY when their ENTIRE canonical
/// tuple (text + subtype id + raw subtype name + degradation tag) matches —
/// see [`fold_param_discriminator`]'s doc for why all four fields are
/// necessary, not just `type_text`.
pub(crate) fn param_type_fp(params: &[crate::engine::deps::symbol_reference::AbiParameter]) -> u64 {
    if params.is_empty() {
        return 0;
    }
    let mut canon = String::new();
    for p in params {
        fold_param_discriminator(&mut canon, p);
    }
    fnv1a(&canon)
}

/// Retain one ABI routine's parameter metadata as an [`AbiParams`] (Task 2,
/// roadmap-closure plan) — `Complete` when the JSON genuinely carried a
/// `Parameters` array (tri-state arity, see
/// `AbiRoutine::parameters_known`'s doc: `Missing` here, NOT a false `0`,
/// whenever the field was absent/unparseable), `Missing` otherwise. Copies
/// each [`crate::engine::deps::symbol_reference::AbiParameter`]'s dispatch-
/// relevant fields verbatim — no canonicalization/object resolution happens
/// here; that is `arg_dispatch::candidate_param_infos_abi`'s job, run at
/// QUERY time against the fully-built graph/index (ingestion happens before
/// either exists). Never returns `CollapsedUntrusted` — that demotion only
/// happens later, in lockstep with `abi_overload_collapsed`, once every
/// app's routines are pooled and a collapse run is detected
/// (`build::dedup_routines_preserving_genuine_overloads`).
fn retain_abi_params(routine: &AbiRoutine) -> AbiParams {
    if !routine.parameters_known {
        return AbiParams::Missing;
    }
    AbiParams::Complete(
        routine
            .parameters
            .iter()
            .map(|p| AbiParamRetained {
                type_text: p.type_text.clone(),
                is_var: p.is_var,
                subtype_id: p.subtype_id,
                subtype_raw_name: p.subtype_raw_name.clone(),
                subtype_tag: p.subtype_tag,
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Process-level cache for parsed SymbolReferenceAbi
// ---------------------------------------------------------------------------

/// Cache key: `(guid, name, publisher, version)`.
type AbiCacheKey = (String, String, String, String);
/// Inner map type for `AbiCache`.
type AbiCacheMap = Mutex<HashMap<AbiCacheKey, Arc<SymbolReferenceAbi>>>;

/// Process-level cache for parsed `SymbolReferenceAbi`.
///
/// Keyed by `(guid, name, publisher, version)` so each distinct app version is
/// cached independently.  Pre-seeded via [`AbiCache::seed`] for test fixtures
/// (no file I/O needed); production code fills it lazily from `.app` files via
/// [`AbiCache::get_or_load`].
#[derive(Default)]
pub struct AbiCache {
    inner: AbiCacheMap,
    /// Number of file reads performed (excludes cache hits and pre-seeded entries).
    pub parse_count: std::sync::atomic::AtomicUsize,
}

impl AbiCache {
    pub fn new() -> Self {
        AbiCache::default()
    }

    /// Pre-seed the cache with an already-parsed `SymbolReferenceAbi`.
    ///
    /// Used in tests to avoid file I/O.  A subsequent `get_or_load` for the
    /// same key returns the seeded value without incrementing `parse_count`.
    pub fn seed(
        &self,
        guid: &str,
        name: &str,
        publisher: &str,
        version: &str,
        abi: Arc<SymbolReferenceAbi>,
    ) {
        let key = (
            guid.to_string(),
            name.to_string(),
            publisher.to_string(),
            version.to_string(),
        );
        self.inner.lock().unwrap().insert(key, abi);
    }

    fn get(
        &self,
        guid: &str,
        name: &str,
        publisher: &str,
        version: &str,
    ) -> Option<Arc<SymbolReferenceAbi>> {
        let key = (
            guid.to_string(),
            name.to_string(),
            publisher.to_string(),
            version.to_string(),
        );
        self.inner.lock().unwrap().get(&key).cloned()
    }

    fn get_or_load(
        &self,
        guid: &str,
        name: &str,
        publisher: &str,
        version: &str,
        app_path: &Path,
    ) -> Arc<SymbolReferenceAbi> {
        {
            let map = self.inner.lock().unwrap();
            let key = (
                guid.to_string(),
                name.to_string(),
                publisher.to_string(),
                version.to_string(),
            );
            if let Some(arc) = map.get(&key) {
                return arc.clone();
            }
        }
        self.parse_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // H-3 (Tier-1 remediation, Task T1.2): an I/O-level failure (can't
        // open the zip, SymbolReference.json missing, invalid UTF-8) used to
        // swallow silently into `SymbolReferenceAbi::default()` — identical
        // to a genuinely-empty dep, with no way to tell the two apart. Route
        // it through the SAME `error` field a JSON parse failure already
        // uses, so both failure modes surface through the one channel
        // `ingest_abi` propagates below.
        let abi = read_symbol_reference_from_app(app_path).unwrap_or_else(|e| SymbolReferenceAbi {
            error: Some(format!("failed to read {}: {e:#}", app_path.display())),
            ..Default::default()
        });
        let arc = Arc::new(abi);
        let key = (
            guid.to_string(),
            name.to_string(),
            publisher.to_string(),
            version.to_string(),
        );
        self.inner.lock().unwrap().insert(key, arc.clone());
        arc
    }
}

// ---------------------------------------------------------------------------
// I/O helpers
// ---------------------------------------------------------------------------

pub(crate) fn read_symbol_reference_from_app(path: &Path) -> anyhow::Result<SymbolReferenceAbi> {
    let mut archive = open_app_zip(path)?;
    let sr_file = archive.by_name("SymbolReference.json")?;
    // T2.2: belt-and-suspenders cap — reject a hostile declared size before
    // decompressing, then bound the read itself (a lying central directory).
    // Errors here propagate through the SAME `abi.error` channel as a JSON
    // parse failure (see `get_or_load`'s `unwrap_or_else` above) — no new
    // wiring needed.
    crate::capped_io::check_declared_size(
        sr_file.size(),
        crate::capped_io::SYMBOL_REFERENCE_JSON_CAP,
    )?;
    // BaseApp's entry is ~58 MB and `parse_symbol_reference` needs it as one
    // `&str`. Decompress it into an anonymous temp file and map that file
    // read-only, instead of one big heap buffer: a read-only file-backed
    // mapping does not count against Windows commit. The parse returns owned
    // data, so the mapping and the file go away when this function returns.
    let file = crate::capped_io::copy_capped_to_tempfile(
        sr_file,
        crate::capped_io::SYMBOL_REFERENCE_JSON_CAP,
    )?;
    // SAFETY: the file is an anonymous temp file only this function can reach
    // (no name, never shared), so nothing changes it while mapped.
    let map = unsafe { crate::capped_io::map_read_only(&file)? };
    let content: &[u8] = map.as_deref().unwrap_or(&[]);
    let json_str = if content.starts_with(&[0xEF, 0xBB, 0xBF]) {
        std::str::from_utf8(&content[3..])?
    } else {
        std::str::from_utf8(content)?
    };
    Ok(parse_symbol_reference(json_str))
}

// ---------------------------------------------------------------------------
// Mapping helpers
// ---------------------------------------------------------------------------

pub(crate) fn object_kind_from_abi_type(object_type: &str) -> ObjectKind {
    match object_type.to_ascii_lowercase().as_str() {
        "codeunit" => ObjectKind::Codeunit,
        "table" => ObjectKind::Table,
        "page" => ObjectKind::Page,
        "report" => ObjectKind::Report,
        "query" => ObjectKind::Query,
        "xmlport" => ObjectKind::XmlPort,
        "interface" => ObjectKind::Interface,
        "enum" | "enumtype" => ObjectKind::Enum,
        "enumextension" | "enumextensiontype" => ObjectKind::EnumExtension,
        "tableextension" => ObjectKind::TableExtension,
        "pageextension" => ObjectKind::PageExtension,
        "reportextension" => ObjectKind::ReportExtension,
        "controladdin" => ObjectKind::ControlAddIn,
        "entitlement" => ObjectKind::Entitlement,
        "permissionset" => ObjectKind::PermissionSet,
        "permissionsetextension" => ObjectKind::PermissionSetExtension,
        "profile" => ObjectKind::Profile,
        _ => ObjectKind::Codeunit,
    }
}

/// Find the `AbiTable` entry matching `(object_number, name_lc)` and project
/// its fields to [`FieldNode`]s (Task 3) — the ABI counterpart of
/// `node_extract::extract_nodes`'s source-side field projection.
///
/// Mirrors the SAME dual `ObjKey::Id`/`ObjKey::Name` matching discipline the
/// caller just applied to build `obj_id`: a non-zero `object_number` matches
/// by number (never by name, so a real numeric collision can't be masked by
/// a name mismatch); `object_number == 0` falls back to a case-insensitive
/// name match (mirrors `ObjKey::Name(abi_obj.name.to_ascii_lowercase())`).
/// Returns an empty `Vec` when no `AbiTable` entry matches — an `AbiObject`
/// with `object_type` "Table"/"TableExtension" but no companion `AbiTable`
/// should not occur (`parse_symbol_reference` always pushes both from the
/// same raw JSON entry), but a mismatch fails closed to "no fields" rather
/// than panicking.
///
/// `AbiField::data_type` already carries Task 2's Subtype-qualified,
/// SOURCE-SHAPED text (`parse_field`) — no reclassification here, exactly
/// like the source-side projection: the raw declared text is carried
/// verbatim so `ResolveIndex::field_in_table`'s consumer classifies it via
/// the SAME `classify_type_text` every other declared type goes through.
fn abi_table_fields(tables: &[AbiTable], object_number: i64, name: &str) -> Vec<FieldNode> {
    tables
        .iter()
        .find(|t| {
            if object_number != 0 {
                t.object_number == object_number
            } else {
                t.name.eq_fold_identifier(name)
            }
        })
        .map(|t| {
            t.fields
                .iter()
                .map(|f| FieldNode {
                    name_lc: f.name.fold_identifier(),
                    type_text: f.data_type.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn abi_routine_kind_from_str(
    routine: &AbiRoutine,
) -> (AbiRoutineKind, AbiEventKind, Option<PublisherKind>) {
    match routine.kind.as_str() {
        "event-publisher" => {
            let (abi_ek, pk) = match &routine.event_kind {
                SrAbiEventKind::Integration => {
                    (AbiEventKind::Integration, Some(PublisherKind::Integration))
                }
                SrAbiEventKind::Business => (AbiEventKind::Business, Some(PublisherKind::Business)),
                SrAbiEventKind::Unknown => (AbiEventKind::Internal, Some(PublisherKind::Internal)),
            };
            (AbiRoutineKind::EventPublisher, abi_ek, pk)
        }
        "event-subscriber" => (AbiRoutineKind::EventSubscriber, AbiEventKind::None, None),
        _ => (AbiRoutineKind::Procedure, AbiEventKind::None, None),
    }
}

/// ABI-tier counterpart of
/// `crate::program::resolve::event::publisher_include_sender` — reads
/// `IncludeSender` (arg index 0) from an ABI routine's raw
/// `[IntegrationEvent]` / `[BusinessEvent]` / `[InternalEvent]` attribute
/// (all three carry it at position 0; see that function's doc for the
/// verified Microsoft Learn signatures). A real dependency probe (Microsoft
/// Base Application `SymbolReference.json`, 13,581 publisher-attribute
/// occurrences across `Codeunits` + every nested `Namespaces[]` level) found
/// 100% coverage — `Arguments[0].Value` was present and parsed to a literal
/// `"True"`/`"False"` on every single entry, zero unparseable — so, like the
/// source path, this is expected to be `Some` in practice. `None` remains
/// the fail-closed contract for the (unobserved) case the JSON's attribute
/// argument is absent or not a recognizable boolean.
pub(crate) fn abi_publisher_include_sender(attrs: &[AttributeInfo]) -> Option<bool> {
    for name in ["IntegrationEvent", "BusinessEvent", "InternalEvent"] {
        if let Some(attr) = find_attribute(attrs, name) {
            return bool_arg(attr, 0);
        }
    }
    None
}

/// Sentinel `RoutineNodeId.params_count` for an ABI routine whose `Parameters`
/// field was absent/unparseable in `SymbolReference.json` (`AbiRoutine::
/// parameters_known == false`) — as opposed to a genuinely 0-arg procedure,
/// which carries an explicit empty array and a KNOWN count of `0`.
///
/// Arity is TRI-STATE (Task 1, round-2 hardening): known-n / known-zero /
/// unknown. `usize::MAX` can never equal a real call site's argument count
/// (`args.len()` is always small — AL procedures cap well under a hundred
/// params), so a candidate carrying this sentinel structurally NEVER
/// arity-matches any real call. `resolve_in_object`'s `rid.params_count ==
/// arity` filter therefore excludes it by construction — an unknown-arity
/// candidate never emits an edge — with zero special-casing needed in
/// `resolver.rs`. It still counts toward the NAME-ONLY existence scan
/// (`object_has_visible_member_candidate`'s SymbolOnly branch, which is
/// arity-deferred by design), matching the tri-state contract: "exists" is
/// knowable even when "matches this arity" is not.
pub(crate) const UNKNOWN_ARITY: usize = usize::MAX;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Result of [`ingest_abi`] — the extracted graph nodes plus an optional
/// per-app ingest diagnostic (Tier-1 remediation, H-3).
pub struct AbiIngestResult {
    pub objects: Vec<ObjectNode>,
    pub routines: Vec<RoutineNode>,
    /// Set when the underlying `SymbolReference.json` could not be read or
    /// parsed (`SymbolReferenceAbi.error`, propagated verbatim — see that
    /// field's doc). Previously this signal existed but had ZERO production
    /// reads: a broken dependency ingested as a silently empty ABI with no
    /// way to tell it apart from a genuinely-empty one.
    pub error: Option<String>,
}

/// Ingest one SymbolOnly dep unit into `ObjectNode` + `RoutineNode` lists.
///
/// Returns empty vecs when the ABI is not available (no `app_path` and not
/// seeded in `cache`) — this specific case is not itself flagged as an
/// `error`; it is the pre-existing, deliberate "no ABI source at all"
/// contract, distinct from "a source existed but failed to read/parse"
/// (H-3's `error` field covers the latter). Local and internal routines are
/// no longer skipped (H-1) — see the loop-entry doc below.
pub fn ingest_abi(unit: &AppUnit, app: AppRef, cache: &AbiCache) -> AbiIngestResult {
    let id = &unit.id;
    let abi: Arc<SymbolReferenceAbi> =
        if let Some(cached) = cache.get(&id.guid, &id.name, &id.publisher, &id.version) {
            cached
        } else if let Some(path) = &unit.app_path {
            cache.get_or_load(&id.guid, &id.name, &id.publisher, &id.version, path)
        } else {
            return AbiIngestResult {
                objects: vec![],
                routines: vec![],
                error: None,
            };
        };

    let mut objects: Vec<ObjectNode> = Vec::new();
    let mut routines: Vec<RoutineNode> = Vec::new();

    for abi_obj in &abi.objects {
        let kind = object_kind_from_abi_type(&abi_obj.object_type);
        let key = if abi_obj.object_number != 0 {
            ObjKey::Id(abi_obj.object_number)
        } else {
            ObjKey::Name(abi_obj.name.fold_identifier())
        };
        let obj_id = ObjectNodeId { app, kind, key };

        // Table fields (Task 3) — Table/TableExtension only. The physical
        // layout (`fields`/`keys`) lives in a SEPARATE parallel `AbiTable`
        // entry (`abi.tables`), not on `AbiObject` itself (see
        // `parse_symbol_reference`'s `Tables`/`TableExtensions` branches,
        // which push BOTH an `AbiObject` — routines only — AND a matching
        // `AbiTable` for the SAME raw JSON entry). Matched here by the SAME
        // dual `object_number`/name-lowercase key `abi_obj`'s own `ObjKey`
        // used above, since both were built from the identical raw entry and
        // so always carry the same number (or both `0`, matched by name).
        let fields = if matches!(kind, ObjectKind::Table | ObjectKind::TableExtension) {
            abi_table_fields(&abi.tables, abi_obj.object_number, &abi_obj.name)
        } else {
            Vec::new()
        };

        objects.push(ObjectNode {
            id: obj_id.clone(),
            name: abi_obj.name.clone(),
            declared_id: if abi_obj.object_number != 0 {
                Some(abi_obj.object_number)
            } else {
                None
            },
            extends_target: abi_obj.extends_target_name.clone(),
            implements: abi_obj.implemented_interfaces.clone().unwrap_or_default(),
            tier: TrustTier::SymbolOnly,
            // ABI/SymbolOnly ingestion does not (yet) project SourceTable/TableNo/
            // page-control data from the dependency symbol reference — additive gap,
            // not a regression (Task 4 scope is the source `extract_nodes` path).
            source_table: None,
            table_no: None,
            source_table_temporary: false,
            page_controls: vec![],
            fields,
            // ABI/SymbolOnly ingestion does not (yet) project report dataitems either —
            // same additive gap as SourceTable/TableNo/page-controls above (dataitem
            // receivers, Task 1: source `extract_nodes` path only).
            dataitems: vec![],
            // ABI ingestion is a JSON deserialization, never a tree-sitter parse — the
            // `parse_incomplete` concept (error-recovered CST) does not apply here; ABI's
            // own honesty is already captured via `TrustTier::SymbolOnly` above.
            parse_incomplete: false,
        });

        for routine in &abi_obj.routines {
            // Tier-1 remediation (H-1): `local`/`internal` ABI routines are no
            // longer dropped here. AL's `local` on an event PUBLISHER
            // restricts RAISING, not SUBSCRIBING — modern BaseApp integration
            // events are `local procedure` + `[IntegrationEvent]` (a real
            // dependency probe found 13,581 such publisher attributes in
            // BaseApp's SymbolReference.json, every one previously discarded
            // here, silently orphaning subscriber wiring downstream —
            // `resolve::index::ResolveIndex::build`'s event-index loop hit
            // `0 => continue` with no record). Dropping `is_internal` also
            // made the InternalsVisibleTo friend map inert for SymbolOnly
            // deps (nothing ever carried `Access::Internal` to gate). Both
            // are now ingested and carry the matching `Access` variant below,
            // so the resolver's EXISTING visibility model
            // (`resolver::object_access_visible_from` /
            // `internal_visible_across`) enforces call-time visibility —
            // ingestion no longer makes that decision by deletion.
            let name_lc = routine.name.fold_identifier();
            // Tri-state arity (Task 1): a genuinely-parsed `Parameters` array
            // (even empty) carries its real `len()`; an absent/unparseable one
            // maps to `UNKNOWN_ARITY`, a sentinel that can never arity-match a
            // real call site — see the constant's doc for the full contract.
            let params_count = if routine.parameters_known {
                routine.parameters.len()
            } else {
                UNKNOWN_ARITY
            };
            let sig_fp = param_type_fp(&routine.parameters);
            let (routine_kind, event_kind, publisher_kind) = abi_routine_kind_from_str(routine);
            let include_sender = abi_publisher_include_sender(&routine.attributes_parsed);

            let rid = RoutineNodeId {
                object: obj_id.clone(),
                name_lc: name_lc.clone(),
                enclosing_member_lc: None,
                params_count,
                sig_fp,
            };

            routines.push(RoutineNode {
                id: rid,
                name: routine.name.clone(),
                is_trigger: false,
                // `protected` ABI members are carried as `Access::Protected` —
                // an extension of the declaring object may call them (Task 1).
                // H-1: `local`/`internal` now map to their matching `Access`
                // variant instead of being dropped (see the loop-entry doc
                // above). Precedence mirrors the source-side modifiers
                // (mutually exclusive in real AL: a routine is at most one of
                // local/internal/protected).
                access: if routine.is_protected {
                    Access::Protected
                } else if routine.is_local {
                    Access::Local
                } else if routine.is_internal {
                    Access::Internal
                } else {
                    Access::Public
                },
                tier: TrustTier::SymbolOnly,
                event_subscribers: vec![],
                subscriber_instance_manual: false,
                publisher_kind,
                include_sender,
                abi_routine_kind: Some(routine_kind),
                abi_event_kind: Some(event_kind),
                // Hardcoded empty — STILL correct post-Task-2, for a DIFFERENT
                // reason than a source routine's real `param_sig_key`. Task 2
                // made `param_type_fp` (hence `rid.sig_fp` above) carry full
                // Subtype fidelity (bare-outer-name fallback + the raw
                // discriminator fold — see `param_type_fp`/
                // `fold_param_discriminator`'s docs), so two genuinely
                // DIFFERENT ABI overloads almost always land on DIFFERENT
                // `sig_fp`s now and never even reach the same `RoutineNodeId`
                // run in `build::dedup_routines_preserving_genuine_overloads`
                // — `param_sig_key` never needs to distinguish them THERE.
                // Within a run that DOES share one `sig_fp` (i.e. every raw
                // entry's ENTIRE canonical discriminator tuple matched), ABI
                // ingestion still has no INDEPENDENT per-overload content
                // signature beyond that tuple to further distinguish them —
                // unlike source params, whose real parsed type text backs
                // `param_sig_key` below — so leaving this empty is safe:
                // every entry in such a run is content-INDISTINGUISHABLE by
                // our discriminator, whether that's because it's a literal
                // re-parse duplicate OR a genuinely-different-but-
                // fingerprint-identical pair (a residual collision the
                // round-2 addendum calls out — "any residual same-key
                // multi-entry group is collapse-marked so collisions
                // OVER-DECLINE, never select"). The safety net is downstream:
                // `dedup_routines_preserving_genuine_overloads` marks that
                // survivor `abi_overload_collapsed` whenever ≥2 raw
                // `SymbolOnly` entries shared a node id, so a later
                // type-query OR plain dispatch (`resolver::resolve_in_object`,
                // Task 2's plain-dispatch marker guard) declines rather than
                // trusts a possibly-wrong candidate.
                param_sig_key: String::new(),
                // Task 2: the reconstructed SOURCE-SHAPED return-type text
                // (see `symbol_reference::reconstruct_return_type_text`'s
                // fail-closed rules) now flows through instead of being
                // hard-discarded — resolution-neutral until Task 3 adds a
                // consumer (nothing reads `RoutineNode.return_type` for an
                // ABI-tier routine yet).
                return_type: routine.return_type_text.clone(),
                // The structured `(name, id)` cross-validation pair, carried
                // alongside the text so Task 3 can reach it via the SAME
                // `RoutineNodeId` lookup regardless of route shape (`AbiSymbol`
                // or `Routine(rid)`) — see `AbiRoutine::return_type_id`'s doc.
                return_type_id: routine.return_type_id.clone(),
                // Never marked here: ingestion emits one `RoutineNode` per RAW
                // ABI routine, with no folding yet — the actual collapse (and
                // thus the only place that can know ≥2 raw entries shared a
                // node id) happens later, once every app's routines are
                // pooled and sorted, in `build::
                // dedup_routines_preserving_genuine_overloads` (Task 3 review
                // fix). See `RoutineNode::abi_overload_collapsed`'s doc.
                abi_overload_collapsed: false,
                // Always `false` for an ABI/`SymbolOnly` routine — see
                // `RoutineNode::source_overload_aliased`'s doc (mutually
                // exclusive with `abi_overload_collapsed` by construction).
                source_overload_aliased: false,
                // Task 2 (roadmap-closure plan): retain the raw parameter
                // metadata now instead of hard-discarding it — see
                // `retain_abi_params`'s doc. `abi_overload_collapsed`'s later
                // demotion to `AbiParams::CollapsedUntrusted` happens in
                // `build::dedup_routines_preserving_genuine_overloads`, not
                // here (ingestion emits one `RoutineNode` per RAW entry, no
                // folding yet — same rationale as `abi_overload_collapsed`
                // above).
                abi_params: retain_abi_params(routine),
            });
        }
    }

    AbiIngestResult {
        objects,
        routines,
        error: abi.error.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::deps::symbol_reference::{
        AbiEventKind as SrAbiEventKind, AbiField, AbiObject, AbiParameter, AbiRoutine, SubtypeTag,
        SymbolReferenceAbi,
    };
    use crate::program::build::build_program_graph;
    use crate::snapshot::compilation::CompilationContext;
    use crate::snapshot::provider::SourceRoot;
    use crate::snapshot::snapshot::AppUnit;
    use crate::snapshot::snapshot::{AppSetSnapshot, World};
    use crate::snapshot::{AppId, Provenance};
    use al_syntax::ir::ObjectKind;
    use std::sync::Arc;

    /// The temp-file + memory-map read of a Ready-to-Run package's
    /// `SymbolReference.json` (BOM-prefixed, NUL-padded) yields the same ABI
    /// as parsing the JSON text directly.
    #[test]
    fn read_symbol_reference_from_ready_to_run_app_matches_direct_parse() {
        use crate::engine::deps::app_package_zip::test_apps;
        let json = r#"{"AppId":"437dbf0e-84ff-417a-965d-ed2bb9650972","Name":"Base Application","Publisher":"Microsoft","Version":"28.4.53241.53758","Codeunits":[{"Id":80,"Name":"Sales-Post","Methods":[{"Name":"Run","Parameters":[{"Name":"Rec","IsVar":true,"TypeDefinition":{"Name":"Record","Subtype":{"Name":"Sales Header"}}}]}]}]}"#;
        let expected = parse_symbol_reference(json);
        assert!(expected.error.is_none(), "{:?}", expected.error);
        assert!(!expected.objects.is_empty());

        let entry = [b"\xEF\xBB\xBF".as_slice(), json.as_bytes(), &[0u8; 64]].concat();
        let app = test_apps::build_app(&[("SymbolReference.json", &entry)]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("Microsoft_Base Application_28.4.53241.53758.app");
        std::fs::write(&path, test_apps::wrap_ready_to_run(&app)).unwrap();

        let abi = read_symbol_reference_from_app(&path).expect("read");
        assert_eq!(abi, expected);
    }

    fn dep_id(name: &str) -> AppId {
        AppId {
            guid: format!("dep-{name}"),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn ws_id() -> AppId {
        AppId {
            guid: "ws-guid".into(),
            name: "Workspace".into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    fn make_dep_pub_abi() -> SymbolReferenceAbi {
        SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 50100,
                name: "Dep Pub".into(),
                routines: vec![
                    AbiRoutine {
                        name: "OnDepEvent".into(),
                        kind: "event-publisher".into(),
                        event_kind: SrAbiEventKind::Integration,
                        parameters: vec![
                            AbiParameter {
                                name: "p1".into(),
                                type_text: "Integer".into(),
                                is_var: false,
                                is_temporary: false,
                                subtype_id: None,
                                subtype_raw_name: None,
                                subtype_tag: SubtypeTag::NoSubtype,
                            },
                            AbiParameter {
                                name: "p2".into(),
                                type_text: "Text".into(),
                                is_var: false,
                                is_temporary: false,
                                subtype_id: None,
                                subtype_raw_name: None,
                                subtype_tag: SubtypeTag::NoSubtype,
                            },
                        ],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "DoDepWork".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![AbiParameter {
                            name: "x".into(),
                            type_text: "Integer".into(),
                            is_var: false,
                            is_temporary: false,
                            subtype_id: None,
                            subtype_raw_name: None,
                            subtype_tag: SubtypeTag::NoSubtype,
                        }],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "F".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![AbiParameter {
                            name: "a".into(),
                            type_text: "Integer".into(),
                            is_var: false,
                            is_temporary: false,
                            subtype_id: None,
                            subtype_raw_name: None,
                            subtype_tag: SubtypeTag::NoSubtype,
                        }],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "F".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![AbiParameter {
                            name: "a".into(),
                            type_text: "Text".into(),
                            is_var: false,
                            is_temporary: false,
                            subtype_id: None,
                            subtype_raw_name: None,
                            subtype_tag: SubtypeTag::NoSubtype,
                        }],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn make_ws_unit(ws: &AppId) -> AppUnit {
        AppUnit {
            id: ws.clone(),
            provenance: Provenance {
                app: ws.clone(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            },
            source: Some(SourceRoot {
                files: Default::default(),
                tier: TrustTier::Workspace,
                content_hash: String::new(),
            }),
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        }
    }

    fn make_symbolonly_dep_unit(dep: &AppId) -> AppUnit {
        AppUnit {
            id: dep.clone(),
            provenance: Provenance {
                app: dep.clone(),
                tier: TrustTier::SymbolOnly,
                content_hash: String::new(),
            },
            source: None,
            compilation: CompilationContext::default(),
            declared_deps: vec![],
            internals_visible_to: vec![],
            abi: None,
            app_path: None,
            app_stamp: None,
        }
    }

    #[test]
    fn symbolonly_dep_nodes_appear_in_graph() {
        let ws = ws_id();
        let dep = dep_id("DepPub");
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(make_dep_pub_abi()),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let dep_obj = g
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case("Dep Pub"))
            .expect("Dep Pub ObjectNode must exist");
        assert_eq!(dep_obj.tier, TrustTier::SymbolOnly);
        assert_eq!(dep_obj.id.kind, ObjectKind::Codeunit);

        let on_dep_event = g
            .routines
            .iter()
            .find(|r| r.id.name_lc == "ondepevent")
            .expect("OnDepEvent must exist");
        assert_eq!(on_dep_event.id.params_count, 2);
        assert_eq!(
            on_dep_event.publisher_kind,
            Some(PublisherKind::Integration)
        );

        let do_dep_work = g
            .routines
            .iter()
            .find(|r| r.id.name_lc == "dodepwork")
            .expect("DoDepWork must exist");
        assert_eq!(do_dep_work.id.params_count, 1);

        let f_overloads: Vec<_> = g.routines.iter().filter(|r| r.id.name_lc == "f").collect();
        assert_eq!(
            f_overloads.len(),
            2,
            "Both F overloads must be distinct nodes"
        );
        assert_ne!(
            f_overloads[0].id.sig_fp, f_overloads[1].id.sig_fp,
            "F(Integer) and F(Text) must have different sig_fp"
        );
        assert!(f_overloads.iter().all(|r| r.id.params_count == 1));
    }

    #[test]
    fn workspace_only_snapshot_graph_unchanged() {
        let ws = ws_id();
        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws)],
            workspace_app: ws,
            world: World::Closed,
        };
        let cache = AbiCache::new();
        let g = build_program_graph(&snap, &cache);
        assert!(g.objects.iter().all(|o| o.tier != TrustTier::SymbolOnly));
        assert!(g.routines.iter().all(|r| r.tier != TrustTier::SymbolOnly));
    }

    #[test]
    fn abi_parse_cached_across_build_cycles() {
        let ws = ws_id();
        let dep = dep_id("DepPub");
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(make_dep_pub_abi()),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };

        let _g1 = build_program_graph(&snap, &cache);
        let _g2 = build_program_graph(&snap, &cache);

        assert_eq!(
            cache.parse_count.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "Pre-seeded cache must not trigger any file reads"
        );
    }

    /// Tier-1 remediation (H-1): `local`/`internal` ABI routines are no longer
    /// dropped at ingest — AL's `local` on an event PUBLISHER restricts
    /// RAISING, not SUBSCRIBING (modern BaseApp integration events are
    /// `local procedure` + `[IntegrationEvent]`), and dropping `internal`
    /// routines made the InternalsVisibleTo friend map inert for SymbolOnly
    /// deps. Both must now survive ingestion, carrying the matching `Access`
    /// variant so the resolver's EXISTING visibility model
    /// (`object_access_visible_from`/`internal_visible_across`) enforces
    /// call-time visibility instead of ingestion silently deleting the node.
    #[test]
    fn local_and_internal_routines_kept_with_correct_access() {
        let dep = dep_id("SkipTest");
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 99,
                name: "Skip Test".into(),
                routines: vec![
                    AbiRoutine {
                        name: "Public".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "LocalProc".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: true,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "InternalProc".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: true,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    // Task 1: `protected` is NOT dropped like local/internal —
                    // it is KEPT and carried as `Access::Protected` (an
                    // extension of the declaring object may call it).
                    AbiRoutine {
                        name: "ProtectedProc".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: true,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(abi),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let routines_in_skip: Vec<_> = g
            .routines
            .iter()
            .filter(|r| r.id.object.id_equals_number(99))
            .collect();

        assert!(
            routines_in_skip.iter().any(|r| r.id.name_lc == "public"),
            "Public must be included"
        );
        let local_proc = routines_in_skip
            .iter()
            .find(|r| r.id.name_lc == "localproc")
            .expect(
                "is_local must be KEPT (H-1: local restricts raising, not \
                 subscribing — dropping it silently orphaned publisher wiring)",
            );
        assert_eq!(
            local_proc.access,
            Access::Local,
            "IsLocal:true must carry Access::Local, never be dropped"
        );
        let internal_proc = routines_in_skip
            .iter()
            .find(|r| r.id.name_lc == "internalproc")
            .expect(
                "is_internal must be KEPT (H-1: dropping it made the \
                 InternalsVisibleTo friend map inert for SymbolOnly deps)",
            );
        assert_eq!(
            internal_proc.access,
            Access::Internal,
            "IsInternal:true must carry Access::Internal, never be dropped"
        );
        let protected_proc = routines_in_skip
            .iter()
            .find(|r| r.id.name_lc == "protectedproc")
            .expect("ProtectedProc must be KEPT (carried as Access::Protected, not dropped)");
        assert_eq!(
            protected_proc.access,
            Access::Protected,
            "IsProtected:true must carry Access::Protected, never Access::Public"
        );
        let public_proc = routines_in_skip
            .iter()
            .find(|r| r.id.name_lc == "public")
            .expect("Public must exist");
        assert_eq!(
            public_proc.access,
            Access::Public,
            "a routine with neither IsLocal/IsInternal/IsProtected must default to Access::Public"
        );
    }

    /// H-1 fixture (Tier-1 remediation, Task T1.2): a SymbolOnly dep whose
    /// SymbolReference declares a `local procedure` `[IntegrationEvent]`
    /// publisher — the real modern BaseApp shape (13,581 publisher
    /// attributes in BaseApp's SymbolReference.json, ALL `local procedure`;
    /// AL's `local` on a PUBLISHER restricts RAISING, not SUBSCRIBING) — plus
    /// a workspace subscriber targeting it. Pre-fix, `ingest_abi` dropped
    /// every `is_local` ABI routine outright, so this publisher never became
    /// a graph node and the subscription silently vanished (`0 => continue`
    /// in `ResolveIndex::build`'s event-index loop, with no diagnostic).
    /// Post-fix: the publisher survives with `Access::Local`, and the
    /// subscription resolves — proving subscribing is not gated by the
    /// publisher's call-visibility (subscribing is not calling).
    #[test]
    fn local_procedure_integration_event_publisher_subscription_resolves() {
        use crate::program::resolve::event::PublisherKind;
        use crate::program::resolve::index::ResolveIndex;
        use crate::snapshot::embedded::SourceFile;
        use crate::snapshot::provider::SourceRoot;

        let dep = dep_id("LocalPubDep");
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 60100,
                name: "Local Pub Dep".into(),
                routines: vec![AbiRoutine {
                    name: "OnDoStuff".into(),
                    kind: "event-publisher".into(),
                    event_kind: SrAbiEventKind::Integration,
                    parameters: vec![],
                    return_type_text: None,
                    return_type_id: None,
                    is_local: true,
                    is_internal: false,
                    is_protected: false,
                    parameters_known: true,
                    attributes: vec![],
                    attributes_parsed: vec![],
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(abi),
        );

        let sub_src = r#"
codeunit 50900 "Subscriber CU"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"Local Pub Dep", 'OnDoStuff', '', false, false)]
    local procedure HandleIt()
    begin
    end;
}
"#;
        let mut ws_unit = make_ws_unit(&ws);
        ws_unit.source = Some(SourceRoot {
            files: vec![SourceFile {
                virtual_path: "Subscriber.al".into(),
                text: sub_src.into(),
            }]
            .into(),
            tier: TrustTier::Workspace,
            content_hash: String::new(),
        });
        // A dependency edge is required — event-subscriber wiring resolves
        // the publisher object at whole-snapshot scope, but `resolve_object`
        // (used to locate that object from the subscriber's app) is still
        // closure-scoped.
        ws_unit.declared_deps = vec![crate::dependencies::AppDependency {
            app_id: dep.guid.clone(),
            name: dep.name.clone(),
            publisher: dep.publisher.clone(),
            version: dep.version.clone(),
        }];

        let snap = AppSetSnapshot {
            apps: vec![ws_unit, make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let publisher = g
            .routines
            .iter()
            .find(|r| r.id.name_lc == "ondostuff")
            .expect(
                "a local-procedure IntegrationEvent publisher must survive \
                 ingestion, not be dropped",
            );
        assert_eq!(
            publisher.access,
            Access::Local,
            "an ABI is_local routine must carry Access::Local"
        );
        assert_eq!(publisher.publisher_kind, Some(PublisherKind::Integration));

        let index = ResolveIndex::build(&g);
        let subs = index.subscribers_of(&publisher.id);
        assert_eq!(
            subs.len(),
            1,
            "the workspace subscriber must bind to the local-procedure \
             publisher — subscribing is not calling, so access must never \
             gate subscription eligibility"
        );
        assert!(
            index.orphaned_subscriptions().is_empty(),
            "no orphan should be recorded once the publisher exists"
        );
    }

    /// H-1 corollary fixture: an `internal` ABI routine on a SymbolOnly dep,
    /// whose manifest grants `InternalsVisibleTo` friendship to the calling
    /// workspace app. Pre-fix, `ingest_abi` dropped `is_internal` routines
    /// outright, making `ProgramGraph.friends` (wired from
    /// `AppUnit.internals_visible_to` in `build_program_graph`'s Step 3b)
    /// INERT for SymbolOnly deps — there was never a node to check
    /// friendship against, regardless of what the manifest declared.
    /// Post-fix: the routine survives as `Access::Internal` and the friend
    /// map is live. (The resolver's `internal_visible_across` friend-check
    /// MECHANICS are tier-agnostic and already proven in
    /// `resolver::tests`'s Task 1.5 suite — this fixture proves the missing
    /// half: that ABI ingestion now feeds it a node to check at all.)
    #[test]
    fn internal_routine_with_internals_visible_to_friend_map_is_live() {
        use crate::app_package::FriendApp;

        let dep = dep_id("FriendDep");
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 60200,
                name: "Friend Dep CU".into(),
                routines: vec![AbiRoutine {
                    name: "Secret".into(),
                    kind: "procedure".into(),
                    event_kind: SrAbiEventKind::Unknown,
                    parameters: vec![],
                    return_type_text: None,
                    return_type_id: None,
                    is_local: false,
                    is_internal: true,
                    is_protected: false,
                    parameters_known: true,
                    attributes: vec![],
                    attributes_parsed: vec![],
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(abi),
        );

        let mut dep_unit = make_symbolonly_dep_unit(&dep);
        // The dep's own manifest grants the workspace app friendship.
        dep_unit.internals_visible_to = vec![FriendApp {
            app_id: ws.guid.clone(),
            name: ws.name.clone(),
            publisher: ws.publisher.clone(),
        }];

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), dep_unit],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let secret = g
            .routines
            .iter()
            .find(|r| r.id.name_lc == "secret")
            .expect("an internal ABI routine must survive ingestion, not be dropped");
        assert_eq!(
            secret.access,
            Access::Internal,
            "an ABI is_internal routine must carry Access::Internal"
        );

        let ws_ref = g
            .apps
            .find_by_name("Workspace")
            .expect("workspace app must be interned");
        let dep_ref = g
            .apps
            .find_by_name("FriendDep")
            .expect("dep app must be interned");
        assert!(
            g.friends.get(&dep_ref).is_some_and(|f| f.contains(&ws_ref)),
            "the dep's InternalsVisibleTo must wire the workspace app as a \
             friend — this map was previously inert for SymbolOnly deps \
             because there was never an Access::Internal node to check it \
             against; got friends: {:?}",
            g.friends
        );
    }

    /// H-3 fixture: an unreadable `.app` path (I/O failure — the file
    /// doesn't exist). Pre-fix, `AbiCache::get_or_load`'s
    /// `unwrap_or_else(|_| SymbolReferenceAbi::default())` swallowed this
    /// into an ABI indistinguishable from a genuinely-empty dependency, with
    /// nothing anywhere recording that ingestion actually failed. Post-fix,
    /// the I/O error is routed through the same `error` channel a JSON
    /// parse failure uses and surfaces as a `ProgramGraph.abi_ingest_errors`
    /// entry — a genuinely-corrupt dep is now observable, never silently
    /// empty.
    #[test]
    fn abi_read_failure_surfaces_as_graph_diagnostic() {
        let dep = dep_id("BrokenDep");
        let ws = ws_id();
        let cache = AbiCache::new(); // not seeded — forces a real disk read.

        let mut dep_unit = make_symbolonly_dep_unit(&dep);
        dep_unit.app_path = Some(std::path::PathBuf::from(
            "Z:/definitely/does/not/exist/broken.app",
        ));

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), dep_unit],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        assert_eq!(
            g.abi_ingest_errors.len(),
            1,
            "an unreadable dep .app must surface exactly one ingest \
             diagnostic, not silently ingest as empty; got {:?}",
            g.abi_ingest_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert!(
            g.abi_ingest_errors[0].message.contains("failed to read"),
            "got: {}",
            g.abi_ingest_errors[0].message
        );
        assert!(
            g.objects
                .iter()
                .all(|o| !o.name.eq_ignore_ascii_case("Broken Dep CU")),
            "an unreadable dep must still fail closed to no objects"
        );
    }

    /// Task T2.2: a dep `.app` whose `SymbolReference.json` declares more
    /// than [`crate::capped_io::SYMBOL_REFERENCE_JSON_CAP`] must surface
    /// through the SAME `abi_ingest_errors` channel as any other unreadable
    /// dep (H-3 above) — never a panic, never a silent empty ingest. Proves
    /// the cap's error flows end-to-end through `get_or_load`'s
    /// `unwrap_or_else`, not just that the isolated `capped_io` helper works.
    #[test]
    fn oversized_symbol_reference_surfaces_as_graph_diagnostic_not_panic() {
        use std::io::Write as _;

        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("NavxManifest.xml", opts).unwrap();
            writer
                .write_all(
                    br#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/navx/2015/manifest">
  <App Id="bbbbbbbb-0000-0000-0000-000000000002" Name="BombDep" Publisher="Test" Version="1.0.0.0" />
</Package>
"#,
                )
                .unwrap();
            writer.start_file("SymbolReference.json", opts).unwrap();
            writer.write_all(b"{}").unwrap();
            const CHUNK: usize = 1024 * 1024;
            let chunk = vec![0u8; CHUNK];
            let mut remaining = crate::capped_io::SYMBOL_REFERENCE_JSON_CAP as usize + 1024;
            while remaining > 0 {
                let n = remaining.min(CHUNK);
                writer.write_all(&chunk[..n]).unwrap();
                remaining -= n;
            }
            writer.finish().unwrap();
        }
        let mut bytes = vec![0u8; 40]; // NAVX_HEADER_SIZE
        bytes.extend_from_slice(&zip_buf.into_inner());

        let dir = tempfile::tempdir().expect("tempdir");
        let app_path = dir.path().join("bomb-dep.app");
        std::fs::write(&app_path, &bytes).expect("write crafted .app");

        let dep = dep_id("BombDep");
        let ws = ws_id();
        let cache = AbiCache::new(); // not seeded — forces a real disk read.

        let mut dep_unit = make_symbolonly_dep_unit(&dep);
        dep_unit.app_path = Some(app_path);

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), dep_unit],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        assert_eq!(
            g.abi_ingest_errors.len(),
            1,
            "an over-cap SymbolReference.json must surface exactly one ingest \
             diagnostic, not silently ingest as empty; got {:?}",
            g.abi_ingest_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert!(
            g.abi_ingest_errors[0].message.contains("failed to read"),
            "got: {}",
            g.abi_ingest_errors[0].message
        );
    }

    /// H-3: a JSON-parse-failure `error` (the OTHER failure mode —
    /// `SymbolReferenceAbi.error` set by `parse_symbol_reference` itself,
    /// simulated directly here via `AbiCache::seed` rather than a real
    /// malformed file — `nul_padded_json_still_parses_full_content` and
    /// `bad_json_yields_error_not_panic` in `engine::deps::symbol_reference`
    /// already prove the parser's own behavior; this proves the SEPARATE
    /// propagation wiring from `abi.error` through `ingest_abi` into
    /// `ProgramGraph.abi_ingest_errors`) must surface, while a HEALTHY dep
    /// in the SAME snapshot produces no diagnostic at all — proving the
    /// channel is per-app and selective, not a blanket flag.
    #[test]
    fn abi_parse_error_propagates_selectively_per_app() {
        let broken = dep_id("BrokenParse");
        let healthy = dep_id("HealthyDep");
        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &broken.guid,
            &broken.name,
            &broken.publisher,
            &broken.version,
            Arc::new(SymbolReferenceAbi {
                error: Some("SymbolReference.json parse failed: trailing characters".into()),
                ..Default::default()
            }),
        );
        cache.seed(
            &healthy.guid,
            &healthy.name,
            &healthy.publisher,
            &healthy.version,
            Arc::new(make_dep_pub_abi()),
        );

        let snap = AppSetSnapshot {
            apps: vec![
                make_ws_unit(&ws),
                make_symbolonly_dep_unit(&broken),
                make_symbolonly_dep_unit(&healthy),
            ],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        assert_eq!(
            g.abi_ingest_errors.len(),
            1,
            "only the broken dep must be flagged; got {:?}",
            g.abi_ingest_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        let broken_ref = g
            .apps
            .find_by_name("BrokenParse")
            .expect("broken dep interned");
        assert_eq!(g.abi_ingest_errors[0].app, broken_ref);
        assert!(
            g.abi_ingest_errors[0]
                .message
                .contains("trailing characters")
        );

        // The healthy dep's own routines must still be fully ingested,
        // unaffected by the OTHER app's ingest failure.
        assert!(
            g.routines.iter().any(|r| r.id.name_lc == "dodepwork"),
            "the healthy dep's routines must still be ingested"
        );
    }

    /// Task 1 (round-2 tri-state arity hardening): an ABI routine whose
    /// `Parameters` field was absent/unparseable (`parameters_known: false`)
    /// must be ingested with the `UNKNOWN_ARITY` sentinel, never a false `0` —
    /// a real 0-arg procedure (explicit `Parameters: []`) is KNOWN-zero and
    /// must stay distinguishable in principle (both are ingested here to prove
    /// the sentinel differs from a real 0).
    #[test]
    fn unknown_arity_routine_gets_sentinel_params_count() {
        let dep = dep_id("ArityTest");
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 199,
                name: "Arity Test".into(),
                routines: vec![
                    AbiRoutine {
                        name: "KnownZero".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: true,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                    AbiRoutine {
                        name: "UnknownArity".into(),
                        kind: "procedure".into(),
                        event_kind: SrAbiEventKind::Unknown,
                        parameters: vec![],
                        return_type_text: None,
                        return_type_id: None,
                        is_local: false,
                        is_internal: false,
                        is_protected: false,
                        parameters_known: false,
                        attributes: vec![],
                        attributes_parsed: vec![],
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(abi),
        );
        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let routines_in_arity: Vec<_> = g
            .routines
            .iter()
            .filter(|r| r.id.object.id_equals_number(199))
            .collect();

        let known_zero = routines_in_arity
            .iter()
            .find(|r| r.id.name_lc == "knownzero")
            .expect("KnownZero must exist");
        assert_eq!(
            known_zero.id.params_count, 0,
            "an explicit empty Parameters array is a KNOWN 0-arity"
        );

        let unknown = routines_in_arity
            .iter()
            .find(|r| r.id.name_lc == "unknownarity")
            .expect("UnknownArity must exist");
        assert_eq!(
            unknown.id.params_count, UNKNOWN_ARITY,
            "an absent Parameters field must map to the UNKNOWN_ARITY sentinel, never 0"
        );
        assert_ne!(
            unknown.id.params_count, known_zero.id.params_count,
            "unknown arity must never collide with a real known-zero arity"
        );
    }

    /// Task 2 invariant (b, ABI half — FLIPPED from the pre-Task-2 discard):
    /// an ABI routine's `RoutineNode.return_type` is now POPULATED from the
    /// underlying `AbiRoutine.return_type_text` (the source-shaped
    /// reconstruction — see `symbol_reference::reconstruct_return_type_text`)
    /// instead of being hard-dropped to `None`. The structured `(name, id)`
    /// cross-validation pair (`AbiRoutine::return_type_id`) is threaded onto
    /// `RoutineNode.return_type_id` alongside it, reachable via the same
    /// `RoutineNodeId` lookup regardless of which `RouteTarget` shape a
    /// consumer (Task 3) ends up resolving through.
    #[test]
    fn abi_routine_return_type_is_populated() {
        let dep = dep_id("RetTypeTest");
        let abi = SymbolReferenceAbi {
            objects: vec![AbiObject {
                object_type: "Codeunit".into(),
                object_number: 99,
                name: "Ret Type Test".into(),
                routines: vec![AbiRoutine {
                    name: "GetHelper".into(),
                    kind: "procedure".into(),
                    event_kind: SrAbiEventKind::Unknown,
                    parameters: vec![],
                    return_type_text: Some("Codeunit \"Helper\"".into()),
                    return_type_id: Some(("Helper".into(), 2354)),
                    is_local: false,
                    is_internal: false,
                    is_protected: false,
                    parameters_known: true,
                    attributes: vec![],
                    attributes_parsed: vec![],
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let ws = ws_id();
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(abi),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let get_helper = g
            .routines
            .iter()
            .find(|r| r.id.name_lc == "gethelper")
            .expect("GetHelper must exist");
        assert_eq!(
            get_helper.return_type.as_deref(),
            Some("Codeunit \"Helper\""),
            "an ABI routine's reconstructed return-type text must now flow through \
             to RoutineNode.return_type (Task 2 flips the prior deliberate discard)"
        );
        assert_eq!(
            get_helper.return_type_id,
            Some(("Helper".to_string(), 2354)),
            "the structured (name, id) cross-validation pair must also be carried \
             onto the graph-level RoutineNode, reachable by RoutineNodeId lookup \
             for Task 3's cross-object chain cross-validation"
        );
    }

    // -------------------------------------------------------------------
    // Task 2 round-2 addendum: `param_type_fp`'s discriminator-bearing fold
    // -------------------------------------------------------------------

    fn abi_param(
        type_text: &str,
        subtype_id: Option<i64>,
        subtype_raw_name: Option<&str>,
        subtype_tag: SubtypeTag,
    ) -> AbiParameter {
        AbiParameter {
            name: "x".into(),
            type_text: type_text.into(),
            is_var: false,
            is_temporary: false,
            subtype_id,
            subtype_raw_name: subtype_raw_name.map(String::from),
            subtype_tag,
        }
    }

    /// (e) TRUE DUPLICATE: two parameters with an IDENTICAL canonical tuple
    /// (same text, same raw id, same raw name, same tag) must still
    /// fingerprint IDENTICALLY — this is the population
    /// `dedup_routines_preserving_genuine_overloads` correctly collapses
    /// (and marks `abi_overload_collapsed`, see `build.rs`'s existing
    /// `abi_sig_fp_collision_marks_survivor_collapsed`).
    #[test]
    fn param_type_fp_identical_discriminator_tuples_collide() {
        let p1 = vec![abi_param(
            "Codeunit \"Dep A\"",
            Some(60130),
            Some("Dep A"),
            SubtypeTag::Full,
        )];
        let p2 = vec![abi_param(
            "Codeunit \"Dep A\"",
            Some(60130),
            Some("Dep A"),
            SubtypeTag::Full,
        )];
        assert_eq!(param_type_fp(&p1), param_type_fp(&p2));
    }

    /// (b) round-1 critical sliver at the fp layer: two Id-only Subtypes
    /// sharing the IDENTICAL bare-fallback `type_text` ("Codeunit") but
    /// DIFFERENT raw ids must fingerprint DIFFERENTLY — this is what lets
    /// `DoIt(Codeunit 10)`/`DoIt(Codeunit 20)` survive as two distinct
    /// `RoutineNodeId`s instead of silently colliding.
    #[test]
    fn param_type_fp_different_id_only_subtypes_never_collide() {
        let p10 = vec![abi_param("Codeunit", Some(10), None, SubtypeTag::IdOnly)];
        let p20 = vec![abi_param("Codeunit", Some(20), None, SubtypeTag::IdOnly)];
        assert_ne!(param_type_fp(&p10), param_type_fp(&p20));
    }

    /// Sibling of the above at the fp layer: two quote-bearing Subtype Names
    /// sharing the IDENTICAL bare-fallback text and the SAME raw id must
    /// still fingerprint differently via the raw NAME discriminator.
    #[test]
    fn param_type_fp_different_quoted_names_never_collide() {
        let pa = vec![abi_param(
            "Codeunit",
            Some(1),
            Some("Weird\"NameA"),
            SubtypeTag::NameQuoted,
        )];
        let pb = vec![abi_param(
            "Codeunit",
            Some(1),
            Some("Weird\"NameB"),
            SubtypeTag::NameQuoted,
        )];
        assert_ne!(param_type_fp(&pa), param_type_fp(&pb));
    }

    /// Control: a genuinely scalar/no-Subtype parameter never collides with
    /// a degraded object-typed parameter that happens to share the SAME
    /// outer keyword text purely by coincidence — the degradation TAG (the
    /// canonical tuple's fourth component) keeps `"no_subtype"` distinct
    /// from `"empty_subtype"` even when id/name are both `None` in either.
    #[test]
    fn param_type_fp_no_subtype_vs_empty_subtype_tag_distinguishes() {
        let no_subtype = vec![abi_param("Codeunit", None, None, SubtypeTag::NoSubtype)];
        let empty_subtype = vec![abi_param("Codeunit", None, None, SubtypeTag::EmptySubtype)];
        assert_ne!(param_type_fp(&no_subtype), param_type_fp(&empty_subtype));
    }

    /// Full-fidelity case: two DIFFERENT named subtypes (the common,
    /// non-degraded shape) fingerprint differently — the ordinary case this
    /// primitive must never regress.
    #[test]
    fn param_type_fp_different_full_subtypes_never_collide() {
        let dep_a = vec![abi_param(
            "Codeunit \"Dep A\"",
            Some(60130),
            Some("Dep A"),
            SubtypeTag::Full,
        )];
        let dep_c = vec![abi_param(
            "Codeunit \"Dep C\"",
            Some(60140),
            Some("Dep C"),
            SubtypeTag::Full,
        )];
        assert_ne!(param_type_fp(&dep_a), param_type_fp(&dep_c));
    }

    // -----------------------------------------------------------------------
    // Task 3: ABI table-field ingestion (`ObjectNode.fields`)
    // -----------------------------------------------------------------------

    /// Fixture (d): an ABI `Table` entry's `AbiTable.fields` (Task 2's
    /// Subtype-qualified `parse_field` — an ABI Enum field carries `Enum
    /// "X"`, not the bare `"Enum"` pre-Task-2 dropped) must project onto the
    /// matching `ObjectNode.fields` as `FieldNode{name_lc, type_text}` —
    /// exactly like the source-side `extract_nodes` projection, so
    /// `ResolveIndex::field_in_table` works identically regardless of tier.
    #[test]
    fn abi_table_fields_populate_object_node_field_nodes() {
        let ws = ws_id();
        let dep = dep_id("DepTable");
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(SymbolReferenceAbi {
                objects: vec![AbiObject {
                    object_type: "Table".into(),
                    object_number: 50200,
                    name: "Dep Table".into(),
                    ..Default::default()
                }],
                tables: vec![AbiTable {
                    object_number: 50200,
                    name: "Dep Table".into(),
                    fields: vec![
                        AbiField {
                            field_number: 1,
                            name: "No.".into(),
                            data_type: "Code[20]".into(),
                            field_class: "Normal".into(),
                            is_blob_like: false,
                        },
                        AbiField {
                            field_number: 2,
                            name: "Status".into(),
                            // Task 2 Subtype-qualified shape — the ABI JSON
                            // carried `TypeDefinition:{Name:"Enum",
                            // Subtype:{Name:"Dep Status",...}}`; `parse_field`
                            // reconstructs it to this SOURCE-SHAPED text.
                            data_type: "Enum \"Dep Status\"".into(),
                            field_class: "Normal".into(),
                            is_blob_like: false,
                        },
                    ],
                    keys: vec![],
                    is_temporary: false,
                }],
                ..Default::default()
            }),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let dep_table = g
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case("Dep Table"))
            .expect("Dep Table ObjectNode must exist");
        assert_eq!(dep_table.fields.len(), 2);
        assert_eq!(dep_table.fields[0].name_lc, "no.");
        assert_eq!(dep_table.fields[0].type_text, "Code[20]");
        assert_eq!(dep_table.fields[1].name_lc, "status");
        assert_eq!(
            dep_table.fields[1].type_text, "Enum \"Dep Status\"",
            "ABI Enum field must carry the Task-2 Subtype-qualified text, not bare \"Enum\""
        );
    }

    /// Control: a non-Table/TableExtension ABI object (e.g. Codeunit) never
    /// gets a field surface, even if (implausibly) an `AbiTable` happened to
    /// share its `object_number` — `abi_table_fields` is only CONSULTED for
    /// Table/TableExtension kinds.
    #[test]
    fn abi_non_table_object_has_no_fields() {
        let ws = ws_id();
        let dep = dep_id("DepCodeunit");
        let cache = AbiCache::new();
        cache.seed(
            &dep.guid,
            &dep.name,
            &dep.publisher,
            &dep.version,
            Arc::new(SymbolReferenceAbi {
                objects: vec![AbiObject {
                    object_type: "Codeunit".into(),
                    object_number: 50201,
                    name: "Dep Codeunit".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );

        let snap = AppSetSnapshot {
            apps: vec![make_ws_unit(&ws), make_symbolonly_dep_unit(&dep)],
            workspace_app: ws,
            world: World::Closed,
        };
        let g = build_program_graph(&snap, &cache);

        let dep_cu = g
            .objects
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case("Dep Codeunit"))
            .expect("Dep Codeunit ObjectNode must exist");
        assert!(dep_cu.fields.is_empty());
    }

    // -------------------------------------------------------------------
    // Task 2 (roadmap-closure plan): `retain_abi_params` — including a REAL
    // generated `SymbolReference.json` fragment (fixture (i)), not only
    // hand-authored text.
    // -------------------------------------------------------------------

    /// A REAL `Methods[].Parameters` fragment extracted verbatim from
    /// `Continia Software_Continia Core_29.0.0.94574.app`'s own
    /// `SymbolReference.json` (the CDO workspace's own `.alpackages`
    /// dependency — see the task report for the extraction method):
    /// `Table 6192869 "CSC Temp. Assisted Setup"`'s method
    /// `RegisterAssistedSetup` (the method's own real `Id`, `707414482`).
    /// Kept byte-for-byte (including the real `ModuleId` field on
    /// `Subtype`, which `RawSubtype`/`RawTypeDef` never declare and serde
    /// silently ignores — proving the parser tolerates real-world extra
    /// JSON fields) so this fixture proves `retain_abi_params` against the
    /// type_text SHAPES as they really are, not an idealized hand-authored
    /// approximation.
    ///
    /// # Provenance correction (Task 2 review fix, Nit)
    ///
    /// Only the `Methods[]` entry below (parameters, method `Id`/`Name`) is
    /// the real, verbatim-extracted fragment. The `Codeunit "ProbeAssistedSetup"`
    /// (id `50700`) it is wrapped in here is a FABRICATED convenience
    /// wrapper — the real SymbolReference.json declares this method on
    /// `Table 6192869 "CSC Temp. Assisted Setup"`, not on any Codeunit; this
    /// fixture re-houses the real Methods[] content under a synthetic
    /// Codeunit purely because `retain_abi_params` operates on an
    /// `AbiRoutine` regardless of its declaring object KIND, so the wrapper's
    /// shape is immaterial to what this test actually proves.
    const REAL_REGISTER_ASSISTED_SETUP_JSON: &str = r#"
    {
      "Codeunits": [
        {
          "Id": 50700,
          "Name": "ProbeAssistedSetup",
          "Methods": [
            {
              "Parameters": [
                { "Name": "AppID", "TypeDefinition": { "Name": "Code[10]" } },
                { "Name": "AssistedSetupPageId", "TypeDefinition": { "Name": "Integer" } },
                { "Name": "BasicSetupCompleted", "TypeDefinition": { "Name": "Boolean" } },
                {
                  "Name": "AssistedSetupCategory",
                  "TypeDefinition": {
                    "Name": "Enum",
                    "Subtype": {
                      "ModuleId": "63ca2fa4-4f03-4f2b-a480-172fef340d3f",
                      "Name": "Assisted Setup Group",
                      "Id": 1815
                    }
                  }
                },
                {
                  "Name": "ManualSetupCategory",
                  "TypeDefinition": {
                    "Name": "Enum",
                    "Subtype": {
                      "ModuleId": "63ca2fa4-4f03-4f2b-a480-172fef340d3f",
                      "Name": "Manual Setup Category",
                      "Id": 1875
                    }
                  }
                }
              ],
              "Id": 707414482,
              "Name": "RegisterAssistedSetup"
            }
          ]
        }
      ]
    }
    "#;

    #[test]
    fn retain_abi_params_on_real_generated_symbol_reference_shape() {
        let abi = parse_symbol_reference(REAL_REGISTER_ASSISTED_SETUP_JSON);
        let obj = abi
            .objects
            .iter()
            .find(|o| o.name == "ProbeAssistedSetup")
            .expect("the fixture's fabricated wrapper Codeunit must parse");
        let routine = obj
            .routines
            .iter()
            .find(|r| r.name == "RegisterAssistedSetup")
            .expect("the real Method entry (verbatim from Table 6192869) must parse");
        assert!(
            routine.parameters_known,
            "a genuinely-present Parameters array must set parameters_known"
        );

        let AbiParams::Complete(params) = retain_abi_params(routine) else {
            panic!("a genuinely-parsed Parameters array must retain as Complete");
        };
        assert_eq!(params.len(), 5, "all 5 real parameters must be retained");

        // Scalars: real type_text verbatim, no Subtype at all.
        assert_eq!(params[0].type_text, "Code[10]");
        assert_eq!(params[0].subtype_id, None);
        assert_eq!(params[0].subtype_raw_name, None);
        assert_eq!(params[1].type_text, "Integer");
        assert_eq!(params[2].type_text, "Boolean");

        // Object-typed (Enum), Subtype Name+Id BOTH real-present — the "full"
        // shape, real ModuleId field on Subtype silently tolerated.
        assert_eq!(params[3].type_text, "Enum \"Assisted Setup Group\"");
        assert_eq!(params[3].subtype_id, Some(1815));
        assert_eq!(
            params[3].subtype_raw_name.as_deref(),
            Some("Assisted Setup Group")
        );
        assert_eq!(params[3].subtype_tag, SubtypeTag::Full);
        assert_eq!(params[4].type_text, "Enum \"Manual Setup Category\"");
        assert_eq!(params[4].subtype_id, Some(1875));
        assert_eq!(
            params[4].subtype_raw_name.as_deref(),
            Some("Manual Setup Category")
        );
        assert_eq!(params[4].subtype_tag, SubtypeTag::Full);

        // None of these are `var` — the real JSON carries no `IsVar` key on
        // any of these 5 (absent → `false`, never a guessed `true`).
        assert!(params.iter().all(|p| !p.is_var));
    }

    /// The tri-state-arity sibling: a routine whose `Parameters` field is
    /// absent entirely (`parameters_known == false`) must retain as
    /// `Missing`, never a false empty `Complete(vec![])`.
    #[test]
    fn retain_abi_params_missing_when_parameters_unknown() {
        let routine = AbiRoutine {
            name: "NoParamsField".into(),
            kind: "procedure".into(),
            event_kind: SrAbiEventKind::Unknown,
            parameters: vec![],
            parameters_known: false,
            return_type_text: None,
            return_type_id: None,
            is_local: false,
            is_internal: false,
            is_protected: false,
            attributes: vec![],
            attributes_parsed: vec![],
        };
        assert_eq!(retain_abi_params(&routine), AbiParams::Missing);
    }
}
