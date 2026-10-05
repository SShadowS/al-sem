//! Rust port of al-sem's `src/symbols/symbol-reference-parser.ts` —
//! `parseSymbolReference` and its helpers.
//!
//! Produces the neutral `SymbolReferenceAbi` DTO from raw `SymbolReference.json`
//! text: object-array dispatch (ROUTINE_BEARING / EXTENSION_ROUTINE_BEARING /
//! BARE / Tables), `parse_method` (event-kind from attributes), `parse_field`,
//! `classify_abi_arg`, `abi_attribute_info`, `parse_abi_interface_name`,
//! `unquote_abi_name`, `raw_object_property`, and InherentCommitBehavior parse.
//!
//! Reuses the shared `AttributeInfo`/`AttributeArg` shape from
//! `crate::engine::l3::al_attributes` — the ABI path is a SECOND producer of the
//! SAME shape (the native AST path is the first), so the event-graph resolver and
//! attribute consumers traverse one normalized representation.
//!
//! Never panics: a JSON parse failure yields a DTO with empty objects/tables and
//! an `error` string (the TS "never throws" / catch posture).

use crate::engine::l3::al_attributes::{AttributeArg, AttributeInfo};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// "integration" | "business" | "unknown" — only meaningful when the routine kind
/// is `event-publisher`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbiEventKind {
    Integration,
    Business,
    Unknown,
}

impl AbiEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AbiEventKind::Integration => "integration",
            AbiEventKind::Business => "business",
            AbiEventKind::Unknown => "unknown",
        }
    }
}

/// The degradation SHAPE behind [`AbiParameter::type_text`]'s reconstruction
/// (round-2 addendum) — a closed set over every arm
/// [`reconstruct_param_field_type`] can return, folded into the fingerprint
/// so a genuinely scalar parameter (no `Subtype` at all) can never
/// fingerprint-collide with an object-typed parameter whose `Subtype`
/// degraded to the same bare outer-name text purely by coincidence of
/// keyword + absent raw id/name.
///
/// Was a `&'static str` carrying exactly these eight values (documented in
/// this module, but — pre-fix-round-1 — only 5 of the 8 were actually
/// asserted anywhere; see the correction below). Changed to an enum because
/// `&'static str` cannot implement `Deserialize` — there is no way to
/// produce a `'static` borrow from arbitrary input — which made every
/// struct reaching this field unserializable. See the dependency pack cache
/// spec, Task 3.
///
/// PROVENANCE (which arms the authoring plan named): the plan that
/// authored this enum documented five variants (`NoName`/`NoSubtype`/
/// `Full`/`NameQuoted`/`IdOnly`) as the closed set. The real production
/// path (`reconstruct_param_field_type`) has three MORE arms beyond that
/// five, already listed in this repo's own PRE-EXISTING doc comment (see
/// `reconstruct_param_field_type`'s doc) even though the plan's five didn't
/// name them: `NoTypeDefinition`, `AlreadyQuoted`, `EmptySubtype`. All
/// eight are represented here; leaving any of the three out would make the
/// enum lossy relative to the string it replaces.
///
/// COVERAGE (a SEPARATE fact from provenance above — do not conflate the
/// two lists, they overlap only coincidentally): pre-fix-round-1, only 5 of
/// the 8 variants had ANY test asserting them via the real parse path
/// anywhere in the workspace, and the 3 with ZERO such coverage were
/// `NoTypeDefinition`, `NoName`, `EmptySubtype` — NOT the same 3 named
/// above (`NoName` was one of the plan's original five yet was untested;
/// `AlreadyQuoted` was outside the plan's five yet WAS already tested, by
/// two pre-existing tests). Fix round 1 closed this by extending
/// `subtype_tag_is_a_closed_enum_over_the_real_parse_path`; all eight are
/// now exercised via the real parse path by a test in this module's `mod
/// tests`. The `as_str()` string VALUES (distinct from variant closure) are
/// separately pinned — see [`SubtypeTag::as_str`]'s doc.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubtypeTag {
    /// No `TypeDefinition` at all.
    NoTypeDefinition,
    /// `TypeDefinition` present, but its outer `Name` is absent (defensive;
    /// should not occur in real `SymbolReference.json`).
    NoName,
    /// The outer `Name` already contains a `"` — a RECORD-typed param's
    /// `Name` carries the FULL source-shaped text already (e.g. `Record
    /// "Normal Table"`); passed through UNCHANGED, even when a `Subtype` is
    /// also present.
    AlreadyQuoted,
    /// No `Subtype` object at all — bare pass-through.
    NoSubtype,
    /// `Subtype.Name` present and quote-free (`Subtype.Id` may or may not
    /// also be present — its presence doesn't affect this tag; only a
    /// quote-free `Name` does).
    Full,
    /// `Subtype.Name` present but contains a `"`; text degrades to the bare
    /// outer keyword (never escaped/synthesized into text), but the raw name
    /// is still folded into the fingerprint discriminator.
    NameQuoted,
    /// `Subtype.Id` present without `Subtype.Name`; text degrades to the
    /// bare outer keyword, but the raw id is still folded.
    IdOnly,
    /// `Subtype` present but carries neither `Name` nor `Id`.
    EmptySubtype,
}

impl SubtypeTag {
    /// The original `&'static str` value this variant replaced — kept so
    /// `abi_ingest::fold_param_discriminator`'s `sig_fp` fold text (and any
    /// other string-shaped consumer) stays byte-identical across the type
    /// change.
    ///
    /// Every literal below is pinned BYTE-FOR-BYTE by
    /// `tests::subtype_tag_as_str_values_are_pinned_and_distinct`, which ALSO
    /// calls THIS method on every variant, collects the results into a
    /// `HashSet<&str>`, and asserts its length is 8 — i.e. distinctness is
    /// checked against `as_str()`'s real output, not against the test's own
    /// literal table (fix round 2 correction — that was the original,
    /// defective form of this test: its distinctness set was built from the
    /// table's own expected strings, never from `as_str()`, so it could not
    /// fail from a production defect at all, only from a test-authoring
    /// mistake).
    ///
    /// Motivation (fix round 1 — a review finding): before ANY test asserted
    /// this method's output, `grep -rn 'as_str()' src/ tests/ | grep -i
    /// subtype` found exactly ONE call site — the production `sig_fp` fold —
    /// and ZERO assertions anywhere; a probe that renamed `Full`'s string to
    /// collide with `NoSubtype`'s left every test in the workspace green
    /// despite creating a genuine `param_type_fp` fingerprint collision, the
    /// exact silent overload-collapse this discriminator exists to prevent.
    pub fn as_str(&self) -> &'static str {
        match self {
            SubtypeTag::NoTypeDefinition => "no_type_definition",
            SubtypeTag::NoName => "no_name",
            SubtypeTag::AlreadyQuoted => "already_quoted",
            SubtypeTag::NoSubtype => "no_subtype",
            SubtypeTag::Full => "full",
            SubtypeTag::NameQuoted => "name_quoted",
            SubtypeTag::IdOnly => "id_only",
            SubtypeTag::EmptySubtype => "empty_subtype",
        }
    }
}

/// A parameter signature from `SymbolReference.json` — no per-run ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiParameter {
    pub name: String,
    /// SOURCE-SHAPED type text with a BARE-OUTER-NAME FALLBACK on decline
    /// shapes (Task 2) — see [`reconstruct_param_field_type`]'s doc for the
    /// full fail-closed rule set. UNLIKE `AbiRoutine::return_type_text`
    /// (which declines the WHOLE mapping to `None` on an Id-only or
    /// quote-bearing Subtype), this NEVER loses the outer keyword: `param_
    /// type_fp`/dedup have no "empty = untrustworthy" contract, so an
    /// Id-only or quote-bearing Subtype degrades gracefully to the bare
    /// outer name instead (e.g. `"Codeunit"`) rather than an empty string
    /// (round-2 addendum).
    pub type_text: String,
    pub is_var: bool,
    /// True when the parameter is declared `temporary` (e.g. `var Rec: Record T
    /// temporary`). Additive — no consumers yet; populated by later tasks (Task 6).
    pub is_temporary: bool,
    /// The raw `Subtype.Id`, independent of whether `Subtype.Name` was ALSO
    /// present (Task 2 round-2 addendum) — UNLIKE `AbiRoutine::return_type_id`
    /// (only `Some` when BOTH Name+Id are proof-grade present, a
    /// cross-validation pair), this carries whatever raw id the JSON held in
    /// ANY shape. Folded into `abi_ingest::param_type_fp`'s canonical
    /// discriminator tuple ALONGSIDE `subtype_raw_name`/`subtype_tag` so two
    /// parameters whose `type_text` degrades to the IDENTICAL bare-outer-name
    /// fallback (two DIFFERENT Id-only Subtypes, e.g. `DoIt(Codeunit 10)` vs
    /// `DoIt(Codeunit 20)`) still fingerprint DIFFERENTLY and never silently
    /// collapse onto one ABI overload survivor (round-1 critical). Never used
    /// to synthesize `type_text` — purely a hash input.
    pub subtype_id: Option<i64>,
    /// The raw `Subtype.Name`, independent of whether it was safely
    /// reconstructible into `type_text` (Task 2 round-2 addendum) — carries a
    /// quote-bearing name verbatim (never escaped/synthesized into text, but
    /// still folded into the fingerprint) so two DIFFERENT quote-bearing
    /// Subtype Names sharing the same outer keyword still fingerprint
    /// differently. `None` when the JSON carried no `Subtype.Name` at all.
    pub subtype_raw_name: Option<String>,
    /// See [`SubtypeTag`].
    pub subtype_tag: SubtypeTag,
}

/// A routine signature from `SymbolReference.json`. No body, no anchor, no per-run id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiRoutine {
    pub name: String,
    /// One of the al-sem `RoutineKind` strings: "procedure" | "event-publisher" |
    /// "event-subscriber".
    pub kind: String,
    pub event_kind: AbiEventKind,
    pub parameters: Vec<AbiParameter>,
    /// Whether `parameters` reflects a genuinely-parsed `Parameters` JSON array
    /// (present in the source JSON, even if empty for a true 0-arg procedure) vs.
    /// the field being absent/unparseable — arity is TRI-STATE (Task 1, round-2
    /// hardening): `false` here means the candidate's arity is UNKNOWN, never
    /// zero. Consumers (`abi_ingest`) must never treat an unknown arity as a
    /// concrete `0` — see `abi_ingest::UNKNOWN_ARITY`.
    pub parameters_known: bool,
    /// Reconstructed SOURCE-SHAPED return-type text (Task 2) — see
    /// [`reconstruct_return_type_text`] for the full fail-closed rule set.
    /// `None` covers both "no return type declared" AND "a return type was
    /// declared but could not be safely reconstructed" (Id-only Subtype, a
    /// Subtype Name containing a quote character) — this field alone cannot
    /// distinguish the two; that distinction does not matter to any consumer
    /// (both mean "do not treat this as a known scalar type").
    pub return_type_text: Option<String>,
    /// The raw `(name, id)` pair from the return type's `Subtype`, present
    /// ONLY when BOTH `Subtype.Name` and `Subtype.Id` were declared in the
    /// source JSON (Task 2 enabling primitive for Task 3's cross-object chain
    /// cross-validation: when a return type's declared Subtype carries both a
    /// Name and an Id, the object the Name resolves to must ALSO carry that
    /// declared Id, or the candidate route declines — name-or-id alone is not
    /// proof of object identity, round-1 C4). Deliberately INDEPENDENT of
    /// `return_type_text`'s fail-closed TEXT reconstruction rules: a Subtype
    /// Name containing a `"` still yields `return_type_text == None` (never
    /// synthesize unescaped text), but the raw identity pair is still carried
    /// here — cross-validation is a structured `==` comparison, never a
    /// text-synthesis operation, so the quote landmine does not apply to it.
    pub return_type_id: Option<(String, i64)>,
    pub is_local: bool,
    pub is_internal: bool,
    /// `protected` visibility modifier (`"IsProtected":true` in
    /// `SymbolReference.json` — verified against real Microsoft System App data:
    /// 10 occurrences, matching its embedded source's 10 `protected procedure`s
    /// 1:1). UNLIKE `is_local`/`is_internal` (dropped entirely at ingestion — AL
    /// never lets an outside caller reach them), a `protected` ABI routine is
    /// KEPT and carried as `Access::Protected`: AL lets a workspace extension of
    /// the declaring object call its `protected` members (Task 1).
    pub is_protected: bool,
    /// Reconstructed attribute strings (back-compat / display).
    pub attributes: Vec<String>,
    /// Structured attributes — same shape as the native AL path's `AttributeInfo`.
    pub attributes_parsed: Vec<AttributeInfo>,
}

