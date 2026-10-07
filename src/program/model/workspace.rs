//! L3 workspace assembly — the WORKSPACE-level model the L3 resolver needs.
//!
//! Where L2 (`l2_workspace::project_workspace`) processed per-file/per-object and
//! emitted a flat allowlisted projection, L3 needs ALL objects + tables +
//! routines across the whole workspace assembled together, in al-sem's EXACT
//! deterministic ingestion order: POSIX-path-sorted files → per-file document
//! order (the same order R0/R1 discovery produces). This order is LOAD-BEARING —
//! the symbol table's name/number collision resolution is LAST-wins, the
//! lexical-scope fallback is LAST-wins, and `merge_extension_fields` is FIRST-wins,
//! all keyed off this iteration order.
//!
//! This module also drives resolution: `build_symbol_table → resolve_record_types
//! → merge_extension_fields` (al-sem's first three resolve sub-steps; calls /
//! events / coverage are LATER gates and OUT of R2a). Record vars / ops get their
//! resolved internal `tableId`, projected to a StableTableId by the test/dump.
//!
//! Object / routine features reuse the L2 body walk verbatim
//! (`project_routine_features`) so record vars / ops / variables match L2
//! byte-for-byte; L3 only adds the table/field index + the cross-file resolution.

use super::extension_fields::merge_extension_fields;
use super::record_types::resolve_routine_record_types;
use super::symbol_table::SymbolTable;
use al_syntax::IdentifierFoldExt;

use crate::engine::ids::{encode_object_id, to_stable_object_id, to_stable_routine_id_from_parts};
use crate::engine::perf_trace as pt;
use crate::program::body::node_util::Utf16Cols;
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// L3 model types — workspace-level, in-memory (NOT the serde projection shape).
// ---------------------------------------------------------------------------

/// A workspace object (the L3-relevant subset of al-sem's `ObjectDecl`).
#[derive(Debug, Clone)]
pub struct L3Object {
    /// Internal object id: `${appGuid}/${objectType}/${objectNumber}`.
    pub id: String,
    pub app_guid: String,
    pub object_type: String,
    pub object_number: i64,
    pub name: String,
    /// Page / PageExtension `SourceTable` (unquoted), else None.
    pub source_table_name: Option<String>,
    /// TableExtension / PageExtension `extends` target (unquoted), else None.
    pub extends_target_name: Option<String>,
    /// Implemented interfaces (Codeunit / Enum / Interface): `Some([])` known-none,
    /// `Some([...])` listed, `None` unknown. Other object types: `None`.
    pub implements_interfaces: Option<Vec<String>>,
    /// Object `Subtype` property (Codeunit only; e.g. "Install" / "Upgrade" /
    /// "Test"), else `None`. Additive L2→L3 forward — L3Object is NOT
    /// Serialize-derived into any gate surface (R0–R3 goldens are field-allowlisted
    /// projections), so adding this never touches a golden. Populated at L3 assembly
    /// (native path) from the `Subtype` property; dep objects forward the ABI
    /// projection's `object_subtype` (it DOES expose it for Codeunits — native+ABI
    /// agree on shape). Consumed by d46 to classify lifecycle objects.
    pub object_subtype: Option<String>,
    /// Object `PageType` property (Page / PageExtension only; e.g. "API" /
    /// "Card" / "List"), else `None`. Additive L2→L3 forward — L3Object is NOT
    /// Serialize-derived into any gate surface (R0–R3 goldens are
    /// field-allowlisted projections), so adding this never touches a golden.
    /// Populated at L3 assembly (native path) from the `PageType` property; dep
    /// objects use `None` (the ABI projection does not expose it — no API-page
    /// fixture in the R4-F corpus exercises it). Consumed by R4-F
    /// `root_classification::kinds_for` to classify `api-page` roots.
    pub page_type: Option<String>,
    /// Object `InherentCommitBehavior` property (Codeunit / Table /
    /// TableExtension only). Canonical lower-case member: "ignore" | "error" |
    /// "allow". `None` when absent or an unrecognised value. Additive L2→L3
    /// forward — L3Object is NOT Serialize-derived into any gate surface, so
    /// adding this never touches a golden. Populated at L3 assembly (native
    /// path) from the `InherentCommitBehavior` property; dep objects forward the
    /// ABI projection's `inherent_commit_behavior` (it carries the same canonical
    /// lower-case form). Consumed by R4-F `return_summary` to merge the
    /// object-level commit behavior into each routine's `commitBehavior`.
    pub inherent_commit_behavior: Option<String>,
    /// Page / PageExtension `SourceTable` temporary flag — `Some(true)` when the
    /// SourceTable object is marked `TableType = Temporary`, `Some(false)` when
    /// confirmed non-temporary, `None` when not yet resolved. Additive — L3Object
    /// is NOT Serialize-derived into any gate surface, so this never touches a
    /// golden. Populated by later tasks (Task 5).
    pub source_table_temporary: Option<bool>,
    /// Page / PageExtension layout controls (`part`/`systempart`/`usercontrol`) — used
    /// to resolve `CurrPage.<control>…` member calls. Empty for non-page objects.
    pub page_controls: Vec<L3PageControl>,
    /// Object `SingleInstance` property (Codeunit only): `Some(true)`/`Some(false)`
    /// when the property is written, `None` when absent. Additive — L3Object is NOT
    /// Serialize-derived into any gate surface. Consumed by d57.
    pub single_instance: Option<bool>,
    /// Page `Editable` / `InsertAllowed` / `ModifyAllowed` / `DeleteAllowed`
    /// property booleans (Page only, `None` when absent). Additive. Consumed by d64.
    pub editable: Option<bool>,
    pub insert_allowed: Option<bool>,
    pub modify_allowed: Option<bool>,
    pub delete_allowed: Option<bool>,
    /// The object DECLARATION's own source anchor (native assembly only; dep
    /// objects `None`). Lets object-level detectors (d64) anchor findings on the
    /// object header instead of borrowing a routine anchor. Additive.
    pub source_anchor: Option<crate::program::body::features::PAnchor>,
}

/// A page layout control relevant to member resolution: a `part`/`systempart`
/// (subpage) or a `usercontrol` (control add-in). `name` is the control name used in
/// `CurrPage.<name>`. `target` is the subpage Page reference (NAME from native source,
/// NUMBER string from dep symbols) for `Part`/`SystemPart`, or the control-add-in name
/// for `UserControl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L3PageControl {
    pub name: String,
    pub kind: PageControlKind,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageControlKind {
    Part,
    SystemPart,
    UserControl,
}

/// A workspace field (the L3-relevant subset of al-sem's `Field`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L3Field {
    pub id: String,
    pub physical_table_id: String,
    pub declaring_object_id: String,
    pub declaring_app_id: String,
    pub field_number: i64,
    pub name: String,
    pub field_class: String,
    pub data_type: String,
    pub is_blob_like: bool,
}

/// A workspace table key (the L3-relevant subset of al-sem's `Key`). Only the
/// fields the cli-b snapshot `deriveSchema` reads are kept (`id` + resolved
/// member field-ids). Additive — `L3Table` is NOT serialized into any R0–R3
/// golden surface, so adding keys never moves an existing golden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L3Key {
    /// Internal key id: `${tableId}/key/${index}` (mirrors al-sem `encodeKeyId`).
    pub id: String,
    /// Resolved member field internal ids (`${tableId}/${fieldNumber}`), in
    /// declaration order. A key field not found in this object is silently
    /// skipped (mirrors al-sem's `fieldsByName` resolution).
    pub fields: Vec<String>,
}

/// A workspace table (the L3-relevant subset of al-sem's `Table`). Both `Table`
/// and `TableExtension` declarations produce one of these (matching al-sem's
/// `index.tables`).
#[derive(Debug, Clone)]
pub struct L3Table {
    /// Internal table id: `${appGuid}/table/${tableNumber}`.
    pub id: String,
    pub app_guid: String,
    pub table_number: i64,
    pub name: String,
    pub fields: Vec<L3Field>,
    /// Table keys (cli-b snapshot `deriveSchema` reads these). Additive.
    pub keys: Vec<L3Key>,
    /// True when the table is declared with `TableType = Temporary`. Additive —
    /// L3Table is NOT Serialize-derived into any gate surface, so this never
    /// touches a golden. Populated by later tasks (Task 4).
    pub is_temporary: bool,
    /// G-5 (docs/engine-gaps.md): true when this entry was indexed from a
    /// `tableextension` declaration (a pseudo-table stub kept so
    /// `merge_extension_fields` can find the extension's own fields). Its `id`
    /// is `${appGuid}/table/${EXTENSION object number}`, which COLLIDES with a
    /// real table sharing that number in the same app — every id/name lookup
    /// must prefer a REAL table over a stub (`table_by_id_preferring_real`,
    /// `SymbolTable`), otherwise ops on the real table render the EXTENSION's
    /// name in rootCause text. Additive — L3Table is not golden-serialized.
    pub is_extension_stub: bool,
}

/// G-5: build an id → table map over `tables` where a REAL table always wins an
/// id collision with a `tableextension` stub (see `L3Table::is_extension_stub`).
/// Within the same kind (real/real or stub/stub) LAST-wins is preserved —
/// matching the al-sem `tableById` semantics every consumer had before.
pub fn table_by_id_preferring_real(
    tables: &[L3Table],
) -> std::collections::HashMap<&str, &L3Table> {
    let mut map: std::collections::HashMap<&str, &L3Table> = std::collections::HashMap::new();
    for t in tables {
        if let Some(prev) = map.get(t.id.as_str()) {
            // Never let a stub clobber a real table.
            if !prev.is_extension_stub && t.is_extension_stub {
                continue;
            }
        }
        map.insert(t.id.as_str(), t);
    }
    map
}

/// A record variable with its (post-resolve) resolved internal table id.
#[derive(Debug, Clone)]
pub struct L3RecordVariable {
    pub id: String,
    pub name: String,
    /// Declared table name (unquoted), or None for a non-record / unparsed type.
    pub table_name: Option<String>,
    /// Resolved internal TableId, set by `resolve_record_types`. None = unresolved.
    pub table_id: Option<String>,
    /// True when this is a parameter record variable (from L2 body walk).
    /// Required by the R3a-2 summary engine to derive RecordRoleSummary per
    /// record parameter (mirrors al-sem `recVar.isParameter`).
    pub is_parameter: bool,
    /// The 0-based parameter index when `is_parameter` is true.
    /// Required by the R3a-2 summary engine (`recVar.parameterIndex`).
    pub parameter_index: Option<u32>,
    /// Temp-state of this record variable (from the L2 body walk). al-sem d3 reads
    /// `recVar.tempState` to skip temporary records (SetLoadFields has no SQL
    /// benefit for in-memory temp records). Additive L2→L3 forward — the L3
    /// record-type projection is field-allowlisted, so this never reaches an
    /// R0–R3 golden. Forwarded verbatim from `PRecordVariable.temp_state`.
    pub temp_state: crate::program::body::features::PTempState,
    /// Variable scope: `"local"` | `"parameter"` | `"global"`. `None` when not
    /// yet populated. Additive — forwarded from `PRecordVariable.scope`;
    /// the L3 record-type projection is field-allowlisted, so this never reaches
    /// a golden. Populated by later tasks.
    pub scope: Option<String>,
}

impl L3RecordVariable {
    /// `recVar.tempState.kind === "known" ? recVar.tempState.value : None`.
    /// Returns `Some(value)` only when the temp state is concretely known.
    pub fn temp_state_known_value(&self) -> Option<bool> {
        if self.temp_state.kind == "known" {
            self.temp_state.value
        } else {
            None
        }
    }
}

/// A record operation with its (post-resolve) resolved internal table id.
#[derive(Debug, Clone)]
pub struct L3RecordOperation {
    pub id: String,
    pub op: String,
    pub record_variable_name: String,
    pub record_variable_id: Option<String>,
    pub table_id: Option<String>,
    /// Temp-state of this operation (from L2 body walk). Required by the
    /// R3a-2 summary engine to derive DbEffect.tempState for base summaries.
    pub temp_state: Option<crate::program::body::features::PTempState>,
    /// Field arguments for ops like Validate (from L2 body walk). Required
    /// by the R3a-2 summary engine for RecordRoleSummary.writesFields.
    pub field_arguments: Option<Vec<String>>,
    /// Source anchor (from L2 body walk). Required by the R3a-2 branch-aware
    /// CFG walker to interleave record ops with field accesses by source
    /// position inside a block (mirrors al-sem `op.sourceAnchor.range`). L2 data
    /// that the L3 record-type projection drops, forwarded here for L4 only.
    pub source_anchor: crate::program::body::features::PAnchor,
    /// The enclosing-loop id stack (from L2 body walk). L2 data that the L3
    /// record-type projection drops, forwarded here for L5 detectors (d4 reads
    /// `op.loopStack.includes(loop.id)`). Additive — the L3 projections are
    /// field-allowlisted, so this never reaches an R0–R3 golden.
    pub loop_stack: Vec<String>,
    /// Structured field-argument classification (from L2 body walk). L2 data that
    /// the L3 record-type projection drops, forwarded here for L5 detectors (d4
    /// reads `op.fieldArgumentInfos[0]` for the literal-key test). Additive.
    pub field_argument_infos: Option<Vec<crate::program::body::features::PExpressionInfo>>,
    /// G-1: `true` when this op sits inside the `until` condition of its nearest
    /// enclosing `repeat` loop — the loop's OWN terminator expression (from L2 body
    /// walk, an exact structural proof). Forwarded for d1 to suppress the terminator
    /// `Next()`. Never serialized (the L3 projection is field-allowlisted).
    pub in_until_condition: bool,
    /// Detector-audit d29 FP-1: literal `RunTrigger` arg of a mutating op
    /// (`Modify`/`Delete`/`DeleteAll`/`ModifyAll`). `Some(false)` ⇒ trigger
    /// re-firing suppressed (no recursive-event loop). Forwarded for d29. Never
    /// serialized (the L3 projection is field-allowlisted).
    pub run_trigger: Option<bool>,
}

/// A lexical variable (params → locals → globals) carrying its declared type, for
/// the record-op lexical-scope fallback.
#[derive(Debug, Clone)]
pub struct L3Variable {
    pub name: String,
    pub declared_type: String,
    /// True when this variable is a routine parameter. Required by the R3a-3 L4
    /// value-source classifier (`classifyIdentifier`: a parameter resolves to a
    /// `parameter` ValueSource, a local to its initializer / `constant-var`).
    pub is_parameter: bool,
    /// 0-based parameter index when `is_parameter`.
    pub parameter_index: Option<u32>,
    /// The L2-captured one-hop initializer (`VariableSymbol.initializer`) as raw
    /// ValueSource JSON. Required by the R3a-3 value-source classifier so local
    /// dispatch / IO targets resolve to their literal/enum source (e.g.
    /// `CodeunitId := 50100; Codeunit.Run(CodeunitId)` → literal 50100).
    pub initializer: Option<serde_json::Value>,
    /// Variable scope forwarded from the L2 `PVariableSymbol.scope`:
    /// `"local" | "parameter" | "global"`. `None` only for construction paths
    /// that lack the L2 symbol (defensive). Additive — L3Variable is not
    /// Serialize-derived. Consumed by d57 (global-collection growth).
    pub scope: Option<String>,
}

