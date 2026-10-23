//! cli-b/b0 — the FULL `CapabilitySnapshot`: the consumed-core (built by
//! `snapshot::compose_snapshot`) PLUS the header + apps / contractFacts /
//! schemaFacts / permissionFacts / inputs / inputsMetadata / workspaceFingerprint
//! derivers that R4-F omitted. Byte-parity port of al-sem `src/snapshot/compose.ts`
//! (the `composeSnapshot` literal — its KEY ORDER is load-bearing for CBOR).
//!
//! ## Why an insertion-ordered tree (`CborValue`)
//!
//! cbor-x encodes objects in INSERTION key order; the `.cbor`/`.cbor.gz` goldens
//! therefore depend on the exact `composeSnapshot` literal order. serde_json's
//! `preserve_order` is OFF for this target, so routing through `serde_json::Value`
//! would alphabetize keys and scramble CBOR. Instead the full snapshot is built as
//! a single insertion-ordered [`CborValue`] tree:
//!   - CBOR        = `cbor::encode(tree)` (insertion order, the literal order).
//!   - raw JSON    = `to_sorted_json(tree)` (recursively SORTED keys — al-sem
//!     `serialize-json.ts` re-sorts, so raw is order-agnostic).
//!   - envelope    = a wrapper tree, serialized sorted with undefined-drop.
//!
//! The existing typed consumed-core derivers (which already emit correct per-fact
//! field order via custom `Serialize`) are folded into the tree through a tiny
//! serde [`Serializer`] that targets `CborValue` and preserves map insertion order
//! (`to_cbor_value`). The header derivers build their `CborValue` directly.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::engine::gate::cbor::CborValue;
use crate::engine::ids::{
    ParamSpec, canonical_routine_signature, locale_compare, object_signature_fingerprint,
    sha256_hex,
};
use crate::engine::l4::capability_cone::{
    CapabilityFact, R3a3SourceBase, R3a5CrossAppBase, build_r3a3_source_only_base,
};
use crate::engine::l5::snapshot::{
    SnapshotAnalysisGap, compose_snapshot_from_base, stable_routine_id,
};
use crate::program::model::workspace::Model;
use crate::program::resolve::semantic_golden::ClosureErrorKind;

mod to_cbor;
pub use to_cbor::to_cbor_value;

/// The snapshot schema version: THE one constant (the composer, the loader and
/// the shard manifest read it). 4 (#220): `analysisUniverse` and `apps[].role`.
pub const SNAPSHOT_SCHEMA_VERSION: i64 = 4;

/// Options for composing the full cli-b snapshot.
pub struct FullSnapshotOptions<'a> {
    /// The command's one read of the workspace (#160), read with
    /// `fingerprint_rows`: the `inputs[]` rows come from it.
    pub inputs: &'a crate::program::inputs::InputSet,
    /// The al-sem version string (`alsemVersion`; e.g. `cli-b-v1`).
    pub driver_version: &'a str,
    /// When true, `generatedAt` is pinned to the Unix epoch for byte-stable output.
    pub deterministic: bool,
    /// True when a `roots.config.json` existed but was ignored via --no-roots-config
    /// (sets `inputsMetadata.rootsConfigIgnored`). Pass false for the cli-b dump.
    pub roots_config_ignored: bool,
    /// #220 (spec §4.3): which model the snapshot describes
    /// (`analysisUniverse`). Production callers pass `SingleApp` until step 9.
    pub universe: SnapshotUniverse<'a>,
}

/// #220 (spec §4.3): the snapshot's universe (`analysisUniverse`).
pub enum SnapshotUniverse<'a> {
    SingleApp,
    CrossApp(CrossAppUniverse<'a>),
}

/// The cross-app universe. Field private: built only by
/// [`SnapshotUniverse::of`], so `rel`, `dependency_apps` and the model all
/// come from ONE base (O18).
pub struct CrossAppUniverse<'a> {
    base: &'a crate::engine::l4::capability_cone::R3a5CrossAppBase,
}

impl<'a> SnapshotUniverse<'a> {
    /// SingleApp -> SingleApp; CrossApp(base) -> CrossApp { base }.
    #[must_use]
    pub fn of(target: &'a crate::engine::gate::run::AnalysisTarget) -> Self {
        match target {
            crate::engine::gate::run::AnalysisTarget::SingleApp(_) => SnapshotUniverse::SingleApp,
            crate::engine::gate::run::AnalysisTarget::CrossApp(b) => {
                SnapshotUniverse::CrossApp(CrossAppUniverse { base: b.as_ref() })
            }
        }
    }

    /// The `analysisUniverse` value.
    fn as_str(&self) -> &'static str {
        match self {
            SnapshotUniverse::SingleApp => "single-app",
            SnapshotUniverse::CrossApp(_) => "cross-app",
        }
    }
}

/// Compose the FULL `CapabilitySnapshot` as an insertion-ordered `CborValue` tree,
/// in the exact `composeSnapshot` literal key order. This is the single source for
/// all four serializers.
/// Compute just the `workspaceFingerprint` string for a workspace, WITHOUT composing
/// a full snapshot tree. Used by the digest pipeline so it doesn't pay for a second
/// `compose_snapshot` (which `compose_full_snapshot` calls internally) just to fish
/// the fingerprint out of a CBOR map (#19).
pub fn workspace_fingerprint_of(
    inputs: &crate::program::inputs::InputSet,
    driver_version: &str,
) -> String {
    compute_workspace_fingerprint(snapshot_rows(inputs), driver_version)
}

/// The reproducibility input rows the command's `InputSet` hashed when it was
/// read (`derive_inputs`' rows, #160). The command declares the need
/// (`InputNeeds::fingerprint_rows`); a set read without it is a wiring bug.
fn snapshot_rows(inputs: &crate::program::inputs::InputSet) -> &[SnapshotInput] {
    inputs
        .snapshot_inputs
        .as_deref()
        .expect("fingerprint rows were not requested")
}

pub fn compose_full_snapshot(resolved: &Model, opts: &FullSnapshotOptions) -> CborValue {
    // O18: two revisions of one workspace share routine ids, so only a pointer
    // check proves the model and the universe come from ONE base.
    if let SnapshotUniverse::CrossApp(u) = &opts.universe {
        assert!(
            std::ptr::eq(resolved, &u.base.resolved),
            "compose_full_snapshot: the model is not the cross-app universe's model"
        );
    }
    // The consumed-core (identities, capabilityFacts, typedEdges, …, eventDeclarations,
    // rootClassifications, [routineOrderFrames]).
    // One base (cone + producer solve) for the core and the permission facts.
    let base = build_r3a3_source_only_base(resolved);
    let core = compose_snapshot_from_base(resolved, &base);

    let inputs = snapshot_rows(opts.inputs);
    let workspace_fingerprint = compute_workspace_fingerprint(inputs, opts.driver_version);

    let apps = project_apps(resolved, &core.identities, &opts.universe);
    let contract_facts = derive_contracts(resolved);
    let schema_facts = derive_schema(resolved);
    let permission_facts = derive_permissions(resolved, &base);

    // Serialize the consumed-core typed parts to ordered CborValues (their custom
    // `Serialize` impls drive the per-fact field order).
    let identities = to_cbor_value(&core.identities);
    let capability_facts = capability_facts_to_cbor(&core.capability_facts);
    let typed_edges = to_cbor_value(&core.typed_edges);
    let operation_index = to_cbor_value(&core.operation_index);
    let callsite_index = to_cbor_value(&core.callsite_index);
    let callsite_resolutions = to_cbor_value(&core.callsite_resolutions);
    let analysis_gaps = match &opts.universe {
        SnapshotUniverse::SingleApp => to_cbor_value(&core.analysis_gaps),
        SnapshotUniverse::CrossApp(u) => {
            to_cbor_value(&reachable_gaps(&core.analysis_gaps, u.base, &base))
        }
    };
    let coverage = to_cbor_value(&core.coverage);
    let event_declarations = to_cbor_value(&core.event_declarations);
    let root_classifications = to_cbor_value(&core.root_classifications);

    // Honor `deterministic`: pinned epoch (the golden form) when true, else a live
    // ISO-8601 timestamp via the shared gate helper. al-sem's `composeSnapshot`
    // sets `generatedAt = deterministic ? "1970-01-01T00:00:00Z" : new Date()…`.
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(opts.deterministic);

    // compose.ts:58-92 LITERAL order.
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert(
        "schemaVersion".into(),
        CborValue::Int(SNAPSHOT_SCHEMA_VERSION),
    );
    m.insert(
        "alsemVersion".into(),
        CborValue::Text(opts.driver_version.to_string()),
    );
    m.insert(
        "workspaceFingerprint".into(),
        CborValue::Text(workspace_fingerprint),
    );
    m.insert("generatedAt".into(), CborValue::Text(generated_at));
    m.insert(
        "analysisUniverse".into(),
        CborValue::Text(opts.universe.as_str().into()),
    );
    m.insert("apps".into(), apps);
    m.insert("identities".into(), identities);
    m.insert("contractFacts".into(), contract_facts);
    m.insert("schemaFacts".into(), schema_facts);
    m.insert("permissionFacts".into(), permission_facts);
    m.insert("rootClassifications".into(), root_classifications);
    m.insert("capabilityFacts".into(), capability_facts);
    m.insert("typedEdges".into(), typed_edges);
    m.insert("operationIndex".into(), operation_index);
    m.insert("callsiteIndex".into(), callsite_index);
    m.insert("callsiteResolutions".into(), callsite_resolutions);
    m.insert("analysisGaps".into(), analysis_gaps);
    m.insert("coverage".into(), coverage);
    m.insert("inputs".into(), to_cbor_inputs(inputs));
    m.insert("eventDeclarations".into(), event_declarations);

    // inputsMetadata + routineOrderFrames are attached AFTER ⇒ LAST (and only when
    // present, mirroring composeSnapshot's conditional `snap.x = …`).
    if opts.roots_config_ignored {
        let mut meta: IndexMap<String, CborValue> = IndexMap::new();
        meta.insert("rootsConfigIgnored".into(), CborValue::Bool(true));
        m.insert("inputsMetadata".into(), CborValue::Map(meta));
    }
    if let Some(frames) = &core.routine_order_frames {
        m.insert("routineOrderFrames".into(), to_cbor_value(frames));
    }

    CborValue::Map(m)
}

// ===========================================================================
// capabilityFacts → CborValue. Mirrors `snapshot::SnapshotCapabilityFact`'s custom
// `Serialize` (the load-bearing per-fact key order), with ONE CBOR-only addition:
//
// al-sem's inherited facts are built by `retag` = `{...rep, subject, provenance,
// via, witnessCallsiteId: edge.callsite}`. The spread ALWAYS sets the
// `witnessCallsiteId` KEY — to a string when the first-hop edge has a callsite, or
// to `undefined` when it does not (e.g. an event-dispatch edge). `JSON.stringify`
// drops the undefined property (so raw/envelope JSON omit it), but cbor-x KEEPS the
// slot as `0xf7`. So: for `provenance == "inherited"`, `witnessCallsiteId` is ALWAYS
// emitted (Text when Some, Undefined when None); for direct facts it is emitted only
// when Some. The key lands LAST in the tail (the spread appends it).
// ===========================================================================

