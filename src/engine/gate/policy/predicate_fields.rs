//! Predicate field registry — port of al-sem `src/policy/predicate-fields.ts`.
//!
//! 16 fields across 3 scopes (routine / fact / model — al-sem has no model-scope
//! field in the shipped registry, so only routine + fact appear), plus the 4
//! pin fields of policy version 2 (`write.table`, `write.field`, `rule.kind`,
//! `rule.atoms`; scopes write and rule; rules-to-policy spec
//! `docs/superpowers/specs/2026-10-30-rules-to-policy-design.md` §2.3). Each field
//! carries its `scope`, `value_shape` (drives the compiler's operator derivation),
//! optional `enum_values` (validated at compile time), and an `evaluate` that
//! reads the [`FieldEvalContext`] and returns a [`FieldValue`].
//!
//! The model inputs (root classifications, capability facts, coverage, table /
//! event names) are byte-parity with al-sem from the earlier R-phases (verified by
//! the cli-c/c2 STEP-0 model-input check), so the policy output is a pure function
//! of them.

use std::collections::HashMap;

use crate::engine::l4::capability_cone::{CapabilityExtra, CapabilityFact};
use crate::program::model::declaring::DeclaringObjects;
use crate::program::model::workspace::{ModelObject, ModelRoutine, ModelTable};

/// Field scope. al-sem `FieldScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldScope {
    Routine,
    Fact,
    #[allow(dead_code)]
    Model,
    /// One write fact of a pin (rules-to-policy spec §2.3): a writer and one
    /// table field it writes. Read in a pin's `when` / `except`.
    Write,
    /// One rule key a writer applies (d65's key). Read in a pin's `requires`.
    Rule,
}

/// A pin's write fact as the write fields read it: the written table's name
/// and the field's name as the table declares it.
#[derive(Debug, Clone, Copy)]
pub struct WriteFact<'a> {
    pub table: &'a str,
    pub field: &'a str,
}

/// One rule key a writer applies, as the rule fields read it: its kind and
/// its atoms on the written table (d65's `RuleKey`).
#[derive(Debug, Clone, Copy)]
pub struct RuleFact<'a> {
    pub kind: &'a str,
    pub atoms: &'a str,
}

/// Field value shape. al-sem `FieldValueShape` (the shipped subset; `numeric` /
/// `tri-state` are unused in the registry).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldValueShape {
    Enum,
    EnumList,
    Glob,
    GlobList,
    StringExact,
    StringList,
}

/// Reason a field evaluated to unknown. al-sem `FieldValue` unknown reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownReason {
    FieldNotApplicable,
    ResourceIdUnresolved,
    #[allow(dead_code)]
    NoRootClassification,
    EvaluatorError,
}

/// A field evaluation result. al-sem `FieldValue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    /// A single string value (`routine.name`, `capability.op`, …).
    KnownStr(String),
    /// A list value (`root.kinds`). Empty list is KNOWN (NOT unknown).
    KnownList(Vec<String>),
    Unknown(UnknownReason),
}

/// Per-run lookup bundle. al-sem `FieldIndexes`. Built once, threaded through every
/// field evaluation. `root_kinds_by_routine_id` maps a routine id → its root kinds
/// (absent ⇒ `[]`, NOT unknown — classification is exhaustive).
pub struct FieldIndexes<'a> {
    /// The routine's declaring object (#279): an `#if`-split id has two rows.
    pub declaring: DeclaringObjects<'a>,
    pub root_kinds_by_routine_id: HashMap<&'a str, &'a [String]>,
    pub tables_by_id: HashMap<&'a str, &'a ModelTable>,
    pub events_by_id: HashMap<&'a str, &'a str>, // event id → event name
}