/// A routine's lexical variable list — **params → locals → non-shadowed object
/// globals** — stored WITHOUT replicating the object's globals into every routine.
///
/// An object's globals are byte-identical for each of its routines (lowercased name,
/// canonicalized declared type, `scope: Some("global")`, never a parameter, never an
/// initializer), so one `Arc<[L3Variable]>` per object is shared by all of them. On
/// the 8020 corpus that removed 2,997,353 copies of 53,186 distinct globals — a 56.4×
/// replication costing ~443 MB of payload (see `.superpowers/sdd/scope-l3-substrate.md`
/// §3.3). This is L4 pattern (a) recurring in L3.
///
/// **This is a representation change only.** [`RoutineVariables::iter`] yields exactly
/// the flat list the L2 projection emits (`PFeatures::variables`) — same elements, same
/// order, same shadowing — and the assembly path asserts that round trip in debug
/// builds. Two invariants the consumers depend on, both preserved:
///
/// * **order is params → locals → globals.** `record_types.rs`'s pass-2b builds a
///   name→declared-type map with `entry().or_insert()` (FIRST-wins), so the innermost
///   declaration must come first; `capability_cone.rs`'s `VarInfo` map is LAST-wins
///   over the same sequence and needs the globals last.
/// * **the sequence is name-unique on the lowercased name.** A param/local shadows a
///   same-named global (recorded in `shadowed` below), and duplicate globals within an
///   object are deduped first-wins by [`crate::program::body::ir_walk::ir_object_globals`].
///
/// Nothing mutates a routine's variables after assembly (unlike `record_variables`,
/// which `record_types.rs` upgrades per routine — see the L-6 hazard note in the
/// scoping report), so sharing cannot leak a per-routine change to a sibling.
#[derive(Debug, Clone, Default)]
pub struct RoutineVariables {
    /// The routine's OWN variables: parameters in declaration order, then locals —
    /// exactly the `scope != "global"` prefix of the L2 list.
    own: Vec<L3Variable>,
    /// The owning object's globals, shared with every sibling routine.
    globals: Arc<[L3Variable]>,
    /// Ascending indices into `globals` this routine SHADOWS (it declares a param or
    /// local of the same lowercased name). Empty for almost every routine, and an
    /// empty `Box<[u32]>` does not allocate — precomputed so [`Self::iter`] never
    /// re-scans `own` by name for every global (an O(own × globals) name scan);
    /// the ascending order lets [`Self::iter`]'s own per-global membership test run
    /// as a `binary_search` (O(log shadowed)) rather than a linear scan.
    shadowed: Box<[u32]>,
}

impl RoutineVariables {
    /// A routine with no object globals in scope — dependency/ABI routines (whose
    /// globals are not modelled at all) and test fixtures.
    pub fn from_own(own: Vec<L3Variable>) -> Self {
        Self {
            own,
            globals: Arc::default(),
            shadowed: Box::default(),
        }
    }

    /// Assemble from the routine's own params/locals plus the owning object's shared
    /// global table.
    ///
    /// Both name sets are already lowercased by the L2 projection
    /// (`ir_variables`/`ir_object_globals` both apply `to_ascii_lowercase`), so the
    /// shadow test is exact string equality — the same test the inlined
    /// `HashSet<String>` in `ir_variables` performs.
    pub fn new(own: Vec<L3Variable>, globals: Arc<[L3Variable]>) -> Self {
        let shadowed: Box<[u32]> = if own.is_empty() || globals.is_empty() {
            Box::default()
        } else {
            globals
                .iter()
                .enumerate()
                .filter(|(_, g)| own.iter().any(|o| o.name == g.name))
                .map(|(i, _)| i as u32)
                .collect()
        };
        Self {
            own,
            globals,
            shadowed,
        }
    }

    /// The routine's full lexical scope, in the L2 order: params → locals → the
    /// object's globals it does not shadow.
    pub fn iter(&self) -> impl Iterator<Item = &L3Variable> {
        let shadowed = &self.shadowed;
        self.own.iter().chain(
            self.globals
                .iter()
                .enumerate()
                .filter(move |(i, _)| shadowed.binary_search(&(*i as u32)).is_err())
                .map(|(_, g)| g),
        )
    }
}

/// A routine parameter (the L3-relevant subset of al-sem's `ParameterSymbol`) —
/// drives arity matching, `calleeParameterIsVar` upgrades, and overload arg-type
/// disambiguation (`typeText`).
#[derive(Debug, Clone)]
pub struct L3Parameter {
    /// Positional index (0-based) — the EventSymbol parameter shape.
    pub index: u32,
    pub name: String,
    pub type_text: String,
    pub is_var: bool,
    /// True when the declared type is a `Record` (drives the event param shape).
    pub is_record: bool,
    /// Record table name (unquoted), when `is_record`.
    pub table_name: Option<String>,
}

/// A workspace routine (the L3-relevant subset).
#[derive(Debug, Clone)]
pub struct L3Routine {
    /// Internal routine id: `${modelInstanceId}/${canonicalRoutineKeyHash}`.
    pub id: String,
    /// StableRoutineId: `${stableObjectId}#${64 lowercase hex}` — the
    /// modelInstanceId-independent key the L3 record-type projection emits, and
    /// the id `l5::fingerprint` substitutes into every `rootCauseKey`. The hex is
    /// the `normalizedSignatureHash` for a member-less routine and its
    /// enclosing-member fold for a member trigger (`enclosing_member` below); see
    /// [`crate::engine::ids::to_stable_routine_id_from_parts`].
    pub stable_routine_id: String,
    /// Owning object's internal id.
    pub object_id: String,
    /// Owning object's type (`Codeunit` / `Page` / `Table` / …) for the projection.
    pub object_type: String,
    pub name: String,
    /// Routine kind (`procedure` / `trigger` / `event-publisher` /
    /// `event-subscriber`) — drives the event-graph publisher/subscriber passes.
    pub kind: String,
    /// Structured attributes (the grammar-derived AttributeInfo shape) — the event
    /// graph reads `[IntegrationEvent]`/`[BusinessEvent]`/`[EventSubscriber]` args.
    pub attributes_parsed: Vec<crate::program::attributes::AttributeInfo>,
    /// Owning object's app guid — the EventEdge `subscriberAppId`.
    pub app_guid: String,
    /// Owning object's number — for the publisher's `publisherObjectId`.
    pub object_number: i64,
    /// The return-type-aware normalized signature hash — the EventSymbol
    /// `signatureHash` for REAL publisher routines.
    pub normalized_signature_hash: String,
    /// L2 `bodyAvailable` — the routine has a `code_block` body (routine-indexer.ts).
    /// The L3 coverage (R2d) counts these. Set from the SAME `find_code_block`
    /// the L2 projection uses so the flag cannot drift.
    pub body_available: bool,
    /// L2 `parseIncomplete` — the routine's subtree has a tree-sitter ERROR node.
    /// R2d projects parse-incomplete routines' StableRoutineIds.
    pub parse_incomplete: bool,
    pub record_variables: Vec<L3RecordVariable>,
    pub record_operations: Vec<L3RecordOperation>,
    /// Field accesses (from L2 body walk). Required by the R3a-2 summary
    /// engine to derive RecordRoleSummary.readsFields per record parameter.
    pub field_accesses: Vec<crate::program::body::features::PFieldAccess>,
    /// The routine's lexical scope (params → locals → non-shadowed object globals).
    /// Iterate with `.iter()`; see [`RoutineVariables`] for why the globals are shared
    /// rather than replicated per routine.
    pub variables: RoutineVariables,
    /// Declared parameters (in order) — drives arity + var-ness + arg-type
    /// disambiguation. Empty for trigger routines with no parameter list.
    pub parameters: Vec<L3Parameter>,
    /// Access modifier from the L2 projection (`local`/`internal`/`protected`; None
    /// = default/public). Additive field — L3Routine is NOT Serialize-derived into
    /// any gate surface (R0–R3 goldens are field-allowlisted projections), so adding
    /// this never touches a golden. Populated by the native assembly path from
    /// `classify_access_modifier`; dep routines use `None` (ABI does not expose it).
    /// Consumed by d32 (scope gate: `local` only).
    pub access_modifier: Option<String>,
    /// Declared return type text (`type_specification` text), if any — used by
    /// `inferCallExprReturnType` for overload arg-type disambiguation.
    pub return_type: Option<String>,
    /// The routine's call sites (L2 body-walk output), the resolver input.
    pub call_sites: Vec<crate::program::body::features::PCallSite>,
    /// The routine's operation sites (L2 body-walk output). Required by the R3a-3
    /// L4 capability extraction (commit family reads `kind === "commit"`, error
    /// family reads `kind === "error-call"`), and the unreachable-exclusion pass
    /// (sites with `controlContext === "unreachable"` are dropped before family
    /// dispatch — mirrors al-sem `extractCapabilities`).
    pub operation_sites: Vec<crate::program::body::features::POperationSite>,
    /// The CFN statement-tree skeleton (L2 body-walk output). Required by the
    /// R3a-2 branch-aware CFG walker (`walkCFG` port) to join role state-sets at
    /// if/case/loop. `None` for opaque / TryFunction / bodyless routines (the
    /// walker then falls back to the straight-line pass, mirroring al-sem).
    pub statement_tree: Option<crate::program::body::features::PCFNNode>,
    /// The routine's loops (L2 body-walk output). L2 data that the L3
    /// record-type projection drops, forwarded here for L5 detectors (d4 reads
    /// `routine.features.loops`). Additive — never reaches an R0–R3 golden.
    pub loops: Vec<crate::program::body::features::PLoop>,
    /// The routine's OWN declaration anchor (the procedure / trigger_declaration
    /// node range, with `syntax_kind` = "procedure" / "trigger_declaration").
    /// al-sem `routine-indexer.ts:419` builds this as the routine's `sourceAnchor`.
    /// Captured during L2/L3 assembly where the routine NODE is available. Read by
    /// d19 (primaryLocation + evidence) and d29 (first evidence step). Additive —
    /// the L3 record-type projection is field-allowlisted so this never reaches an
    /// R0–R3 golden.
    pub source_anchor: crate::program::body::features::PAnchor,
    /// Lowercased / sorted / deduped identifier references in the routine body
    /// (L2 features `identifierReferences`). Read by d19 to test parameter use.
    /// Additive — forwarded verbatim from L2.
    pub identifier_references: Vec<String>,
    /// Unreachable-after-exit statements recorded during the L2 body DFS
    /// (`features.unreachableStatements`). Read by d20. Additive — forwarded verbatim.
    pub unreachable_statements: Vec<crate::program::body::features::PUnreachableStatement>,
    /// Whether the routine body contains any branching (`features.hasBranching`).
    /// Read by d43's `classify_subscriber` / `publisher_branch_facts`. Additive —
    /// forwarded verbatim from L2; dep (bodyless) routines default `false`.
    pub has_branching: bool,
    /// Variable assignments (`features.varAssignments`) — `lhsName` + optional
    /// `rhsLiteralValue`. Read by d43 to detect `IsHandled := true` setters.
    /// Additive — forwarded verbatim from L2.
    pub var_assignments: Vec<crate::program::body::features::PVarAssignment>,
    /// Condition references (`features.conditionReferences`) — identifiers used in
    /// guard positions, with their reference anchors. Read by `enumerate_dispatch_sites`
    /// (d43) to find post-call IsHandled guards. Additive — forwarded verbatim from L2.
    pub condition_references: Vec<crate::program::body::features::PConditionReference>,
    /// Field/control/action/dataitem member name for a member-trigger routine — the
    /// unescaped logical identifier (inner `""` collapsed to `"`) of the enclosing
    /// member wrapper (field_declaration / page_field / action_declaration /
    /// report_dataitem / query_dataitem). `None` for procedures and object-level
    /// triggers (OnRun / OnOpenPage). Additive — `L3Routine` is NOT `Serialize`-derived
    /// (it has only `#[derive(Debug, Clone)]`), so this never reaches an R0–R3 golden.
    /// (RE-3 / RE-4)
    pub enclosing_member: Option<String>,
    /// StableObjectId of the object that DECLARES this trigger (the object decl in
    /// scope at assembly — the EXTENSION object for an extension-declared trigger).
    /// Honest metadata; the AL CPU-profile frame carries no extension identity, so this
    /// is profile-UNJOINABLE for the multi-extension collision (RE-5). `None` for
    /// non-member routines. Additive — never reaches an R0–R3 golden.
    pub originating_object: Option<String>,
    /// Source range of the member WRAPPER node (field_declaration / page_field /
    /// action_declaration / report_dataitem / query_dataitem) — the boundary the
    /// finding-side position discriminator (E3) matches a finding's primaryLocation
    /// against. `None` for non-member routines. Additive — never reaches an R0–R3
    /// golden. (RE-2)
    pub enclosing_member_range: Option<crate::program::body::features::PAnchor>,
    /// G-2 Part 2 (runtime-implied tempness): the lowercased receiver name of a
    /// routine ENTRY guard `if not <X>.IsTemporary[()] then Error(...)` — the
    /// routine's FIRST executable statement, with `<X>` a record var/param
    /// (incl. promoted globals) or the implicit `Rec`/`xRec`. The guard PROVES
    /// `<X>` is temporary for the entire body (the routine errors at runtime
    /// otherwise); `record_types.rs` upgrades `<X>`'s ops to `Known(true)`.
    /// `None` for any other shape (conservative — detectors keep firing).
    /// Additive — `L3Routine` is NOT Serialize-derived, never reaches a golden.
    pub entry_temp_guard_receiver: Option<String>,
}

/// The assembled workspace L3 model (pre-resolve until `resolve` runs).
#[derive(Debug, Clone)]
pub struct L3Workspace {
    pub objects: Vec<L3Object>,
    pub tables: Vec<L3Table>,
    pub routines: Vec<L3Routine>,
}

// ---------------------------------------------------------------------------
// Table / field extraction (object-indexer.ts `indexTable` / `classifyField`).
// ---------------------------------------------------------------------------

/// `index_table` over the owned IR (fields/keys/TableType from `ObjectDecl`). The
/// rare `TableType = Temporary` runtime-contract guard (`if not Rec.IsTemporary then
/// Error` in a table trigger) is matched over the owned IR via
/// [`ir_table_has_temp_contract_guard`]. Fields/keys come from `ObjectDecl`.
#[allow(clippy::too_many_arguments)]
fn index_table_ir(
    o: &al_syntax::ir::ObjectDecl,
    ir_file: &al_syntax::ir::AlFile,
    object_id: &str,
    app_guid: &str,
    table_number: i64,
    table_name: &str,
    is_extension_stub: bool,
) -> L3Table {
    let table_id = format!("{app_guid}/table/{table_number}");
    let fields: Vec<L3Field> = o
        .fields
        .iter()
        .map(|f| L3Field {
            id: format!("{table_id}/{}", f.number),
            physical_table_id: table_id.clone(),
            declaring_object_id: object_id.to_string(),
            declaring_app_id: app_guid.to_string(),
            field_number: f.number,
            name: f.name.clone(),
            field_class: f.field_class.clone(),
            data_type: f.data_type.clone(),
            is_blob_like: f.is_blob_like,
        })
        .collect();
    let fields_by_name: std::collections::HashMap<String, String> = fields
        .iter()
        .map(|f| (f.name.to_lowercase(), f.id.clone()))
        .collect();
    let keys: Vec<L3Key> = o
        .keys
        .iter()
        .enumerate()
        .map(|(index, members)| L3Key {
            id: format!("{table_id}/key/{index}"),
            // `members` are already unquoted + lowercased by the lowerer.
            fields: members
                .iter()
                .filter_map(|name| fields_by_name.get(name).cloned())
                .collect(),
        })
        .collect();
    let is_temporary = o
        .properties
        .iter()
        .find(|p| p.name == "tabletype")
        .map(|p| p.value.trim().to_lowercase() == "temporary")
        .unwrap_or(false)
        || crate::program::body::ir_walk::ir_table_has_temp_contract_guard(ir_file, o);
    L3Table {
        id: table_id,
        app_guid: app_guid.to_string(),
        table_number,
        name: table_name.to_string(),
        fields,
        keys,
        is_temporary,
        is_extension_stub,
    }
}

