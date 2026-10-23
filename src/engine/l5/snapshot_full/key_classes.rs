//! #220 (spec §4.4 D4): key classes, the typed cross-app filter and the
//! reference closure. [`REFERENCES`] is the one statement of which fields
//! name which rows: the filter keeps rows through it and the closure check
//! verifies the result through it.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::{
    SnapshotUniverse, capability_facts_to_cbor, stable_table_from_internal, to_cbor_value,
};
use crate::engine::gate::cbor::CborValue;
use crate::engine::l4::capability_cone::routine_to_stable_of;
use crate::engine::l5::snapshot::CapabilitySnapshot;
use crate::program::model::role::{Role, role_of_stable_id};
use crate::program::model::workspace::Model;

/// The rule that decides which rows of one top-level snapshot key are
/// serialized cross-app (spec §4.4, Decision 13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyClass {
    /// As is.
    Header,
    /// As is.
    Input,
    /// Rows whose subject is primary.
    Inventory,
    /// Rows whose subject is primary (`routineOrderFrames` also keeps the
    /// frames of every kept Evidence row's routine).
    Behaviour,
    /// D2: primary, or a dependency subject reachable over the cone.
    Gap,
    /// Rows of a primary routine plus every row a kept row references.
    Evidence,
}

/// Every top-level key, in insertion order, with its class (spec §3.3, §4.4).
pub const SNAPSHOT_KEYS: &[(&str, KeyClass)] = &[
    ("schemaVersion", KeyClass::Header),
    ("alsemVersion", KeyClass::Header),
    ("workspaceFingerprint", KeyClass::Input),
    ("generatedAt", KeyClass::Header),
    ("analysisUniverse", KeyClass::Header),
    ("apps", KeyClass::Inventory),
    ("identities", KeyClass::Evidence),
    ("contractFacts", KeyClass::Inventory),
    ("schemaFacts", KeyClass::Inventory),
    ("permissionFacts", KeyClass::Inventory),
    ("rootClassifications", KeyClass::Inventory),
    ("capabilityFacts", KeyClass::Behaviour),
    ("typedEdges", KeyClass::Behaviour),
    ("operationIndex", KeyClass::Evidence),
    ("callsiteIndex", KeyClass::Evidence),
    ("callsiteResolutions", KeyClass::Behaviour),
    ("analysisGaps", KeyClass::Gap),
    ("coverage", KeyClass::Behaviour),
    ("inputs", KeyClass::Input),
    ("eventDeclarations", KeyClass::Inventory),
    ("inputsMetadata", KeyClass::Input),
    ("routineOrderFrames", KeyClass::Behaviour),
];

/// The domain a reference field's value names (spec §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// An `identities` row.
    Identity,
    /// An `apps[]` row (a bare app guid, case-insensitive).
    App,
    /// An `operationIndex` row by its `operationId`, verbatim.
    Operation,
    /// A `callsiteIndex` row by its `callsiteId`, verbatim.
    Callsite,
    /// A frame of the referring row's routine's `routineOrderFrames` table.
    Frame,
    /// A `typedEdges` row by its `edgeId`, verbatim.
    Edge,
}

use Domain::{App, Callsite, Edge, Frame, Identity, Operation};

/// A value source's table (spec §4.4 "Value source"): only a `table-field`
/// source has a `tableId`, and a `constant-var` source nests another source
/// under `initializer`, so the bare name, matched at any depth, reads both.
const TABLE_ID: ReferenceField = ("tableId", Identity);