/// An ABI field — table column metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiField {
    pub field_number: i64,
    pub name: String,
    /// SOURCE-SHAPED type text (Task 2 — see [`reconstruct_param_field_type`]),
    /// e.g. `Enum "My Enum"` for an ABI Enum field (previously bare `"Enum"`,
    /// dropping the Subtype entirely). No dedup/fingerprint consumer for
    /// fields (unlike `AbiParameter`, fields are never overloaded), so only
    /// the TEXT is generalized here — `parse_field` discards the raw
    /// discriminator tuple's other components.
    pub data_type: String,
    /// "Normal" | "FlowField" | "FlowFilter".
    pub field_class: String,
    pub is_blob_like: bool,
}

/// An ABI key — references fields by name; index is array position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiKey {
    pub name: String,
    pub field_names: Vec<String>,
}

/// An ABI object — codeunit/page/table/etc.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AbiObject {
    pub object_type: String,
    pub object_number: i64,
    pub name: String,
    pub routines: Vec<AbiRoutine>,
    pub object_subtype: Option<String>,
    pub page_type: Option<String>,
    pub source_table_name: Option<String>,
    pub extends_target_name: Option<String>,
    /// Unquoted, `#guid#`-stripped interface names. `None` = field absent.
    pub implemented_interfaces: Option<Vec<String>>,
    /// Canonical lower-case member: "ignore" | "error" | "allow".
    pub inherent_commit_behavior: Option<String>,
    /// Page controls (name, kind, target). kind ∈ {"part","usercontrol"}.
    /// target = subpage Page NUMBER (string) for parts, control-add-in NAME for usercontrols.
    pub page_controls: Vec<(String, String, String)>,
    /// The object's `Scope` property, ONLY as an application scope: `"OnPrem"` or
    /// `"Cloud"` (#27). The same property name carries page-control layout values
    /// (`Repeater`, `Page`), which are not a scope and are dropped. `None` = absent.
    pub scope: Option<String>,
    /// The object's `InherentPermissions` property value, raw (#27).
    pub inherent_permissions: Option<String>,
    /// The object's `InherentEntitlements` property value, raw (#27).
    pub inherent_entitlements: Option<String>,
}

/// `raw` as an application scope (`"OnPrem"` / `"Cloud"`), or `None` for any
/// other value -- notably the page-control layout values that share the
/// `Scope` name (#27). Accepts a quoted or `Scope::`-qualified spelling.
fn application_scope(raw: &str) -> Option<String> {
    let v = raw.trim().trim_matches('\'');
    let v = v.rsplit("::").next().unwrap_or(v).trim();
    if v.eq_ignore_ascii_case("onprem") {
        Some("OnPrem".to_string())
    } else if v.eq_ignore_ascii_case("cloud") {
        Some("Cloud".to_string())
    } else {
        None
    }
}

/// Fill the #27 object-level properties from a raw `Properties` array.
fn apply_access_properties(abi_object: &mut AbiObject, properties: &Option<Vec<RawProperty>>) {
    abi_object.scope = raw_object_property(properties, "Scope").and_then(|v| application_scope(&v));
    abi_object.inherent_permissions = raw_object_property(properties, "InherentPermissions");
    abi_object.inherent_entitlements = raw_object_property(properties, "InherentEntitlements");
}

impl AbiRoutine {
    /// The routine's `[Scope('OnPrem'|'Cloud')]` attribute (#27), read from the
    /// structured attributes. Any other value is not an application scope.
    #[must_use]
    pub fn scope(&self) -> Option<String> {
        self.attribute_args("Scope")
            .and_then(|args| args.first().and_then(|a| application_scope(a)))
    }

    /// The routine's `[InherentPermissions(...)]` attribute arguments, raw (#27).
    #[must_use]
    pub fn inherent_permissions(&self) -> Option<Vec<String>> {
        self.attribute_args("InherentPermissions")
    }

    /// The routine's `[InherentEntitlements(...)]` attribute arguments, raw (#27).
    #[must_use]
    pub fn inherent_entitlements(&self) -> Option<Vec<String>> {
        self.attribute_args("InherentEntitlements")
    }

    /// The raw argument texts of the first attribute named `name` (any case).
    fn attribute_args(&self, name: &str) -> Option<Vec<String>> {
        self.attributes_parsed
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .map(|a| a.args.iter().map(|x| x.text.clone()).collect())
    }
}

/// An ABI table — physical table layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiTable {
    pub object_number: i64,
    pub name: String,
    pub fields: Vec<AbiField>,
    pub keys: Vec<AbiKey>,
    /// True when the table is declared with `TableType = Temporary`. Additive —
    /// no consumers yet; populated by later tasks (Task 6).
    pub is_temporary: bool,
}

/// The neutral DTO `parse_symbol_reference` produces — no model entities/ids.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SymbolReferenceAbi {
    pub app_guid: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub objects: Vec<AbiObject>,
    pub tables: Vec<AbiTable>,
    /// Set when the JSON could not be parsed; objects/tables are then empty.
    pub error: Option<String>,
}

// --- Raw serde shapes (mirror the TS `Raw*` interfaces) ---------------------

#[derive(Debug, Clone, Deserialize, Default)]
struct RawArg {
    #[serde(rename = "Value")]
    value: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawAttr {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Arguments")]
    arguments: Option<Vec<RawArg>>,
}

/// The nested `Subtype` object `SymbolReference.json` attaches to a
/// `TypeDefinition` for a database/framework type (e.g.
/// `{"Name":"Codeunit","Subtype":{"Name":"Http Content","Id":2354}}`) —
/// carries the AL object's declared name and/or numeric id (Task 2). Either
/// field may be absent independently: a real dependency ABI can declare only
/// a `Name` (id genuinely unknown to the compiler at symbol-export time) or
/// only an `Id` (name unavailable) — see [`reconstruct_return_type_text`] for
/// how each combination is handled fail-closed.
#[derive(Debug, Clone, Deserialize, Default)]
struct RawSubtype {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Id")]
    id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawTypeDef {
    #[serde(rename = "Name")]
    name: Option<String>,
    /// Present when the parameter / return type carries a `temporary` modifier in
    /// the AL source (e.g. `var Rec: Record T temporary`). Additive — populated
    /// by later tasks (Task 6).
    #[serde(rename = "Temporary")]
    temporary: Option<bool>,
    /// The nested database/framework subtype (Task 2) — see [`RawSubtype`].
    /// `None` for a scalar/bare type (`Integer`, `HttpHeaders`) that carries
    /// no nested object identity.
    #[serde(rename = "Subtype")]
    subtype: Option<RawSubtype>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawParam {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "IsVar")]
    is_var: Option<bool>,
    #[serde(rename = "TypeDefinition")]
    type_definition: Option<RawTypeDef>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawMethod {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Parameters")]
    parameters: Option<Vec<RawParam>>,
    #[serde(rename = "ReturnTypeDefinition")]
    return_type_definition: Option<RawTypeDef>,
    #[serde(rename = "Attributes")]
    attributes: Option<Vec<RawAttr>>,
    #[serde(rename = "IsLocal")]
    is_local: Option<bool>,
    #[serde(rename = "IsInternal")]
    is_internal: Option<bool>,
    #[serde(rename = "IsProtected")]
    is_protected: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawProperty {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Value")]
    value: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawField {
    #[serde(rename = "Id")]
    id: Option<i64>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "TypeDefinition")]
    type_definition: Option<RawTypeDef>,
    #[serde(rename = "Properties")]
    properties: Option<Vec<RawProperty>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawKey {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "FieldNames")]
    field_names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawRelatedId {
    #[serde(rename = "Id")]
    id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawControl {
    #[serde(rename = "Kind")]
    kind: Option<i64>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "RelatedPagePartId")]
    related_page_part_id: Option<RawRelatedId>,
    #[serde(rename = "RelatedControlAddIn")]
    related_control_addin: Option<String>,
    #[serde(rename = "Controls")]
    controls: Option<Vec<RawControl>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct RawObject {
    #[serde(rename = "Id")]
    id: Option<i64>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Methods")]
    methods: Option<Vec<RawMethod>>,
    #[serde(rename = "Fields")]
    fields: Option<Vec<RawField>>,
    #[serde(rename = "Keys")]
    keys: Option<Vec<RawKey>>,
    #[serde(rename = "Properties")]
    properties: Option<Vec<RawProperty>>,
    #[serde(rename = "TargetObject")]
    target_object: Option<String>,
    #[serde(rename = "ImplementedInterfaces")]
    implemented_interfaces: Option<Vec<String>>,
    #[serde(rename = "Controls")]
    controls: Option<Vec<RawControl>>,
}

// --- Helpers (mirror the TS module-level functions) -------------------------

fn blob_like(token: &str) -> bool {
    matches!(token, "blob" | "media" | "mediaset")
}

/// Read a named property value from a `Properties` array. Case-insensitive.
fn raw_object_property(
    properties: &Option<Vec<RawProperty>>,
    property_name: &str,
) -> Option<String> {
    let props = properties.as_ref()?;
    let lc = property_name.to_lowercase();
    for p in props {
        if p.name.clone().unwrap_or_default().to_lowercase() == lc {
            return p.value.clone();
        }
    }
    None
}

/// Task 6 (G7, RV-4): a table is declared temporary when it carries the property
/// `{"Name":"TableType","Value":"Temporary"}` (case-insensitive value match). Mirror
/// how `parse_field` reads the `fieldclass` property — structural read, NO
/// string-sniffing. Verified against a real Continia Core 29.0 SymbolReference.json.
fn raw_table_is_temporary(properties: &Option<Vec<RawProperty>>) -> bool {
    raw_object_property(properties, "TableType")
        .map(|v| v.eq_ignore_ascii_case("Temporary"))
        .unwrap_or(false)
}

/// Strip surrounding double-quotes (AL quoted-identifier syntax).
fn unquote_abi_name(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    if chars.len() >= 2 && chars[0] == '"' && chars[chars.len() - 1] == '"' {
        chars[1..chars.len() - 1].iter().collect()
    } else {
        raw.to_string()
    }
}

/// Parse a raw `ImplementedInterfaces` value into an unquoted interface name:
/// strip a leading `#<...>#` cross-app prefix, then strip surrounding quotes.
/// Mirrors `parseAbiInterfaceName` (regex `/^#[^#]*#(.+)$/`).
fn parse_abi_interface_name(raw: &str) -> String {
    // `/^#[^#]*#(.+)$/` — `#`, then zero-or-more non-`#`, then `#`, then `.+`.
    let name = if let Some(stripped) = raw.strip_prefix('#') {
        if let Some(hash_pos) = stripped.find('#') {
            let rest = &stripped[hash_pos + 1..];
            // `.+` requires at least one char after the closing `#`.
            if !rest.is_empty() { rest } else { raw }
        } else {
            raw
        }
    } else {
        raw
    };
    let chars: Vec<char> = name.chars().collect();
    if chars.len() >= 2 && chars[0] == '"' && chars[chars.len() - 1] == '"' {
        chars[1..chars.len() - 1].iter().collect()
    } else {
        name.to_string()
    }
}

/// Stringify a serde_json `Value` the way JS `String(x.Value ?? "")` would for the
/// values that appear in attribute argument lists. Only used for the back-compat
/// `attributes` display strings.
fn js_string_of(v: &Option<Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Reconstruct the back-compat attribute string, e.g. "[IntegrationEvent(False, False)]".
fn attr_string(a: &RawAttr) -> String {
    let name = a.name.clone().unwrap_or_default();
    let args: Vec<String> = a
        .arguments
        .as_ref()
        .map(|args| args.iter().map(|x| js_string_of(&x.value)).collect())
        .unwrap_or_default();
    if args.is_empty() {
        format!("[{name}]")
    } else {
        format!("[{}({})]", name, args.join(", "))
    }
}

/// Object-type keywords distinguishing a `database_reference` from a generic
/// `qualified_enum_value`. Mirrors the TS `OBJECT_TYPE_KEYWORDS` set.
fn is_object_type_keyword(s: &str) -> bool {
    matches!(
        s,
        "database" | "page" | "report" | "codeunit" | "xmlport" | "query" | "table"
    )
}

/// True when every char of `s` is `0-9` (non-empty, optional leading `-`).
fn is_integer_text(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    if b[0] == b'-' {
        i = 1;
        if i == b.len() {
            return false;
        }
    }
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            return false;
        }
        i += 1;
    }
    true
}

/// Classify a single `RawArg.Value` into a typed `AttributeArg`. Mirrors
/// `classifyAbiArg`.
fn classify_abi_arg(raw: &Option<Value>) -> AttributeArg {
    match raw {
        Some(Value::Bool(b)) => {
            let text = if *b { "true" } else { "false" }.to_string();
            return AttributeArg {
                kind: "boolean".to_string(),
                text: text.clone(),
                value: Some(text),
                qualifier: None,
                member: None,
            };
        }
        Some(Value::Number(n)) => {
            // `typeof raw === "number" && Number.isFinite && Number.isInteger`
            if let Some(i) = n.as_i64() {
                let text = i.to_string();
                return AttributeArg {
                    kind: "integer".to_string(),
                    text: text.clone(),
                    value: Some(text),
                    qualifier: None,
                    member: None,
                };
            }
            if let Some(u) = n.as_u64() {
                let text = u.to_string();
                return AttributeArg {
                    kind: "integer".to_string(),
                    text: text.clone(),
                    value: Some(text),
                    qualifier: None,
                    member: None,
                };
            }
            // Non-integer number → falls through to String(raw) path below.
        }
        _ => {}
    }

    // text = String(raw ?? "")
    let text = js_string_of(raw);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return AttributeArg {
            kind: "unknown".to_string(),
            text,
            value: None,
            qualifier: None,
            member: None,
        };
    }