/// PAnchor from an owned-IR `Origin` — byte-identical to the legacy node anchor on the
/// node the origin came from (same row + utf16 column basis + raw `kind_text`).
fn anchor_from_origin(
    origin: &al_syntax::ir::Origin,
    source_unit_id: &str,
    cols: &Utf16Cols,
) -> crate::program::body::features::PAnchor {
    crate::program::body::features::PAnchor {
        source_unit_id: source_unit_id.to_string(),
        start_line: origin.start.row,
        start_column: cols.col(origin.start.row as usize, origin.start.column as usize),
        end_line: origin.end.row,
        end_column: cols.col(origin.end.row as usize, origin.end.column as usize),
        syntax_kind: origin.kind_text.to_string(),
    }
}

// `unescape_al_identifier` (a quoted AL identifier escapes an inner double-quote by
// doubling it, so the logical name collapses each `""` back to one `"` — the
// profiler's display form, RE-4) moved to `l2::scope`: the enclosing-member name it
// normalizes is now ALSO the routine id's member discriminator, so exactly one
// definition may exist. `l2::ir_walk::ir_enclosing_member` is the only caller.

// ---------------------------------------------------------------------------
// Per-file assembly.
// ---------------------------------------------------------------------------

pub const MODEL_INSTANCE_ID_DEFAULT: &str = "r0";

/// Build the L3 workspace contribution for one source file, driven entirely by the
/// owned AL syntax IR (`al_syntax::parse`) — no tree-sitter CST walk.
fn project_file(
    source: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
    cols: &Utf16Cols,
    workspace: &mut L3Workspace,
) {
    // Stages-tier parse-vs-projection split (spec 2026-07-18-tracing-infra.md).
    // `project_file` is the per-file unit of work inside the parallel rayon `.map`
    // closure in `assemble_workspace`/`assemble_workspace_units` — one call here IS
    // one rayon closure invocation, so a `LocalCounters` built and flushed entirely
    // within this function (no threading through the call sites) already satisfies
    // "one LocalCounters per rayon closure, flush each" without a signature change
    // at either caller. Cheapest-on-disabled shape: a single `enabled()` bool read
    // gates every `Instant::now()` call, so tracing-off pays zero clock reads and
    // zero allocation.
    let hot = pt::enabled(pt::Detail::Stages);
    let t_start = hot.then(std::time::Instant::now);
    let ir_file = al_syntax::parse(source);
    let t_parsed = hot.then(std::time::Instant::now);

    project_ir(
        &ir_file,
        &whole_file_population(&ir_file),
        source,
        app_guid,
        model_instance_id,
        source_unit_id,
        cols,
        &std::collections::HashSet::new(),
        workspace,
    );

    if let (Some(t0), Some(t1)) = (t_start, t_parsed) {
        let mut lc = pt::LocalCounters::new();
        lc.add("parse_us", t1.duration_since(t0).as_micros() as u64);
        lc.add("projection_us", t1.elapsed().as_micros() as u64);
        lc.flush("l3.parse_project");
    }
}

/// The declarations of one file to project, by position: each object's index in
/// `AlFile::objects` with the indices of its routines, document order.
pub type FilePopulation = Vec<(usize, Vec<usize>)>;

/// Every declaration in `ir_file` — the population of the disk-backed paths.
pub fn whole_file_population(ir_file: &al_syntax::ir::AlFile) -> FilePopulation {
    ir_file
        .objects
        .iter()
        .enumerate()
        .map(|(oi, o)| (oi, (0..o.routines.len()).collect()))
        .collect()
}

/// The population of each file according to the program graph's physical rows
/// (engine-switch S2b.4): the model projects what the program engine extracted, so
/// one walk decides what exists. Keyed by virtual path.
pub fn rows_population(
    rows: &crate::program::physical::PhysicalIndex,
) -> HashMap<&str, FilePopulation> {
    let mut by_file: HashMap<&str, FilePopulation> = HashMap::new();
    for o in &rows.objects {
        by_file
            .entry(o.file.as_str())
            .or_default()
            .push((o.object_ix as usize, Vec::new()));
    }
    for r in &rows.routines {
        let pop = by_file
            .get_mut(r.file.as_str())
            .expect("a routine row's object has an object row");
        let entry = pop
            .iter_mut()
            .find(|(oi, _)| *oi == r.object_ix as usize)
            .expect("a routine row's object has an object row");
        entry.1.push(r.routine_ix as usize);
    }
    by_file
}