fn capability_facts_to_cbor(
    facts: &[crate::engine::l5::snapshot::SnapshotCapabilityFact],
) -> CborValue {
    CborValue::Array(facts.iter().map(one_capability_fact_to_cbor).collect())
}

fn one_capability_fact_to_cbor(
    f: &crate::engine::l5::snapshot::SnapshotCapabilityFact,
) -> CborValue {
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    // HEAD.
    m.insert("subject".into(), CborValue::Text(f.subject.clone()));
    m.insert("op".into(), CborValue::Text(f.op.clone()));
    m.insert(
        "resourceKind".into(),
        CborValue::Text(f.resource_kind.clone()),
    );
    if let Some(rid) = &f.resource_id {
        m.insert("resourceId".into(), CborValue::Text(rid.clone()));
    }
    if let Some(ras) = &f.resource_arg_source {
        m.insert("resourceArgSource".into(), to_cbor_value(ras));
    }
    m.insert("confidence".into(), CborValue::Text(f.confidence.clone()));
    m.insert("provenance".into(), CborValue::Text(f.provenance.clone()));
    m.insert("via".into(), CborValue::Text(f.via.clone()));

    let is_inherited = f.provenance == "inherited";
    let is_event = f.resource_kind == "event";

    // The witnessCallsiteId slot: present (Text|Undefined) for inherited facts,
    // present-only-when-Some for direct facts.
    let witness_callsite_slot = |m: &mut IndexMap<String, CborValue>| match &f.witness_callsite_id {
        Some(wc) => {
            m.insert("witnessCallsiteId".into(), CborValue::Text(wc.clone()));
        }
        None if is_inherited => {
            m.insert("witnessCallsiteId".into(), CborValue::Undefined);
        }
        None => {}
    };

    if let Some(wo) = &f.witness_operation_id {
        // op-witness family: witnessOperationId, [extra], then witnessCallsiteId.
        m.insert("witnessOperationId".into(), CborValue::Text(wo.clone()));
        if let Some(extra) = &f.extra {
            m.insert("extra".into(), to_cbor_value(extra));
        }
        witness_callsite_slot(&mut m);
    } else if is_event {
        // event family: extra first (direct has no witness), then witnessCallsiteId.
        if let Some(extra) = &f.extra {
            m.insert("extra".into(), to_cbor_value(extra));
        }
        witness_callsite_slot(&mut m);
    } else {
        // callsite-witness family (http/ui/dispatch): witnessCallsiteId, extra.
        witness_callsite_slot(&mut m);
        if let Some(extra) = &f.extra {
            m.insert("extra".into(), to_cbor_value(extra));
        }
    }
    // Last, present only on a trigger-first inherited fact, as in
    // `SnapshotCapabilityFact`'s JSON `Serialize` (#96 D5).
    if let Some(op) = &f.witness_first_hop_operation_id {
        m.insert(
            "witnessFirstHopOperationId".into(),
            CborValue::Text(op.clone()),
        );
    }
    // Last, present only on an event-dispatch-first inherited fact (#98 D1).
    if let Some(ev) = &f.witness_first_hop_event_id {
        m.insert("witnessFirstHopEventId".into(), CborValue::Text(ev.clone()));
    }
    CborValue::Map(m)
}

// ===========================================================================
// raw-JSON serialization (al-sem serialize-json.ts).
// `JSON.stringify(snap, sortedReplacer(), 2) + "\n"` — recursively SORTED keys,
// 2-space indent, trailing newline. Arrays pass through as-is.
// ===========================================================================

/// Serialize a `CborValue` tree as pretty JSON with recursively SORTED object keys
/// (2-space indent), trailing newline. Mirrors al-sem `serialize-json.ts`.
pub fn to_sorted_json(tree: &CborValue) -> String {
    let mut s = String::new();
    // `JSON.stringify` ALWAYS drops undefined-valued properties (raw + envelope
    // alike). `CborValue::Undefined` therefore never appears in JSON output; it is
    // the CBOR-only `f7` slot (e.g. inherited facts' `witnessCallsiteId: undefined`).
    write_sorted_json_inner(tree, 0, true, &mut s);
    s.push('\n');
    s
}