/// Evaluation context. al-sem `FieldEvalContext`. `fact` is `None` in applicability
/// mode (fact-scope fields short-circuit to unknown before reading it). `write`
/// and `rule` are a pin's (rules-to-policy spec §2.3): `write` while its `when`
/// / `except` read a write fact, `rule` while its `requires` reads one applied
/// key; `None` everywhere else, and a write or rule field then reads unknown.
pub struct FieldEvalContext<'a> {
    pub routine: &'a ModelRoutine,
    pub fact: Option<&'a CapabilityFact>,
    pub write: Option<WriteFact<'a>>,
    pub rule: Option<RuleFact<'a>>,
    pub indexes: &'a FieldIndexes<'a>,
}

/// A predicate field definition. al-sem `PredicateFieldDef`.
pub struct PredicateFieldDef {
    pub name: &'static str,
    pub scope: FieldScope,
    pub value_shape: FieldValueShape,
    pub enum_values: Option<&'static [&'static str]>,
}

// ---- enum value tables (mirror predicate-fields.ts) ----

const RESOURCE_KINDS: &[&str] = &[
    "table",
    "event",
    "codeunit",
    "page",
    "report",
    "http",
    "telemetry",
    "isolated-storage",
    "file",
    "transaction",
    "ui",
    "background",
];

const CAPABILITY_OPS: &[&str] = &[
    "read",
    "insert",
    "modify",
    "delete",
    "rename",
    "lock",
    "execute",
    "publish",
    "subscribe",
    "send",
    "log",
    "store-read",
    "store-write",
    "store-delete",
    "commit",
    "open",
    "write-blob",
    "start",
    "ui-confirm",
    "ui-message",
    "ui-error",
];

const CONFIDENCE_VALUES: &[&str] = &[
    "static",
    "boundedDynamic",
    "configDynamic",
    "userDynamic",
    "unresolved",
];

const VIA_VALUES: &[&str] = &[
    "self",
    "call",
    "object-run",
    "event-dispatch",
    "implicit-trigger",
    "dependency",
];

const PROVENANCE_VALUES: &[&str] = &["direct", "inherited"];

const ROUTINE_KINDS: &[&str] = &[
    "procedure",
    "trigger",
    "event-publisher",
    "event-subscriber",
];

const ACCESS_MODIFIERS: &[&str] = &["public", "internal", "local", "protected"];

const OBJECT_KINDS: &[&str] = &[
    "codeunit",
    "table",
    "page",
    "report",
    "xmlport",
    "query",
    "enum",
    "interface",
    "permissionset",
];

const HTTP_METHODS: &[&str] = &["Send", "Get", "Post", "Put", "Delete", "Patch"];

const UI_KINDS: &[&str] = &[
    "confirm",
    "message",
    "error",
    "dialog",
    "modalPage",
    "requestPage",
];