/// Project one ALREADY-PARSED file into `workspace` — [`project_file`] without the
/// parse. Engine-switch S2a: the analyze path hands in the program engine's parse
/// of the same text (`al_syntax::parse` over the same `read_al_source` bytes), so
/// each workspace file is parsed once. S2b.4: `population` names the declarations
/// to project; the analyze path takes it from the program graph's physical rows.
#[allow(clippy::too_many_arguments)]
fn project_ir(
    ir_file: &al_syntax::ir::AlFile,
    population: &FilePopulation,
    source: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
    cols: &Utf16Cols,
    parenless_calls: &std::collections::HashSet<al_syntax::ir::ExprId>,
    workspace: &mut L3Workspace,
) {
    for (oi, routine_ixs) in population {
        let oi = *oi;
        let o = &ir_file.objects[oi];
        let Some(object_type) = crate::program::body::ir_walk::ir_object_type(&o.kind) else {
            continue;
        };
        // Object metadata driven entirely by the owned IR (the loop iterates IR objects
        // — no tree-sitter CST walk).
        let object_number = o.id.unwrap_or(0);
        let name = o.name.clone();
        let object_id = encode_object_id(app_guid, object_type, object_number);

        // Read an object-level property value from the IR (lowercased name match).
        let ir_prop = |name_lc: &str| -> Option<String> {
            o.properties
                .iter()
                .find(|p| p.name == name_lc)
                .map(|p| p.value.clone())
        };

        // Object metadata (object-indexer.ts parity).
        // Engine-switch S5.4: the same derivation as the resolver's `ObjectNode`
        // (`node_extract`): a `SourceTable` that differs between `#if`
        // branches is `None`, never the first branch's value, and every
        // extension kind keeps its `extends` target (Report/Enum extensions
        // lost theirs; the object-fact census found them on CDO, DO and
        // ws-report-dataitem).
        let source_table_name = if object_type == "Page" || object_type == "PageExtension" {
            crate::program::node_extract::singular_object_ref_text(o, "sourcetable")
        } else {
            None
        };
        let extends_target_name = o.extends_target.clone();
        let implements_interfaces =
            if object_type == "Codeunit" || object_type == "Enum" || object_type == "Interface" {
                Some(o.implements.clone())
            } else {
                None
            };
        // Object `Subtype` — Codeunit only (object-indexer.ts parity / L2
        // `extract_object_metadata`). d46 reads this to classify Install/Upgrade
        // lifecycle codeunits.
        let object_subtype = if object_type == "Codeunit" {
            ir_prop("subtype")
        } else {
            None
        };
        // Object `PageType` — Page / PageExtension only (object-indexer.ts
        // `pageType` parity). R4-F `root_classification` reads this to classify
        // `api-page` roots (PageType == "API", case-insensitive).
        let page_type = if object_type == "Page" || object_type == "PageExtension" {
            ir_prop("pagetype")
        } else {
            None
        };
        // Object `InherentCommitBehavior` — Codeunit / Table / TableExtension only
        // (object-indexer.ts parity). The raw value is a qualified_enum_value like
        // "InherentCommitBehavior::Ignore"; extract the member after "::", then
        // lower-case. Unrecognised values → None (conservative).
        let inherent_commit_behavior = if object_type == "Codeunit"
            || object_type == "Table"
            || object_type == "TableExtension"
        {
            ir_prop("inherentcommitbehavior").and_then(|raw| {
                let sep = raw.rfind("::").map(|i| i + 2).unwrap_or(0);
                let member = raw[sep..].to_lowercase();
                match member.as_str() {
                    "ignore" => Some("ignore".to_string()),
                    "error" => Some("error".to_string()),
                    "allow" => Some("allow".to_string()),
                    _ => None,
                }
            })
        } else {
            None
        };
        // Object `SourceTableTemporary` — Page / PageExtension only (Task 5 / G4).
        // Structural boolean property: `SourceTableTemporary = true;` means the
        // page's implicit `Rec` and `xRec` are always temporary. EXACT case-insensitive
        // match against "true" — anything else present → Some(false); absent → None.
        let source_table_temporary = if object_type == "Page" || object_type == "PageExtension" {
            ir_prop("sourcetabletemporary").map(|v| v.trim().to_lowercase() == "true")
        } else {
            None
        };
        // Object `SingleInstance` (Codeunit only) + Page write-surface booleans
        // (`Editable`/`InsertAllowed`/`ModifyAllowed`/`DeleteAllowed`, Page only) —
        // substrate for d57/d64. Same exact-"true" boolean convention as
        // `source_table_temporary` above.
        let bool_prop = |name: &str| ir_prop(name).map(|v| v.trim().to_lowercase() == "true");
        let single_instance = if object_type == "Codeunit" {
            bool_prop("singleinstance")
        } else {
            None
        };
        let (editable, insert_allowed, modify_allowed, delete_allowed) = if object_type == "Page" {
            (
                bool_prop("editable"),
                bool_prop("insertallowed"),
                bool_prop("modifyallowed"),
                bool_prop("deleteallowed"),
            )
        } else {
            (None, None, None, None)
        };
        // Page controls (`part`/`systempart`/`usercontrol`) — used to resolve
        // `CurrPage.<control>…` member calls (Task 6+).
        let page_controls = if object_type == "Page" || object_type == "PageExtension" {
            o.page_controls
                .iter()
                .map(|pc| L3PageControl {
                    name: pc.name.clone(),
                    kind: match pc.kind.as_str() {
                        "systempart" => PageControlKind::SystemPart,
                        "usercontrol" => PageControlKind::UserControl,
                        _ => PageControlKind::Part,
                    },
                    target: pc.target.clone(),
                })
                .collect()
        } else {
            Vec::new()
        };
        // The object DECLARATION's own anchor (native assembly only) — mirrors the
        // routine `source_anchor` construction above, on the object decl's own
        // `Origin` rather than a routine's. Lets d64 anchor object-level findings on
        // the object header instead of borrowing a routine anchor.
        let source_anchor = Some(anchor_from_origin(&o.origin, source_unit_id, cols));

        workspace.objects.push(L3Object {
            id: object_id.clone(),
            app_guid: app_guid.to_string(),
            object_type: object_type.to_string(),
            object_number,
            name: name.clone(),
            source_table_name: source_table_name.clone(),
            extends_target_name,
            implements_interfaces,
            object_subtype,
            page_type,
            inherent_commit_behavior,
            source_table_temporary,
            page_controls,
            single_instance,
            editable,
            insert_allowed,
            modify_allowed,
            delete_allowed,
            source_anchor,
        });

        if object_type == "Table" || object_type == "TableExtension" {
            // G-5: a TableExtension's indexed table is a STUB whose id reuses the
            // EXTENSION's own object number — it must never win an id collision
            // against a real table sharing that number (see `is_extension_stub`).
            workspace.tables.push(index_table_ir(
                o,
                ir_file,
                &object_id,
                app_guid,
                object_number,
                &name,
                object_type == "TableExtension",
            ));
        }

        // Object-global RECORD vars (scope=global) — promoted (below) into each
        // routine's `record_variables`, honoring AL shadowing (a routine's own var wins).
        let object_global_record_vars =
            crate::program::body::ir_walk::ir_object_global_record_vars(o, &object_id);

        // Interface / ControlAddIn: al-sem's L3Workspace never modeled these objects'
        // signature-only members as routines — this frozen legacy pipeline (L2/L3/L4,
        // still differential-gated against the al-sem TS reference) must keep matching
        // that historical behavior byte-for-byte (see `tests/r3a1_vectors.rs`'s "the
        // vectors are the ORACLE... fix is in the Rust code, NEVER the vector").
        // `al_syntax::lower::collect_routines` NOW lowers `interface_procedure` nodes
        // (receiver-closure plan, Task 1 — controladdin/interface signature-only
        // procedures were previously invisible to `RoutineDecl` extraction entirely,
        // a genuine al-syntax gap fixed for `program::resolve`'s NEW, Rust-owned
        // `node_extract::extract_nodes` consumer, which correctly captures them for
        // the `CurrPage.<usercontrol>` closed-if-known member gate). That fix is
        // upstream of BOTH consumers (shared al-syntax IR) — this object-kind skip is
        // what keeps the LEGACY consumer's parity contract intact without reverting
        // the shared lowering fix the new consumer needs. `o.routines` for these two
        // object kinds is, in every REAL AL file, populated ENTIRELY from
        // `interface_procedure` nodes (the grammar's `interface_body`/
        // `controladdin_body` rules permit an ordinary bodied `$.procedure` too, but
        // no real controladdin/interface declares one — a theoretical decompiled edge
        // case this skip would ALSO exclude, matching pre-fix behavior for that
        // hypothetical shape too, since nothing in the differential corpus exercises
        // it).
        if object_type == "Interface" || object_type == "ControlAddIn" {
            continue;
        }

        // L-3: the object's GLOBAL scalar variables, built ONCE and shared (by `Arc`)
        // with every routine of the object instead of copied into each one's
        // `variables`. Identical to the tail `ir_variables` appends per routine —
        // same source, same first-wins dedup, same lowercasing — minus the anchor,
        // which `L3Variable` does not carry. See [`RoutineVariables`].
        let object_globals: Arc<[L3Variable]> = Arc::from(
            crate::program::body::ir_walk::ir_object_globals(ir_file, oi, cols, source_unit_id)
                .into_iter()
                .map(|g| L3Variable {
                    name: g.name,
                    declared_type: g.declared_type,
                    is_parameter: g.is_parameter,
                    parameter_index: g.parameter_index,
                    initializer: g.initializer,
                    scope: Some(g.scope),
                })
                .collect::<Vec<_>>(),
        );

        for ir_routine in routine_ixs.iter().map(|&ri| &o.routines[ri]) {
            let rname = ir_routine.name.clone();
            if rname.is_empty() {
                continue;
            }

            // E1: the enclosing member of a member trigger — the unescaped logical
            // identifier of the field / control / action / dataitem wrapper, `None`
            // for procedures and object-level triggers. Computed ONCE here because
            // it is both the routine id's discriminator (below) and the
            // `L3Routine.enclosing_member` field (further down): the two MUST be the
            // same string, and `ir_enclosing_member` is the single place that
            // unescapes it.
            let enclosing_member = crate::program::body::ir_walk::ir_enclosing_member(ir_routine);

            let (routine_id, mut features) = {
                let kind_for_id = crate::program::body::ir_walk::ir_routine_kind(ir_routine);
                let params_for_id = crate::program::body::ir_walk::ir_parameter_symbols(ir_routine);
                let rid = crate::program::body::scope::compute_routine_id(
                    app_guid,
                    object_type,
                    object_number,
                    kind_for_id,
                    &rname,
                    enclosing_member.as_deref(),
                    &params_for_id,
                    ir_routine.return_type.as_deref(),
                    model_instance_id,
                );
                let feats = crate::program::body::ir_walk::project_routine_features_ir_with(
                    ir_file,
                    oi,
                    ir_routine,
                    &rid,
                    source,
                    source_unit_id,
                    source_table_name.as_deref(),
                    parenless_calls,
                );
                (rid, feats)
            };

            // R1b control-context lattice (the SAME pass `aldump --l2` applies):
            // populate `controlContext` on each callsite/operation site, including the
            // error-call source-range post-pass. Required by the R3a-3 L4 capability
            // extraction's UNREACHABLE EXCLUSION (sites with controlContext ===
            // "unreachable" emit no facts — mirrors al-sem `extractCapabilities`).
            // R3a-2's projection never reads control_context, so this is additive.
            {
                let cc_params = crate::program::body::ir_walk::ir_parameter_symbols(ir_routine);
                let attrs_json =
                    crate::program::body::ir_walk::ir_attributes(ir_routine, ir_file, source).1;
                let attr_names_lc: Vec<String> = attrs_json
                    .iter()
                    .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                    .map(|n| n.to_lowercase())
                    .collect();
                crate::program::body::control_context::apply_control_contexts(
                    &mut features,
                    &attr_names_lc,
                    &cc_params,
                );
            }

            // The routine's OWN record vars (params + locals), built first so they
            // take precedence over any same-named promoted global.
            let mut record_variables: Vec<L3RecordVariable> = features
                .record_variables
                .iter()
                .map(|rv| L3RecordVariable {
                    id: rv.id.clone(),
                    name: rv.name.clone(),
                    table_name: rv.table_name.clone(),
                    table_id: None,
                    is_parameter: rv.is_parameter,
                    parameter_index: rv.parameter_index,
                    temp_state: rv.temp_state.clone(),
                    scope: rv.scope.clone(),
                })
                .collect();
            // Task 3 (temp-state) PROMOTION + SHADOWING: append object-global record
            // vars, re-keyed to a per-routine id, but ONLY those whose (lowercased)
            // name is NOT already declared by the routine's own params/locals — the
            // routine's own var shadows the global (innermost wins). Skipping
            // shadowed globals keeps `record_variables` NAME-UNIQUE, which preserves
            // the documented pass-1 `var_index_by_name` last-wins invariant in
            // `record_types.rs` (a name-duplicated list would otherwise let the
            // global clobber the local in pass 1 — the WRONG result). Each promoted
            // global keeps `scope: Some("global")`, its `table_name`, and its
            // `temp_state` (the Known(true/false) the L2 walk could not derive).
            //
            // Perf: most objects have NO global record vars (the dominant case), so
            // skip the whole block — including the per-routine `own_names` build —
            // entirely when there is nothing to promote.
            if !object_global_record_vars.is_empty() {
                let own_names: std::collections::HashSet<String> = record_variables
                    .iter()
                    .map(|rv| rv.name.to_lowercase())
                    .collect();
                for g in &object_global_record_vars {
                    let lc = g.name.to_lowercase();
                    if own_names.contains(&lc) {
                        continue; // shadowed by the routine's own param/local
                    }
                    record_variables.push(L3RecordVariable {
                        id: format!("{}/rv/{}", routine_id, lc),
                        name: g.name.clone(),
                        table_name: g.table_name.clone(),
                        table_id: None,
                        is_parameter: g.is_parameter,
                        parameter_index: g.parameter_index,
                        temp_state: g.temp_state.clone(),
                        scope: g.scope.clone(),
                    });
                }
            }
            // G-2 Part 2: the routine's entry guard
            // `if not <X>.IsTemporary[()] then Error(...)` (first executable
            // statement, <X> a record var/param or implicit Rec/xRec) proves <X>
            // temporary for the whole body; `record_types.rs` consumes this.
            // Computed AFTER global promotion so a guarded object-global
            // receiver also qualifies.
            let entry_temp_guard_receiver =
                crate::program::body::ir_walk::ir_entry_temp_guard_receiver(ir_file, ir_routine)
                    .filter(|receiver| {
                        receiver == "rec"
                            || receiver == "xrec"
                            || record_variables
                                .iter()
                                .any(|rv| rv.name.eq_fold_identifier(receiver))
                    });
            let record_operations = features
                .record_operations
                .iter()
                .map(|op| L3RecordOperation {
                    id: op.id.clone(),
                    op: op.op.clone(),
                    record_variable_name: op.record_variable_name.clone(),
                    record_variable_id: op.record_variable_id.clone(),
                    table_id: None,
                    temp_state: Some(op.temp_state.clone()),
                    field_arguments: op.field_arguments.clone(),
                    source_anchor: op.source_anchor.clone(),
                    loop_stack: op.loop_stack.clone(),
                    field_argument_infos: op.field_argument_infos.clone(),
                    in_until_condition: op.in_until_condition,
                    run_trigger: op.run_trigger,
                })
                .collect();
            let field_accesses = std::mem::take(&mut features.field_accesses);
            // L-3: keep only the routine's OWN variables (the `scope != "global"`
            // prefix — params then locals); the globals tail is the object's shared
            // table, re-derived per routine by `RoutineVariables::iter`.
            let own_variables: Vec<L3Variable> = features
                .variables
                .iter()
                .filter(|v| v.scope != "global")
                .map(|v| L3Variable {
                    name: v.name.clone(),
                    declared_type: v.declared_type.clone(),
                    is_parameter: v.is_parameter,
                    parameter_index: v.parameter_index,
                    initializer: v.initializer.clone(),
                    scope: Some(v.scope.clone()),
                })
                .collect();
            let variables = RoutineVariables::new(own_variables, Arc::clone(&object_globals));
            // The round trip that makes this a representation change and nothing more:
            // the shared form must re-yield the L2 list element-for-element, in order.
            // (`initializer` is excluded only because it is `None` for every global and
            // moved verbatim for every own var.)
            debug_assert!(
                variables
                    .iter()
                    .map(|v| (
                        v.name.as_str(),
                        v.declared_type.as_str(),
                        v.is_parameter,
                        v.parameter_index,
                        v.scope.as_deref()
                    ))
                    .eq(features.variables.iter().map(|v| (
                        v.name.as_str(),
                        v.declared_type.as_str(),
                        v.is_parameter,
                        v.parameter_index,
                        Some(v.scope.as_str())
                    ))),
                "RoutineVariables must reproduce the flat L2 variable list exactly \
                 (routine {routine_id})"
            );

            // The routine's own parameters + return type for the call resolver, from
            // the matched IR routine (legacy extractors as a fallback). Reuses the SAME
            // ParameterSymbol shape the routine-id/signature-hash path uses so
            // arity/var-ness/type-text cannot drift.
            let param_syms = crate::program::body::ir_walk::ir_parameter_symbols(ir_routine);
            let parameters: Vec<L3Parameter> = param_syms
                .iter()
                .map(|p| L3Parameter {
                    index: p.index,
                    name: p.name.clone(),
                    type_text: p.type_text.clone(),
                    is_var: p.is_var,
                    is_record: p.is_record,
                    table_name: p.table_name.clone(),
                })
                .collect();
            let return_type = ir_routine.return_type.clone();
            let mut call_sites = std::mem::take(&mut features.call_sites);
            // RV-8 (Task 8): scope-honest `sourceKind`. The L2 binding builder
            // labels ANY non-parameter record-var arg `"local"` because scope is
            // not yet known at L2 (object globals are only PROMOTED into
            // `record_variables` here at L3). Now that promotion has run and the
            // record vars carry their `scope`, relabel a binding whose source
            // matches a PROMOTED GLOBAL (`scope == Some("global")`) from
            // `"local"` to `"global"`. Diagnostic-only: it removes the mislabel
            // without changing which args are persistable (a global is a real
            // caller var, persistable exactly like a local). Only "local"
            // bindings are eligible — "parameter" / "implicit-rec" / "expression"
            // are left untouched.
            //
            // G-8 BACKFILL (residual temp gap): the L2 builder only matched the
            // routine's OWN params/locals, so an arg naming an OBJECT-GLOBAL
            // record var came out `sourceKind == "unknown"` with NO
            // `source_temp_state` — both the L4 PD substitution
            // (`substitute_pd_temp_state`) and the L5 per-path resolver
            // (`resolve_temp_along_path`) collapse a missing source to
            // `Unknown`, so a codeunit-global `temporary` record FORWARDED
            // by-var into a helper stayed "temp state uncertain" (the CDO
            // eDocuments-Dispatcher shape). Now that promotion has run, rebind
            // such an "unknown" binding to the promoted global: the arg text
            // must be a BARE identifier (the same identifier-only discipline
            // the L2 builder applies — anything quoted/dotted/called stays
            // untouched), must match a promoted-global record var, and the
            // name's INNERMOST declaration must BE the global (a same-named
            // scalar param/local shadows it → skip, conservative). The
            // backfilled `source_temp_state` is the global's own state — the
            // exact structural `temporary`-keyword signal Task 3 captured —
            // so Known(true) is only ever introduced from that signal.
            {
                // Promoted-global record vars, keyed by lowercased name.
                let global_rv_by_lc: std::collections::HashMap<String, &L3RecordVariable> =
                    record_variables
                        .iter()
                        .filter(|rv| rv.scope.as_deref() == Some("global"))
                        .map(|rv| (rv.name.to_lowercase(), rv))
                        .collect();
                // Names whose INNERMOST declaration (params → locals → globals,
                // first-wins in `features.variables`) is NOT the global —
                // shadowed by a (possibly scalar) param/local → never backfill.
                let mut innermost_scope_by_lc: std::collections::HashMap<&str, &str> =
                    std::collections::HashMap::new();
                for v in &features.variables {
                    innermost_scope_by_lc
                        .entry(v.name.as_str())
                        .or_insert(v.scope.as_str());
                }
                if !global_rv_by_lc.is_empty() {
                    for cs in &mut call_sites {
                        for b in &mut cs.argument_bindings {
                            match b.source_kind.as_str() {
                                "local" => {
                                    if let Some(name_lc) = b.source_variable_name.as_deref() {
                                        // `source_variable_name` is already lowercased
                                        // at L2.
                                        if global_rv_by_lc.contains_key(name_lc) {
                                            b.source_kind = "global".to_string();
                                        }
                                    }
                                }
                                "unknown" => {
                                    // G-8 backfill (see block comment above).
                                    let Some(text) =
                                        cs.argument_texts.get(b.parameter_index as usize)
                                    else {
                                        continue;
                                    };
                                    let lc = text.trim().to_lowercase();
                                    // Bare identifiers only — mirrors the L2 builder's
                                    // `arg_node.kind() == "identifier"` gate.
                                    if lc.is_empty()
                                        || !lc.chars().all(|c| c.is_alphanumeric() || c == '_')
                                    {
                                        continue;
                                    }
                                    let Some(g) = global_rv_by_lc.get(&lc) else {
                                        continue;
                                    };
                                    if innermost_scope_by_lc.get(lc.as_str()).copied()
                                        != Some("global")
                                    {
                                        continue; // shadowed by a scalar param/local
                                    }
                                    b.source_kind = "global".to_string();
                                    b.source_variable_name = Some(lc.clone());
                                    b.source_record_variable_id = Some(g.id.clone());
                                    b.source_temp_state = Some(g.temp_state.clone());
                                }
                                _ => {}
                            }
                        }
                        // The call's receiver may be an object-global record too
                        // (`TempBuf.ClearBuffer()`), with the same shadowing rule.
                        if cs.receiver_temp_state.is_none()
                            && let crate::program::body::features::PCallee::Member {
                                receiver, ..
                            } = &cs.callee
                        {
                            let lc = receiver.trim().trim_matches('"').to_lowercase();
                            if let Some(g) = global_rv_by_lc.get(&lc)
                                && innermost_scope_by_lc.get(lc.as_str()).copied() == Some("global")
                            {
                                cs.receiver_temp_state = Some(g.temp_state.clone());
                            }
                        }
                    }
                }
            }
            let operation_sites = std::mem::take(&mut features.operation_sites);
            let statement_tree = features.statement_tree.take();
            let loops = std::mem::take(&mut features.loops);
            // d19 reads the L2 identifier-reference set (lowercased / sorted /
            // deduped exactly as L2 produced it); d20 reads the unreachable list.
            let identifier_references = std::mem::take(&mut features.identifier_references);
            let unreachable_statements = std::mem::take(&mut features.unreachable_statements);
            // d43 branch-slice surface: hasBranching + varAssignments + conditionReferences.
            let has_branching = features.has_branching;
            let var_assignments = std::mem::take(&mut features.var_assignments);
            let condition_references = std::mem::take(&mut features.condition_references);
            // The routine's OWN declaration anchor (al-sem routine-indexer.ts:419):
            // `syntax_kind` = node.type = "procedure" / "trigger_declaration".
            let source_anchor = anchor_from_origin(&ir_routine.origin, source_unit_id, cols);

            // L2 bodyAvailable / parseIncomplete from the owned IR.
            let body_available = ir_routine.body.is_some();
            let parse_incomplete = ir_routine.parse_incomplete;

            // StableRoutineId = `${stableObjectId}#{64 hex}`. The hash reuses the same
            // param/kind/return extraction as the internal routine id
            // (`routine_normalized_signature_hash`), so they cannot drift, and it takes
            // the SAME `enclosing_member` value computed once at the top of this loop as
            // the internal id's discriminator — one string, one normalization, both ids.
            let stable_object_id = to_stable_object_id(&object_id);
            let norm_hash = {
                let param_specs: Vec<crate::engine::ids::ParamSpec> = param_syms
                    .iter()
                    .map(|p| crate::engine::ids::ParamSpec {
                        type_text: p.type_text.clone(),
                        is_var: p.is_var,
                    })
                    .collect();
                crate::engine::ids::normalized_signature_hash(
                    &rname,
                    &param_specs,
                    return_type.as_deref(),
                )
            };
            let stable_routine_id = to_stable_routine_id_from_parts(
                &stable_object_id,
                &norm_hash,
                enclosing_member.as_deref(),
            );

            // Routine kind + structured attributes (the event-graph inputs). Reuse the
            // SAME L2 attribute indexing that produces the L2 projection's
            // `attributesParsed`, so the AttributeInfo arg shape (kind/value/qualifier/
            // member) cannot drift from R1.
            let kind = crate::program::body::ir_walk::ir_routine_kind(ir_routine).to_string();
            let attributes_parsed_json =
                crate::program::body::ir_walk::ir_attributes(ir_routine, ir_file, source).1;
            let attributes_parsed: Vec<crate::program::attributes::AttributeInfo> =
                attributes_parsed_json
                    .into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect();
            // d32 scope gate: access modifier (`local`/`internal`/`protected`; None = public).
            let access_modifier = ir_routine.access_modifier.clone();

            // E1: enclosing-member capture (additive — never serialized into a golden).
            // A member-trigger (parent is a member-bearing wrapper) gets the WRAPPER
            // node's source range (RE-2/RE-3/RE-4) and `originatingObject` = the
            // StableObjectId of the object decl in scope (the EXTENSION object for an
            // extension-declared trigger — RE-5). Procedures and object-level triggers
            // (OnRun / OnOpenPage) carry a non-member parent → all `None`. The member
            // NAME itself was unescaped once at the top of this loop (`enclosing_member`),
            // because the routine id consumes it too.
            let (enclosing_member_range, originating_object) = match &ir_routine.enclosing_member {
                Some((_member_name, wrapper_origin)) => (
                    Some(anchor_from_origin(wrapper_origin, source_unit_id, cols)),
                    Some(stable_object_id.clone()),
                ),
                None => (None, None),
            };

            workspace.routines.push(L3Routine {
                id: routine_id,
                stable_routine_id,
                object_id: object_id.clone(),
                object_type: object_type.to_string(),
                name: rname,
                kind,
                attributes_parsed,
                app_guid: app_guid.to_string(),
                object_number,
                normalized_signature_hash: norm_hash,
                body_available,
                parse_incomplete,
                record_variables,
                record_operations,
                field_accesses,
                variables,
                parameters,
                access_modifier,
                return_type,
                call_sites,
                operation_sites,
                statement_tree,
                loops,
                source_anchor,
                identifier_references,
                unreachable_statements,
                has_branching,
                var_assignments,
                condition_references,
                enclosing_member,
                originating_object,
                enclosing_member_range,
                entry_temp_guard_receiver,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Public assembly + resolution entry points.
// ---------------------------------------------------------------------------

/// Assemble the workspace L3 model from inline `(name, source)` files, in al-sem's
/// deterministic ingestion order (files sorted by name → per-file document order),
/// then run `resolve_record_types` + `merge_extension_fields`.
///
/// This is the offline entry point the vector test drives. Disk-backed workspaces
/// (the differential / dump in Task 3) sort discovered `.al` files by their
/// workspace-relative POSIX path — the same total order this reproduces.
pub fn assemble_and_resolve(
    files: &[(String, String)],
    app_guid: &str,
    model_instance_id: &str,
) -> L3Resolved {
    let mut workspace = assemble_workspace(files, app_guid, model_instance_id);
    resolve(&mut workspace);
    // Inline path: no disk `roots.config.json` ⇒ AST-only classifications, no infra diagnostics.
    // No disk `app.json` ⇒ primary_app = None.
    let (root_classifications, infra_diagnostics) =
        crate::engine::root_classification::compute_root_classifications(&workspace, None);
    L3Resolved {
        workspace,
        root_classifications,
        primary_app: None,
        infra_diagnostics,
        precomputed_calls: None,
        precomputed_events: None,
    }
}

/// Assemble the workspace L3 model from inline `(name, source)` files WITHOUT
/// resolving — the parse/project half of [`assemble_and_resolve`]. The R2.5b
/// cross-app wiring appends dep entities to the result before calling `resolve`.
pub fn assemble_workspace(
    files: &[(String, String)],
    app_guid: &str,
    model_instance_id: &str,
) -> L3Workspace {
    let _s = pt::span("l3", "l3.parse_project_parallel");
    // Deterministic ingestion order: sort files by name (the `ws:<name>` unit id
    // total order), then walk each file's objects in document order.
    let mut sorted: Vec<&(String, String)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    // Parallel per-file parse+project into PRIVATE fragments on the big-stack
    // pool (the same generous stack the fresh engine parses on), then fold the
    // fragments in the SAME sorted order the old sequential loop appended in —
    // byte-identical Vec order. Object/routine ids are content-derived (no
    // ordinals) and `project_file` only APPENDS into its own `workspace` arg
    // (no cross-file reads), so per-file fragments are independent; the sorted
    // fold reproduces today's exact order. The per-file `ws:<name>` unit id and
    // `Utf16Cols` move inside the closure.
    use rayon::prelude::*;
    let fragments: Vec<L3Workspace> = crate::big_stack::big_stack_pool().install(|| {
        sorted
            .par_iter()
            .map(|(fname, source)| {
                let source_unit_id = format!("ws:{fname}");
                let cols = Utf16Cols::new(source);
                let mut ws = L3Workspace {
                    objects: Vec::new(),
                    tables: Vec::new(),
                    routines: Vec::new(),
                };
                project_file(
                    source,
                    app_guid,
                    model_instance_id,
                    &source_unit_id,
                    &cols,
                    &mut ws,
                );
                ws
            })
            .collect()
    });

    let mut workspace = L3Workspace {
        objects: Vec::new(),
        tables: Vec::new(),
        routines: Vec::new(),
    };
    for mut frag in fragments {
        workspace.objects.append(&mut frag.objects);
        workspace.tables.append(&mut frag.tables);
        workspace.routines.append(&mut frag.routines);
    }
    workspace
}

/// Assemble the workspace L3 model from inline `(source_unit_id, source)` units,
/// using the GIVEN `source_unit_id` verbatim for each file's anchors (instead of
/// the `ws:<name>` form `assemble_workspace` hardcodes). The R3a-4 dependency
/// producer needs this so each embedded `.al` file's op/callsite anchors carry the
/// al-sem `dep:<appGuid>:<relativePath>` source-unit id (the cited-evidence
/// `sourceFile` field), matching `ingestDependencyApp`'s embedded-source path.
///
/// Units are sorted by `source_unit_id` (the same total order
/// `iterateEmbeddedSource` yields: sorted-by-relative-path → here the unit ids are
/// `dep:<appGuid>:<sorted relativePath>`), then walked in document order. NOT
/// resolved — the caller runs `resolve`.
pub fn assemble_workspace_units(
    units: &[(String, String)],
    app_guid: &str,
    model_instance_id: &str,
) -> L3Workspace {
    let _s = pt::span("l3", "l3.parse_project_parallel");
    let mut sorted: Vec<&(String, String)> = units.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    // Parallel per-file parse+project into PRIVATE fragments on the big-stack
    // pool, then fold in the SAME sorted order — byte-identical Vec order (see
    // the sibling `assemble_workspace` for the full rationale). Uses each unit's
    // `source_unit_id` verbatim for anchors (no `ws:<name>` synthesis here).
    use rayon::prelude::*;
    let fragments: Vec<L3Workspace> = crate::big_stack::big_stack_pool().install(|| {
        sorted
            .par_iter()
            .map(|(source_unit_id, source)| {
                let cols = Utf16Cols::new(source);
                let mut ws = L3Workspace {
                    objects: Vec::new(),
                    tables: Vec::new(),
                    routines: Vec::new(),
                };
                project_file(
                    source,
                    app_guid,
                    model_instance_id,
                    source_unit_id,
                    &cols,
                    &mut ws,
                );
                ws
            })
            .collect()
    });

    let mut workspace = L3Workspace {
        objects: Vec::new(),
        tables: Vec::new(),
        routines: Vec::new(),
    };
    for mut frag in fragments {
        workspace.objects.append(&mut frag.objects);
        workspace.tables.append(&mut frag.tables);
        workspace.routines.append(&mut frag.routines);
    }
    workspace
}

/// Convenience: assemble + resolve with the default model-instance id (`r0`).
pub fn assemble_and_resolve_default(files: &[(String, String)], app_guid: &str) -> L3Resolved {
    assemble_and_resolve(files, app_guid, MODEL_INSTANCE_ID_DEFAULT)
}

/// Disk-backed assemble + resolve over a workspace directory (the emitter +
/// differential entry point). Reuses L2's discovery so the file order, BOM
/// strip, app-guid read, and fail-closed layout detection match al-sem EXACTLY:
/// a sound workspace is ONE AL app (readable root `app.json` `id`, single
/// `app.json` excl. node_modules/.alpackages). The inline `ws:<relPosix>` unit
/// ids match `project_workspace`.
///
/// `skip_roots_config`: when true, skip loading/overlaying `roots.config.json`
/// even if present — AST-only root classification (`--no-roots-config` on the
/// `alsem fingerprint` CLI; `compute_root_classifications` already supports
/// this via `workspace_root: None`, so this just threads the caller's choice
/// through instead of hardcoding `Some(workspace)`).
///
/// Returns `None` on an unsound / empty layout (fail-closed) — never throws.
pub fn assemble_and_resolve_workspace(
    workspace: &std::path::Path,
    model_instance_id: &str,
    skip_roots_config: bool,
) -> Option<L3Resolved> {
    let ws = assemble_l3_workspace_from_disk(workspace, model_instance_id)?;
    finish_resolved(ws, workspace, skip_roots_config)
}

/// [`assemble_and_resolve_workspace`] over the program engine's parse (engine-switch
/// S2a): the same file set, text, order and passes, but each file is projected from
/// `ctx`'s already-parsed tree instead of being parsed a second time.
///
/// The file set is L3's own: app-scoped discovery (nested apps skipped), while the
/// program engine's workspace unit includes nested apps (`provider.rs`), so the
/// program files are FILTERED to the app-scoped set. If any app-scoped file is
/// missing from the program's parse (the two walks disagreeing, e.g. a file created
/// between them), the model is built from disk exactly as before.
///
/// `parenless` is the resolution's [`crate::program::resolve::full::ParenlessCalls`]
/// (`ProgramReport::parenless_calls`): the body walk takes those reads as calls.
pub fn assemble_and_resolve_workspace_from_program(
    workspace: &std::path::Path,
    model_instance_id: &str,
    skip_roots_config: bool,
    ctx: &crate::program::resolve::full::ProgramContext,
    parenless: &crate::program::resolve::full::ParenlessCalls,
) -> Option<L3Resolved> {
    let ws = assemble_l3_workspace_from_program(workspace, model_instance_id, ctx, parenless)?;
    finish_resolved(ws, workspace, skip_roots_config)
}

/// The CROSS-APP detector model (engine-switch S7.2): the workspace's rows exactly as
/// [`assemble_and_resolve_workspace_from_program`] builds them, then every dependency
/// the workspace requires (its declared closure, S7.4: not an app that depends on
/// the workspace) appended: first each symbol-only dependency's ABI rows (bodyless,
/// [`crate::program::model::abi_rows`]), then each source-bearing dependency's files
/// projected whole from the program's own parse, as source unit
/// `dep:<appGuid>:<path>`. Dependencies are taken in `.app` path order, files in path
/// order. The order is the legacy merged model's, and it is load-bearing: the symbol
/// table is last-wins and the extension-field merge first-wins.
///
/// Needs a context that keeps dependency bodies (`FULL`). Also returns
/// [`AbiRowIds`], the join from a symbol-only routine's program id to its row.
///
/// `demand` (engine-switch S8.2, `program::resolve::demand`): when given, a
/// source-bearing dependency file keeps every object but only the routines in it.
pub fn assemble_and_resolve_cross_app_from_program(
    workspace: &std::path::Path,
    model_instance_id: &str,
    skip_roots_config: bool,
    ctx: &crate::program::resolve::full::ProgramContext,
    demand: Option<&std::collections::HashSet<crate::program::node::RoutineNodeId>>,
    parenless: &crate::program::resolve::full::ParenlessCalls,
) -> Option<(L3Resolved, AbiRowIds)> {
    let mut ws = assemble_l3_workspace_from_program(workspace, model_instance_id, ctx, parenless)?;
    let abi_rows = append_dependency_rows(&mut ws, model_instance_id, ctx, demand, parenless);
    Some((finish_resolved(ws, workspace, skip_roots_config)?, abi_rows))
}

/// One dependency app in a cross-app model's snapshot (engine-switch S7.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyApp {
    /// The app guid as its manifest spells it (the model rows' `app_guid`).
    pub guid: String,
    pub name: String,
    pub version: String,
    /// Whether the snapshot holds its source (embedded or local).
    pub has_source: bool,
}

/// One dependency the workspace declares `{appGuid, name, minVersion}`, explicit or
/// the implicit Microsoft Application/Platform tier: the d17-relevant subset of a
/// manifest dependency. A missing `version` reads as `"0.0.0.0"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredDependencyDecl {
    pub app_guid: String,
    pub name: String,
    pub min_version: String,
}

/// Program routine id -> model routine id, for the symbol-only dependency rows of a
/// cross-app model (engine-switch S7.3). Built from the same parsed ABI as the
/// rows; an id two ABI entries share (a collapsed overload) is left out, so a call
/// to it stays a dependency target.
pub type AbiRowIds = HashMap<crate::program::node::RoutineNodeId, String>;

fn append_dependency_rows(
    ws: &mut L3Workspace,
    model_instance_id: &str,
    ctx: &crate::program::resolve::full::ProgramContext,
    demand: Option<&std::collections::HashSet<crate::program::node::RoutineNodeId>>,
    parenless: &crate::program::resolve::full::ParenlessCalls,
) -> AbiRowIds {
    let bodies = ctx
        .dep_bodies()
        .expect("the cross-app model needs DependencyBodies::Keep");
    let snap = &ctx.snap;
    let mut deps: Vec<&crate::snapshot::AppUnit> = snap
        .apps
        .iter()
        .filter(|u| ctx.is_required_dependency(&u.id))
        .collect();
    deps.sort_by(|a, b| a.app_path.cmp(&b.app_path));

    let mut objects = Vec::new();
    let mut tables = Vec::new();
    let mut routines = Vec::new();
    let mut ids: HashMap<crate::program::node::RoutineNodeId, Option<String>> = HashMap::new();
    for unit in deps.iter().filter(|u| u.source.is_none()) {
        let Some(app) = ctx.graph().apps.find(&unit.id) else {
            continue;
        };
        if let Some(abi) = crate::program::abi_ingest::load_symbol_reference(unit) {
            let p = crate::engine::deps::projection::project_abi_to_index(
                &abi,
                &unit.id.guid,
                model_instance_id,
            );
            // `project_abi_to_index` projects one routine per ABI routine, objects
            // and routines in order.
            let node_ids = abi.objects.iter().flat_map(|o| {
                let obj = crate::program::abi_ingest::abi_object_node_id(app, o);
                o.routines
                    .iter()
                    .map(move |r| crate::program::abi_ingest::abi_routine_node_id(&obj, r))
            });
            for (node_id, row) in node_ids.zip(&p.routines) {
                ids.entry(node_id)
                    .and_modify(|v| *v = None)
                    .or_insert_with(|| Some(row.id.clone()));
            }
            objects.extend(p.objects);
            tables.extend(p.tables);
            routines.extend(p.routines);
        }
    }
    crate::program::model::abi_rows::append_dep_entities(ws, &objects, &tables, &routines);

    for unit in deps.iter().filter(|u| u.source.is_some()) {
        let Some(parsed) = bodies.iter().find(|b| b.app == unit.id) else {
            continue;
        };
        let guid = &unit.id.guid;
        let app = ctx.graph().apps.find(&unit.id);
        let mut files: Vec<&crate::snapshot::parse::ParsedFile> = parsed.files.iter().collect();
        files.sort_by(|a, b| a.virtual_path.cmp(&b.virtual_path));
        for pf in files {
            let cols = Utf16Cols::new(&pf.text);
            let population = match (demand, app) {
                (Some(d), Some(app)) => demanded_population(&pf.file, app, d),
                _ => whole_file_population(&pf.file),
            };
            let no_calls = std::collections::HashSet::new();
            let calls = app
                .and_then(|a| parenless.get(&(a, pf.virtual_path.clone())))
                .unwrap_or(&no_calls);
            project_ir(
                &pf.file,
                &population,
                &pf.text,
                guid,
                model_instance_id,
                &format!("dep:{guid}:{}", pf.virtual_path),
                &cols,
                calls,
                ws,
            );
        }
    }
    ids.into_iter().filter_map(|(k, v)| Some((k, v?))).collect()
}

/// Every object of a dependency file, with only its demanded routines (S8.2). The
/// routine ids are built as the resolver builds a caller's
/// (`resolve_file_obligations`): the owning app, the object's number or folded name.
fn demanded_population(
    ir_file: &al_syntax::ir::AlFile,
    app: crate::program::node::AppRef,
    demand: &std::collections::HashSet<crate::program::node::RoutineNodeId>,
) -> FilePopulation {
    use crate::program::node::{ObjKey, ObjectNodeId};
    use al_syntax::IdentifierFoldExt;
    ir_file
        .objects
        .iter()
        .enumerate()
        .map(|(oi, o)| {
            let object = ObjectNodeId {
                app,
                kind: o.kind,
                key: match o.id {
                    Some(n) => ObjKey::Id(n),
                    None => ObjKey::Name(o.name.fold_identifier()),
                },
            };
            let routines = o
                .routines
                .iter()
                .enumerate()
                .filter(|(_, r)| {
                    demand.contains(&crate::program::sig_fp::source_routine_node_id(
                        object.clone(),
                        r,
                    ))
                })
                .map(|(ri, _)| ri)
                .collect();
            (oi, routines)
        })
        .collect()
}

/// Which files the program-backed assembly projects.
pub enum ProgramFiles<'c> {
    /// Every app-scoped file, each from the program's parse, in discovery order.
    Selected {
        app_guid: String,
        files: Vec<(&'c str, &'c crate::snapshot::parse::ParsedFile)>,
    },
    /// An app-scoped file the program did not parse (named): no model.
    Missing(String),
}

/// Select L3's app-scoped file set from `ctx`'s parse. `None` on the disk path's
/// fail-closed conditions (no root `app.json` id, unreadable workspace).
pub fn select_program_files<'c>(
    workspace: &std::path::Path,
    ctx: &'c crate::program::resolve::full::ProgramContext,
) -> Option<ProgramFiles<'c>> {
    use crate::program::body::l2_workspace::{discover_al_files_app_scoped, read_root_app_guid};
    use crate::snapshot::parse::ParsedFile;

    let app_guid = read_root_app_guid(workspace)?;
    let discovered = discover_al_files_app_scoped(workspace).ok()?;
    let by_path: std::collections::HashMap<&str, &ParsedFile> = ctx
        .parsed()
        .iter()
        .flat_map(|u| u.files.iter())
        .map(|f| (f.virtual_path.as_str(), f))
        .collect();
    let mut files = Vec::with_capacity(discovered.len());
    for f in &discovered {
        match by_path.get(f.rel_posix.as_str()) {
            // `virtual_path == rel_posix` (the lookup key): both come from
            // `source_text::discover_al_files`.
            Some(pf) => files.push((pf.virtual_path.as_str(), *pf)),
            None => return Some(ProgramFiles::Missing(f.rel_posix.clone())),
        }
    }
    Some(ProgramFiles::Selected { app_guid, files })
}