    let lower = trimmed.to_lowercase();
    if lower == "true" || lower == "false" {
        return AttributeArg {
            kind: "boolean".to_string(),
            text: trimmed.to_string(),
            value: Some(lower),
            qualifier: None,
            member: None,
        };
    }
    if is_integer_text(trimmed) {
        return AttributeArg {
            kind: "integer".to_string(),
            text: trimmed.to_string(),
            value: Some(trimmed.to_string()),
            qualifier: None,
            member: None,
        };
    }
    let tchars: Vec<char> = trimmed.chars().collect();
    if tchars.len() >= 2 && tchars[0] == '\'' && tchars[tchars.len() - 1] == '\'' {
        let inner: String = tchars[1..tchars.len() - 1].iter().collect();
        return AttributeArg {
            kind: "string_literal".to_string(),
            text: trimmed.to_string(),
            value: Some(inner),
            qualifier: None,
            member: None,
        };
    }
    if tchars.len() >= 2 && tchars[0] == '"' && tchars[tchars.len() - 1] == '"' {
        let inner: String = tchars[1..tchars.len() - 1].iter().collect();
        return AttributeArg {
            kind: "quoted_identifier".to_string(),
            text: trimmed.to_string(),
            value: Some(inner),
            qualifier: None,
            member: None,
        };
    }

    // `::`-qualified
    if let Some(colon_idx) = trimmed.find("::")
        && colon_idx > 0
    {
        let qualifier = trimmed[..colon_idx].trim().to_string();
        let member_raw = trimmed[colon_idx + 2..].trim();
        let mraw_chars: Vec<char> = member_raw.chars().collect();
        let member = if mraw_chars.len() >= 2
            && mraw_chars[0] == '"'
            && mraw_chars[mraw_chars.len() - 1] == '"'
        {
            mraw_chars[1..mraw_chars.len() - 1].iter().collect()
        } else {
            member_raw.to_string()
        };
        let kind = if is_object_type_keyword(&qualifier.to_lowercase()) {
            "database_reference"
        } else {
            "qualified_enum_value"
        };
        return AttributeArg {
            kind: kind.to_string(),
            text: trimmed.to_string(),
            value: Some(member.clone()),
            qualifier: Some(qualifier),
            member: Some(member),
        };
    }

    // Bare token → identifier.
    AttributeArg {
        kind: "identifier".to_string(),
        text: trimmed.to_string(),
        value: Some(trimmed.to_string()),
        qualifier: None,
        member: None,
    }
}

/// Build an `AttributeInfo` from a structured `RawAttr`. Mirrors `abiAttributeInfo`.
fn abi_attribute_info(a: &RawAttr) -> AttributeInfo {
    let name = a.name.clone().unwrap_or_default();
    let args: Vec<AttributeArg> = a
        .arguments
        .as_ref()
        .map(|args| args.iter().map(|x| classify_abi_arg(&x.value)).collect())
        .unwrap_or_default();
    let raw = if args.is_empty() {
        format!("[{name}]")
    } else {
        let joined: Vec<String> = args.iter().map(|x| x.text.clone()).collect();
        format!("[{}({})]", name, joined.join(", "))
    };
    AttributeInfo { name, args, raw }
}

/// Reconstruct AL SOURCE-SHAPED return-type text from a parsed `RawTypeDef`
/// (Task 2) — fail-closed per the task's round-1/round-2 landmines. Declining
/// to `None` is always preferred over synthesizing a plausible-looking but
/// possibly-wrong type reference; this function NEVER escapes, truncates, or
/// approximates.
///
/// - No `Name` at all → `None` (nothing to reconstruct).
/// - `Name` present, no `Subtype` → the bare `Name` text UNCHANGED (e.g.
///   `HttpHeaders`, or a generic/container shape like `List of [Codeunit
///   "X"]` — passed through as-is; a downstream scalar-typed consumer
///   declines a non-scalar shape itself, this function never approximates a
///   container into a scalar).
/// - `Subtype.Name` present and quote-free → `"{Name} \"{Subtype.Name}\""`
///   (source-shaped, quoted — e.g. `Codeunit "Http Content"`). A
///   namespace/dot-qualified `Subtype.Name` is carried verbatim, never
///   truncated.
/// - `Subtype.Name` present but CONTAINS a `"` → `None`. Re-quoting a name
///   that already carries a quote character would require escaping, and this
///   function must never synthesize escaped text a downstream text
///   classifier could misparse (round-1 M2 / round-2 gemini landmine) —
///   decline rather than guess at an escaping convention.
/// - `Subtype.Id` present but NO `Subtype.Name` → `None`. AL object ids are
///   NOT cross-app unique — a bare numeric reconstruction could resolve to
///   the WRONG app's object. Never synthesize a numeric type reference
///   (round-1 critical).
/// - `Subtype` present but carries neither `Name` nor `Id` → `None`
///   (defensive; nothing usable to reconstruct).
fn reconstruct_return_type_text(t: &RawTypeDef) -> Option<String> {
    let outer_name = t.name.as_deref()?;
    match &t.subtype {
        None => Some(outer_name.to_string()),
        Some(sub) => match &sub.name {
            Some(sub_name) if !sub_name.contains('"') => {
                Some(format!("{outer_name} \"{sub_name}\""))
            }
            _ => None,
        },
    }
}

/// Extract the raw `(name, id)` cross-validation pair from a `RawTypeDef`'s
/// `Subtype` — `Some` ONLY when both fields are present (Task 2; see
/// [`AbiRoutine::return_type_id`]'s doc for why this is independent of
/// [`reconstruct_return_type_text`]'s fail-closed TEXT rules).
fn return_type_subtype_id(t: &RawTypeDef) -> Option<(String, i64)> {
    let sub = t.subtype.as_ref()?;
    Some((sub.name.clone()?, sub.id?))
}