/// A reference field: its path and the domain its value names.
pub type ReferenceField = (&'static str, Domain);

/// One key's entry of [`REFERENCES`]: key, defining field, reference fields.
pub type KeyReferences = (
    &'static str,
    Option<&'static str>,
    &'static [ReferenceField],
);

/// Per key: its defining field and its reference fields with their domains
/// (spec §4.4). Field paths are dotted, relative to the row, array indices
/// dropped; `*` matches one segment (or, as the LAST segment, the rest of the
/// path), and inside a segment it globs (`*Anchor`); an entry without a dot or
/// `*` also matches that field name at any depth (a value source's `tableId`).
/// A field listed with two domains (`analysisGaps.subject`) is decided per
/// value by its shape: a bare guid is an app, anything else an identity (F24).
/// `routineOrderFrames` rows are its frames (its map key also defines it).
pub const REFERENCES: &[KeyReferences] = &[
    ("apps", Some("appGuid"), &[]),
    ("identities", Some("stableIds"), &[]),
    ("contractFacts", Some("stableId"), &[]),
    ("schemaFacts", Some("stableId"), &[]),
    (
        "permissionFacts",
        None,
        &[
            ("subject", Identity),
            ("target", Identity),
            ("permissionSet", Identity),
            ("derivedFromCapability.witnessCallsiteId", Callsite),
            (
                "derivedFromCapability.witnessFirstHopOperationId",
                Operation,
            ),
            ("derivedFromCapability.witnessFirstHopEventId", Identity),
        ],
    ),
    (
        "rootClassifications",
        None,
        &[
            ("routineId", Identity),
            ("sourceAnchor.enclosingRoutineId", Identity),
        ],
    ),
    (
        "eventDeclarations",
        None,
        &[
            ("routine", Identity),
            ("eventId", Identity),
            ("binding.publisherObject", Identity),
            ("sourceAnchor.enclosingRoutineId", Identity),
        ],
    ),
    (
        "capabilityFacts",
        None,
        &[
            ("subject", Identity),
            ("resourceId", Identity),
            ("witnessFirstHopEventId", Identity),
            ("witnessOperationId", Operation),
            ("witnessFirstHopOperationId", Operation),
            ("witnessCallsiteId", Callsite),
            TABLE_ID,
        ],
    ),
    (
        "coverage",
        None,
        &[("subject", Identity), ("unknownTargets", Identity)],
    ),
    (
        "callsiteResolutions",
        None,
        &[
            ("from", Identity),
            ("candidates", Identity),
            ("unresolvedCandidates.objectId", Identity),
            ("callsiteId", Callsite),
            ("resolvedEdges", Edge),
        ],
    ),
    (
        "typedEdges",
        Some("edgeId"),
        &[
            ("from", Identity),
            ("to", Identity),
            ("eventId", Identity),
            ("targetObject", Identity),
            ("*Anchor.enclosingRoutineId", Identity),
            ("callsiteId", Callsite),
            ("operationId", Operation),
            TABLE_ID,
        ],
    ),
    (
        "analysisGaps",
        None,
        &[("subject", Identity), ("subject", App)],
    ),
    (
        "operationIndex",
        Some("operationId"),
        &[("routine", Identity), ("order.frameId", Frame)],
    ),
    (
        "callsiteIndex",
        Some("callsiteId"),
        &[("routine", Identity), ("order.frameId", Frame)],
    ),
    (
        "routineOrderFrames",
        Some("frameId"),
        &[("parentFrameId", Frame)],
    ),
];

/// Per key, the fields that are data, never references (spec §4.4, ON3),
/// matched as [`REFERENCES`]' paths are: a bare name matches at any depth, but
/// only inside its own key. Completed from the serde names of every row the
/// composer emits (`snapshot.rs` row structs, `snapshot_full.rs` derivers),
/// numbers and booleans included; `closure_fields_are_classified_and_closed`
/// checks every leaf of DD's and R5B's trees against both lists.
/// Value sources nest (`initializer`), so their data names (`kind`, `value`,
/// `enumName`, `member`, `varName`, `fieldName`; a literal's `value` is data,
/// S9) stay bare in the two keys that carry them.
pub const NON_REFERENCE_FIELDS: &[(&str, &[&str])] = &[
    ("apps", &["publisher", "name", "version", "role"]),
    ("identities", &["displayNames"]),
    (
        "contractFacts",
        &[
            "kind",
            "visibility",
            "signatureFingerprint",
            "attributes.*",
            "obsoleteState",
            "obsoleteReason",
            "routineKind",
        ],
    ),
    ("schemaFacts", &["kind", "shapeFingerprint", "name"]),
    (
        "permissionFacts",
        &[
            "kind",
            "targetKind",
            "rights",
            "coverage",
            "derivedFromCapability.op",
        ],
    ),
    (
        "rootClassifications",
        &[
            "kinds",
            "source",
            "confidence",
            "configEntryId",
            "resolutionStatus",
            "externallyReachable",
            "sourceAnchor.sourceUnitId",
            "sourceAnchor.syntaxKind",
            "sourceAnchor.range.*",
        ],
    ),
    (
        "eventDeclarations",
        &[
            "kind",
            "binding.eventName",
            "sourceAnchor.sourceUnitId",
            "sourceAnchor.syntaxKind",
            "sourceAnchor.range.*",
        ],
    ),
    (
        "capabilityFacts",
        &[
            "op",
            "resourceKind",
            "confidence",
            "provenance",
            "via",
            "opSubtype",
            "recordVariableId",
            "extra.tempState.parameterIndex",
            "objectType",
            "modal",
            "eventClass",
            "includeSender",
            "method",
            "scope",
            "kind",
            "value",
            "enumName",
            "member",
            "index",
            "varName",
            "fieldName",
        ],
    ),
    (
        "typedEdges",
        &[
            "kind",
            "receiverType",
            "interfaceName",
            "candidateCount",
            "triggerKind",
            "fires",
            "objectType",
            "*Anchor.sourceUnitId",
            "*Anchor.syntaxKind",
            "*Anchor.range.*",
            "value",
            "enumName",
            "member",
            "index",
            "varName",
            "fieldName",
        ],
    ),
    (
        "operationIndex",
        &[
            "sourceFile",
            "startLine",
            "startColumn",
            "endLine",
            "endColumn",
            "displayText",
            "controlContext",
            "order.orderId",
            "order.onSuccessPath",
            "order.dominatesSuccessReturn",
            "underAsserterror",
        ],
    ),
    (
        "callsiteIndex",
        &[
            "sourceFile",
            "startLine",
            "startColumn",
            "endLine",
            "endColumn",
            "calleeDisplay",
            "controlContext",
            "order.orderId",
            "order.onSuccessPath",
            "order.dominatesSuccessReturn",
            "underAsserterror",
        ],
    ),
    (
        "callsiteResolutions",
        &[
            "calleeDisplay",
            "dispatchKind",
            "status",
            "openWorld",
            "unresolvedCandidates.reason",
            "resultConsumed",
            "underAsserterror",
        ],
    ),
    ("analysisGaps", &["kind", "detail", "role"]),
    ("coverage", &["directStatus", "inheritedStatus", "reasons"]),
    ("inputs", &["kind", "path", "contentHash"]),
    ("inputsMetadata", &["rootsConfigIgnored"]),
    (
        "routineOrderFrames",
        &[
            "kind",
            "branchAlwaysTerminates",
            "branchMayFallThrough",
            "branchTerminatesBy",
        ],
    ),
];

/// Whether `path` is a REFERENCE field of `key` (not its defining field).
#[doc(hidden)]
#[must_use]
pub fn is_reference<S: AsRef<str>>(key: &str, path: &[S]) -> bool {
    REFERENCES
        .iter()
        .filter(|(k, _, _)| *k == key)
        .any(|(_, _, refs)| refs.iter().any(|(f, _)| path_matches(f, path)))
}

/// Whether `path` of a `key` row is classified: its defining field, a
/// reference field or a data field of that key (T9-closure reads it).
#[doc(hidden)]
#[must_use]
pub fn is_classified<S: AsRef<str>>(key: &str, path: &[S]) -> bool {
    let defining = REFERENCES
        .iter()
        .filter(|(k, _, _)| *k == key)
        .any(|(_, def, _)| def.is_some_and(|d| path_matches(d, path)));
    defining
        || is_reference(key, path)
        || NON_REFERENCE_FIELDS
            .iter()
            .filter(|(k, _)| *k == key)
            .any(|(_, data)| data.iter().any(|f| path_matches(f, path)))
}

/// One reference of a serialized row that resolves to no serialized row of
/// its domain (spec §4.4 "Closure").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosureViolation {
    /// The top-level key of the referring row.
    pub key: String,
    /// The reference field's path inside the row.
    pub field: String,
    pub domain: Domain,
    pub value: String,
}