fn assemble_l3_workspace_from_program(
    workspace: &std::path::Path,
    model_instance_id: &str,
    ctx: &crate::program::resolve::full::ProgramContext,
    parenless: &crate::program::resolve::full::ParenlessCalls,
) -> Option<L3Workspace> {
    let selected = {
        let _s = pt::span("l3", "l3.select_program_parse");
        select_program_files(workspace, ctx)?
    };
    let (app_guid, files) = match selected {
        ProgramFiles::Selected { app_guid, files } => (app_guid, files),
        // The program parse is the only parse (engine-switch S9.1): a file the
        // disk discovery lists but the program did not parse is an inconsistency
        // between the two discoveries, so the model fails closed rather than
        // falling back to L3's own disk assembly.
        ProgramFiles::Missing(path) => {
            log::warn!("program parse lacks {path}; no model");
            return None;
        }
    };
    if files.is_empty() {
        return None;
    }

    let _s = pt::span("l3", "l3.project_parallel");
    // S2b.4: what exists is what the program engine extracted.
    let population = rows_population(&ctx.graph().workspace_rows);
    let no_decls = FilePopulation::new();
    let no_calls = std::collections::HashSet::new();
    // Same deterministic order and fold as `assemble_workspace`.
    let mut sorted = files;
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    use rayon::prelude::*;
    let fragments: Vec<L3Workspace> = crate::big_stack::big_stack_pool().install(|| {
        sorted
            .par_iter()
            .map(|(fname, pf)| {
                let source_unit_id = format!("ws:{fname}");
                let cols = Utf16Cols::new(&pf.text);
                let mut ws = L3Workspace {
                    objects: Vec::new(),
                    tables: Vec::new(),
                    routines: Vec::new(),
                };
                // Every parsed workspace file has its rows recorded; only a file
                // with no declarations has none.
                let pop = population.get(fname).unwrap_or_else(|| {
                    assert!(
                        pf.file.objects.is_empty(),
                        "{fname} declares objects but has no physical rows"
                    );
                    &no_decls
                });
                let calls = parenless
                    .get(&(ctx.primary_app_ref, pf.virtual_path.clone()))
                    .unwrap_or(&no_calls);
                project_ir(
                    &pf.file,
                    pop,
                    &pf.text,
                    &app_guid,
                    model_instance_id,
                    &source_unit_id,
                    &cols,
                    calls,
                    &mut ws,
                );
                ws
            })
            .collect()
    });
    let mut workspace = L3Workspace {
        objects: Vec::new(),
        tables: Vec::new(),
        routines: Vec::new(),
    };
    for mut frag in fragments {
        workspace.objects.append(&mut frag.objects);
        workspace.tables.append(&mut frag.tables);
        workspace.routines.append(&mut frag.routines);
    }
    Some(workspace)
}

