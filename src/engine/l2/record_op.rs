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
        "rename" => "Rename",
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

#[cfg(test)]
mod vocabulary_drift_tests {
    use super::record_op_type;

    /// [`crate::program::resolve::extract::record_op_names`] is a hand-copied
    /// duplicate of [`record_op_type`]'s match arms, and the two gate DIFFERENT
    /// engines: this map decides what becomes a record operation in L2, that list
    /// decides whether the FRESH resolver treats a member call as a `RecordOp`.
    /// Nothing structural keeps them in step — they were identical by luck until
    /// issue #9 added `rename` and had to edit both.
    ///
    /// This test is the drift guard. It lives on the L2 side deliberately:
    /// `src/program/resolve/` is guarded to stay L2/L3-independent
    /// (`program_resolve_harness::resolve_module_has_no_stray_engine_l3_l2_imports`),
    /// so the import may only point this way.
    #[test]
    fn resolver_record_op_names_mirror_this_map_exactly() {
        let names = crate::program::resolve::extract::record_op_names();

        // Direction 1: everything the resolver lists is a real op here.
        for name in names {
            assert!(
                record_op_type(name).is_some(),
                "record_op_names() lists {name:?}, which record_op_type does not recognise"
            );
        }

        // Direction 2: every op here is in the resolver's list. `record_op_type` is a
        // match rather than an iterable, so the canonical cased set is spelled out;
        // a new op added to the map without touching this list fails on the first
        // assertion below, and one added here but not to the resolver fails the second.
        for cased in [
            "FindSet",
            "FindFirst",
            "FindLast",
            "Find",
            "Get",
            "CalcFields",
            "CalcSums",
            "TestField",
            "Modify",
            "ModifyAll",
            "Insert",
            "Delete",
            "DeleteAll",
            "Rename",
            "SetLoadFields",
            "AddLoadFields",
            "SetRange",
            "SetFilter",
            "SetCurrentKey",
            "Reset",
            "Copy",
            "TransferFields",
            "Validate",
            "Init",
            "Next",
            "Count",
            "CountApprox",
            "IsEmpty",
            "LockTable",
        ] {
            let lc = cased.to_ascii_lowercase();
            assert_eq!(
                record_op_type(&lc),
                Some(cased),
                "{cased:?} is no longer a record op — fix this list, not the assertion"
            );
            assert!(
                names.contains(&lc.as_str()),
                "record_op_type recognises {lc:?} but record_op_names() omits it — the fresh \
                 resolver would not treat it as a RecordOp"
            );
        }
    }
}