/// [`REFERENCES`]' path rule (its doc) against a row-relative path.
fn path_matches<S: AsRef<str>>(pattern: &str, path: &[S]) -> bool {
    let path: Vec<&str> = path.iter().map(AsRef::as_ref).collect();
    if !pattern.contains('.') && !pattern.contains('*') {
        return path.last() == Some(&pattern);
    }
    let pat: Vec<&str> = pattern.split('.').collect();
    let seg = |p: &str, s: &str| match p.split_once('*') {
        None => p == s,
        Some((pre, post)) => s.starts_with(pre) && s.ends_with(post),
    };
    for (i, p) in pat.iter().enumerate() {
        if *p == "*" && i == pat.len() - 1 {
            return path.len() >= pat.len();
        }
        match path.get(i) {
            Some(s) if seg(p, s) => {}
            _ => return false,
        }
    }
    path.len() == pat.len()
}

/// `8-4-4-4-12` hex: a bare app guid (an app-level gap subject).
fn is_bare_guid(v: &str) -> bool {
    v.len() == 36
        && v.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// Every reference of one row of `key`: `(field path, domain, value)`. Only
/// the fields [`REFERENCES`] lists are read (S9); integers are rendered as
/// text (frame ids).
fn row_references(key: &str, row: &CborValue, out: &mut dyn FnMut(&str, Domain, String)) {
    let Some((_, _, refs)) = REFERENCES.iter().find(|(k, _, _)| *k == key) else {
        return;
    };
    fn walk<'a>(
        v: &'a CborValue,
        path: &mut Vec<&'a str>,
        refs: &[ReferenceField],
        out: &mut dyn FnMut(&str, Domain, String),
    ) {
        let value = match v {
            CborValue::Text(s) => s.clone(),
            CborValue::Int(n) => n.to_string(),
            CborValue::Array(a) => return a.iter().for_each(|x| walk(x, path, refs, out)),
            CborValue::Map(m) => {
                for (k, x) in m {
                    path.push(k);
                    walk(x, path, refs, out);
                    path.pop();
                }
                return;
            }
            _ => return,
        };
        let domains: Vec<Domain> = refs
            .iter()
            .filter(|(p, _)| path_matches(p, path.as_slice()))
            .map(|(_, d)| *d)
            .collect();
        let domain = match domains.as_slice() {
            [] => return,
            [d] => *d,
            many if many.contains(&App) && is_bare_guid(&value) => App,
            many => match many.iter().find(|d| **d != App) {
                Some(d) => *d,
                None => return,
            },
        };
        out(&path.join("."), domain, value);
    }
    walk(row, &mut Vec::new(), refs, out);
}