/// The finishing half shared by the disk-backed and program-backed entries:
/// resolve, classify roots, read the primary app, and refuse an empty model.
fn finish_resolved(
    mut ws: L3Workspace,
    workspace: &std::path::Path,
    skip_roots_config: bool,
) -> Option<L3Resolved> {
    let resolved = {
        resolve(&mut ws);
        // R4-F: classify AST roots, then overlay `<workspace>/roots.config.json`.
        // `workspace` is the root where the config lives (mirrors al-sem's
        // index.ts: `loadRootsConfig(workspaceRoot)`).
        let roots_config_root = if skip_roots_config {
            None
        } else {
            Some(workspace)
        };
        let (root_classifications, infra_diagnostics) =
            crate::engine::root_classification::compute_root_classifications(
                &ws,
                roots_config_root,
            );
        // Disk-backed path: read the primary app's identity from `app.json`.
        // Mirrors al-sem `model.identity.primaryApp`. Never throws — returns None
        // on unreadable / malformed app.json (fail-closed / engine-never-throws).
        let primary_app = read_primary_app_from_disk(workspace);
        L3Resolved {
            workspace: ws,
            root_classifications,
            primary_app,
            infra_diagnostics,
            precomputed_calls: None,
            precomputed_events: None,
        }
    };
    // Empty fail-closed model (no objects/routines) → treat as not-analyzable.
    if resolved.workspace.objects.is_empty() && resolved.workspace.routines.is_empty() {
        return None;
    }
    Some(resolved)
}

/// Assemble the workspace L3 model from disk WITHOUT resolving — the pre-resolve
/// assembly half of [`assemble_and_resolve_workspace`], exposed so the R2.5b
/// cross-app wiring can append dep entities BEFORE running `resolve` over the
/// merged whole. Fail-closed: an unsound/empty native layout yields `None`
/// (readable root `app.json` `id`, single `app.json`, ≥1 readable `.al`).
pub fn assemble_l3_workspace_from_disk(
    workspace: &std::path::Path,
    model_instance_id: &str,
) -> Option<L3Workspace> {
    use crate::program::body::l2_workspace::{
        discover_al_files_app_scoped, read_al_source, read_root_app_guid,
    };

    let (app_guid, files): (String, Vec<(String, String)>) = {
        let _s = pt::span("l3", "l3.discover_read");
        // Fail-closed: need a readable root app.json with a string `id`. The single-app
        // guard is gone — a nested `app.json` is a SEPARATE project, so discovery is
        // scoped to THIS app (nested sub-apps are excluded). A monorepo / `Modules/`
        // layout (root app + nested apps) thus analyzes the root app; each nested app is
        // analyzed by pointing the workspace at its own root. (The gate keeps its own
        // multi-app provider check in `workspace_diagnostics` — this only relaxes the L3
        // analysis path that `aldump` / cross-app stats use.)
        let app_guid = read_root_app_guid(workspace)?;
        let discovered = discover_al_files_app_scoped(workspace).ok()?;

        // Build (relPosix, source) pairs in discovery (rel-posix-sorted) order; the
        // inline assembler re-sorts by name, which is the same total order.
        let mut files: Vec<(String, String)> = Vec::new();
        for f in &discovered {
            match read_al_source(&f.abs_path) {
                Ok(src) => files.push((f.rel_posix.clone(), src)),
                Err(e) => {
                    eprintln!("warning: skipping {} (read error: {e})", f.rel_posix);
                }
            }
        }
        (app_guid, files)
    };

    if files.is_empty() {
        return None;
    }

    Some(assemble_workspace(&files, &app_guid, model_instance_id))
}

/// Disk-backed convenience with the default model-instance id (`r0`).
pub fn assemble_and_resolve_workspace_default(workspace: &std::path::Path) -> Option<L3Resolved> {
    assemble_and_resolve_workspace(workspace, MODEL_INSTANCE_ID_DEFAULT, false)
}

/// Read the primary app's identity from the workspace root `app.json`.
/// Mirrors `run::read_workspace_apps` but returns `Option<App>` instead of `Vec<App>`.
/// Engine-never-throws: returns `None` on any read/parse failure.
fn read_primary_app_from_disk(
    workspace: &std::path::Path,
) -> Option<crate::engine::gate::app_attribution::App> {
    let text = std::fs::read_to_string(workspace.join("app.json")).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let app_guid = v
        .get("id")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())?;
    let publisher = v
        .get("publisher")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let name = v
        .get("name")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_string();
    let version = v
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();
    Some(crate::engine::gate::app_attribution::App {
        app_guid: app_guid.to_string(),
        publisher,
        name,
        version,
    })
}

/// Run the three L3 resolve sub-steps over an assembled workspace IN ORDER:
/// `build_symbol_table → resolve_record_types → merge_extension_fields`.
/// `tableId` is set by record-types and never re-touched by the merge.
///
/// L3-ONLY BOUNDARY (R2.5b Rev 2 #5): the input `L3Workspace`
/// (objects/tables/routines) is an L3-only merged index. Its entity structs carry
/// NO L4/cone/summary field — there is no `summary`, `intraAppCallEdges`,
/// `citedOperationEvidence`, `depOrderIndex`, capability-cone, or typed-edge field
/// anywhere on `L3Object`/`L3Table`/`L3Routine`. So L4 state CANNOT influence L3:
/// the boundary is enforced by the TYPE, not a runtime strip. When R2.5b feeds the
/// merged (workspace + `.app`-dep) index here (`deps::cross_app_l3`), the dep side
/// likewise comes from `project_abi_to_index`, which emits only these L3 structs.
/// DO NOT add an L4 field to these entity structs (it would breach the boundary the
/// `cross_app_l3_poison` test guards). NOTHING in `resolve` reads beyond them.
pub fn resolve(workspace: &mut L3Workspace) {
    let _s = pt::span("l3", "l3.resolve");
    // ROUTINE-FREE index (see `SymbolTable::build_without_routines`): the loop
    // below takes `&mut workspace.routines`, so the table — which borrows the
    // workspace rather than cloning it — must not hold `&workspace.routines`.
    // It never needed to: `resolve_routine_record_types` only ever asks the
    // table about objects and tables. `objects`/`tables` are disjoint fields, so
    // borrowing them immutably alongside the mutable routine walk is fine.
    let symbols = SymbolTable::build_without_routines(&workspace.objects, &workspace.tables);

    // objectId → object, so a routine maps back to its owning object. Borrowed,
    // not cloned — `resolve_routine_record_types` takes `Option<&L3Object>`, and
    // `HashMap::insert` is LAST-wins on a duplicate id either way.
    use std::collections::HashMap;
    let object_by_id: HashMap<&str, &L3Object> = workspace
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o))
        .collect();

    for routine in &mut workspace.routines {
        let object = object_by_id.get(routine.object_id.as_str()).copied();
        resolve_routine_record_types(routine, object, &symbols);
    }

    merge_extension_fields(workspace);
}

// ---------------------------------------------------------------------------
// Resolved-projection accessors (StableTableId form) — the test/dump surface.
// ---------------------------------------------------------------------------

/// Project an internal TableId (`${appGuid}/table/${n}`) to its StableTableId
/// (`${appGuid}:Table:${n}`). Mirrors al-sem `toStableTableId`.
pub fn to_stable_table_id(internal: &str) -> String {
    let parts: Vec<&str> = internal.split('/').collect();
    if parts.len() == 3 && parts[1] == "table" {
        format!("{}:Table:{}", parts[0], parts[2])
    } else {
        // Defensive: never panic in the engine. Return as-is (will fail compare).
        internal.to_string()
    }
}

/// A resolved workspace, exposing the StableTableId-projected lookups the parity
/// surface compares.
pub struct L3Resolved {
    pub workspace: L3Workspace,
    /// R4-F root classifications (`model.rootClassifications`): the AST root
    /// classifier overlaid with any `<workspace>/roots.config.json`. Computed at
    /// the disk-backed resolve entry (`assemble_and_resolve_workspace`, where the
    /// workspace root is known); the inline / cross-app constructors that have no
    /// disk config populate the AST-only set (empty config). Consumed by the L5
    /// `DetectorContext` (d50/d51) and the R4-F stable projection.
    pub root_classifications: Vec<crate::engine::root_classification::RootClassification>,
    /// The primary app's identity (`model.identity.primaryApp`): name / publisher /
    /// version read from the workspace `app.json`. Populated by the disk-backed
    /// assembly path (`assemble_and_resolve_workspace`); `None` in the inline /
    /// cross-app constructors (no disk `app.json` to read). Consumed by the html
    /// formatter's masthead/title (Stage A3) and any future envelope that needs the
    /// primary app description. Additive — `L3Resolved` is NOT serialized into any
    /// golden surface, so adding this field never moves a golden.
    pub primary_app: Option<crate::engine::gate::app_attribution::App>,
    /// Infrastructure diagnostics from the root-classification overlay (e.g.
    /// `kinds-mismatch` warnings from `roots.config.json`). Empty for inline /
    /// cross-app paths that have no disk config. Propagated to the JSON envelope.
    pub infra_diagnostics: Vec<crate::engine::root_classification::InfraDiagnostic>,
    /// An externally supplied call resolution. `Some` on the `alsem analyze` path
    /// and in the r4/r4f test helper (both set by
    /// `program_calls::attach_program_calls`: the program engine's calls); `None`
    /// for every other consumer, which runs `resolve_calls` itself (CLAUDE.md lists
    /// them). When `Some`, these sites read it via `call_resolver::calls_for`:
    /// - on the `alsem analyze` path: the detector context
    ///   (`build_detector_context`), the ordering-facts base
    ///   (`build_r3a3_source_only_base`) and coverage (`project_coverage`);
    /// - switched but NOT on the analyze path: `project_r3a3` and
    ///   `compute_r3a3_real_matrix`;
    /// - the cross-app base (`capability_cone::build_cross_app_base`, engine-switch
    ///   S7.4) reads the cross-app model's, set by
    ///   `program_calls::assemble_and_resolve_cross_app_program`;
    /// - still L3's own resolver: `project_coverage_cross_app` and the `--l3-*`
    ///   cross-app modes, which measure L3 (S9).
    pub precomputed_calls: Option<std::sync::Arc<super::calls::ResolvedCalls>>,
    /// The detector event graph built from the program engine's subscription
    /// inventory (engine-switch S4.2). Set together with `precomputed_calls`, by
    /// the same function, for the same consumers; they read it through
    /// `event_graph::events_for`. `None` everywhere else, which builds L3's
    /// source-only event graph.
    pub precomputed_events: Option<std::sync::Arc<super::events::ProgramEvents>>,
}

// ---------------------------------------------------------------------------
// Serde projection — the golden-shaped L3 record-type projection (matches
// `scripts/r2a-l3-projection.ts` / `*.l3rt.golden.json` EXACTLY).
//
// ALLOWLIST: only the record-type surface. Every field is built key-by-key, so
// later-gate fields (callGraph/eventGraph/coverage/typedEdges/resourceId/
// argumentBindings upgrades) cannot leak through. `tableId` is OMITTED when
// unresolved (serde `skip_serializing_if`), matching al-sem (never guessed).
// ---------------------------------------------------------------------------