fn write_sorted_json_inner(v: &CborValue, indent: usize, drop_undefined: bool, out: &mut String) {
    match v {
        CborValue::Null | CborValue::Undefined => out.push_str("null"),
        CborValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CborValue::Int(n) => out.push_str(&n.to_string()),
        CborValue::Float(f) => out.push_str(&json_number(*f)),
        CborValue::Text(s) => out.push_str(&json_string(s)),
        CborValue::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let pad = "  ".repeat(indent + 1);
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad);
                write_sorted_json_inner(item, indent + 1, drop_undefined, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push(']');
        }
        CborValue::Map(entries) => {
            // Sort keys, optionally dropping undefined values (envelope replacer).
            let mut keys: Vec<&String> = entries
                .iter()
                .filter(|(_, val)| !(drop_undefined && matches!(val, CborValue::Undefined)))
                .map(|(k, _)| k)
                .collect();
            keys.sort();
            if keys.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let pad = "  ".repeat(indent + 1);
            for (i, k) in keys.iter().enumerate() {
                out.push_str(&pad);
                out.push_str(&json_string(k));
                out.push_str(": ");
                write_sorted_json_inner(&entries[*k], indent + 1, drop_undefined, out);
                if i + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
    }
}

/// JSON-encode a string exactly as `JSON.stringify` (escape `"`, `\`, control
/// chars; non-ASCII passed through verbatim — al-sem output is UTF-8).
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render a non-integer number as JS `JSON.stringify` would. The snapshot has no
/// non-integer numeric fields, but render faithfully for completeness.
fn json_number(f: f64) -> String {
    if f == f.trunc() && f.is_finite() {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

// ===========================================================================
// CBOR inputs helper — build the inputs array in the per-input field order
// al-sem `SnapshotInput` declares: kind, path, contentHash.
// ===========================================================================

fn to_cbor_inputs(inputs: &[SnapshotInput]) -> CborValue {
    let arr = inputs
        .iter()
        .map(|i| {
            let mut m: IndexMap<String, CborValue> = IndexMap::new();
            m.insert("kind".into(), CborValue::Text(i.kind.clone()));
            m.insert("path".into(), CborValue::Text(i.path.clone()));
            m.insert(
                "contentHash".into(),
                CborValue::Text(i.content_hash.clone()),
            );
            CborValue::Map(m)
        })
        .collect();
    CborValue::Array(arr)
}

// ===========================================================================
// Gaps, serialized cross-app (#220 D2): every primary gap; a dependency gap
// only when its subject is reachable over the cone.
// ===========================================================================

/// The routines (internal ids) reachable over `rel.cone()` from every primary
/// routine of the model (spec §4.2: the cone, not the cut, F9).
fn reach(x: &R3a5CrossAppBase) -> HashSet<String> {
    use crate::program::model::role::Role;
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in x.rel.cone() {
        if let Some(to) = &e.to {
            adj.entry(e.from.as_str()).or_default().push(to.as_str());
        }
    }
    let m = &x.resolved;
    let mut stack: Vec<&str> = m
        .workspace
        .routines
        .iter()
        .filter(|r| m.routine_role(r) == Role::Primary)
        .map(|r| r.id.as_str())
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(id) = stack.pop() {
        if seen.insert(id.to_string()) {
            stack.extend(adj.get(id).into_iter().flatten().copied());
        }
    }
    seen
}

/// Keep a gap when it is primary, or its subject is a reachable routine's
/// stable id, or a bare app guid owning a reachable routine (`object_id`'s
/// first segment, case-insensitive).
fn reachable_gaps(
    gaps: &[SnapshotAnalysisGap],
    x: &R3a5CrossAppBase,
    source: &R3a3SourceBase,
) -> Vec<SnapshotAnalysisGap> {
    use crate::program::model::role::Role;
    let reach = reach(x);
    let mut routines: HashSet<String> = HashSet::new();
    let mut apps: HashSet<String> = HashSet::new();
    for r in x.resolved.workspace.routines.iter() {
        if reach.contains(&r.id) {
            routines.insert(stable_routine_id(&r.id, &source.routine_to_stable));
            apps.insert(
                r.object_id
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase(),
            );
        }
    }
    let kept: Vec<_> = gaps
        .iter()
        .filter(|g| {
            g.role == Role::Primary.as_str()
                || routines.contains(&g.subject)
                || apps.contains(&g.subject.to_ascii_lowercase())
        })
        .cloned()
        .collect();
    // F21: an opaque boundary is a source-less app.
    debug_assert!(
        kept.iter()
            .filter(|g| g.kind == "symbol-only-boundary")
            .all(|g| !x
                .dependency_apps
                .iter()
                .any(|a| a.guid.eq_ignore_ascii_case(&g.subject) && a.has_source)),
        "a symbol-only-boundary gap names an app with source"
    );
    kept
}

// ===========================================================================
// apps (compose.ts projectApps). Per-app field order: appGuid, publisher,
// name, version, role (#220). The primary row first; cross-app, then every
// dependency app that owns an `identities` row, sorted by appGuid.
// ===========================================================================

fn project_apps(
    resolved: &Model,
    identities: &crate::engine::l5::snapshot::SnapshotIdentityTable,
    universe: &SnapshotUniverse,
) -> CborValue {
    use crate::program::model::role::{Role, role_of_stable_id};
    let row = |guid: &str, publisher: &str, name: &str, version: &str, role: Role| {
        let mut m: IndexMap<String, CborValue> = IndexMap::new();
        m.insert("appGuid".into(), CborValue::Text(guid.to_string()));
        m.insert("publisher".into(), CborValue::Text(publisher.to_string()));
        m.insert("name".into(), CborValue::Text(name.to_string()));
        m.insert("version".into(), CborValue::Text(version.to_string()));
        m.insert("role".into(), CborValue::Text(role.as_str().into()));
        CborValue::Map(m)
    };
    let mut arr: Vec<CborValue> = Vec::new();
    let primary = resolved
        .primary_app
        .as_ref()
        .filter(|app| !app.app_guid.is_empty());
    if let Some(app) = primary {
        arr.push(row(
            &app.app_guid,
            &app.publisher,
            &app.name,
            &app.version,
            Role::Primary,
        ));
    }
    if let SnapshotUniverse::CrossApp(u) = universe {
        // The dependency apps owning an identity, matched case-insensitively (O13).
        let primary_guid = primary.map(|a| a.app_guid.as_str());
        let owners: std::collections::HashSet<String> = identities
            .stable_ids
            .iter()
            .filter(|id| role_of_stable_id(primary_guid, id) == Role::Dependency)
            .filter_map(|id| id.split_once(':').map(|(g, _)| g.to_ascii_lowercase()))
            .collect();
        let mut deps: Vec<&crate::program::model::workspace::DependencyApp> = u
            .base
            .dependency_apps
            .iter()
            .filter(|a| owners.contains(&a.guid.to_ascii_lowercase()))
            .collect();
        // F22: every dependency row comes from a required app.
        debug_assert!(
            owners
                .iter()
                .all(|g| deps.iter().any(|a| a.guid.eq_ignore_ascii_case(g))),
            "a serialized identity's app is missing from dependency_apps"
        );
        // al-sem `projectApps`: `.sort((x,y) => x.appGuid.localeCompare(y.appGuid))`.
        deps.sort_by(|a, b| locale_compare(&a.guid, &b.guid));
        arr.extend(
            deps.into_iter()
                .map(|a| row(&a.guid, &a.publisher, &a.name, &a.version, Role::Dependency)),
        );
    }
    CborValue::Array(arr)
}

// ===========================================================================
// contractFacts (derive/contracts.ts). One per object + routine.
// Field order (per the existing SnapshotCapabilityFact precedent): kind, stableId,
// visibility, signatureFingerprint, attributes, [obsoleteState], [obsoleteReason].
// The corpus carries no obsolete state, but emit faithfully when present.
// Sorted by stableId.
// ===========================================================================

fn derive_contracts(resolved: &Model) -> CborValue {
    let ws = &resolved.workspace;
    // (sort_key, CborValue) pairs.
    let mut rows: Vec<(String, CborValue)> = Vec::new();

    for obj in &ws.objects {
        let stable_id = crate::engine::ids::to_stable_object_id(&obj.id);
        let mut m: IndexMap<String, CborValue> = IndexMap::new();
        m.insert("kind".into(), CborValue::Text("object".into()));
        m.insert("stableId".into(), CborValue::Text(stable_id.clone()));
        // Objects don't carry accessModifier in the model — default "public".
        m.insert("visibility".into(), CborValue::Text("public".into()));
        m.insert(
            "signatureFingerprint".into(),
            CborValue::Text(object_signature_fingerprint(
                &obj.object_type,
                obj.object_number,
                &obj.name,
            )),
        );
        // Objects carry no attributesParsed in the model — default [].
        m.insert("attributes".into(), CborValue::Array(Vec::new()));
        rows.push((stable_id, CborValue::Map(m)));
    }

    for r in &ws.routines {
        // ⟨task-4-review.md finding I-1⟩ Read the stable id the assembly already
        // minted — do not re-derive it from parts. A future mint-site change (e.g.
        // the OUTSTANDING.md path-qualified-member wake condition) is not obligated
        // to touch `to_stable_routine_id_from_parts`'s signature, so a recompute
        // here would silently diverge from `r.stable_routine_id` with no test to
        // catch it.
        let stable_id = r.stable_routine_id.clone();
        let mut m: IndexMap<String, CborValue> = IndexMap::new();
        m.insert("kind".into(), CborValue::Text("routine".into()));
        m.insert("stableId".into(), CborValue::Text(stable_id.clone()));
        m.insert(
            "visibility".into(),
            CborValue::Text(map_visibility(r.access_modifier.as_deref())),
        );
        let params: Vec<ParamSpec> = r
            .parameters
            .iter()
            .map(|p| ParamSpec {
                type_text: p.type_text.clone(),
                is_var: p.is_var,
            })
            .collect();
        let sig_text = canonical_routine_signature(&r.name, &params, r.return_type.as_deref());
        m.insert(
            "signatureFingerprint".into(),
            CborValue::Text(sha256_hex(&sig_text)),
        );
        m.insert(
            "attributes".into(),
            fingerprint_attributes(&r.attributes_parsed),
        );
        rows.push((stable_id, CborValue::Map(m)));
    }

    // al-sem `deriveContracts`: `.sort((a,b) => a.stableId.localeCompare(b.stableId))`.
    rows.sort_by(|a, b| locale_compare(&a.0, &b.0));
    CborValue::Array(rows.into_iter().map(|(_, v)| v).collect())
}

fn map_visibility(am: Option<&str>) -> String {
    match am.map(|s| s.to_lowercase()).as_deref() {
        Some("public") => "public",
        Some("internal") => "internal",
        Some("protected") => "protected",
        Some("local") => "local",
        _ => "public",
    }
    .to_string()
}

/// `[{name, argsHash}]` where argsHash = sha256(canonicalJson(args)). Each attribute
/// fingerprint is sorted-key `{name, argsHash}` in the output, but the array order
/// follows declaration order (al-sem maps over `attributesParsed`).
fn fingerprint_attributes(attrs: &[crate::program::attributes::AttributeInfo]) -> CborValue {
    let arr = attrs
        .iter()
        .map(|a| {
            let mut m: IndexMap<String, CborValue> = IndexMap::new();
            m.insert("name".into(), CborValue::Text(a.name.clone()));
            m.insert(
                "argsHash".into(),
                CborValue::Text(sha256_hex(&canonical_attr_args_json(&a.args))),
            );
            CborValue::Map(m)
        })
        .collect();
    CborValue::Array(arr)
}

/// `canonicalJson(a.args ?? [])` — the args array serialized with SORTED keys,
/// `undefined` (None) keys dropped. Mirrors contracts.ts `canonicalJson`. Each
/// `AttributeArg` has keys {kind, text, value?, qualifier?, member?}; sorted that
/// is {kind, member, qualifier, text, value} with absent optionals dropped.
fn canonical_attr_args_json(args: &[crate::program::attributes::AttributeArg]) -> String {
    let parts: Vec<String> = args.iter().map(canonical_one_attr_arg).collect();
    format!("[{}]", parts.join(","))
}

fn canonical_one_attr_arg(a: &crate::program::attributes::AttributeArg) -> String {
    // Build sorted (key, json-value) pairs, dropping None optionals.
    let mut pairs: Vec<(&str, String)> = Vec::new();
    pairs.push(("kind", json_string(&a.kind)));
    pairs.push(("text", json_string(&a.text)));
    if let Some(v) = &a.value {
        pairs.push(("value", json_string(v)));
    }
    if let Some(q) = &a.qualifier {
        pairs.push(("qualifier", json_string(q)));
    }
    if let Some(mem) = &a.member {
        pairs.push(("member", json_string(mem)));
    }
    pairs.sort_by(|x, y| x.0.cmp(y.0));
    let body: Vec<String> = pairs
        .into_iter()
        .map(|(k, v)| format!("{}:{}", json_string(k), v))
        .collect();
    format!("{{{}}}", body.join(","))
}

// ===========================================================================
// schemaFacts (derive/schema.ts). table + field + key facts. Sorted by stableId.
// ===========================================================================

fn derive_schema(resolved: &Model) -> CborValue {
    let ws = &resolved.workspace;
    let mut rows: Vec<(String, CborValue)> = Vec::new();

    for tbl in &ws.tables {
        let stable_table = crate::engine::ids::to_stable_table_id(&tbl.app_guid, tbl.table_number);
        // table fact: shape {number, name}.
        let table_shape = sha256_hex(&format!(
            "{{{}:{},{}:{}}}",
            json_string("name"),
            json_string(&tbl.name),
            json_string("number"),
            tbl.table_number
        ));
        let mut tm: IndexMap<String, CborValue> = IndexMap::new();
        tm.insert("kind".into(), CborValue::Text("table".into()));
        tm.insert("stableId".into(), CborValue::Text(stable_table.clone()));
        tm.insert("shapeFingerprint".into(), CborValue::Text(table_shape));
        rows.push((stable_table.clone(), CborValue::Map(tm)));

        for fld in &tbl.fields {
            let stable_field = format!("{stable_table}#{}", fld.field_number);
            // field shape: {dataType, fieldClass, isBlobLike}. canonicalJson sorts
            // keys → {dataType, fieldClass, isBlobLike} (already alphabetical).
            let field_shape = sha256_hex(&format!(
                "{{{}:{},{}:{},{}:{}}}",
                json_string("dataType"),
                json_string(&fld.data_type),
                json_string("fieldClass"),
                json_string(&fld.field_class),
                json_string("isBlobLike"),
                if fld.is_blob_like { "true" } else { "false" }
            ));
            let mut fm: IndexMap<String, CborValue> = IndexMap::new();
            fm.insert("kind".into(), CborValue::Text("field".into()));
            fm.insert("stableId".into(), CborValue::Text(stable_field.clone()));
            fm.insert("shapeFingerprint".into(), CborValue::Text(field_shape));
            rows.push((stable_field, CborValue::Map(fm)));
        }

        for key in &tbl.keys {
            // KeyId `${tableId}/key/${index}` → `${stableTableId}#K${index}`.
            let key_index = key
                .id
                .rsplit_once("/key/")
                .map(|(_, idx)| idx.to_string())
                .unwrap_or_else(|| key.id.clone());
            let stable_key = format!("{stable_table}#K{key_index}");
            // key shape: {fields: stableFieldIds.sort(), isEnabled:true}.
            let mut stable_fields: Vec<String> = key
                .fields
                .iter()
                .map(|fid| internal_field_to_stable(fid, &tbl.app_guid))
                .collect();
            stable_fields.sort();
            let fields_json = format!(
                "[{}]",
                stable_fields
                    .iter()
                    .map(|f| json_string(f))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            // canonicalJson({fields, isEnabled}) sorts keys → {fields, isEnabled}.
            let key_shape = sha256_hex(&format!(
                "{{{}:{},{}:true}}",
                json_string("fields"),
                fields_json,
                json_string("isEnabled")
            ));
            let mut km: IndexMap<String, CborValue> = IndexMap::new();
            km.insert("kind".into(), CborValue::Text("key".into()));
            km.insert("stableId".into(), CborValue::Text(stable_key.clone()));
            km.insert("shapeFingerprint".into(), CborValue::Text(key_shape));
            rows.push((stable_key, CborValue::Map(km)));
        }
    }

    // al-sem `deriveSchema`: `.sort((a,b) => a.stableId.localeCompare(b.stableId))`.
    rows.sort_by(|a, b| locale_compare(&a.0, &b.0));
    CborValue::Array(rows.into_iter().map(|(_, v)| v).collect())
}

/// Internal field id `${appGuid}/table/${tableNumber}/${fieldNumber}` →
/// stable `${appGuid}:Table:${tableNumber}#${fieldNumber}`.
fn internal_field_to_stable(internal: &str, app_guid: &str) -> String {
    // Format: appGuid/table/N/F. Recover N + F.
    let suffix = internal.strip_prefix(&format!("{app_guid}/table/"));
    match suffix.and_then(|s| s.rsplit_once('/')) {
        Some((table_n, field_n)) => format!("{app_guid}:Table:{table_n}#{field_n}"),
        None => internal.to_string(),
    }
}

// ===========================================================================
// permissionFacts (derive/permissions.ts). Required-only (declared needs
// permissionSet projection not in the model). Per spec §3.8:
//   read|insert|modify|delete on table T → R|I|M|D on TableData T
//   execute on codeunit|page|report O    → X on O
// Sorted by permKey = "R|subject|target|targetKind|rights".
// Field order (RequiredPermissionFact): kind, subject, target, targetKind, rights,
// derivedFromCapability, coverage.
// ===========================================================================

fn derive_permissions(
    resolved: &Model,
    base: &crate::engine::l4::capability_cone::R3a3SourceBase,
) -> CborValue {
    let mut rows: Vec<(String, CborValue)> = Vec::new();

    for r in &resolved.workspace.routines {
        // r.summary present iff the cone ran (base.cones has the routine).
        let Some(cone) = base.cones.get(&r.id) else {
            continue;
        };
        // ⟨task-4-review.md finding I-1⟩ Same as `derive_contracts` above: read the
        // already-minted stable id instead of re-deriving it.
        let stable_subject = r.stable_routine_id.clone();

        // coverage = inheritedStatus ?? directStatus ?? "unknown".
        let coverage = {
            let inh = &cone.coverage.inherited_status;
            let dir = &cone.coverage.direct_status;
            if !inh.is_empty() {
                inh.clone()
            } else if !dir.is_empty() {
                dir.clone()
            } else {
                "unknown".to_string()
            }
        };

        // facts = direct ∪ inherited.
        let mut facts: Vec<&CapabilityFact> = Vec::new();
        if let Some(d) = base.direct_full.get(&r.id) {
            facts.extend(d.iter());
        }
        facts.extend(cone.inherited.iter());

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        for f in facts {
            if f.resource_kind == "table" && f.resource_id.is_some() {
                let Some(right) = table_op_to_right(f.op) else {
                    continue;
                };
                // #13: a provably temporary record needs no TableData permission.
                // Skipped BEFORE the `seen` dedup, so a physical access to the
                // same table and right still claims the row (with its own
                // witness). Safe since #33: the cone keys temp and physical facts
                // apart, so a physical access is never hidden behind a temp one.
                // Unknown / parameter-dependent temp state still requires.
                if crate::engine::l4::cone_derived::fact_is_known_temp(f) {
                    continue;
                }
                let stable_table = stable_table_from_internal(f.resource_id.as_deref().unwrap());
                let key = format!("{stable_subject}|{stable_table}|{right}");
                if !seen.insert(key) {
                    continue;
                }
                rows.push(make_required_perm(
                    &stable_subject,
                    &stable_table,
                    "TableData",
                    right,
                    f,
                    &coverage,
                ));
            } else if f.op == "execute"
                && f.resource_id.is_some()
                && matches!(f.resource_kind, "codeunit" | "page" | "report")
            {
                let stable_obj =
                    crate::engine::ids::to_stable_object_id(f.resource_id.as_deref().unwrap());
                let key = format!("{stable_subject}|{stable_obj}|X");
                if !seen.insert(key) {
                    continue;
                }
                let target_kind = match f.resource_kind {
                    "codeunit" => "Codeunit",
                    "page" => "Page",
                    _ => "Report",
                };
                rows.push(make_required_perm(
                    &stable_subject,
                    &stable_obj,
                    target_kind,
                    "X",
                    f,
                    &coverage,
                ));
            }
        }
    }

    // al-sem `derivePermissions`: `.sort((a,b) => permKey(a).localeCompare(permKey(b)))`.
    rows.sort_by(|a, b| locale_compare(&a.0, &b.0));
    CborValue::Array(rows.into_iter().map(|(_, v)| v).collect())
}

fn table_op_to_right(op: &str) -> Option<&'static str> {
    match op {
        "read" => Some("R"),
        "insert" => Some("I"),
        "modify" => Some("M"),
        "delete" => Some("D"),
        _ => None,
    }
}

/// Internal table id `${appGuid}/table/${N}` → stable `${appGuid}:Table:${N}`.
fn stable_table_from_internal(internal: &str) -> String {
    let parts: Vec<&str> = internal.split('/').collect();
    if parts.len() == 3 && parts[1] == "table" {
        format!("{}:Table:{}", parts[0], parts[2])
    } else {
        internal.to_string()
    }
}

/// One required-permission row, derived from capability fact `f`.
fn make_required_perm(
    subject: &str,
    target: &str,
    target_kind: &str,
    right: &str,
    f: &CapabilityFact,
    coverage: &str,
) -> (String, CborValue) {
    let perm_key = format!("R|{subject}|{target}|{target_kind}|{right}");
    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert("kind".into(), CborValue::Text("required".into()));
    m.insert("subject".into(), CborValue::Text(subject.into()));
    m.insert("target".into(), CborValue::Text(target.into()));
    m.insert("targetKind".into(), CborValue::Text(target_kind.into()));
    m.insert(
        "rights".into(),
        CborValue::Array(vec![CborValue::Text(right.into())]),
    );
    // derivedFromCapability: { op, [witnessCallsiteId], [witnessFirstHopOperationId],
    // [witnessFirstHopEventId] }. A fact inherited through an `implicit-trigger`
    // first hop has no call site; its witness is the firing record op (#96 D5).
    // Through an `event-dispatch` first hop it is the dispatched event (#98 D1).
    let mut dfc: IndexMap<String, CborValue> = IndexMap::new();
    dfc.insert("op".into(), CborValue::Text(f.op.into()));
    if let Some(wc) = &f.witness_callsite_id {
        dfc.insert("witnessCallsiteId".into(), CborValue::Text(wc.clone()));
    }
    if let Some(op) = &f.witness_first_hop_operation_id {
        dfc.insert(
            "witnessFirstHopOperationId".into(),
            CborValue::Text(op.clone()),
        );
    }
    if let Some(ev) = &f.witness_first_hop_event_id {
        dfc.insert("witnessFirstHopEventId".into(), CborValue::Text(ev.clone()));
    }
    m.insert("derivedFromCapability".into(), CborValue::Map(dfc));
    m.insert("coverage".into(), CborValue::Text(coverage.into()));
    (perm_key, CborValue::Map(m))
}

// ===========================================================================
// inputs (derive/inputs.ts) + workspaceFingerprint (derive/workspace-fingerprint.ts).
// ===========================================================================

/// One reproducibility input (kind, path, contentHash).
pub struct SnapshotInput {
    pub kind: String,
    pub path: String,
    pub content_hash: String,
}

/// The inputs that contribute to snapshot identity, over the files the
/// command's `InputSet` read (#160; `InputSet::read` calls this once, under
/// `InputNeeds::fingerprint_rows`):
///   app.json (readable bytes, parse or no parse) → "app-json"; every
///   dependency-closure `.app` the loader reads (own and ancestor
///   `.alpackages`) → "dep-package", keyed `.alpackages/x.app` /
///   `../.alpackages/y.app`; a closure entry that cannot be listed or read →
///   "dep-package-unreadable"; roots.config.json (only when it loads and
///   validates) → "roots-config"; al-sem.coverage.yaml (if present) → "policy".
/// `None` = no row. The closure `.app` files are hashed here. Paths
/// workspace-relative, forward-slash normalized. Sorted by (kind|path).
pub(crate) fn derive_inputs_from_parts(
    workspace_dir: &std::path::Path,
    app_json: Option<&[u8]>,
    closure: crate::program::resolve::semantic_golden::ClosureEntries,
    roots_config: Option<&[u8]>,
    coverage_yaml: Option<&[u8]>,
) -> Vec<SnapshotInput> {
    let mut out: Vec<SnapshotInput> = Vec::new();
    let row = |kind: &str, name: &str, bytes: &[u8]| SnapshotInput {
        kind: kind.into(),
        path: rel(workspace_dir, &workspace_dir.join(name)),
        content_hash: crate::engine::ids::sha256_bytes_hex(bytes),
    };

    // app.json.
    if let Some(bytes) = app_json {
        out.push(row("app-json", "app.json", bytes));
    }

    // The dependency closure the loader reads (own AND ancestor caches, up to
    // the git boundary), from the one shared discovery. An unreadable entry
    // fingerprints by its stable category, never by its OS message, so the
    // same failure fingerprints the same wherever the tree lives.
    let unreadable = |key: String, kind: ClosureErrorKind| SnapshotInput {
        kind: "dep-package-unreadable".into(),
        path: key,
        content_hash: crate::engine::ids::sha256_bytes_hex(kind.as_str().as_bytes()),
    };
    for (key, path) in closure.entries {
        match hash_file(&path) {
            Some(hash) => out.push(SnapshotInput {
                kind: "dep-package".into(),
                path: key,
                content_hash: hash,
            }),
            None => out.push(unreadable(key, ClosureErrorKind::ReadFailed)),
        }
    }
    for (key, kind, _os_message) in closure.errors {
        out.push(unreadable(key, kind));
    }

    // roots.config.json — included iff it loads+validates (al-sem reads
    // model.identity.rootsConfig, set only when the config was actually loaded).
    if let Some(bytes) = roots_config {
        out.push(row("roots-config", "roots.config.json", bytes));
    }
    // al-sem.coverage.yaml → "policy".
    if let Some(bytes) = coverage_yaml {
        out.push(row("policy", "al-sem.coverage.yaml", bytes));
    }

    // al-sem `deriveInputs`: `.sort((a,b) => `${a.kind}|${a.path}`.localeCompare(…))`.
    out.sort_by(|a, b| {
        locale_compare(
            &format!("{}|{}", a.kind, a.path),
            &format!("{}|{}", b.kind, b.path),
        )
    });
    out
}

/// SHA-256 over the sorted (kind, path, contentHash) triples + alsemVersion.
fn compute_workspace_fingerprint(inputs: &[SnapshotInput], driver_version: &str) -> String {
    let mut sorted: Vec<&SnapshotInput> = inputs.iter().collect();
    // al-sem `computeWorkspaceFingerprint`: same localeCompare(kind|path) sort.
    sorted.sort_by(|a, b| {
        locale_compare(
            &format!("{}|{}", a.kind, a.path),
            &format!("{}|{}", b.kind, b.path),
        )
    });
    let mut lines: Vec<String> = sorted
        .iter()
        .map(|i| format!("{}\t{}\t{}", i.kind, i.path, i.content_hash))
        .collect();
    lines.push(format!("alsemVersion\t{driver_version}"));
    sha256_hex(&lines.join("\n"))
}

fn hash_file(p: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    Some(crate::engine::ids::sha256_bytes_hex(&bytes))
}

/// Forward-slash-normalized workspace-relative path.
fn rel(from: &std::path::Path, p: &std::path::Path) -> String {
    let rel = p.strip_prefix(from).unwrap_or(p);
    rel.to_string_lossy().replace('\\', "/")
}

// ===========================================================================
// The four serializers (al-sem serialize-json / serialize-cbor / serialize-cbor-gz)
// + the envelope projection (contracts/snapshot.ts + contracts/document.ts) +
// sharding (snapshot/shard.ts).
// ===========================================================================

/// raw-JSON — `serializeJson(snap)`: sorted-key pretty JSON + trailing newline.
pub fn serialize_json(tree: &CborValue) -> String {
    to_sorted_json(tree)
}

/// CBOR — `serializeCbor(snap)`: cbor-x-compatible bytes (insertion order).
pub fn serialize_cbor(tree: &CborValue) -> Vec<u8> {
    crate::engine::gate::cbor::encode(tree)
}

/// gzip DEFLATE level for the `.cbor.gz` serializer — Bun's zlib default (6). The
/// goldens were produced with `Bun.gzipSync` (level 6); the byte stream matches at
/// this level via flate2's vendored zlib-ng.
const GZIP_LEVEL: u32 = 6;

/// CBOR + gzip — `serializeCborGz(snap)` with the gzip OS header byte normalized
/// to 0x03 (Unix), level-6 DEFLATE (zlib-ng), header `1f 8b 08 00 00000000 00 03`.
pub fn serialize_cbor_gz(tree: &CborValue) -> Vec<u8> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let cbor = serialize_cbor(tree);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(GZIP_LEVEL));
    // Total over in-memory bytes; the writes cannot fail (Vec sink).
    let _ = encoder.write_all(&cbor);
    let mut out = encoder.finish().unwrap_or_default();
    // Normalize the gzip OS byte (offset 9) to 0x03 (Unix) for host-independence.
    if out.len() >= 10 {
        out[9] = 0x03;
    }
    out
}

/// envelope-JSON — `projectSnapshotDocument(snap, …)` → `serializeDocument(doc)`.
/// Builds the `DocumentEnvelope<"capability-snapshot">`: kind, schemaVersion
/// (contract semver 1.1.0), alsemVersion, deterministic, generatedAt, diagnostics
/// (versionDiagnostic + projected analyzer diagnostics), payload (the snapshot with
/// schemaVersion → snapshotSchemaVersion, alsemVersion + generatedAt LIFTED out).
/// Then serialized sorted-key with `undefined` dropped (no undefined in practice).
pub fn serialize_envelope(
    tree: &CborValue,
    driver_version: &str,
    deterministic: bool,
    diagnostics: &[EnvelopeDiagnostic],
) -> String {
    let doc = build_envelope(tree, driver_version, deterministic, diagnostics);
    let mut s = String::new();
    write_sorted_json_inner(&doc, 0, true, &mut s);
    s.push('\n');
    s
}

/// A projected diagnostic (code, severity, message) for the envelope diagnostics
/// channel. The cli-b dump passes the analyzer diagnostics; with the version
/// override set, `versionDiagnostic()` is None, so the corpus has an empty list.
pub struct EnvelopeDiagnostic {
    pub code: String,
    pub severity: String,
    pub message: String,
}

fn build_envelope(
    tree: &CborValue,
    driver_version: &str,
    deterministic: bool,
    diagnostics: &[EnvelopeDiagnostic],
) -> CborValue {
    // Build the payload: clone the snapshot map, RENAME schemaVersion →
    // snapshotSchemaVersion, DROP alsemVersion + generatedAt (lifted to envelope).
    let CborValue::Map(snap) = tree else {
        return CborValue::Null;
    };
    let mut payload: IndexMap<String, CborValue> = IndexMap::new();
    for (k, v) in snap {
        match k.as_str() {
            "alsemVersion" | "generatedAt" => {} // lifted to the envelope.
            "schemaVersion" => {
                payload.insert("snapshotSchemaVersion".into(), v.clone());
            }
            _ => {
                payload.insert(k.clone(), v.clone());
            }
        }
    }

    // Honor `deterministic`: pinned epoch when true (the golden form), else a live
    // ISO-8601 timestamp — the SAME helper the analyze JSON envelope uses, so the
    // `--deterministic` contract is identical across both envelopes.
    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    let diags: Vec<CborValue> = diagnostics
        .iter()
        .map(|d| {
            let mut m: IndexMap<String, CborValue> = IndexMap::new();
            m.insert("code".into(), CborValue::Text(d.code.clone()));
            m.insert("severity".into(), CborValue::Text(d.severity.clone()));
            m.insert("message".into(), CborValue::Text(d.message.clone()));
            CborValue::Map(m)
        })
        .collect();

    let mut env: IndexMap<String, CborValue> = IndexMap::new();
    env.insert("kind".into(), CborValue::Text("capability-snapshot".into()));
    // Contract semver — al-sem SNAPSHOT_CONTRACT_VERSION = "1.1.0".
    env.insert("schemaVersion".into(), CborValue::Text("1.1.0".into()));
    env.insert(
        "alsemVersion".into(),
        CborValue::Text(driver_version.to_string()),
    );
    env.insert("deterministic".into(), CborValue::Bool(deterministic));
    env.insert("generatedAt".into(), CborValue::Text(generated_at));
    env.insert("diagnostics".into(), CborValue::Array(diags));
    env.insert("payload".into(), CborValue::Map(payload));
    CborValue::Map(env)
}

// ===========================================================================
// `--inventory-only` lean projection: routine-inventory document.
//
// The doc kind is "routine-inventory", schemaVersion "1.0.0". It reuses the
// ALREADY-composed full-snapshot CborValue tree to extract apps / identities /
// coverage / rootClassifications — byte-identical to the full snapshot's
// corresponding sub-values (projection-subset self-consistency). The heavy
// keys (capabilityFacts, typedEdges, operationIndex, callsiteIndex,
// callsiteResolutions, analysisGaps, inputs, inputsMetadata) are omitted.
//
// The per-routine inventory list (`routineInventory`) is derived directly from
// `resolved.workspace.routines` — every routine's (objectType, objectNumber,
// routineName, stableRoutineId). The source is the same workspace the full
// snapshot's contractFacts and identities derive from, so there is no risk of
// drift; the stableRoutineId is the same field `ModelRoutine::stable_routine_id`
// that the identity table indexes.
//
// Serialized sorted-key JSON (the same `write_sorted_json_inner` the full
// snapshot envelope uses), so consumers can rely on stable key order.
// ===========================================================================

/// Build and serialize the lean `routine-inventory` DocumentEnvelope as sorted-key
/// JSON. Reuses `tree` (the already-composed full-snapshot `CborValue`) to extract
/// the shared sub-values, so they are byte-identical to those in the full snapshot.
pub fn build_inventory_envelope(
    tree: &CborValue,
    resolved: &Model,
    driver_version: &str,
    deterministic: bool,
) -> String {
    let doc = build_inventory_doc(tree, resolved, driver_version, deterministic);
    let mut s = String::new();
    write_sorted_json_inner(&doc, 0, true, &mut s);
    s.push('\n');
    s
}

/// The schemaVersion for the routine-inventory document kind.
/// 1.1.0 (engine-e2): additive optional per-routine fields `enclosingMember` and
/// `originatingObject` for member-trigger routines (field/control/action/dataitem
/// trigger). Rust-only projection (not in the byte-parity harness).
/// 1.2.0 (#220): additive `apps[].role` and `payload.analysisUniverse`.
pub const INVENTORY_SCHEMA_VERSION: &str = "1.2.0";

/// Case-insensitive secondary-sort comparator for the inventory `enclosingMember`
/// key (RE-6). `None` orders before `Some`; two `Some`s compare on their
/// lowercased form so duplicate-`stableRoutineId` rows are content-stable
/// regardless of developer casing. Deterministic (locale-compare on lowercased
/// text; ties resolve only on the tertiary originatingObject key in the caller).
///
/// ⟨task 4⟩ **Kept deliberately, and no longer the ordinary case.** The stable id
/// now folds the enclosing member in, so two field triggers of one object have
/// DISTINCT `stableRoutineId` and the primary key already separates them. What
/// still reaches this comparator is the measured residual — a same member NAME at
/// two XMLport nesting paths, or `#if`/`#else` alternatives of one member (15
/// groups on BC Base App, 0 on DO) — where the members compare equal and the
/// tertiary key decides. Fail-closed for any future duplicate shape; pinned
/// directly by `member_tie_break_is_case_insensitive_and_none_first` because no
/// real fixture can reach it through the projection any more.
fn case_insensitive_compare_opt(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(x), Some(y)) => locale_compare(&x.to_lowercase(), &y.to_lowercase()),
    }
}