/// The rows of a top-level value: an array's items, else the value itself.
fn rows_of(v: &CborValue) -> &[CborValue] {
    match v {
        CborValue::Array(a) => a,
        other => std::slice::from_ref(other),
    }
}

/// The `identities` row an identity reference names (spec §4.4's matching
/// rule), or `None` for the literal `"unknown"`. A stable routine, object or
/// table id names itself; a stable event id its object and a stable field id
/// (`<guid>:Table:N#F`) its table; an internal routine id its stable id; an
/// internal table/field/key id `<guid>:Table:N` (`stable_table_from_internal`
/// of its first three `/` segments, never a `/`->`:` rewrite); any other
/// internal object-rooted id its stable object id. An internal routine id
/// (`<model>/<hash>`, as the anchors' `enclosingRoutineId` carry it) resolves
/// only through `routine_to_stable`; one the map lacks comes back unchanged and
/// names no row. A `::`-bearing stable id (an event id) is cut to its first
/// three `:` segments, its object.
fn identity_key(v: &str, routine_to_stable: &HashMap<String, String>) -> Option<String> {
    if v == "unknown" {
        return None;
    }
    if let Some(stable) = routine_to_stable.get(v) {
        return Some(stable.clone());
    }
    if v.contains(':') {
        let segs: Vec<&str> = v.splitn(4, ':').collect();
        if v.contains("::") && segs.len() >= 3 {
            return Some(segs[..3].join(":"));
        }
        if let [guid, "Table", n] = segs.as_slice()
            && let Some((table, field)) = n.split_once('#')
            && !field.is_empty()
            && field.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(format!("{guid}:Table:{table}"));
        }
        return Some(v.to_string());
    }
    let parts: Vec<&str> = v.split('/').collect();
    Some(match parts.as_slice() {
        [_, "table", _, ..] => stable_table_from_internal(&parts[..3].join("/")),
        [_, _, _, ..] => crate::engine::ids::to_stable_object_id(&parts[..3].join("/")),
        _ => v.to_string(),
    })
}

/// What the kept rows reference, per domain.
#[derive(Default)]
struct Referenced {
    identities: HashSet<String>,
    operations: HashSet<String>,
    callsites: HashSet<String>,
    /// Read by the shard closure only: the cross-app filter keeps edges by
    /// their own subject.
    edges: HashSet<String>,
    /// Lower-case app guids.
    apps: HashSet<String>,
}