/// Reconstruct a PARAMETER/FIELD's SOURCE-SHAPED type text with a
/// BARE-OUTER-NAME FALLBACK on decline shapes, PLUS the raw Subtype
/// discriminator tuple (Task 2 round-2 addendum) — the sibling of
/// [`reconstruct_return_type_text`]/[`return_type_subtype_id`], but with a
/// DIFFERENT fail-closed contract: `abi_ingest::param_type_fp`/dedup have no
/// "empty = untrustworthy" signal (an empty/bare string there is a
/// legitimate, if degraded, dedup key already in wide use), so unlike the
/// return-type reconstruction (which declines the WHOLE mapping to `None` on
/// an Id-only or quote-bearing Subtype), a param/field NEVER loses its outer
/// keyword — it degrades gracefully to that bare keyword instead. Reused by
/// BOTH `parse_method` (params) and `parse_field` (fields) — one shared
/// helper, not two independently-drifting copies.
///
/// Returns `(text, subtype_id, subtype_raw_name, degradation_tag)`. The last
/// three are the RAW discriminator `abi_ingest::param_type_fp` folds into a
/// length-delimited canonical tuple ALONGSIDE `text` (params only — fields
/// have no fingerprint consumer) so two parameters whose `text` degrades to
/// the IDENTICAL bare-outer-name fallback (two DIFFERENT Id-only Subtypes;
/// two DIFFERENT quote-bearing Subtype Names) still fingerprint DIFFERENTLY
/// and never silently collapse onto one ABI overload survivor (round-1
/// critical) — never used to synthesize `text` itself, purely fingerprint
/// input.
///
/// - No `TypeDefinition` at all → `("", None, None, SubtypeTag::NoTypeDefinition)`.
/// - `TypeDefinition` present, no `Name` (defensive; should not occur) →
///   `("", None, None, SubtypeTag::NoName)`.
/// - `Name` ALREADY CONTAINS A `"` (a RECORD-typed param's `Name` is observed
///   to carry the FULL source-shaped text already, e.g. `Record "Normal
///   Table"` — a real Continia Core 29.0 shape, see `tests/temp_state_abi.rs`)
///   → `(Name, Subtype.Id, Subtype.Name, SubtypeTag::AlreadyQuoted)` UNCHANGED,
///   even when a Subtype is ALSO present (sometimes redundantly naming the SAME
///   object) — NEVER re-append `"{Subtype.Name}"`, which would double-quote-
///   corrupt an already-complete string (round-2 fold-in landmine).
/// - `Name` present (bare, no embedded quote), no `Subtype` → `(Name, None,
///   None, SubtypeTag::NoSubtype)` — bare pass-through, UNCHANGED from today's
///   fidelity.
/// - `Subtype.Name` present, quote-free → `("{Name} \"{Subtype.Name}\"",
///   Subtype.Id, Some(Subtype.Name), SubtypeTag::Full)` — full source-shaped
///   text, e.g. `Codeunit "Dep A"` / `Enum "My Enum"`.
/// - `Subtype.Name` present but contains a `"` → `(Name, Subtype.Id,
///   Some(Subtype.Name), SubtypeTag::NameQuoted)` — TEXT degrades to the bare
///   outer name (never escapes/synthesizes into text), but the raw name is
///   STILL folded into the fingerprint discriminator.
/// - `Subtype.Id` present, no `Name` → `(Name, Some(Id), None,
///   SubtypeTag::IdOnly)` — TEXT degrades to the bare outer name; the raw id
///   is STILL folded.
/// - `Subtype` present but carries neither `Name` nor `Id` → `(Name, None,
///   None, SubtypeTag::EmptySubtype)`.
fn reconstruct_param_field_type(
    t: Option<&RawTypeDef>,
) -> (String, Option<i64>, Option<String>, SubtypeTag) {
    let Some(t) = t else {
        return (String::new(), None, None, SubtypeTag::NoTypeDefinition);
    };
    let Some(outer_name) = t.name.as_deref() else {
        return (String::new(), None, None, SubtypeTag::NoName);
    };
    // ALREADY-QUOTED LANDMINE (round-2 fold-in fix): a RECORD-typed param's
    // `TypeDefinition.Name` is observed to carry the FULL source-shaped text
    // ALREADY (`Record "Normal Table"`, quote embedded) rather than a bare
    // keyword like `"Codeunit"` — verified against a real Continia Core 29.0
    // `SymbolReference.json` (see `parse_method`'s Task 6 doc; pinned by
    // `tests/temp_state_abi.rs`'s `TempMarkedParam` fixture, which carries
    // BOTH this shape AND a redundant same-named `Subtype`). When `outer_name`
    // already contains a `"`, it is complete on its own — appending `"{sub_
    // name}"` again (the `Some(sub_name)` branch below) would DOUBLE-QUOTE-
    // CORRUPT it into `Record "Normal Table" "Normal Table"`. Any accompanying
    // Subtype is carried into the discriminator (harmless/redundant for the
    // fp — `outer_name` alone already fully discriminates two different
    // already-quoted texts) but NEVER re-appended to the text.
    if outer_name.contains('"') {
        let sub = t.subtype.as_ref();
        return (
            outer_name.to_string(),
            sub.and_then(|s| s.id),
            sub.and_then(|s| s.name.clone()),
            SubtypeTag::AlreadyQuoted,
        );
    }
    match &t.subtype {
        None => (outer_name.to_string(), None, None, SubtypeTag::NoSubtype),
        Some(sub) => match &sub.name {
            Some(sub_name) if !sub_name.contains('"') => (
                format!("{outer_name} \"{sub_name}\""),
                sub.id,
                Some(sub_name.clone()),
                SubtypeTag::Full,
            ),
            Some(sub_name) => (
                outer_name.to_string(),
                sub.id,
                Some(sub_name.clone()),
                SubtypeTag::NameQuoted,
            ),
            None => match sub.id {
                Some(id) => (outer_name.to_string(), Some(id), None, SubtypeTag::IdOnly),
                None => (outer_name.to_string(), None, None, SubtypeTag::EmptySubtype),
            },
        },
    }
}

/// Classify a method as a routine, deriving event-publisher kind from attributes.
/// Mirrors `parseMethod`.
fn parse_method(m: &RawMethod) -> AbiRoutine {
    let raw_attrs = m.attributes.clone().unwrap_or_default();
    let attributes: Vec<String> = raw_attrs.iter().map(attr_string).collect();
    let attributes_parsed: Vec<AttributeInfo> = raw_attrs.iter().map(abi_attribute_info).collect();

    let mut kind = "procedure".to_string();
    let mut event_kind = AbiEventKind::Unknown;
    for info in &attributes_parsed {
        match info.name.to_lowercase().as_str() {
            "integrationevent" => {
                kind = "event-publisher".to_string();
                event_kind = AbiEventKind::Integration;
            }
            "businessevent" => {
                kind = "event-publisher".to_string();
                event_kind = AbiEventKind::Business;
            }
            "eventsubscriber" => {
                kind = "event-subscriber".to_string();
            }
            _ => {}
        }
    }

    // Tri-state arity (Task 1, round-2 hardening): `Some(_)` — even `Some(vec![])`
    // for a genuine 0-arg procedure — means the JSON carried a real `Parameters`
    // array; `None` means the field was absent/unparseable, so the arity is
    // UNKNOWN, never zero. `abi_ingest` maps `!parameters_known` to a sentinel
    // that can never arity-match a real call site.
    let parameters_known = m.parameters.is_some();

    AbiRoutine {
        name: m.name.clone().unwrap_or_default(),
        kind,
        event_kind,
        parameters: m
            .parameters
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let (type_text, subtype_id, subtype_raw_name, subtype_tag) =
                    reconstruct_param_field_type(p.type_definition.as_ref());
                AbiParameter {
                    name: p.name.clone().unwrap_or_default(),
                    // Task 2: bare-fallback SOURCE-SHAPED text (see
                    // `reconstruct_param_field_type`'s doc) — replaces the old
                    // outer-keyword-only mapping.
                    type_text,
                    is_var: p.is_var == Some(true),
                    // Task 6 (G7, RV-4): a record param carries `TypeDefinition.Temporary`
                    // when declared `temporary` in source (verified against a real Continia
                    // Core 29.0 SymbolReference.json). Read it so the ABI→L3 projection can
                    // model the same Known(true) temp shape the native path produces.
                    is_temporary: p.type_definition.as_ref().and_then(|t| t.temporary)
                        == Some(true),
                    subtype_id,
                    subtype_raw_name,
                    subtype_tag,
                }
            })
            .collect(),
        parameters_known,
        return_type_text: m
            .return_type_definition
            .as_ref()
            .and_then(reconstruct_return_type_text),
        return_type_id: m
            .return_type_definition
            .as_ref()
            .and_then(return_type_subtype_id),
        is_local: m.is_local == Some(true),
        is_internal: m.is_internal == Some(true),
        is_protected: m.is_protected == Some(true),
        attributes,
        attributes_parsed,
    }
}

/// Mirror `parseField`.
fn parse_field(f: &RawField) -> AbiField {
    // Task 2: same bare-fallback SOURCE-SHAPED reconstruction as a param's
    // `type_text` (see `reconstruct_param_field_type`'s doc) — an ABI Enum
    // field now carries `Enum "My Enum"` instead of the bare `"Enum"` this
    // dropped before. Fields have no fingerprint/dedup consumer, so the raw
    // discriminator tuple's other three components are discarded here.
    let (data_type, _subtype_id, _subtype_raw_name, _subtype_tag) =
        reconstruct_param_field_type(f.type_definition.as_ref());
    let mut field_class = "Normal".to_string();
    for p in f.properties.clone().unwrap_or_default().iter() {
        if p.name.clone().unwrap_or_default().to_lowercase() == "fieldclass" {
            let v = p.value.clone().unwrap_or_default().to_lowercase();
            if v.contains("flowfield") {
                field_class = "FlowField".to_string();
            } else if v.contains("flowfilter") {
                field_class = "FlowFilter".to_string();
            }
        }
    }
    // isBlobLike = BLOB_LIKE.has((dataType.split("[")[0] ?? dataType).toLowerCase())
    let base_token = data_type
        .split('[')
        .next()
        .unwrap_or(&data_type)
        .to_lowercase();
    AbiField {
        field_number: f.id.unwrap_or(0),
        name: f.name.clone().unwrap_or_default(),
        data_type,
        field_class,
        is_blob_like: blob_like(&base_token),
    }
}

fn parse_key(k: &RawKey) -> AbiKey {
    AbiKey {
        name: k.name.clone().unwrap_or_default(),
        field_names: k.field_names.clone().unwrap_or_default(),
    }
}

/// Recursively walk a page's control tree and collect Kind-6 (Part) and Kind-10
/// (UserControl) entries into `out` as `(name, kind_str, target)` tuples.
fn collect_page_controls(controls: &[RawControl], out: &mut Vec<(String, String, String)>) {
    for c in controls {
        let name = c.name.clone().unwrap_or_default();
        match c.kind {
            Some(6) => {
                if let Some(id) = c.related_page_part_id.as_ref().and_then(|r| r.id) {
                    out.push((name, "part".into(), id.to_string()));
                }
            }
            Some(10) => {
                if let Some(addin) = &c.related_control_addin {
                    out.push((name, "usercontrol".into(), addin.clone()));
                }
            }
            _ => {}
        }
        if let Some(sub) = &c.controls {
            collect_page_controls(sub, out);
        }
    }
}

/// Extract a typed array of `RawObject` from the top-level JSON map at `key`.
/// Missing / wrong-typed → empty (mirrors `(raw[key] as RawObject[]) ?? []`).
/// Collect every object array under `key` — at the top level AND recursively inside
/// every `Namespaces[]` node. BC 24+ symbol files nest objects under namespace nodes
/// (e.g. `Namespaces[].Microsoft.Sales.Document.Pages`); older files keep them flat
/// at the root. Both are gathered so a modern dependency `.app` projects its FULL
/// object set — without namespace recursion ~99% of a modern Base Application's
/// objects (and every routine/table they carry) are silently dropped, which is the
/// dominant cross-app resolution hole.
fn raw_objects(sections: &Sections<'_>, key: &str) -> Vec<RawObject> {
    let mut out = Vec::new();
    collect_raw_objects(sections, key, &mut out);
    out
}

fn collect_raw_objects(sections: &Sections<'_>, key: &str, out: &mut Vec<RawObject>) {
    if let Some(raw) = sections.members.get(key)
        && let Some(objs) = section_objects(raw.get())
    {
        out.extend(objs);
    }
    for ns in &sections.namespaces {
        collect_raw_objects(ns, key, out);
    }
}

/// One section's objects, read straight from its text. A section the direct
/// read rejects is retried the old way (`Value`, then `from_value`), because
/// the two differ: a duplicate key inside an object fails the direct read
/// ("duplicate field") but a `Value` keeps the last one. Only a section that
/// fails both is skipped, as before. Valid sections never pay for the retry.
fn section_objects(text: &str) -> Option<Vec<RawObject>> {
    serde_json::from_str::<Vec<RawObject>>(text)
        .ok()
        .or_else(|| {
            serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|v| serde_json::from_value(v).ok())
        })
}