/// The full three-key inventory-row sort (RE-6): primary `stableRoutineId`
/// (locale_compare) → secondary `enclosingMember` (case-insensitive, `None`
/// first) → tertiary `originatingObject` (locale_compare). Named and called
/// directly by `build_inventory_doc`'s `sort_by` below so that a test exercising
/// this function pins THE SORT ITSELF — not a hand-reimplementation of it that
/// could silently drift from the real comparator chain. See
/// `hand_stated_collision_discriminates_by_member_case_insensitively`
/// (⟨task-4-review.md finding I-2⟩): the previous coverage of this chain's
/// secondary key exercised `case_insensitive_compare_opt` in isolation, so
/// deleting its `.then_with` call from the sort left every test green.
fn inventory_row_cmp(
    a: &(String, Option<String>, Option<String>, CborValue),
    b: &(String, Option<String>, Option<String>, CborValue),
) -> std::cmp::Ordering {
    locale_compare(&a.0, &b.0)
        .then_with(|| case_insensitive_compare_opt(&a.1, &b.1))
        .then_with(|| match (&a.2, &b.2) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => locale_compare(x, y),
        })
}

fn build_inventory_doc(
    tree: &CborValue,
    resolved: &Model,
    driver_version: &str,
    deterministic: bool,
) -> CborValue {
    let CborValue::Map(snap) = tree else {
        return CborValue::Null;
    };

    // Extract the shared sub-values verbatim from the full snapshot CborValue,
    // mirroring build_envelope's key-lift pattern (apps/identities/coverage/
    // rootClassifications are in the snapshot map, not the envelope wrapper).
    // A missing key defaults to `Null` — NOT `[]`: these keys are always composed
    // today, but `coverage` is an object, so an `[]` fallback would be a shape lie.
    // `Null` fails honestly if a future snapshot-map change drops a key.
    let get = |key: &str| -> CborValue { snap.get(key).cloned().unwrap_or(CborValue::Null) };
    let apps = get("apps");
    let identities = get("identities");
    let coverage = get("coverage");
    let root_classifications = get("rootClassifications");

    // Per-routine inventory: every source routine → (objectType, objectNumber,
    // routineName, [enclosingMember], [originatingObject], stableRoutineId). The
    // two member fields (engine-e2, schema 1.1.0) are emitted only for member-
    // trigger routines (field/control/action/dataitem trigger). Sort is three-key
    // (RE-6): primary stableRoutineId (locale_compare) → secondary enclosingMember
    // (case-insensitive; None first) → tertiary originatingObject (locale_compare).
    // ⟨task 4⟩ The primary key now separates two field triggers of one object on
    // its own (the member is folded into the stable id); the secondary/tertiary
    // keys remain as fail-closed cover for the residual duplicate-stableRoutineId
    // shapes — see `inventory_row_cmp` (the sort below) and `case_insensitive_compare_opt`.
    let mut routine_rows: Vec<(String, Option<String>, Option<String>, CborValue)> = resolved
        .workspace
        .routines
        .iter()
        .map(|r| {
            let mut m: IndexMap<String, CborValue> = IndexMap::new();
            m.insert("objectType".into(), CborValue::Text(r.object_type.clone()));
            m.insert("objectNumber".into(), CborValue::Int(r.object_number));
            m.insert("routineName".into(), CborValue::Text(r.name.clone()));
            if let Some(member) = &r.enclosing_member {
                m.insert("enclosingMember".into(), CborValue::Text(member.clone()));
            }
            if let Some(obj) = &r.originating_object {
                m.insert("originatingObject".into(), CborValue::Text(obj.clone()));
            }
            m.insert(
                "stableRoutineId".into(),
                CborValue::Text(r.stable_routine_id.clone()),
            );
            (
                r.stable_routine_id.clone(),
                r.enclosing_member.clone(),
                r.originating_object.clone(),
                CborValue::Map(m),
            )
        })
        .collect();
    routine_rows.sort_by(inventory_row_cmp);
    let routine_inventory =
        CborValue::Array(routine_rows.into_iter().map(|(_, _, _, v)| v).collect());

    let generated_at = crate::engine::gate::format_json::pinned_or_now_iso8601(deterministic);

    // Build the lean payload (omits all heavy keys).
    let mut payload: IndexMap<String, CborValue> = IndexMap::new();
    payload.insert("analysisUniverse".into(), get("analysisUniverse"));
    payload.insert("apps".into(), apps);
    payload.insert("identities".into(), identities);
    payload.insert("routineInventory".into(), routine_inventory);
    payload.insert("coverage".into(), coverage);
    payload.insert("rootClassifications".into(), root_classifications);

    // Build the envelope (same shape as the capability-snapshot envelope but
    // with a different kind + schemaVersion).
    let mut env: IndexMap<String, CborValue> = IndexMap::new();
    env.insert("kind".into(), CborValue::Text("routine-inventory".into()));
    env.insert(
        "schemaVersion".into(),
        CborValue::Text(INVENTORY_SCHEMA_VERSION.to_string()),
    );
    env.insert(
        "alsemVersion".into(),
        CborValue::Text(driver_version.to_string()),
    );
    env.insert("deterministic".into(), CborValue::Bool(deterministic));
    env.insert("generatedAt".into(), CborValue::Text(generated_at));
    env.insert("diagnostics".into(), CborValue::Array(Vec::new()));
    env.insert("payload".into(), CborValue::Map(payload));
    CborValue::Map(env)
}