impl Referenced {
    fn add(&mut self, key: &str, rows: &CborValue, routine_to_stable: &HashMap<String, String>) {
        for row in rows_of(rows) {
            row_references(key, row, &mut |_, domain, value| match domain {
                Identity => self
                    .identities
                    .extend(identity_key(&value, routine_to_stable)),
                Operation => {
                    self.operations.insert(value);
                }
                Callsite => {
                    self.callsites.insert(value);
                }
                App => {
                    self.apps.insert(value.to_ascii_lowercase());
                }
                Edge => {
                    self.edges.insert(value);
                }
                // A frame table follows its Evidence row (step 4).
                Frame => {}
            });
        }
    }
}

/// The keys a shard is closed over, with each row's id field (`""`: the
/// `identities` key, whose rows are its `stableIds`).
const SHARD_CLOSED: [(&str, &str); 4] = [
    ("identities", ""),
    ("operationIndex", "operationId"),
    ("callsiteIndex", "callsiteId"),
    ("typedEdges", "edgeId"),
];

/// #220 (spec §4.4 "Sharding", S3, S3N1, O3N1): add to `shard` every row of
/// `full` that a shard row references in the identity, operation, callsite or
/// edge domain, and scan the added rows in turn, to a fixpoint. The frame and
/// app domains are never closed: a shard has no `routineOrderFrames` and keeps
/// one `apps[]` row. In single-app every reached row is primary and already
/// sliced in, so nothing is added (F26).
///
/// `routine_to_stable` is the model's internal-to-stable routine map: anchors
/// (`*Anchor.enclosingRoutineId`) name routines by their internal id, which
/// only the model maps to an `identities` row.
pub(crate) fn close_shard(
    full: &IndexMap<String, CborValue>,
    shard: &mut IndexMap<String, CborValue>,
    routine_to_stable: &HashMap<String, String>,
) {
    // Every row is scanned once: the first pass reads the sliced rows, each
    // later pass only the rows the pass before added.
    let mut refs = Referenced::default();
    for (key, rows) in shard.iter() {
        refs.add(key, rows, routine_to_stable);
    }
    let mut held: Vec<HashSet<String>> = SHARD_CLOSED
        .iter()
        .map(|(key, id_field)| {
            column(shard.get(*key), "stableIds")
                .iter()
                .filter_map(|r| row_id(r, id_field))
                .map(str::to_string)
                .collect()
        })
        .collect();
    loop {
        let mut next = Referenced::default();
        let mut grew = false;
        for (i, (key, id_field)) in SHARD_CLOSED.iter().enumerate() {
            let wanted = [
                &refs.identities,
                &refs.operations,
                &refs.callsites,
                &refs.edges,
            ][i];
            let added = close_rows(full, shard, key, id_field, wanted, &mut held[i]);
            if !added.is_empty() {
                grew = true;
                next.add(key, &CborValue::Array(added), routine_to_stable);
            }
        }
        if !grew {
            return;
        }
        refs = next;
    }
}

/// A row's id: the row itself for `identities` (`id_field` empty), else its
/// `id_field`.
fn row_id<'a>(row: &'a CborValue, id_field: &str) -> Option<&'a str> {
    if id_field.is_empty() {
        text(Some(row))
    } else {
        field(row, id_field)
    }
}

/// A key's rows, borrowed: an array's items, or one column of `identities`.
fn column<'a>(v: Option<&'a CborValue>, name: &str) -> &'a [CborValue] {
    match v {
        Some(CborValue::Map(m)) => match m.get(name) {
            Some(CborValue::Array(a)) => a,
            _ => &[],
        },
        Some(CborValue::Array(a)) => a,
        _ => &[],
    }
}