/// The registry: al-sem's 16 fields (declaration order mirrors
/// predicate-fields.ts), then the 4 pin fields.
pub const PREDICATE_FIELDS: &[PredicateFieldDef] = &[
    // ---- Routine-scope fields ----
    PredicateFieldDef {
        name: "root.kinds",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::EnumList,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "routine.name",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "routine.kind",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::Enum,
        enum_values: Some(ROUTINE_KINDS),
    },
    PredicateFieldDef {
        name: "routine.accessModifier",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(ACCESS_MODIFIERS),
    },
    PredicateFieldDef {
        name: "object.name",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "object.kind",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(OBJECT_KINDS),
    },
    PredicateFieldDef {
        name: "object.appGuid",
        scope: FieldScope::Routine,
        value_shape: FieldValueShape::StringExact,
        enum_values: None,
    },
    // ---- Fact-scope fields ----
    PredicateFieldDef {
        name: "capability.op",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(CAPABILITY_OPS),
    },
    PredicateFieldDef {
        name: "capability.resourceKind",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(RESOURCE_KINDS),
    },
    PredicateFieldDef {
        name: "capability.resource.table.name",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "capability.resource.event.name",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "capability.resource.http.method",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(HTTP_METHODS),
    },
    PredicateFieldDef {
        name: "capability.resource.ui.kind",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(UI_KINDS),
    },
    PredicateFieldDef {
        name: "capability.confidence",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(CONFIDENCE_VALUES),
    },
    PredicateFieldDef {
        name: "capability.origin",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(PROVENANCE_VALUES),
    },
    PredicateFieldDef {
        name: "capability.via",
        scope: FieldScope::Fact,
        value_shape: FieldValueShape::EnumList,
        enum_values: Some(VIA_VALUES),
    },
    // ---- Pin fields (policy version 2; rules-to-policy spec §2.3) ----
    PredicateFieldDef {
        name: "write.table",
        scope: FieldScope::Write,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "write.field",
        scope: FieldScope::Write,
        value_shape: FieldValueShape::Glob,
        enum_values: None,
    },
    PredicateFieldDef {
        name: "rule.kind",
        scope: FieldScope::Rule,
        value_shape: FieldValueShape::Enum,
        enum_values: Some(RULE_KINDS),
    },
    // Exact text: a rule with one more atom fires in fewer cases (a weaker
    // rule), so "contains this atom" would count it as applied.
    PredicateFieldDef {
        name: "rule.atoms",
        scope: FieldScope::Rule,
        value_shape: FieldValueShape::StringExact,
        enum_values: None,
    },
];

/// `rule.kind`'s values: d65's kind texts (`RuleKind`).
const RULE_KINDS: &[&str] = &[
    "Error",
    "TestField",
    "FieldError",
    "ConfirmError",
    "ConfirmExit",
];

/// Look up a field definition by name. al-sem `getFieldDef`.
pub fn get_field_def(name: &str) -> Option<&'static PredicateFieldDef> {
    PREDICATE_FIELDS.iter().find(|f| f.name == name)
}