/// A resolved record VARIABLE: lowercased name + resolved StableTableId (omitted
/// when unresolved).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PRecordVariable {
    pub name: String,
    #[serde(rename = "tableId", skip_serializing_if = "Option::is_none")]
    pub table_id: Option<String>,
}

/// A resolved record OPERATION keyed by operationId.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PRecordOperation {
    #[serde(rename = "operationId")]
    pub operation_id: String,
    pub op: String,
    #[serde(rename = "recordVariableName")]
    pub record_variable_name: String,
    #[serde(rename = "tableId", skip_serializing_if = "Option::is_none")]
    pub table_id: Option<String>,
}

/// Per-routine record-type resolution surface, keyed by StableRoutineId.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PRoutineRecordTypes {
    #[serde(rename = "stableRoutineId")]
    pub stable_routine_id: String,
    pub name: String,
    #[serde(rename = "objectType")]
    pub object_type: String,
    #[serde(rename = "recordVariables")]
    pub record_variables: Vec<PRecordVariable>,
    #[serde(rename = "recordOperations")]
    pub record_operations: Vec<PRecordOperation>,
}

/// One MERGED field (post `merge_extension_fields`) in stable id form.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PMergedField {
    #[serde(rename = "fieldNumber")]
    pub field_number: i64,
    pub name: String,
    #[serde(rename = "dataType")]
    pub data_type: String,
    #[serde(rename = "fieldClass")]
    pub field_class: String,
    #[serde(rename = "declaringObjectId")]
    pub declaring_object_id: String,
}

/// A Table with its MERGED field set (base + TableExtension fields).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PTableRecordTypes {
    #[serde(rename = "stableTableId")]
    pub stable_table_id: String,
    pub name: String,
    pub fields: Vec<PMergedField>,
}

/// The full L3 record-type projection — the golden document shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct L3RecordTypeProjection {
    pub tables: Vec<PTableRecordTypes>,
    pub routines: Vec<PRoutineRecordTypes>,
}

impl L3Resolved {
    /// Project the resolved workspace to the golden-shaped L3 record-type
    /// projection. Tables sorted by StableTableId; routines by StableRoutineId;
    /// record vars by (name, tableId); record ops by operationId; fields by
    /// (fieldNumber, name) — matching `scripts/r2a-l3-projection.ts`.
    pub fn project(&self) -> L3RecordTypeProjection {
        let mut tables: Vec<PTableRecordTypes> = self
            .workspace
            .tables
            .iter()
            .map(|t| {
                let mut fields: Vec<PMergedField> = t
                    .fields
                    .iter()
                    .map(|f| PMergedField {
                        field_number: f.field_number,
                        name: f.name.clone(),
                        data_type: f.data_type.clone(),
                        field_class: f.field_class.clone(),
                        declaring_object_id: to_stable_object_id(&f.declaring_object_id),
                    })
                    .collect();
                fields.sort_by(|a, b| {
                    a.field_number
                        .cmp(&b.field_number)
                        .then_with(|| a.name.cmp(&b.name))
                });
                PTableRecordTypes {
                    stable_table_id: to_stable_table_id(&t.id),
                    name: t.name.clone(),
                    fields,
                }
            })
            .collect();
        tables.sort_by(|a, b| a.stable_table_id.cmp(&b.stable_table_id));

        let mut routines: Vec<PRoutineRecordTypes> = self
            .workspace
            .routines
            .iter()
            .map(|r| {
                let mut record_variables: Vec<PRecordVariable> = r
                    .record_variables
                    .iter()
                    .map(|v| PRecordVariable {
                        name: v.name.to_lowercase(),
                        table_id: v.table_id.as_deref().map(to_stable_table_id),
                    })
                    .collect();
                record_variables.sort_by(|a, b| {
                    a.name.cmp(&b.name).then_with(|| {
                        a.table_id
                            .clone()
                            .unwrap_or_default()
                            .cmp(&b.table_id.clone().unwrap_or_default())
                    })
                });

                let mut record_operations: Vec<PRecordOperation> = r
                    .record_operations
                    .iter()
                    .map(|op| PRecordOperation {
                        operation_id: op.id.clone(),
                        op: op.op.clone(),
                        record_variable_name: op.record_variable_name.to_lowercase(),
                        table_id: op.table_id.as_deref().map(to_stable_table_id),
                    })
                    .collect();
                record_operations.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));

                PRoutineRecordTypes {
                    stable_routine_id: r.stable_routine_id.clone(),
                    name: r.name.clone(),
                    object_type: r.object_type.clone(),
                    record_variables,
                    record_operations,
                }
            })
            .collect();
        routines.sort_by(|a, b| a.stable_routine_id.cmp(&b.stable_routine_id));

        L3RecordTypeProjection { tables, routines }
    }
}

/// A merged field projected for comparison (StableObjectId declaring provenance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectedField {
    pub field_number: i64,
    pub name: String,
    pub data_type: String,
    pub field_class: String,
    pub declaring_object_id: String,
}

/// A routine view exposing resolved record var / op StableTableIds by name.
pub struct RoutineView<'a> {
    routine: &'a L3Routine,
}

impl L3Resolved {
    /// Find a routine by name (first match in assembled order).
    pub fn routine_by_name(&self, name: &str) -> Option<RoutineView<'_>> {
        self.workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .map(|routine| RoutineView { routine })
    }

    /// Find a table by name (case-insensitive, LAST-wins — matching the symbol
    /// table the resolution queried).
    pub fn table_by_name(&self, name: &str) -> Option<TableView<'_>> {
        let want = name.to_lowercase();
        let mut found = None;
        for t in &self.workspace.tables {
            if t.name.to_lowercase() == want {
                found = Some(t); // LAST-wins
            }
        }
        found.map(|table| TableView { table })
    }
}

impl RoutineView<'_> {
    /// Resolved StableTableId for the named record variable, or None if unresolved
    /// / absent.
    pub fn record_var_table_id(&self, name: &str) -> Option<String> {
        let want = name.to_lowercase();
        self.routine
            .record_variables
            .iter()
            .find(|v| v.name.to_lowercase() == want)
            .and_then(|v| v.table_id.as_deref().map(to_stable_table_id))
    }

    pub fn record_var_count(&self) -> usize {
        self.routine.record_variables.len()
    }

    /// The `scope` (`"local"` | `"parameter"` | `"global"`) of the named record
    /// variable, or None if absent / unset. Test-facing accessor for the Task 3
    /// global-promotion path.
    pub fn record_var_scope(&self, name: &str) -> Option<String> {
        let want = name.to_lowercase();
        self.routine
            .record_variables
            .iter()
            .find(|v| v.name.to_lowercase() == want)
            .and_then(|v| v.scope.clone())
    }

    /// The resolved `temp_state` Known value of the named record variable, or None
    /// if the var is absent or its temp_state is not `known`.
    pub fn record_var_temp_known(&self, name: &str) -> Option<bool> {
        let want = name.to_lowercase();
        self.routine
            .record_variables
            .iter()
            .find(|v| v.name.to_lowercase() == want)
            .and_then(|v| v.temp_state_known_value())
    }

    /// The resolved `temp_state` Known value of the FIRST record OP on the named
    /// record variable, or None if absent / not `known`. Test-facing accessor for
    /// the Task 3 member-var op temp_state backfill. Returns the FIRST matching op's
    /// state (walk order) — sufficient for the single-op-per-var test fixtures.
    pub fn first_record_op_temp_known(&self, var_name: &str) -> Option<bool> {
        let want = var_name.to_lowercase();
        self.routine
            .record_operations
            .iter()
            .find(|op| op.record_variable_name.to_lowercase() == want)
            .and_then(|op| op.temp_state.as_ref())
            .and_then(|ts| if ts.kind == "known" { ts.value } else { None })
    }

    /// All record ops in walk order as `(op, recordVariableName, Option<StableTableId>)`.
    pub fn record_ops(&self) -> Vec<(String, String, Option<String>)> {
        self.routine
            .record_operations
            .iter()
            .map(|op| {
                (
                    op.op.clone(),
                    op.record_variable_name.clone(),
                    op.table_id.as_deref().map(to_stable_table_id),
                )
            })
            .collect()
    }
}

/// A table view exposing the merged fields (StableObjectId provenance).
pub struct TableView<'a> {
    table: &'a L3Table,
}

