//! Structured-attribute model + accessors (moved from `engine::l3::al_attributes` in
//! engine-switch S1: the program engine's ABI ingestion reads it, so it lives here,
//! not in the legacy engine). Rust port of al-sem's
//! `src/model/attributes.ts` (`AttributeInfo` / `AttributeArg` +
//! `findAttribute` / `qualifiedArg` / `stringArg` / `boolArg`).
//!
//! The grammar-derived `AttributeArg` shape (kind / text / value / qualifier /
//! member) is ALREADY produced verbatim by R1's L2 attribute indexing
//! (`src/engine/l2/l2_workspace.rs::attr_arg_from_node`, which mirrors al-sem's
//! `attribute-from-node.ts`). This module mirrors that JSON shape as a typed
//! struct so the event-graph resolver can query attributes structurally, and so
//! the R2c parity vectors can round-trip the al-sem serialization arg-for-arg.
//!
//! `value`, `qualifier`, `member` are OMITTED (None → skipped) when absent — the
//! exact serde contract that makes the R2c attribute-shape vectors round-trip
//! byte-for-byte against al-sem's serialized `attributesParsed`.

use serde::{Deserialize, Serialize};

/// One positional argument inside an attribute's argument list. Mirrors al-sem's
/// `AttributeArg`. `value`/`qualifier`/`member` are skipped when absent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttributeArg {
    /// Grammar-aligned argument kind (`boolean`, `string_literal`,
    /// `qualified_enum_value`, `database_reference`, …, else `unknown`).
    pub kind: String,
    /// Raw, source-faithful slice (includes quotes for string_literal /
    /// quoted_identifier).
    pub text: String,
    /// Unquoted / derived value:
    /// - string_literal / quoted_identifier: contents between the quotes
    /// - qualified_enum_value / database_reference: RHS of `::` (unquoted)
    /// - boolean / integer / identifier: the text verbatim
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub value: Option<String>,
    /// LHS of `::` for qualified_enum_value / database_reference.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub qualifier: Option<String>,
    /// RHS of `::` for qualified_enum_value / database_reference (UNQUOTED).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub member: Option<String>,
}

/// One `[Name(args...)]` attribute on a routine. Mirrors al-sem's `AttributeInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttributeInfo {
    /// Case-preserved name as written.
    pub name: String,
    /// Positional args.
    pub args: Vec<AttributeArg>,
    /// The full `[…]` source slice.
    pub raw: String,
}

/// The qualifier/member halves of a `::`-qualified arg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedArg {
    pub qualifier: String,
    pub member: String,
}

/// Case-insensitive lookup of the FIRST attribute matching `name`. Returns None
/// when absent — call sites branch on that, never panic. Mirrors `findAttribute`.
pub fn find_attribute<'a>(attrs: &'a [AttributeInfo], name: &str) -> Option<&'a AttributeInfo> {
    let lc = name.to_lowercase();
    attrs.iter().find(|a| a.name.to_lowercase() == lc)
}

/// True when at least one attribute matches `name` (case-insensitive).
pub fn has_attribute(attrs: &[AttributeInfo], name: &str) -> bool {
    find_attribute(attrs, name).is_some()
}

/// Read the `value` of arg at `index` when it is a `string_literal`. Returns None
/// when the arg is absent, of the wrong kind, or value is unset. Mirrors `stringArg`.
pub fn string_arg(attr: &AttributeInfo, index: usize) -> Option<String> {
    let a = attr.args.get(index)?;
    if a.kind != "string_literal" {
        return None;
    }
    a.value.clone()
}

/// Read a boolean positional arg at `index`. Returns Some(true)/Some(false) for the
/// literal `true`/`false` (case-insensitive), None when the arg is absent, of a
/// non-boolean kind, or has no value. Mirrors `boolArg`.
pub fn bool_arg(attr: &AttributeInfo, index: usize) -> Option<bool> {
    let a = attr.args.get(index)?;
    if a.kind != "boolean" {
        return None;
    }
    let v = a.value.as_ref()?;
    match v.to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Read a `qualified_enum_value` / `database_reference` arg's qualifier + member.
/// Returns None when the arg is absent or of a non-qualified kind. Mirrors
/// `qualifiedArg`.
pub fn qualified_arg(attr: &AttributeInfo, index: usize) -> Option<QualifiedArg> {
    let a = attr.args.get(index)?;
    if a.kind != "qualified_enum_value" && a.kind != "database_reference" {
        return None;
    }
    let qualifier = a.qualifier.clone()?;
    let member = a.member.clone()?;
    Some(QualifiedArg { qualifier, member })
}

/// The `[Obsolete(...)]` state — Removed only when the third arg is the qualified
/// enum value `ObsoleteState::Removed`; otherwise Pending. Absent `[Obsolete]` →
/// `obsolete_state == None`. Mirrors al-sem `RoutineAttributes["obsoleteState"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsoleteState {
    Pending,
    Removed,
}

/// Structured info parsed from a routine's `attributesParsed`. Port of al-sem
/// `RoutineAttributes` (`src/engine/attribute-parser.ts`). Only the fields the
/// ported detectors read are modelled (obsolete_state / obsolete_reason for D38;
/// internal_proc for parity with the al-sem shape).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineAttributes {
    pub obsolete_state: Option<ObsoleteState>,
    pub obsolete_reason: Option<String>,
    pub internal_proc: bool,
}

/// `parseRoutineAttributes` — parse `[Obsolete(reason, version[, ObsoleteState::X])]`
/// + `[InternalProc]` out of a routine's structured attributes. Logic EXACTLY al-sem:
///   - `[Obsolete]` present → `obsolete_reason = string_arg(0)`; `obsolete_state` is
///     `Removed` IFF `args[2]` exists AND its kind is `qualified_enum_value` AND its
///     member lowercased == "removed", else `Pending`. Absent → `obsolete_state` None.
///   - `internal_proc = has_attribute("InternalProc")`.
pub fn parse_routine_attributes(attrs: &[AttributeInfo]) -> RoutineAttributes {
    let mut obsolete_state: Option<ObsoleteState> = None;
    let mut obsolete_reason: Option<String> = None;

    if let Some(obsolete) = find_attribute(attrs, "Obsolete") {
        obsolete_reason = string_arg(obsolete, 0);
        // State is Removed only when arg 2 is a qualified enum value whose member is
        // "Removed" (e.g. `ObsoleteState::Removed`). Anything else — absent,
        // `ObsoleteState::Pending`, or any other shape — means Pending.
        let removed = match obsolete.args.get(2) {
            Some(state_arg) => {
                state_arg.kind == "qualified_enum_value"
                    && state_arg.member.as_deref().unwrap_or("").to_lowercase() == "removed"
            }
            None => false,
        };
        obsolete_state = Some(if removed {
            ObsoleteState::Removed
        } else {
            ObsoleteState::Pending
        });
    }

    let internal_proc = has_attribute(attrs, "InternalProc");

    RoutineAttributes {
        obsolete_state,
        obsolete_reason,
        internal_proc,
    }
}