/// Evaluate a field over the context. al-sem `PredicateFieldDef.evaluate`. Returns
/// the `FieldValue`. Fact-scope fields require `ctx.fact` to be `Some`; the engine
/// guarantees it for the full-eval path (applicability mode short-circuits them to
/// unknown BEFORE calling this).
pub fn evaluate_field(def: &PredicateFieldDef, ctx: &FieldEvalContext) -> FieldValue {
    match def.name {
        // ---- routine-scope ----
        "root.kinds" => {
            // Exhaustive classification: a miss is `[]` (KNOWN), never unknown.
            let kinds = ctx
                .indexes
                .root_kinds_by_routine_id
                .get(ctx.routine.id.as_str())
                .copied()
                .unwrap_or(&[]);
            FieldValue::KnownList(kinds.to_vec())
        }
        "routine.name" => FieldValue::KnownStr(ctx.routine.name.clone()),
        "routine.kind" => FieldValue::KnownStr(ctx.routine.kind.clone()),
        "routine.accessModifier" => {
            // Absent/None means AL default ("public").
            let am = ctx
                .routine
                .access_modifier
                .clone()
                .unwrap_or_else(|| "public".to_string());
            FieldValue::KnownStr(am)
        }
        "object.name" => match object_of(ctx) {
            Some(o) => FieldValue::KnownStr(o.name.clone()),
            None => FieldValue::Unknown(UnknownReason::FieldNotApplicable),
        },
        "object.kind" => match object_of(ctx) {
            // objectType is verbatim from AL source — lowercase for matching.
            Some(o) => FieldValue::KnownStr(o.object_type.to_lowercase()),
            None => FieldValue::Unknown(UnknownReason::FieldNotApplicable),
        },
        "object.appGuid" => match object_of(ctx) {
            Some(o) => FieldValue::KnownStr(o.app_guid.clone()),
            None => FieldValue::Unknown(UnknownReason::FieldNotApplicable),
        },
        // ---- fact-scope ----
        "capability.op" => match ctx.fact {
            Some(f) => FieldValue::KnownStr(f.op.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "capability.resourceKind" => match ctx.fact {
            Some(f) => FieldValue::KnownStr(f.resource_kind.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "capability.resource.table.name" => {
            let Some(f) = ctx.fact else {
                return FieldValue::Unknown(UnknownReason::EvaluatorError);
            };
            if f.resource_kind != "table" {
                return FieldValue::Unknown(UnknownReason::FieldNotApplicable);
            }
            let Some(table_id) = f.resource_id.as_deref() else {
                return FieldValue::Unknown(UnknownReason::ResourceIdUnresolved);
            };
            match ctx.indexes.tables_by_id.get(table_id) {
                Some(t) => FieldValue::KnownStr(t.name.clone()),
                None => FieldValue::Unknown(UnknownReason::ResourceIdUnresolved),
            }
        }
        "capability.resource.event.name" => {
            let Some(f) = ctx.fact else {
                return FieldValue::Unknown(UnknownReason::EvaluatorError);
            };
            if f.resource_kind != "event" {
                return FieldValue::Unknown(UnknownReason::FieldNotApplicable);
            }
            let Some(event_id) = f.resource_id.as_deref() else {
                return FieldValue::Unknown(UnknownReason::ResourceIdUnresolved);
            };
            match ctx.indexes.events_by_id.get(event_id) {
                Some(name) => FieldValue::KnownStr((*name).to_string()),
                None => FieldValue::Unknown(UnknownReason::ResourceIdUnresolved),
            }
        }
        "capability.resource.http.method" => {
            let Some(f) = ctx.fact else {
                return FieldValue::Unknown(UnknownReason::EvaluatorError);
            };
            if f.resource_kind != "http" {
                return FieldValue::Unknown(UnknownReason::FieldNotApplicable);
            }
            match &f.extra {
                Some(CapabilityExtra::Http { method, .. }) => FieldValue::KnownStr(method.clone()),
                _ => FieldValue::Unknown(UnknownReason::FieldNotApplicable),
            }
        }
        "capability.resource.ui.kind" => {
            let Some(f) = ctx.fact else {
                return FieldValue::Unknown(UnknownReason::EvaluatorError);
            };
            if f.resource_kind != "ui" {
                return FieldValue::Unknown(UnknownReason::FieldNotApplicable);
            }
            match f.op {
                "ui-confirm" => FieldValue::KnownStr("confirm".to_string()),
                "ui-message" => FieldValue::KnownStr("message".to_string()),
                "ui-error" => FieldValue::KnownStr("error".to_string()),
                _ => FieldValue::Unknown(UnknownReason::FieldNotApplicable),
            }
        }
        "capability.confidence" => match ctx.fact {
            Some(f) => FieldValue::KnownStr(f.confidence.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "capability.origin" => match ctx.fact {
            // Maps to CapabilityFact.provenance ("direct" | "inherited").
            Some(f) => FieldValue::KnownStr(f.provenance.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "capability.via" => match ctx.fact {
            Some(f) => FieldValue::KnownStr(f.via.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        // ---- pin fields ----
        "write.table" => match ctx.write {
            Some(w) => FieldValue::KnownStr(w.table.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "write.field" => match ctx.write {
            Some(w) => FieldValue::KnownStr(w.field.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "rule.kind" => match ctx.rule {
            Some(r) => FieldValue::KnownStr(r.kind.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        "rule.atoms" => match ctx.rule {
            Some(r) => FieldValue::KnownStr(r.atoms.to_string()),
            None => FieldValue::Unknown(UnknownReason::EvaluatorError),
        },
        // Unknown field name (compiler rejects these, but mirror the evaluator's
        // defensive `evaluator-error`).
        _ => FieldValue::Unknown(UnknownReason::EvaluatorError),
    }
}

/// The containing object for a routine, when present. al-sem `objectOf`.
fn object_of<'a>(ctx: &'a FieldEvalContext<'a>) -> Option<&'a ModelObject> {
    ctx.indexes.declaring.of(ctx.routine)
}