/// Merge into `shard[key]` the rows of `full[key]` whose id `wanted` names and
/// `held` does not (the first row per id), in `full`'s order: the shard's rows
/// are a subsequence of it, matched by id, so a held row is never re-selected
/// and a repeated id adds one row. For `identities` the `displayNames` column
/// follows. Only added rows are cloned. Returns them.
fn close_rows(
    full: &IndexMap<String, CborValue>,
    shard: &mut IndexMap<String, CborValue>,
    key: &str,
    id_field: &str,
    wanted: &HashSet<String>,
    held: &mut HashSet<String>,
) -> Vec<CborValue> {
    let all = column(full.get(key), "stableIds");
    let mut new: HashSet<&str> = all
        .iter()
        .filter_map(|r| row_id(r, id_field))
        .filter(|id| wanted.contains(*id) && !held.contains(*id))
        .collect();
    let Some(slot) = shard.get_mut(key) else {
        return Vec::new();
    };
    if new.is_empty() {
        return Vec::new();
    }
    let full_names = column(full.get(key), "displayNames");
    let (rows, names) = match slot {
        CborValue::Map(m) => {
            let mut take = |name: &str| match m.get_mut(name) {
                Some(CborValue::Array(a)) => std::mem::take(a),
                _ => Vec::new(),
            };
            (take("stableIds"), Some(take("displayNames")))
        }
        CborValue::Array(a) => (std::mem::take(a), None),
        _ => return Vec::new(),
    };
    let identities = names.is_some();
    let mut kept = rows
        .into_iter()
        .zip(
            names
                .into_iter()
                .flatten()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .peekable();
    let (mut out_rows, mut out_names, mut added) = (Vec::new(), Vec::new(), Vec::new());
    for (i, r) in all.iter().enumerate() {
        let id = row_id(r, id_field);
        if kept.peek().is_some_and(|(k, _)| row_id(k, id_field) == id) {
            let (k, n) = kept.next().unwrap_or((CborValue::Null, None));
            out_rows.push(k);
            out_names.push(n);
        } else if let Some(id) = id
            && new.remove(id)
        {
            held.insert(id.to_string());
            out_rows.push(r.clone());
            out_names.push(full_names.get(i).cloned());
            added.push(r.clone());
        }
    }
    // A sliced row the full tree does not hold in that order: kept, last.
    for (k, n) in kept {
        out_rows.push(k);
        out_names.push(n);
    }
    *slot = if identities {
        let mut m = IndexMap::new();
        m.insert("stableIds".to_string(), CborValue::Array(out_rows));
        let blank = || CborValue::Text(String::new());
        m.insert(
            "displayNames".to_string(),
            CborValue::Array(
                out_names
                    .into_iter()
                    .map(|n| n.unwrap_or_else(blank))
                    .collect(),
            ),
        );
        CborValue::Map(m)
    } else {
        CborValue::Array(out_rows)
    };
    added
}

fn text(v: Option<&CborValue>) -> Option<&str> {
    match v {
        Some(CborValue::Text(s)) => Some(s),
        _ => None,
    }
}

fn field<'a>(row: &'a CborValue, f: &str) -> Option<&'a str> {
    match row {
        CborValue::Map(m) => text(m.get(f)),
        _ => None,
    }
}

