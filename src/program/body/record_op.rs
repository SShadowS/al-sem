//! Record-op name map + field-args op set (intraprocedural-body.ts).

/// The record-op name map. It lives in [`crate::record_ops`] so the program
/// extractor reads the same table (#9).
pub use crate::record_ops::record_op_type;

/// How a record method uses a field it names as an argument (agent-tasks
/// design §7.2, `alsem query touches --field`). Defined PER OPERATION, not by
/// membership in [`FIELD_ARGS_OPS`]: `Next`'s argument is a step, and `Get` /
/// `Find*` take key values, so none of them names a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldUse {
    /// The field's value is read.
    Read,
    /// The field's value is written (an assignment, `ModifyAll`).
    Write,
    /// `Validate(F, …)`: writes the field through its `OnValidate`.
    Validate,
    /// `SetRange` / `SetFilter` (and the filter readers `GetFilter`,
    /// `GetRangeMin`, `GetRangeMax`, `CopyFilter`'s source field).
    Filter,
    /// `SetCurrentKey`: the field is named as a sort key.
    KeyUse,
    /// `SetLoadFields` / `AddLoadFields`: the field is loaded.
    Load,
    /// `CalcFields` / `CalcSums` / `SetAutoCalcFields`: a FlowField or a sum.
    Calc,
    /// `TestField` / `FieldError`: a rule on the field's value.
    Rule,
    /// `FieldNo` / `FieldCaption` / `FieldName` / `FieldActive`: the field's
    /// metadata (a rename or a removal breaks it; its value is not read).
    Metadata,
    /// `TransferFields`: every field of both tables may be copied.
    CopyCandidate,
    /// A `RecordRef.Field(<literal number>)` site.
    ByNumber,
}

impl FieldUse {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FieldUse::Read => "read",
            FieldUse::Write => "write",
            FieldUse::Validate => "validate",
            FieldUse::Filter => "filter",
            FieldUse::KeyUse => "key-use",
            FieldUse::Load => "load",
            FieldUse::Calc => "calc",
            FieldUse::Rule => "rule",
            FieldUse::Metadata => "metadata",
            FieldUse::CopyCandidate => "copy-candidate",
            FieldUse::ByNumber => "by-number",
        }
    }
}

/// Which arguments of a record method name a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldArgs {
    /// Argument 0 only (`Validate(F, V)`, `SetRange(F, …)`).
    First,
    /// Every argument (`SetCurrentKey(F1, F2)`, `CalcFields(F1, F2)`).
    All,
}

/// The field use and field-argument positions of a record method called on a
/// record (`method_lc` lowercased), or `None` when its arguments name no
/// field. `TransferFields` and `RecordRef.Field` are not here: their argument
/// is a record or a number, read by the query on its own terms.
#[must_use]
pub fn field_method(method_lc: &str) -> Option<(FieldUse, FieldArgs)> {
    use FieldArgs::{All, First};
    Some(match method_lc {
        "validate" => (FieldUse::Validate, First),
        "setrange" | "setfilter" | "getfilter" | "getrangemin" | "getrangemax" | "copyfilter" => {
            (FieldUse::Filter, First)
        }
        "setcurrentkey" => (FieldUse::KeyUse, All),
        "setloadfields" | "addloadfields" => (FieldUse::Load, All),
        "calcfields" | "calcsums" | "setautocalcfields" => (FieldUse::Calc, All),
        "testfield" | "fielderror" => (FieldUse::Rule, First),
        "modifyall" => (FieldUse::Write, First),
        "fieldno" | "fieldcaption" | "fieldname" | "fieldactive" => (FieldUse::Metadata, First),
        _ => return None,
    })
}

/// Record ops for which all field arguments are captured.
pub const FIELD_ARGS_OPS: &[&str] = &[
    // `Next` is here for its STEP argument, not a field: `Next(2)` advances two
    // rows, so a loop built on it does not visit every row. d5/d60 cannot tell
    // that from `Next(1)` without the argument, and before issue #21 they did
    // not receive it at all -- every `Next` reached L5 with `field_arguments:
    // None`, so both detectors advised a set-based rewrite for a loop that
    // skips rows. See `whole_set_break` in `engine/l5/detectors/mod.rs`.
    "Next",
    "SetRange",
    "SetFilter",
    "SetLoadFields",
    "AddLoadFields",
    "SetCurrentKey",
    "Validate",
    "Get",
    "Find",
    "FindFirst",
    "FindLast",
    "FindSet",
    "CalcFields",
    "CalcSums",
    "TestField",
];
