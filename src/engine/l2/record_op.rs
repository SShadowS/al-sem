//! Record-op name map + field-args op set (intraprocedural-body.ts).

/// Canonical record-op name (lowercase) → properly-cased RecordOpType.
pub fn record_op_type(method_lc: &str) -> Option<&'static str> {
    Some(match method_lc {
        "findset" => "FindSet",
        "findfirst" => "FindFirst",
        "findlast" => "FindLast",
        "find" => "Find",
        "get" => "Get",
        "calcfields" => "CalcFields",
        "calcsums" => "CalcSums",
        "testfield" => "TestField",
        "modify" => "Modify",
        "modifyall" => "ModifyAll",
        "insert" => "Insert",
        "delete" => "Delete",
        "deleteall" => "DeleteAll",
        "setloadfields" => "SetLoadFields",
        "addloadfields" => "AddLoadFields",
        "setrange" => "SetRange",
        "setfilter" => "SetFilter",
        "setcurrentkey" => "SetCurrentKey",
        "reset" => "Reset",
        "copy" => "Copy",
        "transferfields" => "TransferFields",
        "validate" => "Validate",
        "init" => "Init",
        "next" => "Next",
        "count" => "Count",
        "countapprox" => "CountApprox",
        "isempty" => "IsEmpty",
        "locktable" => "LockTable",
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