/// #220 (spec §4.4, O7): the cross-app class filter on the composer's own
/// typed rows, before CBOR, in the six-step order. `permission_facts` is the
/// derived (already primary-only) array whose references count too; the gaps
/// are already filtered by reach (D2). Returns the lower-case app guids the
/// kept rows reference (step 6 reads them).
pub(crate) fn filter_by_class(
    core: &mut CapabilitySnapshot,
    permission_facts: &CborValue,
    primary_app_guid: &str,
    routine_to_stable: &HashMap<String, String>,
) -> HashSet<String> {
    let is_primary = |id: &str| role_of_stable_id(Some(primary_app_guid), id) == Role::Primary;
    // (1) Inventory and Behaviour rows by subject: the key's class in
    // `SNAPSHOT_KEYS` decides; a subject-classed key with no arm is a bug.
    for (key, class) in SNAPSHOT_KEYS {
        match (class, *key) {
            (KeyClass::Header | KeyClass::Input | KeyClass::Gap | KeyClass::Evidence, _) => {}
            (_, "capabilityFacts") => core.capability_facts.retain(|f| is_primary(&f.subject)),
            (_, "coverage") => core.coverage.retain(|c| is_primary(&c.subject)),
            (_, "callsiteResolutions") => core.callsite_resolutions.retain(|r| is_primary(&r.from)),
            (_, "typedEdges") => core.typed_edges.retain(|e| is_primary(e.edge_from())),
            (_, "rootClassifications") => core
                .root_classifications
                .retain(|r| is_primary(&r.routine_id)),
            (_, "eventDeclarations") => core.event_declarations.retain(|e| is_primary(&e.routine)),
            // Derived primary-only by the composer (`derive_*`), step (6)
            // (`project_apps`), and step (4) (frame tables).
            (_, "contractFacts" | "schemaFacts" | "permissionFacts" | "apps")
            | (_, "routineOrderFrames") => {}
            (_, other) => unreachable!("{other}: a subject-classed key with no subject filter"),
        }
    }
    // The producer solve indexes the unfiltered facts and edges; it is never
    // serialized, so the composer's copy drops it rather than keep stale indices.
    core.pd_solution = Default::default();

    // (2) Their references, resolved against the unfiltered rows below.
    let mut refs = Referenced::default();
    let r2s = routine_to_stable;
    refs.add(
        "capabilityFacts",
        &capability_facts_to_cbor(&core.capability_facts),
        r2s,
    );
    refs.add("coverage", &to_cbor_value(&core.coverage), r2s);
    refs.add(
        "callsiteResolutions",
        &to_cbor_value(&core.callsite_resolutions),
        r2s,
    );
    refs.add("typedEdges", &to_cbor_value(&core.typed_edges), r2s);
    refs.add(
        "rootClassifications",
        &to_cbor_value(&core.root_classifications),
        r2s,
    );
    refs.add(
        "eventDeclarations",
        &to_cbor_value(&core.event_declarations),
        r2s,
    );
    refs.add("analysisGaps", &to_cbor_value(&core.analysis_gaps), r2s);
    refs.add("permissionFacts", permission_facts, r2s);

    // (3) Evidence index rows of a primary routine or referenced (verbatim).
    core.operation_index
        .retain(|o| is_primary(&o.routine) || refs.operations.contains(&o.operation_id));
    core.callsite_index
        .retain(|c| is_primary(&c.routine) || refs.callsites.contains(&c.callsite_id));

    // (4) The kept Evidence rows' references; each one's frame table joins.
    refs.add("operationIndex", &to_cbor_value(&core.operation_index), r2s);
    refs.add("callsiteIndex", &to_cbor_value(&core.callsite_index), r2s);
    let evidence_routines: HashSet<&str> = core
        .operation_index
        .iter()
        .map(|o| o.routine.as_str())
        .chain(core.callsite_index.iter().map(|c| c.routine.as_str()))
        .collect();
    if let Some(frames) = &mut core.routine_order_frames {
        frames.retain(|routine| is_primary(routine) || evidence_routines.contains(routine));
    }

    // (5) identities: every primary id plus every referenced one.
    let ids = &mut core.identities;
    let keep: Vec<bool> = ids
        .stable_ids
        .iter()
        .map(|id| is_primary(id) || refs.identities.contains(id))
        .collect();
    let mut k = keep.iter();
    ids.stable_ids.retain(|_| *k.next().unwrap_or(&false));
    let mut k = keep.iter();
    ids.display_names.retain(|_| *k.next().unwrap_or(&false));

    // (6) is `project_apps`: the primary row, the owners of kept identities
    // and these referenced apps.
    refs.apps
}

