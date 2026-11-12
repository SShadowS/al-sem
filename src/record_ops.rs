//! The AL record operations both engines recognise -- ONE table.
//!
//! The legacy L2 walk (`program::body::record_op`, which re-exports this) and the
//! program extractor (`program::resolve::extract`) used to keep their own
//! copies, and the extractor's was a hand copy that drifted: it lacked
//! `rename` (#9). The table lives at the crate root because it is a fact about
//! AL, not about either engine, and `program::resolve` must not import
//! `engine::l2`/`engine::l3` (`resolve_module_has_no_stray_engine_l3_l2_imports`).

/// The element type of an `array[N, ...] of T` type text: `T`. `None` for any
/// other type text. One definition for the body walk, the resolver's site
/// extraction and receiver typing (ENG-12: an array of records is a record
/// receiver through its element, `Recs[1].FindFirst()`).
pub fn array_element_type(ty: &str) -> Option<&str> {
    let t = ty.trim();
    if !t.get(..5)?.eq_ignore_ascii_case("array") {
        return None;
    }
    let close = t.find(']')?;
    let rest = t[close + 1..].trim_start();
    if !rest.get(..2)?.eq_ignore_ascii_case("of") {
        return None;
    }
    let element = rest[2..].trim();
    (!element.is_empty() && rest[2..].starts_with(char::is_whitespace)).then_some(element)
}

/// A declared type as a record receiver sees it: an array's element type, else
/// the type itself (ENG-12).
pub fn receiver_type_of(ty: &str) -> &str {
    array_element_type(ty).unwrap_or(ty)
}

/// The variable a record-op receiver expression names: `X` for `X` and for an
/// array element `X[i]` (ENG-12, L36). `None` for any other shape (a call
/// result, a member). Shared by both engines so they classify the same calls.
pub fn receiver_variable(ir: &al_syntax::ir::Ir, e: al_syntax::ir::ExprId) -> Option<&str> {
    use al_syntax::ir::ExprKind;
    match &ir.expr(e).kind {
        ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => Some(x.as_str()),
        ExprKind::Index { base, .. } => match &ir.expr(*base).kind {
            ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => Some(x.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Canonical record-op name (lowercase) → properly-cased RecordOpType.
pub fn record_op_type(method_lc: &str) -> Option<&'static str> {
    Some(match method_lc {
        "findset" => "FindSet",
        "findfirst" => "FindFirst",
        "findlast" => "FindLast",
        "find" => "Find",
        "get" => "Get",
        // ENG-12 (L35): reads the record by its SystemId, as `Get` does.
        "getbysystemid" => "GetBySystemId",
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