/// A JSON value checked as strictly as a `Value` parse checks it (string
/// escapes, number range, nesting depth), without building anything.
/// `IgnoredAny` is not enough: it skips over a lone surrogate or `1e400`
/// without complaint, where a `Value` parse fails.
struct Validated;

impl<'de> Deserialize<'de> for Validated {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ValidatedVisitor)
    }
}

struct ValidatedVisitor;

impl<'de> serde::de::Visitor<'de> for ValidatedVisitor {
    type Value = Validated;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_str<E>(self, _: &str) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_unit<E>(self) -> Result<Validated, E> {
        Ok(Validated)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Validated, A::Error> {
        while seq.next_element::<Validated>()?.is_some() {}
        Ok(Validated)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Validated, A::Error> {
        while map.next_entry::<Validated, Validated>()?.is_some() {}
        Ok(Validated)
    }
}

/// One JSON object of a `SymbolReference.json` — the root or a `Namespaces[]`
/// node — with every member kept as raw JSON text BORROWED from the input,
/// plus its namespace children, parsed the same way.
///
/// Never a `serde_json::Value` tree: Base Application's file is ~58 MB, and
/// as a `Value` tree (plus the per-section `clone` the conversion needed) it
/// peaked at ~1 GB transient while the finished ABI is a small fraction of
/// that (BC 28.4, 2026-10). Each section is converted to `Vec<RawObject>`
/// straight from its text, so only the typed objects are ever allocated.
struct Sections<'a> {
    members: std::collections::HashMap<String, &'a serde_json::value::RawValue>,
    namespaces: Vec<Sections<'a>>,
}

impl<'a> Sections<'a> {
    fn from_members(
        members: std::collections::HashMap<String, &'a serde_json::value::RawValue>,
    ) -> Self {
        // Same leniency as the `Value` walk this replaces: a `Namespaces`
        // that is not an array, or an entry that is not an object, is skipped.
        let namespaces = members
            .get("Namespaces")
            .and_then(|raw| {
                serde_json::from_str::<Vec<&'a serde_json::value::RawValue>>(raw.get()).ok()
            })
            .map(|nodes| {
                nodes
                    .into_iter()
                    .filter_map(|node| serde_json::from_str(node.get()).ok())
                    .map(Sections::from_members)
                    .collect()
            })
            .unwrap_or_default();
        Sections {
            members,
            namespaces,
        }
    }

    /// A top-level string property, rendered exactly as the `Value` walk
    /// did: the string itself, `""` for null/absent, else compact JSON.
    fn str_prop(&self, key: &str) -> String {
        let Some(raw) = self.members.get(key) else {
            return String::new();
        };
        match serde_json::from_str::<Value>(raw.get()) {
            Ok(Value::String(s)) => s,
            Ok(Value::Null) | Err(_) => String::new(),
            Ok(other) => other.to_string(),
        }
    }
}

/// Parse the raw `SymbolReference.json` text into the neutral ABI DTO. Never panics.
/// Mirrors `parseSymbolReference`.
///
/// H-3 (Tier-1 remediation, Task T1.2): uses the SAME NUL-tolerant
/// first-JSON-value parse `app_package::parse_symbols` (the legacy path)
/// always used — a strict full-string parse fails on the trailing NUL
/// padding some `.app` emitters append after the real JSON content, which
/// previously made a genuinely well-formed dependency silently ingest as an
/// EMPTY ABI (see `app_package::parse_first_json_value`'s doc).
pub fn parse_symbol_reference(json: &str) -> SymbolReferenceAbi {
    // The old path parsed the whole first value into a `Value`, so invalid
    // JSON ANYWHERE in it (even inside a member nothing reads) made the file
    // an error. The borrowed-section read below only checks what it touches,
    // so check the whole value first, allocation-free.
    let valid = matches!(
        serde_json::Deserializer::from_str(json)
            .into_iter::<Validated>()
            .next(),
        Some(Ok(_))
    );
    // First JSON value only (NUL-tolerant, like `parse_first_json_value`),
    // as an object of borrowed raw members — see [`Sections`].
    if valid
        && let Some(Ok(members)) = serde_json::Deserializer::from_str(json)
            .into_iter::<std::collections::HashMap<String, &serde_json::value::RawValue>>()
            .next()
    {
        return abi_from_sections(&Sections::from_members(members));
    }
    // Not an object, or not valid JSON: decide which exactly as before. Only
    // malformed input reaches this, so building a `Value` here costs nothing
    // on any real dependency.
    let parsed: Result<Value, _> = crate::app_package::parse_first_json_value(json);
    match parsed {
        // Non-object JSON: TS would treat property access as undefined and emit
        // empty collections with empty identity (no error). Reproduce that.
        Ok(_) => SymbolReferenceAbi::default(),
        Err(e) => SymbolReferenceAbi {
            error: Some(format!("SymbolReference.json parse failed: {e}")),
            ..Default::default()
        },
    }
}

/// Project a parsed `SymbolReference.json` object into the neutral ABI DTO.
fn abi_from_sections(root: &Sections<'_>) -> SymbolReferenceAbi {
    let mut objects: Vec<AbiObject> = Vec::new();
    let mut tables: Vec<AbiTable> = Vec::new();

    // ROUTINE_BEARING
    const ROUTINE_BEARING: [(&str, &str); 6] = [
        ("Codeunits", "Codeunit"),
        ("Pages", "Page"),
        ("Reports", "Report"),
        ("XmlPorts", "XMLport"),
        ("Queries", "Query"),
        ("Interfaces", "Interface"),
    ];
    for (key, object_type) in ROUTINE_BEARING {
        for o in raw_objects(root, key) {
            let mut abi_object = AbiObject {
                object_type: object_type.to_string(),
                object_number: o.id.unwrap_or(0),
                name: o.name.clone().unwrap_or_default(),
                routines: o
                    .methods
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(parse_method)
                    .collect(),
                ..Default::default()
            };
            if object_type == "Codeunit"
                && let Some(subtype) = raw_object_property(&o.properties, "Subtype")
            {
                abi_object.object_subtype = Some(subtype);
            }
            if object_type == "Page" {
                if let Some(pt) = raw_object_property(&o.properties, "PageType") {
                    abi_object.page_type = Some(pt);
                }
                if let Some(st) = raw_object_property(&o.properties, "SourceTable") {
                    abi_object.source_table_name = Some(unquote_abi_name(&st));
                }
                let mut page_controls: Vec<(String, String, String)> = Vec::new();
                if let Some(controls) = &o.controls {
                    collect_page_controls(controls, &mut page_controls);
                }
                abi_object.page_controls = page_controls;
            }
            if let Some(ifaces) = &o.implemented_interfaces {
                abi_object.implemented_interfaces =
                    Some(ifaces.iter().map(|s| parse_abi_interface_name(s)).collect());
            }
            apply_access_properties(&mut abi_object, &o.properties);
            if let Some(icb_raw) = raw_object_property(&o.properties, "InherentCommitBehavior") {
                let member = match icb_raw.rfind("::") {
                    Some(sep) => icb_raw[sep + 2..].to_lowercase(),
                    None => icb_raw.to_lowercase(),
                };
                match member.as_str() {
                    "ignore" => abi_object.inherent_commit_behavior = Some("ignore".to_string()),
                    "error" => abi_object.inherent_commit_behavior = Some("error".to_string()),
                    "allow" => abi_object.inherent_commit_behavior = Some("allow".to_string()),
                    _ => {}
                }
            }
            objects.push(abi_object);
        }
    }

    // EXTENSION_ROUTINE_BEARING
    const EXTENSION_ROUTINE_BEARING: [(&str, &str); 2] = [
        ("TableExtensions", "TableExtension"),
        ("PageExtensions", "PageExtension"),
    ];
    for (key, object_type) in EXTENSION_ROUTINE_BEARING {
        for o in raw_objects(root, key) {
            let mut abi_object = AbiObject {
                object_type: object_type.to_string(),
                object_number: o.id.unwrap_or(0),
                name: o.name.clone().unwrap_or_default(),
                routines: o
                    .methods
                    .clone()
                    .unwrap_or_default()
                    .iter()
                    .map(parse_method)
                    .collect(),
                ..Default::default()
            };
            if let Some(target) = &o.target_object {
                abi_object.extends_target_name = Some(unquote_abi_name(target));
            }
            apply_access_properties(&mut abi_object, &o.properties);
            objects.push(abi_object);
            if object_type == "TableExtension" {
                tables.push(AbiTable {
                    object_number: o.id.unwrap_or(0),
                    name: o.name.clone().unwrap_or_default(),
                    fields: o
                        .fields
                        .clone()
                        .unwrap_or_default()
                        .iter()
                        .map(parse_field)
                        .collect(),
                    keys: o
                        .keys
                        .clone()
                        .unwrap_or_default()
                        .iter()
                        .map(parse_key)
                        .collect(),
                    // A TableExtension never declares TableType, but read it
                    // structurally for uniformity (yields false absent the property).
                    is_temporary: raw_table_is_temporary(&o.properties),
                });
            }
        }
    }

    // Tables → both an AbiTable and an AbiObject.
    for t in raw_objects(root, "Tables") {
        let object_number = t.id.unwrap_or(0);
        tables.push(AbiTable {
            object_number,
            name: t.name.clone().unwrap_or_default(),
            fields: t
                .fields
                .clone()
                .unwrap_or_default()
                .iter()
                .map(parse_field)
                .collect(),
            keys: t
                .keys
                .clone()
                .unwrap_or_default()
                .iter()
                .map(parse_key)
                .collect(),
            // Task 6 (G7, RV-4): read the table-level `TableType = Temporary` marker.
            is_temporary: raw_table_is_temporary(&t.properties),
        });
        let mut table_object = AbiObject {
            object_type: "Table".to_string(),
            object_number,
            name: t.name.clone().unwrap_or_default(),
            routines: t
                .methods
                .clone()
                .unwrap_or_default()
                .iter()
                .map(parse_method)
                .collect(),
            ..Default::default()
        };
        apply_access_properties(&mut table_object, &t.properties);
        objects.push(table_object);
    }

    // BARE
    const BARE: [(&str, &str); 7] = [
        ("EnumTypes", "Enum"),
        // T4-C medium (b): the legacy `app_package.rs` parser has always read
        // `EnumExtensionTypes` (`enum_extension_types` → `ObjectType::EnumExtension`
        // at app_package.rs:158/467); this engine path lacked the matching BARE
        // entry, so a dependency's enum-extension objects never entered `objects`
        // at all. `program::abi_ingest` already normalizes both "enumextension"
        // and "enumextensiontype" to `ObjectKind::EnumExtension` (case-insensitive
        // match on `object_type`), so the consumer side was always ready for this
        // — only the producer was missing the entry.
        ("EnumExtensionTypes", "EnumExtension"),
        ("ControlAddIns", "ControlAddIn"),
        ("PermissionSets", "PermissionSet"),
        ("PermissionSetExtensions", "PermissionSetExtension"),
        ("ReportExtensions", "ReportExtension"),
        ("DotNetPackages", "DotNetPackage"),
    ];
    for (key, object_type) in BARE {
        for o in raw_objects(root, key) {
            let mut abi_object = AbiObject {
                object_type: object_type.to_string(),
                object_number: o.id.unwrap_or(0),
                name: o.name.clone().unwrap_or_default(),
                routines: Vec::new(),
                ..Default::default()
            };
            if let Some(ifaces) = &o.implemented_interfaces {
                abi_object.implemented_interfaces =
                    Some(ifaces.iter().map(|s| parse_abi_interface_name(s)).collect());
            }
            apply_access_properties(&mut abi_object, &o.properties);
            objects.push(abi_object);
        }
    }

    SymbolReferenceAbi {
        app_guid: root.str_prop("AppId"),
        name: root.str_prop("Name"),
        publisher: root.str_prop("Publisher"),
        version: root.str_prop("Version"),
        objects,
        tables,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_boolean_primitive() {
        let a = classify_abi_arg(&Some(Value::Bool(false)));
        assert_eq!(a.kind, "boolean");
        assert_eq!(a.text, "false");
        assert_eq!(a.value.as_deref(), Some("false"));
    }

    #[test]
    fn classify_database_reference() {
        let a = classify_abi_arg(&Some(Value::String("Codeunit::\"X\"".to_string())));
        assert_eq!(a.kind, "database_reference");
        assert_eq!(a.qualifier.as_deref(), Some("Codeunit"));
        assert_eq!(a.member.as_deref(), Some("X"));
        assert_eq!(a.value.as_deref(), Some("X"));
    }

    #[test]
    fn classify_qualified_enum_value() {
        let a = classify_abi_arg(&Some(Value::String("ObjectType::Codeunit".to_string())));
        assert_eq!(a.kind, "qualified_enum_value");
        assert_eq!(a.qualifier.as_deref(), Some("ObjectType"));
        assert_eq!(a.member.as_deref(), Some("Codeunit"));
    }

    #[test]
    fn classify_string_literal_and_empty() {
        let a = classify_abi_arg(&Some(Value::String("'Event'".to_string())));
        assert_eq!(a.kind, "string_literal");
        assert_eq!(a.value.as_deref(), Some("Event"));
        let b = classify_abi_arg(&Some(Value::String("''".to_string())));
        assert_eq!(b.kind, "string_literal");
        assert_eq!(b.value.as_deref(), Some(""));
    }

    #[test]
    fn interface_name_strips_guid_prefix_and_quotes() {
        assert_eq!(parse_abi_interface_name("\"IFoo\""), "IFoo");
        assert_eq!(parse_abi_interface_name("IBar"), "IBar");
        assert_eq!(
            parse_abi_interface_name("#63ca2fa4#\"Telemetry Logger\""),
            "Telemetry Logger"
        );
    }

    #[test]
    fn bad_json_yields_error_not_panic() {
        let abi = parse_symbol_reference("{ not json");
        assert!(abi.error.is_some());
        assert!(abi.objects.is_empty());
    }

    /// The error the old whole-file `Value` parse produced for `json`.
    fn old_parse_error(json: &str) -> String {
        let e = crate::app_package::parse_first_json_value::<Value>(json).unwrap_err();
        format!("SymbolReference.json parse failed: {e}")
    }

    /// Invalid JSON inside a member nothing reads (a lone surrogate, an
    /// out-of-range number) still makes the whole file an error, exactly as
    /// the old whole-file `Value` parse did.
    #[test]
    fn invalid_json_anywhere_is_an_error_like_the_old_parse() {
        for json in [
            r#"{"Codeunits":[{"Id":1,"Name":"A"}],"Other":"\ud800"}"#,
            r#"{"Codeunits":[{"Id":1,"Name":"A"}],"Other":[1e400]}"#,
            r#"{"Codeunits":[{"Id":1,"Name":"A"}],"\ud800":1}"#,
        ] {
            let abi = parse_symbol_reference(json);
            assert_eq!(abi.error, Some(old_parse_error(json)), "{json}");
            assert!(abi.objects.is_empty(), "{json}");
        }
    }

    #[test]
    fn non_object_root_is_an_empty_abi_without_error() {
        for json in ["[1,2]", "42", "\"x\"", "null"] {
            assert_eq!(
                parse_symbol_reference(json),
                SymbolReferenceAbi::default(),
                "{json}"
            );
        }
    }

    /// A duplicate key inside an object keeps the LAST value, as the old
    /// `Value` walk did; it does not drop the section.
    #[test]
    fn duplicate_key_in_an_object_keeps_the_last_value() {
        let abi = parse_symbol_reference(r#"{"Codeunits":[{"Id":1,"Name":"A","Name":"B"}]}"#);
        assert_eq!(abi.error, None);
        let names: Vec<&str> = abi.objects.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["B"]);
    }

    /// Malformed namespace shapes are skipped exactly as the old walk did;
    /// objects outside them are still ingested.
    #[test]
    fn malformed_namespaces_are_skipped() {
        let not_array = r#"{"Codeunits":[{"Id":1,"Name":"Root"}],"Namespaces":{"Codeunits":[{"Id":2,"Name":"In"}]}}"#;
        let bad_entry = r#"{"Codeunits":[{"Id":1,"Name":"Root"}],"Namespaces":[7,"x",{"Codeunits":[{"Id":2,"Name":"In"}]}]}"#;
        let names = |json: &str| -> Vec<String> {
            let abi = parse_symbol_reference(json);
            assert_eq!(abi.error, None, "{json}");
            abi.objects.into_iter().map(|o| o.name).collect()
        };
        assert_eq!(names(not_array), ["Root"]);
        assert_eq!(names(bad_entry), ["Root", "In"]);
    }

    /// H-3 (Tier-1 remediation, Task T1.2): some `.app` emitters pad
    /// `SymbolReference.json` with trailing NUL bytes after the real JSON
    /// content. The legacy `app_package.rs` parser (`StreamDeserializer`,
    /// "parse only the first JSON value") already tolerates this; this
    /// engine path used a STRICT `serde_json::from_str` that requires the
    /// ENTIRE string to be valid JSON (trailing NUL is not JSON whitespace),
    /// so a genuinely well-formed dependency silently ingested as an EMPTY
    /// ABI — no objects, no error surfaced anywhere. Fixed via the SAME
    /// tolerant first-JSON-value parse both paths now share
    /// (`app_package::parse_first_json_value`).
    #[test]
    fn nul_padded_json_still_parses_full_content() {
        let json = r#"{"Codeunits":[{"Id":50100,"Name":"NulPadded","Methods":[{"Name":"DoIt","Parameters":[]}]}]}"#;
        let mut padded = json.to_string();
        padded.push_str("\0\0\0\0\0\0\0\0");

        let abi = parse_symbol_reference(&padded);
        assert!(
            abi.error.is_none(),
            "NUL padding after valid JSON must NOT surface as a parse error; got {:?}",
            abi.error
        );
        let cu = abi
            .objects
            .iter()
            .find(|o| o.name == "NulPadded")
            .expect("the Codeunit must be ingested despite trailing NUL padding");
        assert_eq!(cu.routines.len(), 1);
        assert_eq!(cu.routines[0].name, "DoIt");
    }

    /// T4-C medium (b): `EnumExtensionTypes` was missing from the BARE table, so a
    /// dependency's enum-extension objects silently never entered `objects`.
    #[test]
    fn bare_table_ingests_enum_extension_types() {
        let json = r#"{
            "EnumExtensionTypes":[{"Id":50100,"Name":"MyEnum Ext"}]
        }"#;
        let abi = parse_symbol_reference(json);
        let obj = abi
            .objects
            .iter()
            .find(|o| o.name == "MyEnum Ext")
            .expect("EnumExtensionTypes entry must be ingested into `objects`");
        assert_eq!(obj.object_type, "EnumExtension");
        assert_eq!(obj.object_number, 50100);
    }

    // -- Task 1: `IsProtected` parsing + tri-state arity ---------------------

    #[test]
    fn parse_method_reads_is_protected() {
        let json = r##"{
            "RuntimeVersion":"11.0",
            "Tables":[],
            "Codeunits":[{"Id":50100,"Name":"ProtTest","Methods":[
                {"Name":"P","IsProtected":true,"Parameters":[]},
                {"Name":"Pub","Parameters":[]}
            ]}],
            "Pages":[],"Reports":[],"XmlPorts":[],"Queries":[],"Interfaces":[],
            "EnumTypes":[],"TableExtensions":[],
            "AppId":"x","Name":"x","Publisher":"x","Version":"1.0.0.0"
        }"##;
        let abi = parse_symbol_reference(json);
        let cu = abi.objects.first().expect("Codeunit must parse");
        let p = cu
            .routines
            .iter()
            .find(|r| r.name == "P")
            .expect("P must exist");
        assert!(p.is_protected, "IsProtected:true must map to is_protected");
        let pub_r = cu
            .routines
            .iter()
            .find(|r| r.name == "Pub")
            .expect("Pub must exist");
        assert!(
            !pub_r.is_protected,
            "absent IsProtected must default to false"
        );
    }