/// #220 (spec §4.4 "Closure"): every reference of every serialized row of
/// `tree` that resolves to no serialized row of its domain. The only
/// exceptions: an identity value `"unknown"`, or rooted in a guid that is
/// neither the primary nor a `dependency_apps` guid and equal to no model
/// routine id (`EXTERNAL_IDENTITY_SHAPES`); a frame's `parentFrameId = -1`;
/// and (F25) the `callsiteId` of a resolution row with no `resolvedEdges`
/// that no kept typed edge or fact witness names.
#[must_use]
pub fn closure_violations(
    tree: &CborValue,
    resolved: &Model,
    universe: &SnapshotUniverse,
) -> Vec<ClosureViolation> {
    let CborValue::Map(snap) = tree else {
        return Vec::new();
    };
    let ids_of = |key: &str, f: &str| -> HashSet<&str> {
        snap.get(key)
            .map(rows_of)
            .unwrap_or_default()
            .iter()
            .filter_map(|r| field(r, f))
            .collect()
    };
    let identities: HashSet<&str> = match snap.get("identities") {
        Some(CborValue::Map(m)) => match m.get("stableIds") {
            Some(CborValue::Array(a)) => a.iter().filter_map(|v| text(Some(v))).collect(),
            _ => HashSet::new(),
        },
        _ => HashSet::new(),
    };
    let operations = ids_of("operationIndex", "operationId");
    let callsites = ids_of("callsiteIndex", "callsiteId");
    let edges = ids_of("typedEdges", "edgeId");
    let apps: HashSet<String> = ids_of("apps", "appGuid")
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();
    let mut frames: HashMap<&str, HashSet<String>> = HashMap::new();
    if let Some(CborValue::Map(tables)) = snap.get("routineOrderFrames") {
        for (routine, table) in tables {
            let ids = rows_of(table).iter().filter_map(|f| match f {
                CborValue::Map(m) => match m.get("frameId") {
                    Some(CborValue::Int(n)) => Some(n.to_string()),
                    _ => None,
                },
                _ => None,
            });
            frames.insert(routine, ids.collect());
        }
    }
    let routine_to_stable = routine_to_stable_of(&resolved.workspace);
    // The call sites a kept typed edge or fact witness names: the anchors
    // `derive_callsite_evidence` keeps an index row for (F25's complement).
    let mut anchored: HashSet<String> = HashSet::new();
    for key in ["typedEdges", "capabilityFacts", "permissionFacts"] {
        for row in snap.get(key).map(rows_of).unwrap_or_default() {
            row_references(key, row, &mut |_, domain, value| {
                if domain == Callsite {
                    anchored.insert(value);
                }
            });
        }
    }
    let mut model_apps: HashSet<String> = HashSet::new();
    if let Some(p) = &resolved.primary_app {
        model_apps.insert(p.app_guid.to_ascii_lowercase());
    }
    if let SnapshotUniverse::CrossApp(u) = universe {
        model_apps.extend(
            u.base
                .dependency_apps
                .iter()
                .map(|a| a.guid.to_ascii_lowercase()),
        );
    }
    // `EXTERNAL_IDENTITY_SHAPES` (spec §4.4): the literal "unknown", or an id
    // rooted in a bare app guid the model does not hold. Anything else (an
    // empty or malformed value, an internal id whose root is no guid) must
    // resolve to an `identities` row.
    let external_identity = |v: &str| {
        let root = v.split([':', '/']).next().unwrap_or("");
        v == "unknown"
            || (v.len() > root.len()
                && is_bare_guid(root)
                && !model_apps.contains(&root.to_ascii_lowercase())
                && !routine_to_stable.contains_key(v))
    };

    let mut out = Vec::new();
    let mut check = |key: &str, routine: Option<&str>, row: &CborValue| {
        // F25 (#220 discovery, pre-existing on the base): the ledger has a row
        // per syntactic call site, but `derive_callsite_evidence` keeps only
        // call sites a fact witnesses or a typed edge anchors, so an
        // unanchored site's `callsiteId` (no `resolvedEdges`, named by no kept
        // edge or fact witness) has no index row.
        let unanchored_site = key == "callsiteResolutions"
            && matches!(row, CborValue::Map(m)
                if matches!(m.get("resolvedEdges"), Some(CborValue::Array(e)) if e.is_empty()));
        row_references(key, row, &mut |path, domain, value| {
            let resolves = match domain {
                Identity => {
                    identity_key(&value, &routine_to_stable)
                        .is_some_and(|k| identities.contains(k.as_str()))
                        || external_identity(&value)
                }
                App => apps.contains(&value.to_ascii_lowercase()),
                Operation => operations.contains(value.as_str()),
                Callsite => {
                    callsites.contains(value.as_str())
                        || (unanchored_site && path == "callsiteId" && !anchored.contains(&value))
                }
                Edge => edges.contains(value.as_str()),
                Frame => {
                    (path.ends_with("parentFrameId") && value == "-1")
                        || routine
                            .and_then(|r| frames.get(r))
                            .is_some_and(|t| t.contains(&value))
                }
            };
            if !resolves {
                out.push(ClosureViolation {
                    key: key.to_string(),
                    field: path.to_string(),
                    domain,
                    value,
                });
            }
        });
    };
    for (key, v) in snap {
        match (key.as_str(), v) {
            ("routineOrderFrames", CborValue::Map(tables)) => {
                for (routine, table) in tables {
                    for frame in rows_of(table) {
                        check(key, Some(routine), frame);
                    }
                }
            }
            _ => {
                for row in rows_of(v) {
                    check(key, field(row, "routine"), row);
                }
            }
        }
    }
    out
}