impl TableView<'_> {
    pub fn stable_table_id(&self) -> String {
        to_stable_table_id(&self.table.id)
    }

    /// True when the table is declared `TableType = Temporary` (Task 4 Part A).
    pub fn is_temporary(&self) -> bool {
        self.table.is_temporary
    }

    /// Merged fields in stored order, declaringObjectId projected to StableObjectId.
    pub fn merged_fields(&self) -> Vec<ProjectedField> {
        self.table
            .fields
            .iter()
            .map(|f| ProjectedField {
                field_number: f.field_number,
                name: f.name.clone(),
                data_type: f.data_type.clone(),
                field_class: f.field_class.clone(),
                declaring_object_id: to_stable_object_id(&f.declaring_object_id),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(name: &str, ty: &str, scope: &str) -> L3Variable {
        L3Variable {
            name: name.to_string(),
            declared_type: ty.to_string(),
            is_parameter: scope == "parameter",
            parameter_index: None,
            initializer: None,
            scope: Some(scope.to_string()),
        }
    }

    fn names(v: &RoutineVariables) -> Vec<&str> {
        v.iter().map(|x| x.name.as_str()).collect()
    }

    /// L-3: the shared form must yield params → locals → globals, with any global
    /// the routine's own params/locals shadow dropped — the flat list `ir_variables`
    /// used to materialize per routine.
    #[test]
    fn routine_variables_yields_own_then_unshadowed_globals() {
        let globals: Arc<[L3Variable]> = Arc::from(vec![
            var("ga", "Integer", "global"),
            var("shadowed", "Integer", "global"),
            var("gb", "Text", "global"),
        ]);

        // No shadowing: every global is visible, after the routine's own vars.
        let plain = RoutineVariables::new(
            vec![var("p", "Integer", "parameter"), var("l", "Text", "local")],
            Arc::clone(&globals),
        );
        assert_eq!(names(&plain), ["p", "l", "ga", "shadowed", "gb"]);

        // A local of the same lowercased name hides exactly that one global, and the
        // survivors keep their relative order.
        let shadowing = RoutineVariables::new(
            vec![
                var("p", "Integer", "parameter"),
                var("shadowed", "Code[20]", "local"),
            ],
            Arc::clone(&globals),
        );
        assert_eq!(names(&shadowing), ["p", "shadowed", "ga", "gb"]);
        assert_eq!(
            shadowing
                .iter()
                .find(|v| v.name == "shadowed")
                .unwrap()
                .declared_type,
            "Code[20]",
            "the routine's OWN declaration must win, not the object global's",
        );

        // Degenerate shapes: no globals, and no own vars.
        assert_eq!(
            names(&RoutineVariables::from_own(vec![var(
                "only", "Integer", "local"
            )])),
            ["only"]
        );
        assert_eq!(
            names(&RoutineVariables::new(Vec::new(), Arc::clone(&globals))),
            ["ga", "shadowed", "gb"]
        );
        assert!(names(&RoutineVariables::default()).is_empty());
    }

    /// The globals are SHARED, not copied: two routines of the same object hold the
    /// same allocation. This is the whole point of the representation.
    #[test]
    fn routine_variables_share_one_global_allocation() {
        let globals: Arc<[L3Variable]> = Arc::from(vec![var("g", "Integer", "global")]);
        let a = RoutineVariables::new(vec![var("x", "Integer", "local")], Arc::clone(&globals));
        let b = RoutineVariables::new(vec![var("y", "Integer", "local")], Arc::clone(&globals));
        assert!(
            std::ptr::eq(
                a.iter().find(|v| v.name == "g").unwrap(),
                b.iter().find(|v| v.name == "g").unwrap(),
            ),
            "sibling routines must see the SAME global element, not per-routine copies",
        );
    }

    /// Duplicate object globals (the `#if`/`#else` shape — the IR carries both
    /// branches) are deduped first-wins ONCE, at the object level.
    ///
    /// ⟨task-5-review.md finding F3⟩ The prior version of this test asserted
    /// name-uniqueness over `tests/fixtures/props`, whose only routine-bearing
    /// object declares exactly one global and one routine — three distinct names,
    /// so the `seen.insert` assertion was unfalsifiable there: it passed whether or
    /// not `ir_object_globals`'s dedup existed at all. It also attributed its
    /// coverage to the `#if`/`#else` shape while no `#if` fixture existed anywhere
    /// in the repo (`grep -rl "#if" tests/ --include=*.al` returned zero files).
    /// This version hand-states the precondition: a REAL `#if`/`#else` global
    /// declaration (the lowerer never evaluates the condition — see
    /// `is_preproc_wrapper`'s module doc in `crates/al-syntax/src/lower/mod.rs` —
    /// so both branches are always collected into `ObjectDecl::globals`), run
    /// through the real dedup via full assembly, asserting both that the
    /// duplicate collapses to ONE entry and that the FIRST branch's declared type
    /// is the one that survives.
    #[test]
    fn object_globals_are_deduped_first_wins() {
        let src = r#"
codeunit 50816 "T5 F3 Dedup"
{
    var
#if T5F3COND
        MyGlobal: Integer;
#else
        MyGlobal: Text;
#endif

    procedure DoIt()
    begin
    end;
}
"#;
        let files = vec![("src/T5F3Dedup.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_default(&files, "66666666-0000-0000-0000-0000000cp5f3");
        let routine = resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case("DoIt"))
            .expect("fixture precondition: the DoIt procedure");

        let globals: Vec<&L3Variable> = routine
            .variables
            .iter()
            .filter(|v| v.name == "myglobal")
            .collect();
        assert_eq!(
            globals.len(),
            1,
            "the #if/#else branches must collapse to ONE `myglobal` entry, not two \
             (got {globals:?})"
        );
        assert_eq!(
            globals[0].declared_type, "Integer",
            "first-wins: the #if branch's `Integer` must survive over the #else \
             branch's `Text`"
        );
    }

    // Task 2 (BCQuality wave): `SingleInstance` (Codeunit) + the Page write-surface
    // booleans (`Editable`/`InsertAllowed`/`ModifyAllowed`/`DeleteAllowed`) forward
    // onto `L3Object`, plus the object decl's own `source_anchor` — substrate for
    // d57/d64. Fixture: `tests/fixtures/props/`.
    #[test]
    fn object_property_forwards_single_instance_and_page_write_surface() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/props");
        let resolved = assemble_and_resolve_workspace_default(&dir).expect("assemble");
        let cu = resolved
            .workspace
            .objects
            .iter()
            .find(|o| o.name == "Si")
            .unwrap();
        assert_eq!(cu.single_instance, Some(true));
        let pg = resolved
            .workspace
            .objects
            .iter()
            .find(|o| o.name == "Api")
            .unwrap();
        assert_eq!(pg.editable, Some(false));
        assert_eq!(pg.insert_allowed, Some(false));
        assert_eq!(pg.modify_allowed, None);
        assert_eq!(pg.delete_allowed, None);
        assert!(
            pg.source_anchor.is_some(),
            "native objects carry their decl anchor"
        );
    }

    // Task 3 (BCQuality wave): `PVariableSymbol.scope` forwards onto `L3Variable` —
    // substrate for d57. Fixture: `tests/fixtures/props/` (adds `src/b.al`).
    #[test]
    fn variable_scope_forwarded_to_l3() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/props");
        let resolved = assemble_and_resolve_workspace_default(&dir).expect("assemble");
        let r = resolved
            .workspace
            .routines
            .iter()
            .find(|r| r.name == "P")
            .unwrap();
        let scope_of = |n: &str| {
            r.variables
                .iter()
                .find(|v| v.name.eq_ignore_ascii_case(n))
                .and_then(|v| v.scope.clone())
        };
        assert_eq!(scope_of("GlobalNames").as_deref(), Some("global"));
        assert_eq!(scope_of("LocalN").as_deref(), Some("local"));
        assert_eq!(scope_of("ParamN").as_deref(), Some("parameter"));
    }

    /// Task 3: the member discriminator, end-to-end on real assembled source.
    ///
    /// Two page actions and two page fields each declare `trigger OnAction()` /
    /// `trigger OnValidate()`. Every one of the four is identical in all six legacy
    /// key parts (app / object type / object number / kind / name / signature), so
    /// before this change each PAIR shared one internal routine id — the defect that
    /// collapsed 23.9 % of DO routines. They must now be pairwise distinct, and the
    /// object-level `OnOpenPage` trigger and the plain procedure (no enclosing
    /// member) must still be distinct from everything.
    #[test]
    fn same_name_member_triggers_get_distinct_routine_ids() {
        let src = r#"
page 50813 "CP3 Wizard"
{
    PageType = Card;
    SourceTable = "CP3 Setup";

    layout
    {
        area(Content)
        {
            field(Alpha; Rec."No.") { trigger OnValidate() begin end; }
            field("Be""ta"; Rec."No.") { trigger OnValidate() begin end; }
        }
    }

    actions
    {
        area(Processing)
        {
            action(First) { trigger OnAction() begin end; }
            action(Second) { trigger OnAction() begin end; }
        }
    }

    trigger OnOpenPage() begin end;

    local procedure Touch() begin end;
}
"#;
        let files = vec![("src/CP3Wizard.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_default(&files, "33333333-0000-0000-0000-0000000cp003");
        let routines = &resolved.workspace.routines;

        let ids_named = |n: &str| -> Vec<String> {
            let mut v: Vec<String> = routines
                .iter()
                .filter(|r| r.name.eq_ignore_ascii_case(n))
                .map(|r| r.id.clone())
                .collect();
            v.sort();
            v
        };

        for name in ["OnValidate", "OnAction"] {
            let ids = ids_named(name);
            assert_eq!(ids.len(), 2, "fixture precondition: two `{name}` bodies");
            assert_ne!(
                ids[0], ids[1],
                "two `{name}` triggers under different members must not share an id"
            );
        }

        // …and the discriminator is the ONLY reason they differ. Re-mint each pair's
        // key with `enclosing_member: None` — the pre-Task-3 six-part schema — and
        // assert the pair collides there. This states "these two DID share an id
        // before the member discriminator" as an executable fact rather than a claim
        // in a comment, so the test cannot pass for an unrelated reason.
        for name in ["OnValidate", "OnAction"] {
            let legacy: Vec<String> = routines
                .iter()
                .filter(|r| r.name.eq_ignore_ascii_case(name))
                .map(|r| {
                    crate::engine::ids::encode_canonical_routine_key(
                        &crate::engine::ids::CanonicalRoutineKey {
                            app_guid: r.app_guid.clone(),
                            object_type: r.object_type.clone(),
                            object_number: r.object_number,
                            routine_kind: r.kind.clone(),
                            routine_name: r.name.clone(),
                            normalized_signature_hash: r.normalized_signature_hash.clone(),
                            enclosing_member: None,
                        },
                    )
                })
                .collect();
            assert_eq!(
                legacy[0], legacy[1],
                "precondition: without the member discriminator both `{name}` bodies \
                 hash to one key — that is the defect being closed"
            );
        }

        // The member NAME is what separates them — carried, unescaped, on the model.
        let mut members: Vec<String> = routines
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case("OnValidate"))
            .filter_map(|r| r.enclosing_member.clone())
            .collect();
        members.sort();
        assert_eq!(members, vec!["Alpha".to_string(), r#"Be"ta"#.to_string()]);

        // Whole-object closure: every routine in the fixture has its own id, and the
        // two member-less routines are in that set too.
        let all: std::collections::HashSet<&str> = routines.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            all.len(),
            routines.len(),
            "no two routines in this object may share an internal id"
        );
        assert!(
            routines
                .iter()
                .any(|r| r.name.eq_ignore_ascii_case("OnOpenPage") && r.enclosing_member.is_none()),
            "an object-level trigger carries no enclosing member (its id is unchanged)"
        );
    }

    /// Task 4: the same discriminator on the **STABLE** id — the one that reaches
    /// user baselines.
    ///
    /// Task 3 separated the four sibling bodies of the fixture above in the model,
    /// but their FINDINGS still hashed to one fingerprint, because
    /// `l5::fingerprint::fingerprint_of` substitutes internal ids to their STABLE
    /// image before hashing and the stable id was still member-blind. Same fixture,
    /// same closure, one level up — plus the executable precondition that the
    /// pre-Task-4 form (member-less) DID collide, so this cannot pass for an
    /// unrelated reason.
    #[test]
    fn same_name_member_triggers_get_distinct_stable_routine_ids() {
        let src = r#"
page 50815 "CP5 Wizard"
{
    PageType = Card;
    SourceTable = "CP5 Setup";

    layout
    {
        area(Content)
        {
            field(Alpha; Rec."No.") { trigger OnValidate() begin end; }
            field("Be""ta"; Rec."No.") { trigger OnValidate() begin end; }
        }
    }

    actions
    {
        area(Processing)
        {
            action(First) { trigger OnAction() begin end; }
            action(Second) { trigger OnAction() begin end; }
        }
    }

    trigger OnOpenPage() begin end;

    local procedure Touch() begin end;
}
"#;
        let files = vec![("src/CP5Wizard.al".to_string(), src.to_string())];
        let resolved = assemble_and_resolve_default(&files, "55555555-0000-0000-0000-0000000cp005");
        let routines = &resolved.workspace.routines;

        let named = |n: &str| -> Vec<&L3Routine> {
            routines
                .iter()
                .filter(|r| r.name.eq_ignore_ascii_case(n))
                .collect()
        };

        for name in ["OnValidate", "OnAction"] {
            let pair = named(name);
            assert_eq!(pair.len(), 2, "fixture precondition: two `{name}` bodies");
            assert_ne!(
                pair[0].stable_routine_id, pair[1].stable_routine_id,
                "two `{name}` triggers under different members must not share a STABLE id"
            );
            // The precondition, executable: the pre-Task-4 form — the same object and
            // the same signature hash, with no member — collides for the pair.
            let legacy: Vec<String> = pair
                .iter()
                .map(|r| {
                    crate::engine::ids::to_stable_routine_id_from_parts(
                        &to_stable_object_id(&r.object_id),
                        &r.normalized_signature_hash,
                        None,
                    )
                })
                .collect();
            assert_eq!(
                legacy[0], legacy[1],
                "precondition: without the member discriminator both `{name}` bodies \
                 share ONE stable id — that is what made their findings share a \
                 fingerprint and be cross-suppressed by a single baseline entry"
            );
        }

        // The member-LESS routines keep the legacy stable id byte-for-byte, which is
        // why only member-anchored fingerprints move in a user's baseline.
        for name in ["OnOpenPage", "Touch"] {
            let r = named(name);
            assert_eq!(r.len(), 1, "fixture precondition: one `{name}`");
            let r = r[0];
            assert!(r.enclosing_member.is_none());
            assert_eq!(
                r.stable_routine_id,
                format!(
                    "{}#{}",
                    to_stable_object_id(&r.object_id),
                    r.normalized_signature_hash
                ),
                "a routine with no enclosing member keeps `{{stableObjectId}}#{{normalizedSignatureHash}}`"
            );
        }

        // Whole-object closure on the stable id too.
        let all: std::collections::HashSet<&str> = routines
            .iter()
            .map(|r| r.stable_routine_id.as_str())
            .collect();
        assert_eq!(
            all.len(),
            routines.len(),
            "no two routines in this object may share a stable routine id"
        );
    }

    /// ⟨task-3 fix wave, review M-8⟩ The L2 and L3 id paths must agree on an
    /// **ESCAPED** member name.
    ///
    /// Two independent paths mint the internal routine id for the same source
    /// routine: `l2_workspace::build_proutine` and `l3_workspace::project_file`.
    /// Both take the discriminator from `ir_walk::ir_enclosing_member`, which is the
    /// single place `scope::unescape_al_identifier` is applied — the raw IR value is
    /// only outer-quote-stripped, so a path that fed the raw value would mint a
    /// DIFFERENT id for the same routine and silently split the cross-path join.
    /// That normalization split was called out as *the* trap of this design, and
    /// until now the guarantee was structural only: no golden fixture has an escaped
    /// member, and `same_name_member_triggers_get_distinct_routine_ids` exercises
    /// `field("Be""ta"; …)` through the L3 path alone. This pins it executably.
    ///
    /// The internal id is not a field on `PRoutine` (which carries only
    /// `stableRoutineId`), but it IS observable: `build_proutine` feeds it to
    /// `project_routine_features_ir`, which mints every op/callsite id as
    /// `{routine_id}/op{n}` / `{routine_id}/cs{n}`. Stripping the last `/`-segment
    /// recovers exactly the id the L2 path computed — the same recovery
    /// `call_graph_projection::StableMap::stable_site` performs in production.
    #[test]
    fn l2_and_l3_agree_on_the_id_of_an_escaped_enclosing_member() {
        // ONE `OnValidate`, under a member whose logical name contains a literal `"`
        // (`Be"ta`, written `"Be""ta"`), so `project_named_routine`'s first-match
        // lookup is unambiguous. Its body has a db op purely so the L2 features carry
        // an id to recover the routine id from.
        let src = r#"
table 50814 "CP4 Setup"
{
    fields { field(1; "No."; Code[20]) { } }
    keys { key(PK; "No.") { } }
}

page 50814 "CP4 Wizard"
{
    PageType = Card;
    SourceTable = "CP4 Setup";

    layout
    {
        area(Content)
        {
            field("Be""ta"; Rec."No.")
            {
                trigger OnValidate()
                var
                    Setup: Record "CP4 Setup";
                begin
                    Setup.Insert();
                end;
            }
        }
    }
}
"#;
        const APP_GUID: &str = "44444444-0000-0000-0000-0000000cp004";
        const UNIT: &str = "ws:src/CP4Wizard.al";

        // --- L3 path -------------------------------------------------------
        let files = vec![(UNIT.trim_start_matches("ws:").to_string(), src.to_string())];
        let resolved = assemble_and_resolve_default(&files, APP_GUID);
        let l3: Vec<&L3Routine> = resolved
            .workspace
            .routines
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case("OnValidate"))
            .collect();
        assert_eq!(l3.len(), 1, "fixture precondition: exactly one OnValidate");
        let l3_routine = l3[0];
        assert_eq!(
            l3_routine.enclosing_member.as_deref(),
            Some(r#"Be"ta"#),
            "the stored member must be the UNESCAPED logical identifier"
        );

        // --- L2 path -------------------------------------------------------
        let l2 = crate::program::body::l2_workspace::project_named_routine(
            src,
            "OnValidate",
            APP_GUID,
            UNIT,
        )
        .expect("L2 must project the OnValidate trigger");
        let witness = l2
            .features
            .record_operations
            .first()
            .expect("fixture precondition: the trigger body has a record operation");
        let (l2_routine_id, _) = witness
            .id
            .rsplit_once('/')
            .expect("an op id is `{routine_id}/op{n}`");

        assert_eq!(
            l2_routine_id, l3_routine.id,
            "the L2 and L3 id paths must mint the SAME internal routine id for a \
             routine under an ESCAPED enclosing member — a divergence here means one \
             path stopped going through `ir_enclosing_member`'s single unescape"
        );

        // ⟨task 4⟩ The same agreement on the STABLE id, which `PRoutine` carries
        // directly. This is the half that reaches user baselines: a normalization
        // split here would give the same routine two fingerprints depending on which
        // path produced it.
        assert_eq!(
            l2.stable_routine_id, l3_routine.stable_routine_id,
            "the L2 and L3 paths must mint the SAME STABLE routine id for a routine \
             under an ESCAPED enclosing member"
        );
        // …and it is genuinely discriminated, not accidentally equal to the legacy
        // member-less form.
        assert_ne!(
            l3_routine.stable_routine_id,
            crate::engine::ids::to_stable_routine_id_from_parts(
                &to_stable_object_id(&l3_routine.object_id),
                &l3_routine.normalized_signature_hash,
                None,
            ),
            "a member trigger's stable id must differ from its pre-discriminator form"
        );
        assert_ne!(
            l3_routine.stable_routine_id,
            crate::engine::ids::to_stable_routine_id_from_parts(
                &to_stable_object_id(&l3_routine.object_id),
                &l3_routine.normalized_signature_hash,
                Some(r#"Be""ta"#),
            ),
            "the escaped and unescaped member texts must fold differently into the \
             STABLE id too — otherwise this test could not tell a missing unescape \
             from a present one"
        );

        // …and the agreement is on the UNESCAPED form specifically. Re-mint the id
        // from the RAW (still-escaped) member text and assert it differs: without
        // this, both paths agreeing on a wrongly-escaped string would pass the
        // assertion above and the test would prove nothing about the normalization.
        let params = crate::program::body::ir_walk::ir_parameter_symbols(
            al_syntax::parse(src).objects[1]
                .routines
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case("OnValidate"))
                .expect("IR must carry the OnValidate trigger"),
        );
        let raw_escaped_id = crate::program::body::scope::compute_routine_id(
            APP_GUID,
            "Page",
            50814,
            "trigger",
            "OnValidate",
            Some(r#"Be""ta"#),
            &params,
            None,
            MODEL_INSTANCE_ID_DEFAULT,
        );
        assert_ne!(
            raw_escaped_id, l3_routine.id,
            "the escaped and unescaped member texts must hash differently — otherwise \
             this test could not tell a missing unescape from a present one"
        );
    }
}

#[cfg(test)]
mod population_tests {
    use super::*;

    /// S2b.4: the analyze model projects what the program graph's physical rows
    /// name. Hand-stated precondition: one routine's row is removed from a real
    /// program context. The model must lose exactly that routine — a model that
    /// walked the syntax tree on its own would still have it.
    #[test]
    fn model_population_follows_the_physical_rows() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let (mut ctx, _report, _) =
            crate::program::resolve::full::build_program_with_coverage(&ws).unwrap();
        let full =
            assemble_and_resolve_workspace_from_program(&ws, "r0", true, &ctx, &Default::default())
                .unwrap();

        // Drop the first codeunit routine row (interfaces are skipped anyway).
        let k = ctx
            .graph
            .workspace_rows
            .routines
            .iter()
            .position(|r| r.node.object.kind == al_syntax::ir::ObjectKind::Codeunit)
            .unwrap();
        let dropped = ctx.graph.workspace_rows.routines.remove(k);
        let fewer =
            assemble_and_resolve_workspace_from_program(&ws, "r0", true, &ctx, &Default::default())
                .unwrap();

        assert_eq!(
            fewer.workspace.routines.len() + 1,
            full.workspace.routines.len()
        );
        let at = |r: &L3Routine| {
            (
                r.source_anchor.source_unit_id.clone(),
                r.source_anchor.start_line,
                r.source_anchor.start_column,
            )
        };
        let gone = (
            format!("ws:{}", dropped.file),
            dropped.start.row,
            dropped.start.col,
        );
        assert!(full.workspace.routines.iter().any(|r| at(r) == gone));
        assert!(!fewer.workspace.routines.iter().any(|r| at(r) == gone));
    }
}