    #[test]
    fn parse_method_arity_is_tri_state() {
        // "Parameters":[] (present, empty) → known arity 0.
        let json_present = r##"{"Name":"Foo","Parameters":[]}"##;
        let raw_present: RawMethod = serde_json::from_str(json_present).unwrap();
        let m_present = parse_method(&raw_present);
        assert!(
            m_present.parameters_known,
            "an explicit empty Parameters array is a KNOWN 0-arity, not unknown"
        );
        assert_eq!(m_present.parameters.len(), 0);

        // No "Parameters" key at all → unknown arity, never zero.
        let json_absent = r##"{"Name":"Foo"}"##;
        let raw_absent: RawMethod = serde_json::from_str(json_absent).unwrap();
        let m_absent = parse_method(&raw_absent);
        assert!(
            !m_absent.parameters_known,
            "an absent Parameters field must be UNKNOWN arity, never zero"
        );
        assert_eq!(
            m_absent.parameters.len(),
            0,
            "the parameters Vec is still empty for display purposes, but \
             parameters_known=false is what callers must gate on"
        );
    }

    // -- Task 2: structured ABI return types (`Subtype`) ---------------------
    //
    // `reconstruct_return_type_text` / `return_type_subtype_id` — source-shaped
    // reconstruction, fail-closed per the task brief's round-1/round-2
    // landmines. Every case below drives the FULL `parse_method` path from raw
    // JSON (never calls the private helpers directly), matching the style of
    // `parse_method_reads_is_protected` above.