// ===========================================================================
// Sharding (snapshot/shard.ts). Per-app shard files + manifest.json. Source-only:
// the single primary app → one `primary.<ext>` shard. format "json" only for the
// committed goldens.
// ===========================================================================

/// One sharded output file (name → bytes).
pub struct ShardFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Split the snapshot into per-app shards + a manifest. `primary_only` drops
/// dependency shards. The committed goldens use the `json` format; each shard is a
/// raw-JSON `CapabilitySnapshot` that inherits `generatedAt` from `tree` (so the
/// determinism mode is ALREADY baked into the tree by `compose_full_snapshot` — no
/// separate `deterministic` arg here). The manifest carries no diagnostics channel,
/// so it takes none either.
pub fn serialize_sharded(
    tree: &CborValue,
    driver_version: &str,
    primary_only: bool,
) -> Vec<ShardFile> {
    let mut out: Vec<ShardFile> = Vec::new();

    let CborValue::Map(snap) = tree else {
        return out;
    };
    let apps: Vec<CborValue> = match snap.get("apps") {
        Some(CborValue::Array(a)) => a.clone(),
        _ => Vec::new(),
    };

    if apps.is_empty() {
        out.push(ShardFile {
            name: "manifest.json".into(),
            bytes: encode_manifest(snap, driver_version, &[]).into_bytes(),
        });
        return out;
    }

    let app_guid = |app: &CborValue| -> String {
        match app {
            CborValue::Map(m) => match m.get("appGuid") {
                Some(CborValue::Text(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        }
    };

    let primary_guid = app_guid(&apps[0]);
    let mut shard_entries: Vec<(String, String, String)> = Vec::new(); // (appGuid, role, file)

    for app in &apps {
        let guid = app_guid(app);
        let role = if guid == primary_guid {
            "primary"
        } else {
            "dependency"
        };
        if primary_only && role != "primary" {
            continue;
        }
        let shard = slice_for_app(snap, &guid);
        let file_base = if role == "primary" {
            "primary".to_string()
        } else {
            format!("dep-{guid}")
        };
        let file_name = format!("{file_base}.json");
        // JSON format: serialize the shard snapshot as raw sorted JSON.
        let bytes = to_sorted_json(&CborValue::Map(shard)).into_bytes();
        out.push(ShardFile {
            name: file_name.clone(),
            bytes,
        });
        shard_entries.push((guid, role.to_string(), file_name));
    }

    out.push(ShardFile {
        name: "manifest.json".into(),
        bytes: encode_manifest(snap, driver_version, &shard_entries).into_bytes(),
    });
    out
}

/// Narrow the snapshot to a single app's facts (mirrors `sliceForApp`). A fact
/// belongs to the shard whose appGuid prefixes its stableId (`${appGuid}:`).
fn slice_for_app(
    snap: &IndexMap<String, CborValue>,
    app_guid: &str,
) -> IndexMap<String, CborValue> {
    let prefix = format!("{app_guid}:");
    let in_app = |s: &str| s.starts_with(&prefix);

    // Preserve the compose key order for the shard (it's a CapabilitySnapshot).
    // NOTE: al-sem `sliceForApp` builds a fresh `CapabilitySnapshot` literal that
    // does NOT include `routineOrderFrames` — the shard omits it. `inputsMetadata`
    // IS mirrored (side-band, workspace-scoped). So we SKIP routineOrderFrames here.
    let mut shard: IndexMap<String, CborValue> = IndexMap::new();
    for (k, v) in snap {
        if k == "routineOrderFrames" {
            continue;
        }
        let narrowed = match k.as_str() {
            // Workspace-scoped fields copied verbatim.
            "schemaVersion"
            | "alsemVersion"
            | "workspaceFingerprint"
            | "generatedAt"
            | "inputs"
            | "analysisGaps"
            | "inputsMetadata" => v.clone(),
            "apps" => {
                // Keep only this app.
                if let CborValue::Array(arr) = v {
                    CborValue::Array(
                        arr.iter()
                            .filter(|a| match a {
                                CborValue::Map(m) => {
                                    matches!(m.get("appGuid"), Some(CborValue::Text(g)) if g == app_guid)
                                }
                                _ => false,
                            })
                            .cloned()
                            .collect(),
                    )
                } else {
                    v.clone()
                }
            }
            "identities" => slice_identities(v, &prefix),
            "contractFacts" | "schemaFacts" => filter_array_by_field(v, "stableId", &in_app),
            "permissionFacts" => filter_permission_facts(v, &in_app),
            "rootClassifications" => filter_array_by_field(v, "routineId", &in_app),
            "capabilityFacts" => filter_array_by_field(v, "subject", &in_app),
            "typedEdges" => filter_typed_edges(v, &in_app),
            "operationIndex" | "callsiteIndex" => filter_array_by_field(v, "routine", &in_app),
            "callsiteResolutions" => filter_array_by_field(v, "from", &in_app),
            "coverage" => filter_array_by_field(v, "subject", &in_app),
            "eventDeclarations" => filter_event_declarations(v, &in_app),
            _ => v.clone(),
        };
        shard.insert(k.clone(), narrowed);
    }
    shard
}

fn slice_identities(v: &CborValue, prefix: &str) -> CborValue {
    let CborValue::Map(m) = v else {
        return v.clone();
    };
    let stable_ids = match m.get("stableIds") {
        Some(CborValue::Array(a)) => a,
        _ => return v.clone(),
    };
    let display_names = match m.get("displayNames") {
        Some(CborValue::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let mut out_ids: Vec<CborValue> = Vec::new();
    let mut out_names: Vec<CborValue> = Vec::new();
    for (i, id) in stable_ids.iter().enumerate() {
        if let CborValue::Text(s) = id
            && s.starts_with(prefix)
        {
            out_ids.push(id.clone());
            out_names.push(
                display_names
                    .get(i)
                    .cloned()
                    .unwrap_or(CborValue::Text(String::new())),
            );
        }
    }
    let mut out: IndexMap<String, CborValue> = IndexMap::new();
    out.insert("stableIds".into(), CborValue::Array(out_ids));
    out.insert("displayNames".into(), CborValue::Array(out_names));
    CborValue::Map(out)
}

fn filter_array_by_field(v: &CborValue, field: &str, in_app: &dyn Fn(&str) -> bool) -> CborValue {
    let CborValue::Array(arr) = v else {
        return v.clone();
    };
    CborValue::Array(
        arr.iter()
            .filter(|item| match item {
                CborValue::Map(m) => match m.get(field) {
                    Some(CborValue::Text(s)) => in_app(s),
                    _ => false,
                },
                _ => false,
            })
            .cloned()
            .collect(),
    )
}

fn filter_permission_facts(v: &CborValue, in_app: &dyn Fn(&str) -> bool) -> CborValue {
    let CborValue::Array(arr) = v else {
        return v.clone();
    };
    CborValue::Array(
        arr.iter()
            .filter(|item| match item {
                CborValue::Map(m) => {
                    let kind = match m.get("kind") {
                        Some(CborValue::Text(s)) => s.as_str(),
                        _ => "",
                    };
                    let field = if kind == "declared" {
                        "permissionSet"
                    } else {
                        "subject"
                    };
                    match m.get(field) {
                        Some(CborValue::Text(s)) => in_app(s),
                        _ => false,
                    }
                }
                _ => false,
            })
            .cloned()
            .collect(),
    )
}

fn filter_typed_edges(v: &CborValue, in_app: &dyn Fn(&str) -> bool) -> CborValue {
    let CborValue::Array(arr) = v else {
        return v.clone();
    };
    CborValue::Array(
        arr.iter()
            .filter(|item| match item {
                CborValue::Map(m) => {
                    let from_ok = matches!(m.get("from"), Some(CborValue::Text(s)) if in_app(s));
                    let to_ok = matches!(m.get("to"), Some(CborValue::Text(s)) if in_app(s));
                    from_ok || to_ok
                }
                _ => false,
            })
            .cloned()
            .collect(),
    )
}

fn filter_event_declarations(v: &CborValue, in_app: &dyn Fn(&str) -> bool) -> CborValue {
    let CborValue::Array(arr) = v else {
        return v.clone();
    };
    CborValue::Array(
        arr.iter()
            .filter(|item| match item {
                CborValue::Map(m) => {
                    let routine_ok =
                        matches!(m.get("routine"), Some(CborValue::Text(s)) if in_app(s));
                    let binding_ok = match m.get("binding") {
                        Some(CborValue::Map(b)) => {
                            matches!(b.get("publisherObject"), Some(CborValue::Text(s)) if in_app(s))
                        }
                        _ => false,
                    };
                    routine_ok || binding_ok
                }
                _ => false,
            })
            .cloned()
            .collect(),
    )
}

/// Encode the shard manifest (kind snapshot-shard-manifest). Field order:
/// kind, schemaVersion, alsemVersion, workspaceFingerprint, shards (sorted by
/// appGuid). Per-shard: appGuid, role, file. Serialized via `JSON.stringify(m,
/// null, 2)` — NOT sorted (al-sem uses the default replacer here).
fn encode_manifest(
    snap: &IndexMap<String, CborValue>,
    driver_version: &str,
    shards: &[(String, String, String)],
) -> String {
    let workspace_fingerprint = match snap.get("workspaceFingerprint") {
        Some(CborValue::Text(s)) => s.clone(),
        _ => String::new(),
    };
    let mut sorted = shards.to_vec();
    // al-sem shard manifest: `.sort((a,b) => a.appGuid.localeCompare(b.appGuid))`.
    sorted.sort_by(|a, b| locale_compare(&a.0, &b.0));

    let mut m: IndexMap<String, CborValue> = IndexMap::new();
    m.insert(
        "kind".into(),
        CborValue::Text("snapshot-shard-manifest".into()),
    );
    m.insert(
        "schemaVersion".into(),
        CborValue::Int(SNAPSHOT_SCHEMA_VERSION),
    );
    m.insert(
        "alsemVersion".into(),
        CborValue::Text(driver_version.to_string()),
    );
    m.insert(
        "workspaceFingerprint".into(),
        CborValue::Text(workspace_fingerprint),
    );
    let shard_arr: Vec<CborValue> = sorted
        .into_iter()
        .map(|(guid, role, file)| {
            let mut sm: IndexMap<String, CborValue> = IndexMap::new();
            sm.insert("appGuid".into(), CborValue::Text(guid));
            sm.insert("role".into(), CborValue::Text(role));
            sm.insert("file".into(), CborValue::Text(file));
            CborValue::Map(sm)
        })
        .collect();
    m.insert("shards".into(), CborValue::Array(shard_arr));

    // INSERTION-order pretty JSON (the manifest uses the default JSON.stringify
    // replacer — NOT sorted keys).
    let mut s = String::new();
    write_insertion_json(&CborValue::Map(m), 0, &mut s);
    s.push('\n');
    s
}

/// Pretty JSON preserving INSERTION key order (manifest.json uses the default
/// `JSON.stringify(m, null, 2)` — no sorting).
fn write_insertion_json(v: &CborValue, indent: usize, out: &mut String) {
    match v {
        CborValue::Null | CborValue::Undefined => out.push_str("null"),
        CborValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        CborValue::Int(n) => out.push_str(&n.to_string()),
        CborValue::Float(f) => out.push_str(&json_number(*f)),
        CborValue::Text(s) => out.push_str(&json_string(s)),
        CborValue::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            let pad = "  ".repeat(indent + 1);
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad);
                write_insertion_json(item, indent + 1, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push(']');
        }
        CborValue::Map(entries) => {
            if entries.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push_str("{\n");
            let pad = "  ".repeat(indent + 1);
            let n = entries.len();
            for (i, (k, val)) in entries.iter().enumerate() {
                out.push_str(&pad);
                out.push_str(&json_string(k));
                out.push_str(": ");
                write_insertion_json(val, indent + 1, out);
                if i + 1 < n {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(indent));
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #70 row 2e: one discovery for the loader, the closure digest and the
    /// fingerprint. Over a hand-built tree (a repository root holding an
    /// ancestor cache with `b.app`, the workspace below it with its own cache
    /// holding `a.app`), the `dep-package` input paths are exactly the closure
    /// keys: `.alpackages/a.app` (byte-equal to the pre-#70 own-cache path) and
    /// `../.alpackages/b.app`.
    ///
    /// Mutation: a second private walk that drifts (e.g. drops ancestors) ->
    /// `../.alpackages/b.app` missing.
    #[test]
    fn derive_inputs_uses_the_closure_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join(".alpackages")).unwrap();
        std::fs::write(repo.join(".alpackages").join("b.app"), b"ancestor package").unwrap();
        let ws = repo.join("ws");
        std::fs::create_dir_all(ws.join(".alpackages")).unwrap();
        std::fs::write(ws.join(".alpackages").join("a.app"), b"own package").unwrap();
        std::fs::write(ws.join("app.json"), b"{}").unwrap();

        let needs = crate::program::inputs::InputNeeds {
            fingerprint_rows: true,
            ..Default::default()
        };
        let inputs = crate::program::inputs::InputSet::read(&ws, &needs);
        let mut got: Vec<String> = snapshot_rows(&inputs)
            .iter()
            .filter(|i| i.kind == "dep-package")
            .map(|i| i.path.clone())
            .collect();
        got.sort();
        assert_eq!(
            got,
            // Byte order: `../` sorts before `.a`.
            vec![
                "../.alpackages/b.app".to_string(),
                ".alpackages/a.app".to_string()
            ]
        );
    }

    /// ⟨task 4⟩ The RE-6 member tie-break, pinned where it lives.
    ///
    /// The stable id now folds the enclosing member in, so the inventory's PRIMARY
    /// key separates two field triggers of one object by itself and no assembled
    /// fixture can drive this comparator's discriminating branch through
    /// `build_inventory_doc` any more. Its previous coverage
    /// (`cli_p1_inventory`'s two-field ordering assertion) therefore stopped
    /// testing it; this is the replacement, exercising the comparator directly so
    /// the tie-break kept for the residual duplicate-stableRoutineId shapes is not
    /// silently deletable.
    #[test]
    fn member_tie_break_is_case_insensitive_and_none_first() {
        use std::cmp::Ordering;
        let alpha = Some("alpha field".to_string());
        let bravo = Some("Bravo Field".to_string());
        // Case-insensitive: the lowercase `alpha field` still sorts before the
        // uppercase `Bravo Field` (an ordinal compare would put `B` first).
        assert_eq!(case_insensitive_compare_opt(&alpha, &bravo), Ordering::Less);
        assert_eq!(
            case_insensitive_compare_opt(&bravo, &alpha),
            Ordering::Greater
        );
        // Same member differing only in case is a TIE — which is exactly the
        // residual case (one member name, two XMLport nesting paths): the tertiary
        // originatingObject key decides, not this one.
        assert_eq!(
            case_insensitive_compare_opt(&Some("Ctry".to_string()), &Some("ctry".to_string())),
            Ordering::Equal
        );
        // `None` (an object-level trigger / procedure) orders first.
        assert_eq!(case_insensitive_compare_opt(&None, &alpha), Ordering::Less);
        assert_eq!(
            case_insensitive_compare_opt(&alpha, &None),
            Ordering::Greater
        );
        assert_eq!(case_insensitive_compare_opt(&None, &None), Ordering::Equal);
    }

    /// ⟨task-4-review.md finding I-2⟩ The test above pins `case_insensitive_compare_opt`
    /// as a function — it does NOT prove `build_inventory_doc`'s actual `sort_by` still
    /// calls it. Concretely: deleting `.then_with(|| case_insensitive_compare_opt(&a.1,
    /// &b.1))` from the sort left that test (and everything else, including every
    /// golden) green. This test closes the gap the way
    /// `hand_stated_id_collision_keeps_a_real_summary_and_derived_row`
    /// (`detector_context.rs`) closed the analogous gap for the drained-map skip: it
    /// does not ask the id schema for a collision (two real member triggers now get
    /// DISTINCT stable ids — that is this whole arc's point) — it STATES one, by
    /// direct field assignment after assembly, then runs the real
    /// `build_inventory_doc` end to end and asserts on its actual output order.
    #[test]
    fn hand_stated_collision_discriminates_by_member_case_insensitively() {
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let src = r#"
page 50816 "T4 I-2 Wizard"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action("Bravo Field")
            {
                trigger OnAction()
                begin
                end;
            }
            action("alpha field")
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}
"#;
        let mut resolved = assemble_and_resolve_inline_program_default(
            &[("T4I2Wizard.Page.al".to_string(), src.to_string())],
            "66666666-0000-0000-0000-0000000cp006",
        );
        assert_eq!(
            resolved.workspace.routines.len(),
            2,
            "fixture precondition: exactly the two OnAction triggers"
        );

        // Force the identical-stable-id collision by hand (real source gives these
        // two DISTINCT stable ids under the member discriminator — that is correct
        // and is what makes the collision this comparator guards against otherwise
        // unreachable). Also pin the pre-sort Vec order to "Bravo Field" before
        // "alpha field" — the ordinally-WRONG order — so a working case-insensitive
        // tie-break must visibly swap them, and a `sort_by` that silently drops the
        // tie-break (a stable sort) would leave them in this same wrong order.
        let forced_id = resolved.workspace.routines[0].stable_routine_id.clone();
        for r in &mut resolved.workspace.routines {
            r.stable_routine_id = forced_id.clone();
        }
        let bravo_idx = resolved
            .workspace
            .routines
            .iter()
            .position(|r| r.enclosing_member.as_deref() == Some("Bravo Field"))
            .expect("fixture precondition: a 'Bravo Field' OnAction trigger");
        let alpha_idx = resolved
            .workspace
            .routines
            .iter()
            .position(|r| r.enclosing_member.as_deref() == Some("alpha field"))
            .expect("fixture precondition: an 'alpha field' OnAction trigger");
        if alpha_idx < bravo_idx {
            resolved.workspace.routines.swap(alpha_idx, bravo_idx);
        }
        assert_eq!(
            resolved.workspace.routines[0].enclosing_member.as_deref(),
            Some("Bravo Field"),
            "precondition: 'Bravo Field' precedes 'alpha field' pre-sort"
        );

        let tree = CborValue::Map(IndexMap::new());
        let doc = build_inventory_doc(&tree, &resolved, "test", true);

        let rows = match &doc {
            CborValue::Map(env) => match env.get("payload") {
                Some(CborValue::Map(p)) => match p.get("routineInventory") {
                    Some(CborValue::Array(rows)) => rows,
                    other => panic!("routineInventory not an array: {other:?}"),
                },
                other => panic!("payload not a map: {other:?}"),
            },
            other => panic!("doc not a map: {other:?}"),
        };
        assert_eq!(rows.len(), 2);

        let member_of = |row: &CborValue| -> Option<String> {
            let CborValue::Map(m) = row else {
                panic!("row not a map: {row:?}")
            };
            match m.get("enclosingMember") {
                Some(CborValue::Text(s)) => Some(s.clone()),
                _ => None,
            }
        };
        let stable_id_of = |row: &CborValue| -> String {
            let CborValue::Map(m) = row else {
                panic!("row not a map: {row:?}")
            };
            match m.get("stableRoutineId") {
                Some(CborValue::Text(s)) => s.clone(),
                other => panic!("missing stableRoutineId: {other:?}"),
            }
        };

        assert_eq!(stable_id_of(&rows[0]), forced_id);
        assert_eq!(stable_id_of(&rows[1]), forced_id);
        // The discriminating assertion: case-insensitive fold puts "alpha field"
        // first even though it was declared AFTER "Bravo Field" and an ordinal
        // compare ('B' = 0x42 < 'a' = 0x61) would keep "Bravo Field" first.
        assert_eq!(
            member_of(&rows[0]),
            Some("alpha field".to_string()),
            "case-insensitive secondary key must place 'alpha field' before 'Bravo Field'"
        );
        assert_eq!(member_of(&rows[1]), Some("Bravo Field".to_string()));
    }

    /// ⟨review-branch-final I-3⟩ The test above proves the SECONDARY key
    /// (`enclosingMember`) discriminates; it never reaches the TERTIARY key
    /// (`originatingObject`) at all — that arm needs two rows agreeing on BOTH
    /// `stableRoutineId` AND `enclosingMember`, which the test above does not
    /// construct (it forces only the stable-id collision, then relies on the
    /// members differing). No assembled fixture reaches it either: the member
    /// discriminator already gives same-member, same-object triggers distinct
    /// stable ids. This is the same hand-stated pattern one key further: two
    /// DIFFERENT pages each declare an action with the SAME name, so
    /// `enclosingMember` ("Shared Action") agrees BY CONSTRUCTION — no forcing
    /// needed for that part — while `originatingObject` (each page's own
    /// StableObjectId) differs naturally. Only `stableRoutineId` is forced.
    /// Deleting `inventory_row_cmp`'s tertiary `.then_with` arm must fail this
    /// test (verified by removal, not asserted).
    #[test]
    fn hand_stated_collision_discriminates_by_originating_object_when_member_also_ties() {
        use crate::program::model::program_calls::assemble_and_resolve_inline_program_default;

        let files = vec![
            (
                "T5I3WizardBravo.Page.al".to_string(),
                r#"
page 50817 "T5 I-3 Wizard Bravo"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action("Shared Action")
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}
"#
                .to_string(),
            ),
            (
                "T5I3WizardAlpha.Page.al".to_string(),
                r#"
page 50818 "T5 I-3 Wizard Alpha"
{
    PageType = Card;

    actions
    {
        area(Processing)
        {
            action("Shared Action")
            {
                trigger OnAction()
                begin
                end;
            }
        }
    }
}
"#
                .to_string(),
            ),
        ];
        let mut resolved = assemble_and_resolve_inline_program_default(
            &files,
            "66666666-0000-0000-0000-0000000cp007",
        );
        assert_eq!(
            resolved.workspace.routines.len(),
            2,
            "fixture precondition: exactly the two OnAction triggers"
        );

        // Preconditions the whole test rests on: enclosingMember agrees without any
        // forcing (both actions are literally named "Shared Action"); originatingObject
        // does not (real per-page StableObjectId) — exactly what makes the tertiary
        // key reachable at all.
        assert_eq!(
            resolved.workspace.routines[0].enclosing_member,
            resolved.workspace.routines[1].enclosing_member,
            "fixture precondition: both actions share the member name 'Shared Action'"
        );
        assert_ne!(
            resolved.workspace.routines[0].originating_object,
            resolved.workspace.routines[1].originating_object,
            "fixture precondition: the two pages have distinct StableObjectIds"
        );

        // Force the identical-stable-id collision by hand (real source gives these
        // two DISTINCT stable ids — different objects, different object numbers —
        // which is correct; the collision is otherwise unreachable here too).
        let forced_id = resolved.workspace.routines[0].stable_routine_id.clone();
        for r in &mut resolved.workspace.routines {
            r.stable_routine_id = forced_id.clone();
        }

        // Determine the real comparator's order over the two originatingObject
        // values, then pin the pre-sort Vec to the OPPOSITE order — so a working
        // tertiary key must visibly swap them, and a `sort_by` that silently drops
        // the tertiary arm (a stable sort) would leave them in this same wrong order.
        let (obj0, obj1) = (
            resolved.workspace.routines[0].originating_object.clone(),
            resolved.workspace.routines[1].originating_object.clone(),
        );
        let correct_first = match (&obj0, &obj1) {
            (Some(x), Some(y)) if locale_compare(x, y) == std::cmp::Ordering::Greater => 1,
            _ => 0,
        };
        // Pre-sort Vec order: put the correct-SECOND row first (the wrong order).
        if correct_first == 0 {
            resolved.workspace.routines.swap(0, 1);
        }

        let tree = CborValue::Map(IndexMap::new());
        let doc = build_inventory_doc(&tree, &resolved, "test", true);

        let rows = match &doc {
            CborValue::Map(env) => match env.get("payload") {
                Some(CborValue::Map(p)) => match p.get("routineInventory") {
                    Some(CborValue::Array(rows)) => rows,
                    other => panic!("routineInventory not an array: {other:?}"),
                },
                other => panic!("payload not a map: {other:?}"),
            },
            other => panic!("doc not a map: {other:?}"),
        };
        assert_eq!(rows.len(), 2);

        let stable_id_of = |row: &CborValue| -> String {
            let CborValue::Map(m) = row else {
                panic!("row not a map: {row:?}")
            };
            match m.get("stableRoutineId") {
                Some(CborValue::Text(s)) => s.clone(),
                other => panic!("missing stableRoutineId: {other:?}"),
            }
        };
        let originating_object_of = |row: &CborValue| -> Option<String> {
            let CborValue::Map(m) = row else {
                panic!("row not a map: {row:?}")
            };
            match m.get("originatingObject") {
                Some(CborValue::Text(s)) => Some(s.clone()),
                _ => None,
            }
        };

        assert_eq!(stable_id_of(&rows[0]), forced_id);
        assert_eq!(stable_id_of(&rows[1]), forced_id);
        let (expected_first, expected_second) = if correct_first == 0 {
            (&obj0, &obj1)
        } else {
            (&obj1, &obj0)
        };
        assert_eq!(
            &originating_object_of(&rows[0]),
            expected_first,
            "tertiary originatingObject key must place the locale-lesser StableObjectId first"
        );
        assert_eq!(&originating_object_of(&rows[1]), expected_second);
    }

    /// #96 review CR4: a fact inherited through an `implicit-trigger` first hop
    /// has no call site, and its witness is the firing op. The CBOR fact
    /// carries `witnessFirstHopOperationId` last, as the JSON snapshot does, and
    /// a permission row derived from it carries it in `derivedFromCapability`.
    #[test]
    fn trigger_first_fact_keeps_its_first_hop_op_in_cbor_and_permissions() {
        let op = "app/caller/op3".to_string();
        let snap = crate::engine::l5::snapshot::SnapshotCapabilityFact {
            subject: "s".to_string(),
            op: "insert".to_string(),
            resource_kind: "table".to_string(),
            resource_id: Some("app/table/1".to_string()),
            resource_arg_source: None,
            confidence: "high".to_string(),
            provenance: "inherited".to_string(),
            via: "OnValidate".to_string(),
            witness_operation_id: Some("app/trigger/op0".to_string()),
            witness_callsite_id: None,
            extra: None,
            witness_first_hop_operation_id: Some(op.clone()),
            witness_first_hop_event_id: None,
        };
        let CborValue::Map(m) = one_capability_fact_to_cbor(&snap) else {
            panic!("a fact is a map");
        };
        assert!(
            matches!(
                m.last(),
                Some((k, CborValue::Text(v))) if k == "witnessFirstHopOperationId" && *v == op
            ),
            "last key: {:?}",
            m.last()
        );

        let fact = CapabilityFact {
            subject: "s".to_string(),
            op: "insert",
            resource_kind: "table",
            resource_id: Some("app/table/1".to_string()),
            resource_arg_source: None,
            confidence: "high",
            provenance: "inherited",
            via: "OnValidate",
            witness_operation_id: Some("app/trigger/op0".to_string()),
            witness_callsite_id: None,
            witness_first_hop_operation_id: Some(op.clone()),
            witness_first_hop_event_id: None,
            extra: None,
        };
        let (_, row) = make_required_perm("s", "t", "TableData", "I", &fact, "complete");
        let CborValue::Map(row) = row else {
            panic!("a row is a map");
        };
        let Some(CborValue::Map(dfc)) = row.get("derivedFromCapability") else {
            panic!("derivedFromCapability is a map");
        };
        assert!(
            matches!(dfc.get("witnessFirstHopOperationId"), Some(CborValue::Text(v)) if *v == op),
            "derivedFromCapability: {dfc:?}"
        );
        assert!(!dfc.contains_key("witnessCallsiteId"));
        assert!(!dfc.contains_key("witnessFirstHopEventId"));
        assert!(!m.contains_key("witnessFirstHopEventId"));
    }

    /// #98 A4: a fact inherited through an `event-dispatch` first hop has no
    /// call site and no op; its witness is the dispatched event. The CBOR fact
    /// carries `witnessFirstHopEventId` last, and a permission row derived from
    /// it carries it in `derivedFromCapability`. A call-first fact writes
    /// neither key.
    #[test]
    fn event_first_fact_keeps_its_first_hop_event_in_cbor_and_permissions() {
        let ev = "app/Codeunit/1/event/onx".to_string();
        let snap_fact = |first_hop_event: Option<String>, callsite: Option<String>| {
            crate::engine::l5::snapshot::SnapshotCapabilityFact {
                subject: "s".to_string(),
                op: "insert".to_string(),
                resource_kind: "table".to_string(),
                resource_id: Some("app/table/1".to_string()),
                resource_arg_source: None,
                confidence: "high".to_string(),
                provenance: "inherited".to_string(),
                via: "event-dispatch".to_string(),
                witness_operation_id: Some("app/sub/op0".to_string()),
                witness_callsite_id: callsite,
                extra: None,
                witness_first_hop_operation_id: None,
                witness_first_hop_event_id: first_hop_event,
            }
        };
        let CborValue::Map(m) = one_capability_fact_to_cbor(&snap_fact(Some(ev.clone()), None))
        else {
            panic!("a fact is a map");
        };
        assert!(
            matches!(
                m.last(),
                Some((k, CborValue::Text(v))) if k == "witnessFirstHopEventId" && *v == ev
            ),
            "last key: {:?}",
            m.last()
        );
        let CborValue::Map(m) =
            one_capability_fact_to_cbor(&snap_fact(None, Some("app/caller/cs0".to_string())))
        else {
            panic!("a fact is a map");
        };
        assert!(
            !m.contains_key("witnessFirstHopEventId"),
            "call fact: {m:?}"
        );

        let fact = |first_hop_event: Option<String>, callsite: Option<String>| CapabilityFact {
            subject: "s".to_string(),
            op: "insert",
            resource_kind: "table",
            resource_id: Some("app/table/1".to_string()),
            resource_arg_source: None,
            confidence: "high",
            provenance: "inherited",
            via: "event-dispatch",
            witness_operation_id: Some("app/sub/op0".to_string()),
            witness_callsite_id: callsite,
            witness_first_hop_operation_id: None,
            witness_first_hop_event_id: first_hop_event,
            extra: None,
        };
        let dfc_of = |f: &CapabilityFact| {
            let (_, row) = make_required_perm("s", "t", "TableData", "I", f, "complete");
            let CborValue::Map(mut row) = row else {
                panic!("a row is a map");
            };
            let Some(CborValue::Map(dfc)) = row.shift_remove("derivedFromCapability") else {
                panic!("derivedFromCapability is a map");
            };
            dfc
        };
        let dfc = dfc_of(&fact(Some(ev.clone()), None));
        assert!(
            matches!(dfc.get("witnessFirstHopEventId"), Some(CborValue::Text(v)) if *v == ev),
            "derivedFromCapability: {dfc:?}"
        );
        assert!(!dfc.contains_key("witnessCallsiteId"));
        assert!(!dfc.contains_key("witnessFirstHopOperationId"));
        let dfc = dfc_of(&fact(None, Some("app/caller/cs0".to_string())));
        assert!(
            !dfc.contains_key("witnessFirstHopEventId"),
            "call fact derivedFromCapability: {dfc:?}"
        );
    }
}
