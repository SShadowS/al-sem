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
/// array element `X[i]` (ENG-12, L36); `this.x` (the `this.` key, the member
/// unquoted) for an object global through `this` (`this.X`, `this."X"`,
/// `this.X[i]`; ENG-12 gate: CDO's dependencies use it), which the receiver
/// sets hold only for a record global no parameter, local or return value of
/// the routine shadows ([`this_key`]). Parentheses around the receiver are read
/// through (`(X)` is `X`, ENG-178). `None` for any other shape (a call result,
/// another member). Shared by both engines so they classify the same calls.
pub fn receiver_variable(
    ir: &al_syntax::ir::Ir,
    e: al_syntax::ir::ExprId,
) -> Option<std::borrow::Cow<'_, str>> {
    use al_syntax::ir::ExprKind;
    use std::borrow::Cow;
    let named = |e: al_syntax::ir::ExprId| match &ir.expr(e).kind {
        ExprKind::Identifier(x) | ExprKind::QuotedIdentifier(x) => Some(Cow::Borrowed(x.as_str())),
        ExprKind::Member { object, member, .. } if is_this(ir, *object) => {
            let m = member.trim();
            let m = m
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(m);
            Some(Cow::Owned(this_key(m)))
        }
        _ => None,
    };
    // ENG-178 (Fable r2 on ENG-173): `(X).Modify()` is `X.Modify()`: parentheses around the
    // receiver change nothing, so both engines read through them.
    let mut e = e;
    while let ExprKind::Parenthesized(inner) = &ir.expr(e).kind {
        e = *inner;
    }
    match &ir.expr(e).kind {
        ExprKind::Index { base, .. } => named(*base),
        _ => named(e),
    }
}

/// The receiver-set key of an object global `name` reached through `this`
/// (`this.<name>`, the name as written; callers fold it as they fold names).
pub fn this_key(name: &str) -> String {
    format!("this.{name}")
}

/// Whether `e` is the `this` keyword (the object instance).
fn is_this(ir: &al_syntax::ir::Ir, e: al_syntax::ir::ExprId) -> bool {
    matches!(&ir.expr(e).kind, al_syntax::ir::ExprKind::Identifier(t) if t.eq_ignore_ascii_case("this"))
}

/// Whether the record method (lowercase) reads, writes or locks its table.
pub fn touches_table(method_lc: &str) -> bool {
    matches!(
        method_lc,
        "find"
            | "findfirst"
            | "findlast"
            | "findset"
            | "get"
            | "getbysystemid"
            | "next"
            | "count"
            | "countapprox"
            | "isempty"
            | "calcfields"
            | "calcsums"
            | "modify"
            | "modifyall"
            | "insert"
            | "delete"
            | "deleteall"
            | "rename"
            | "locktable"
    )
}

/// A record-op receiver's text as both engines key it: an object global
/// through `this` without the `this.` (`this.G` and `G` are one record: a
/// `this.X` receiver is classified only when no local shadows `X`, ENG-12
/// review); an indexed receiver without whitespace outside quotes
/// (`Recs[ 1 ]` and `Recs[1]` are one element); parentheses around the whole
/// text dropped (`(Rec)` is `Rec`, as [`receiver_variable`] reads it, ENG-178);
/// any other text unchanged.
pub fn receiver_key_text(t: &str) -> String {
    let mut t = t.trim_start();
    while let Some(inner) = t
        .trim_end()
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        && encloses_all(inner)
    {
        t = inner.trim();
    }
    let t = match t.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("this.") => t[5..].trim_start(),
        _ => t,
    };
    if !t.contains('[') {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len());
    let mut quoted = false;
    for c in t.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether `s` (the text inside an outer `(`…`)`) never closes that outer
/// parenthesis early: `A) + (B` does, so `(A) + (B)` is not one parenthesised
/// whole. Parentheses inside quotes do not count.
fn encloses_all(s: &str) -> bool {
    let (mut depth, mut quoted) = (0i32, false);
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0
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

#[cfg(test)]
mod tests {
    use super::receiver_key_text;

    /// ENG-178: parentheses around the whole receiver go; any other parentheses stay.
    #[test]
    fn receiver_key_text_drops_only_enclosing_parentheses() {
        assert_eq!(receiver_key_text("(Rec)"), "Rec");
        assert_eq!(receiver_key_text("( (this.G) )"), "G");
        assert_eq!(receiver_key_text("(Recs[ 1 ])"), "Recs[1]");
        assert_eq!(receiver_key_text("(\"A)B\")"), "\"A)B\"");
        assert_eq!(receiver_key_text("(A) + (B)"), "(A) + (B)");
        assert_eq!(receiver_key_text("Rec"), "Rec");
    }
}