    // (a) Name + Subtype{Name, Id} both present → quoted source-shaped text,
    // AND the structured (name, id) pair is retained for downstream
    // cross-validation.
    #[test]
    fn parse_method_return_type_subtype_reconstructs_quoted_source_shape() {
        let json = r##"{
            "Name":"GetHttpContent",
            "ReturnTypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"Http Content","Id":2354}}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(
            m.return_type_text.as_deref(),
            Some("Codeunit \"Http Content\""),
            "Name-preferred quoted source shape"
        );
        assert_eq!(
            m.return_type_id,
            Some(("Http Content".to_string(), 2354)),
            "the structured (name, id) pair must be retained for Task 3's \
             cross-object chain cross-validation"
        );
    }

    // (b) bare Name, no Subtype at all → unchanged pass-through.
    #[test]
    fn parse_method_return_type_bare_name_passthrough() {
        let json = r##"{"Name":"Foo","ReturnTypeDefinition":{"Name":"HttpHeaders"}}"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(m.return_type_text.as_deref(), Some("HttpHeaders"));
        assert_eq!(
            m.return_type_id, None,
            "no Subtype at all means no (name, id) pair to carry"
        );
    }

    // (c) Subtype carries an Id but NO Name → DECLINE to `None`. AL object ids
    // are NOT cross-app unique; a bare numeric reconstruction could resolve to
    // the WRONG app's object (round-1 critical — fail closed, never
    // synthesize).
    #[test]
    fn parse_method_return_type_id_only_declines() {
        let json = r##"{
            "Name":"Foo",
            "ReturnTypeDefinition":{"Name":"Codeunit","Subtype":{"Id":2354}}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(
            m.return_type_text, None,
            "Id-only Subtype must decline the WHOLE reconstruction, not fall \
             back to the bare outer Name"
        );
        assert_eq!(
            m.return_type_id, None,
            "no Name means no valid (name, id) pair — a lone id is not proof \
             of identity"
        );
    }

    // (f) FORMAT LANDMINE: Subtype.Name contains a quote character → the TEXT
    // reconstruction must strictly decline (never escape — downstream text
    // classification must never see synthesized escaping), but the raw
    // (name, id) identity pair is STILL carried: cross-validation is a
    // structured `==` comparison, never text synthesis, so the quote landmine
    // does not apply to it.
    #[test]
    fn parse_method_return_type_subtype_name_with_quote_declines_text_only() {
        let json = r##"{
            "Name":"Foo",
            "ReturnTypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"Weird\"Name","Id":1}}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(
            m.return_type_text, None,
            "a Subtype Name containing a quote must never be synthesized/escaped \
             into source-shaped text"
        );
        assert_eq!(
            m.return_type_id,
            Some(("Weird\"Name".to_string(), 1)),
            "the raw identity pair is independent of the TEXT landmine — it is \
             never used to synthesize text"
        );
    }

    // (f) FORMAT LANDMINE: a namespace/dot-qualified Subtype.Name must be
    // carried verbatim — never truncated.
    #[test]
    fn parse_method_return_type_namespace_qualified_subtype_name_not_truncated() {
        let json = r##"{
            "Name":"Foo",
            "ReturnTypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"My.Namespace.Http Content","Id":9}}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(
            m.return_type_text.as_deref(),
            Some("Codeunit \"My.Namespace.Http Content\""),
            "a dot-qualified name must be carried verbatim, never truncated at \
             the first `.`"
        );
        assert_eq!(
            m.return_type_id,
            Some(("My.Namespace.Http Content".to_string(), 9))
        );
    }

    // (f) FORMAT LANDMINE: a generic/container return (`List of [...]`, no
    // Subtype) passes through as-is — never approximated into a scalar.
    #[test]
    fn parse_method_return_type_generic_container_passthrough() {
        let json = r##"{
            "Name":"Foo",
            "ReturnTypeDefinition":{"Name":"List of [Codeunit \"Http Content\"]"}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(
            m.return_type_text.as_deref(),
            Some("List of [Codeunit \"Http Content\"]"),
            "a generic/container return with no Subtype passes through as-is — \
             scalar-declined downstream, never approximated here"
        );
        assert_eq!(m.return_type_id, None);
    }

    // Defensive: a Subtype object present but carrying NEITHER Name nor Id →
    // decline (nothing usable to reconstruct or cross-validate).
    #[test]
    fn parse_method_return_type_empty_subtype_declines() {
        let json = r##"{
            "Name":"Foo",
            "ReturnTypeDefinition":{"Name":"Codeunit","Subtype":{}}
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(m.return_type_text, None);
        assert_eq!(m.return_type_id, None);
    }

    // No ReturnTypeDefinition at all → both fields `None` (control).
    #[test]
    fn parse_method_no_return_type_definition_yields_none() {
        let json = r##"{"Name":"Foo"}"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(m.return_type_text, None);
        assert_eq!(m.return_type_id, None);
    }

    // -- Task 2 round-2 addendum: PARAM/FIELD Subtype fidelity ----------------
    //
    // `reconstruct_param_field_type` — bare-outer-name-fallback TEXT + the raw
    // discriminator tuple `param_type_fp` folds. Every case drives the FULL
    // `parse_method`/`parse_field` path from raw JSON.

    // (a) Task 2 brief fixture: a param with `Subtype{Name:"Dep A",Id:..}`
    // yields fully source-shaped `type_text = Codeunit "Dep A"`, and the raw
    // discriminator pair is carried alongside it.
    #[test]
    fn parse_method_param_subtype_reconstructs_quoted_source_shape() {
        let json = r##"{
            "Name":"Get",
            "Parameters":[{"Name":"X","TypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"Dep A","Id":60130}}}]
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        assert_eq!(m.parameters.len(), 1);
        let p = &m.parameters[0];
        assert_eq!(p.type_text, "Codeunit \"Dep A\"");
        assert_eq!(p.subtype_id, Some(60130));
        assert_eq!(p.subtype_raw_name.as_deref(), Some("Dep A"));
        assert_eq!(p.subtype_tag, SubtypeTag::Full);
    }

    // (b) round-1 critical sliver: an Id-only Subtype (no Name — a real
    // observed ABI shape, see `RawSubtype`'s doc) FALLS BACK to the bare
    // outer name for TEXT (never empty — that would regress dedup), but the
    // raw `Id` is STILL carried as the fingerprint discriminator. TWO
    // DIFFERENT Id-only subtypes (`DoIt(Codeunit 10)` vs `DoIt(Codeunit
    // 20)`) must therefore carry DIFFERENT `subtype_id`s despite IDENTICAL
    // `type_text` — the primitive `abi_ingest::param_type_fp` needs to keep
    // them from silently colliding.
    #[test]
    fn parse_method_param_id_only_subtype_falls_back_to_bare_name_but_keeps_id() {
        let json_10 = r##"{
            "Name":"DoIt",
            "Parameters":[{"Name":"X","TypeDefinition":{"Name":"Codeunit","Subtype":{"Id":10}}}]
        }"##;
        let json_20 = r##"{
            "Name":"DoIt",
            "Parameters":[{"Name":"X","TypeDefinition":{"Name":"Codeunit","Subtype":{"Id":20}}}]
        }"##;
        let m10 = parse_method(&serde_json::from_str(json_10).unwrap());
        let m20 = parse_method(&serde_json::from_str(json_20).unwrap());
        let p10 = &m10.parameters[0];
        let p20 = &m20.parameters[0];

        assert_eq!(
            p10.type_text, "Codeunit",
            "an Id-only Subtype must fall back to the bare outer name, never empty"
        );
        assert_eq!(p20.type_text, "Codeunit");
        assert_eq!(
            p10.type_text, p20.type_text,
            "the TEXT is identical by construction — this is exactly the \
             sliver the raw discriminator must still distinguish"
        );
        assert_eq!(p10.subtype_id, Some(10));
        assert_eq!(p20.subtype_id, Some(20));
        assert_ne!(
            p10.subtype_id, p20.subtype_id,
            "two DIFFERENT Id-only subtypes must carry DIFFERENT raw \
             discriminators despite identical bare-fallback text"
        );
        assert_eq!(p10.subtype_tag, SubtypeTag::IdOnly);
        assert_eq!(p20.subtype_tag, SubtypeTag::IdOnly);
    }

    // (b) sibling FORMAT LANDMINE: a Subtype.Name containing a `"` also falls
    // back to the bare outer name for TEXT (never escapes/synthesizes), but
    // the raw (quote-bearing) name is STILL carried as the discriminator so
    // two DIFFERENT quote-bearing names never collide either.
    #[test]
    fn parse_method_param_quoted_subtype_name_falls_back_but_keeps_raw_name() {
        let json_a = r##"{
            "Name":"DoIt",
            "Parameters":[{"Name":"X","TypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"Weird\"NameA","Id":1}}}]
        }"##;
        let json_b = r##"{
            "Name":"DoIt",
            "Parameters":[{"Name":"X","TypeDefinition":{"Name":"Codeunit","Subtype":{"Name":"Weird\"NameB","Id":1}}}]
        }"##;
        let ma = parse_method(&serde_json::from_str(json_a).unwrap());
        let mb = parse_method(&serde_json::from_str(json_b).unwrap());
        let pa = &ma.parameters[0];
        let pb = &mb.parameters[0];

        assert_eq!(pa.type_text, "Codeunit");
        assert_eq!(pb.type_text, "Codeunit");
        assert_eq!(pa.subtype_raw_name.as_deref(), Some("Weird\"NameA"));
        assert_eq!(pb.subtype_raw_name.as_deref(), Some("Weird\"NameB"));
        assert_ne!(
            pa.subtype_raw_name, pb.subtype_raw_name,
            "two DIFFERENT quote-bearing Subtype Names sharing the same Id \
             must still carry different raw-name discriminators"
        );
        assert_eq!(pa.subtype_tag, SubtypeTag::NameQuoted);
        assert_eq!(pb.subtype_tag, SubtypeTag::NameQuoted);
    }

    // Control: a bare scalar param (no Subtype at all) is UNCHANGED — the
    // pre-Task-2 fidelity, tagged distinctly from a degraded object-typed
    // param so the two families can never collide by coincidence.
    #[test]
    fn parse_method_param_no_subtype_bare_passthrough() {
        let json = r##"{
            "Name":"Foo",
            "Parameters":[{"Name":"n","TypeDefinition":{"Name":"Integer"}}]
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        let p = &m.parameters[0];
        assert_eq!(p.type_text, "Integer");
        assert_eq!(p.subtype_id, None);
        assert_eq!(p.subtype_raw_name, None);
        assert_eq!(p.subtype_tag, SubtypeTag::NoSubtype);
    }

    // ALREADY-QUOTED LANDMINE regression (round-2 fold-in fix, found by
    // `tests/temp_state_abi.rs`'s real-Continia-shaped fixture): a
    // RECORD-typed param's outer `Name` is ALREADY the full source-shaped
    // text (`Record "Normal Table"`), sometimes ALONGSIDE a redundant
    // same-named `Subtype` — the naive "append the Subtype name" rule would
    // double-quote-corrupt this into `Record "Normal Table" "Normal Table"`.
    // Must pass through UNCHANGED, never re-append.
    #[test]
    fn parse_method_param_already_quoted_outer_name_with_redundant_subtype_not_doubled() {
        let json = r##"{
            "Name":"TempMarkedParam",
            "Parameters":[{"Name":"Rec","TypeDefinition":{"Name":"Record \"Normal Table\"","Subtype":{"Name":"Normal Table"}}}]
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        let p = &m.parameters[0];
        assert_eq!(
            p.type_text, "Record \"Normal Table\"",
            "an already-quoted outer Name must never be re-appended to"
        );
        assert_eq!(p.subtype_tag, SubtypeTag::AlreadyQuoted);
    }

    // Sibling control: the SAME already-quoted outer Name with NO Subtype at
    // all (the more common real shape — see `ByVarUnmarked`/`ByValueUnmarked`
    // in `tests/temp_state_abi.rs`) must ALSO pass through unchanged.
    #[test]
    fn parse_method_param_already_quoted_outer_name_no_subtype_passthrough() {
        let json = r##"{
            "Name":"ByVarUnmarked",
            "Parameters":[{"Name":"Rec","TypeDefinition":{"Name":"Record \"Normal Table\""}}]
        }"##;
        let raw: RawMethod = serde_json::from_str(json).unwrap();
        let m = parse_method(&raw);
        let p = &m.parameters[0];
        assert_eq!(p.type_text, "Record \"Normal Table\"");
        assert_eq!(p.subtype_tag, SubtypeTag::AlreadyQuoted);
    }

    /// Parses a single parameter's JSON shape (a `Parameters[0]` element body,
    /// e.g. `{"Name":"p","TypeDefinition":{...}}`) through the SAME
    /// production path every other test in this module uses (`parse_method`)
    /// — wraps it in a minimal `RawMethod` envelope rather than inventing a
    /// second, divergent parsing route.
    /// #27: Scope / InherentPermissions / InherentEntitlements reach the ABI, at
    /// the object level (`Properties`) AND the member level (`Attributes`), and a
    /// page-control layout value sharing the `Scope` name is NOT a scope.
    #[test]
    fn scope_and_inherent_properties_reach_the_abi() {
        let json = r#"{
            "AppId": "11111111-0000-0000-0000-000000000027",
            "Name": "Dep",
            "Codeunits": [{
                "Id": 50100, "Name": "OnPremOnly",
                "Properties": [
                    {"Name": "Scope", "Value": "OnPrem"},
                    {"Name": "InherentPermissions", "Value": "X"},
                    {"Name": "InherentEntitlements", "Value": "X"}
                ],
                "Methods": [
                    {"Name": "Elevated", "Parameters": [],
                     "Attributes": [
                        {"Name": "Scope", "Arguments": [{"Value": "OnPrem"}]},
                        {"Name": "InherentPermissions",
                         "Arguments": [{"Value": "PermissionObjectType::TableData"},
                                       {"Value": "Database::\"Customer\""},
                                       {"Value": "R"}]}
                     ]},
                    {"Name": "Plain", "Parameters": []}
                ]
            }],
            "Pages": [{
                "Id": 50101, "Name": "ListPart",
                "Properties": [{"Name": "Scope", "Value": "Repeater"}]
            }]
        }"#;
        let abi = parse_symbol_reference(json);
        let cu = abi
            .objects
            .iter()
            .find(|o| o.name == "OnPremOnly")
            .expect("codeunit");
        assert_eq!(cu.scope.as_deref(), Some("OnPrem"));
        assert_eq!(cu.inherent_permissions.as_deref(), Some("X"));
        assert_eq!(cu.inherent_entitlements.as_deref(), Some("X"));

        let elevated = cu.routines.iter().find(|r| r.name == "Elevated").unwrap();
        assert_eq!(elevated.scope().as_deref(), Some("OnPrem"));
        assert_eq!(elevated.inherent_permissions().map(|a| a.len()), Some(3));
        let plain = cu.routines.iter().find(|r| r.name == "Plain").unwrap();
        assert_eq!(plain.scope(), None);
        assert_eq!(plain.inherent_permissions(), None);

        let page = abi
            .objects
            .iter()
            .find(|o| o.name == "ListPart")
            .expect("page");
        assert_eq!(
            page.scope, None,
            "Repeater is page layout, not an application scope"
        );
    }

    fn parse_param_json(param_json: &str) -> AbiParameter {
        let wrapped = format!(r#"{{"Name":"Get","Parameters":[{param_json}]}}"#);
        let raw: RawMethod = serde_json::from_str(&wrapped).unwrap();
        let m = parse_method(&raw);
        m.parameters.into_iter().next().unwrap()
    }

    /// Fix round 1 — review finding (Important 1): `SubtypeTag::as_str()` is
    /// load-bearing (it feeds `abi_ingest::fold_param_discriminator`'s
    /// `sig_fp`/`param_type_fp` fold — the exact primitive that keeps two
    /// different ABI overloads from silently colliding onto one
    /// `RoutineNodeId`), but before this test existed nothing anywhere in
    /// the workspace asserted its exact string output: `grep -rn 'as_str()'
    /// src/ tests/ | grep -i subtype` found exactly ONE line, the production
    /// call site itself, zero assertions. Two review probes proved this was
    /// a real gap: renaming `Full`'s literal to a novel string left every
    /// test green (nothing pins the bytes), and renaming it to COLLIDE with
    /// `NoSubtype`'s literal ALSO left every test green — a genuine
    /// `param_type_fp` fingerprint collision between a `Full`-tagged and a
    /// `NoSubtype`-tagged parameter. This test pins both failure modes:
    /// the literal value of each variant, and that all eight are pairwise
    /// distinct.
    #[test]
    fn subtype_tag_as_str_values_are_pinned_and_distinct() {
        let pairs = [
            (SubtypeTag::NoTypeDefinition, "no_type_definition"),
            (SubtypeTag::NoName, "no_name"),
            (SubtypeTag::AlreadyQuoted, "already_quoted"),
            (SubtypeTag::NoSubtype, "no_subtype"),
            (SubtypeTag::Full, "full"),
            (SubtypeTag::NameQuoted, "name_quoted"),
            (SubtypeTag::IdOnly, "id_only"),
            (SubtypeTag::EmptySubtype, "empty_subtype"),
        ];
        for (tag, expected) in pairs {
            assert_eq!(tag.as_str(), expected, "{tag:?}'s as_str() literal");
        }
        let distinct: std::collections::HashSet<&str> =
            pairs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            distinct.len(),
            pairs.len(),
            "every SubtypeTag must fold to a DISTINCT sig_fp discriminator \
             string -- a collision here silently collapses two genuinely \
             different ABI overloads onto one param_type_fp"
        );
    }

    /// Pins the USE — that the real parse path produces each documented
    /// `SubtypeTag` value — rather than testing a standalone converter.
    /// Hand-stated preconditions: one raw parameter shape per documented tag,
    /// constructed literally so this survives any change to how the parser
    /// reaches these shapes.
    ///
    /// Review finding (fix round 1, Important 2): the original version of
    /// this test asserted only 4 of the 8 variants (`NoSubtype`/`Full`/
    /// `IdOnly`/`NameQuoted`), all of which pre-existing tests in this module
    /// ALSO already covered on the same production arm — so despite the
    /// name, `NoTypeDefinition`/`NoName`/`EmptySubtype` had ZERO parse-path
    /// coverage anywhere in the workspace. Extended (fix round 1) to close
    /// those three, then extended again (fix round 2) with `AlreadyQuoted`
    /// — the remaining variant this test itself hadn't asserted, even
    /// though `AlreadyQuoted` was already independently covered by two
    /// pre-existing tests (`parse_method_param_already_quoted_outer_name_*`).
    /// All 8 variants are now asserted directly by this one test.
    ///
    /// STATED LIMIT (per this repo's Testing Philosophy: a discrimination
    /// proof that passes is evidence about the test, not the code, and a
    /// break caught by a REDUNDANT second path must be recorded, not left
    /// implicit): the original commit's Step 9 discrimination proof (flipping
    /// `IdOnly` → `Full` in `reconstruct_param_field_type`; only the
    /// RECORDING below is new, added in fix round 1) is caught by this
    /// test's `id_only` assertion, but is EQUALLY caught by the pre-existing
    /// `parse_method_param_id_only_subtype_falls_back_to_bare_name_but_keeps_id`
    /// test above — this test's marginal, EXCLUSIVE discrimination power over
    /// that specific break is zero. Its real (non-redundant) value is the
    /// four variants below (the three from fix round 1 plus `AlreadyQuoted`
    /// from fix round 2), none of which any OTHER single test in the
    /// workspace covers via this exact minimal-JSON shape.
    #[test]
    fn subtype_tag_is_a_closed_enum_over_the_real_parse_path() {
        let no_subtype = parse_param_json(r#"{"Name":"p","TypeDefinition":{"Name":"Integer"}}"#);
        assert_eq!(no_subtype.subtype_tag, SubtypeTag::NoSubtype);

        let full = parse_param_json(
            r#"{"Name":"p","TypeDefinition":{"Name":"Record","Subtype":{"Id":18,"Name":"Customer"}}}"#,
        );
        assert_eq!(full.subtype_tag, SubtypeTag::Full);

        let id_only = parse_param_json(
            r#"{"Name":"p","TypeDefinition":{"Name":"Record","Subtype":{"Id":18}}}"#,
        );
        assert_eq!(id_only.subtype_tag, SubtypeTag::IdOnly);

        // NOTE (deviation from the task brief's verbatim fixture): the brief's
        // JSON for this arm used `"Subtype":{"Name":"Sales Header"}` — a
        // plain, quote-free name. Per `reconstruct_param_field_type`'s own
        // arms, a quote-FREE `Subtype.Name` is exactly the `Full` case
        // (Id is irrelevant to the branch), so that fixture actually produced
        // `SubtypeTag::Full`, not `NameQuoted` — proven by running it: it
        // failed with `left: Full, right: NameQuoted`. `NameQuoted` requires
        // an embedded `"` inside `Subtype.Name` (see the pre-existing
        // `parse_method_param_quoted_subtype_name_falls_back_but_keeps_raw_name`
        // test above, which uses the same `Weird\"Name` shape). Corrected here
        // so the fixture matches the tag it claims to pin.
        let name_quoted = parse_param_json(
            r#"{"Name":"p","TypeDefinition":{"Name":"Record","Subtype":{"Name":"Sales \"Header\""}}}"#,
        );
        assert_eq!(name_quoted.subtype_tag, SubtypeTag::NameQuoted);

        // -- fix round 1 additions: the three previously-uncovered variants --

        // No `TypeDefinition` key at all.
        let no_type_definition = parse_param_json(r#"{"Name":"p"}"#);
        assert_eq!(no_type_definition.subtype_tag, SubtypeTag::NoTypeDefinition);

        // `TypeDefinition` present, but its own `Name` is absent (defensive
        // arm; should not occur in real `SymbolReference.json`).
        let no_name = parse_param_json(r#"{"Name":"p","TypeDefinition":{}}"#);
        assert_eq!(no_name.subtype_tag, SubtypeTag::NoName);

        // `Subtype` present but carries neither `Name` nor `Id`.
        let empty_subtype =
            parse_param_json(r#"{"Name":"p","TypeDefinition":{"Name":"Record","Subtype":{}}}"#);
        assert_eq!(empty_subtype.subtype_tag, SubtypeTag::EmptySubtype);

        // -- fix round 2 addition: the eighth variant, previously asserted
        // only by other tests, never by this one -- the outer `Name` already
        // contains a `"`.
        let already_quoted =
            parse_param_json(r#"{"Name":"p","TypeDefinition":{"Name":"Record \"Widget\""}}"#);
        assert_eq!(already_quoted.subtype_tag, SubtypeTag::AlreadyQuoted);
    }

    // (c) `parse_field` gets the SAME treatment: an ABI Enum field now
    // carries `Enum "X"` instead of the bare `"Enum"` this dropped before.
    #[test]
    fn parse_field_enum_subtype_reconstructs_quoted_source_shape() {
        let json = r##"{
            "Id":1,
            "Name":"Status",
            "TypeDefinition":{"Name":"Enum","Subtype":{"Name":"X","Id":50100}}
        }"##;
        let raw: RawField = serde_json::from_str(json).unwrap();
        let f = parse_field(&raw);
        assert_eq!(f.data_type, "Enum \"X\"");
    }

    // `parse_field` control: a scalar field (no Subtype) is unchanged.
    #[test]
    fn parse_field_no_subtype_bare_passthrough() {
        let json = r##"{"Id":1,"Name":"Amount","TypeDefinition":{"Name":"Decimal"}}"##;
        let raw: RawField = serde_json::from_str(json).unwrap();
        let f = parse_field(&raw);
        assert_eq!(f.data_type, "Decimal");
    }
}
