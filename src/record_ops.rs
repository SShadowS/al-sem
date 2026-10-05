//! The AL record operations both engines recognise -- ONE table.
//!
//! The legacy L2 walk (`engine::l2::record_op`, which re-exports this) and the
//! program extractor (`program::resolve::extract`) used to keep their own
//! copies, and the extractor's was a hand copy that drifted: it lacked
//! `rename` (#9). The table lives at the crate root because it is a fact about
//! AL, not about either engine, and `program::resolve` must not import
//! `engine::l2`/`engine::l3` (`resolve_module_has_no_stray_engine_l3_l2_imports`).

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
        // A write: changes the primary key and commits it. Measured on BC 28
        // (#9): fires OnRename only, never OnModify; takes no RunTrigger.
        "rename" => "Rename",
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
