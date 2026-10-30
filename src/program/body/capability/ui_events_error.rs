//! Port of al-sem `src/index/capability/ui.ts`, `ui-window-open.ts`,
//! `events.ts` (SUBSCRIBE only), `error.ts`.
//!
//! - `extract_ui` — bare `Confirm`/`Message`/`Error` → ui-confirm/ui-message/
//!   ui-error (presence facts, static, no resourceArgSource).
//! - `extract_ui_window_open` — `StrMenu` (bare), `Page.Run`/`Report.Run`
//!   (object-run), `Run`/`RunModal` on the static keyword OR a Page/Report-typed
//!   variable → ui-window-open.
//! - `extract_events` — a routine with an `[EventSubscriber(...)]` attribute →
//!   one `subscribe` fact (EventExtra eventClass=Integration). Publisher-decorated
//!   routines emit NOTHING (publish is L4-injected).
//! - `extract_error` — `operationSites` kind "error-call" → error-throw.

use super::super::features::PCallee;
use super::super::features::PRoutine;
use super::super::features::{FixedLocal, PCallSite, PExpressionInfo};
use super::{CapabilityExtra, CapabilityFact, CoverageReason, EventExtra, ExtractionContext};

// ─── UI primitives ───────────────────────────────────────────────────────────

fn ui_primitive_op(name_lc: &str) -> Option<&'static str> {
    match name_lc {
        "confirm" => Some("ui-confirm"),
        "message" => Some("ui-message"),
        "error" => Some("ui-error"),
        _ => None,
    }
}

pub fn extract_ui(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let PCallee::Bare { name } = &cs.callee else {
            continue;
        };
        let Some(op) = ui_primitive_op(&name.to_lowercase()) else {
            continue;
        };

        facts.push(CapabilityFact {
            op: op.to_string(),
            resource_kind: "ui".to_string(),
            confidence: "static".to_string(),
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: None,
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: None,
        });
    }

    (facts, vec![])
}

// ─── UI window-open ──────────────────────────────────────────────────────────

fn is_page_or_report_type(declared_type: &str) -> bool {
    let lower = declared_type.to_lowercase();
    if lower == "page" || lower == "report" {
        return true;
    }
    lower.starts_with("page ") || lower.starts_with("report ")
}

pub fn extract_ui_window_open(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let matched = match &cs.callee {
            PCallee::Bare { name } => name.to_lowercase() == "strmenu",
            PCallee::ObjectRun { object_kind, .. } => {
                object_kind == "Page" || object_kind == "Report"
            }
            PCallee::Member { receiver, method } => {
                let method_lc = method.to_lowercase();
                let receiver_lc = receiver.to_lowercase();
                // `Run` too (G15): `MyPage.Run()` opens the page as `RunModal`
                // does, only without waiting; it was missed.
                if method_lc == "runmodal" || method_lc == "run" {
                    if receiver_lc == "page" || receiver_lc == "report" {
                        true
                    } else {
                        let declared_type = ctx.receiver_type_of(&receiver_lc);
                        declared_type != "unknown" && is_page_or_report_type(&declared_type)
                    }
                } else {
                    false
                }
            }
            _ => false,
        };

        if matched {
            facts.push(CapabilityFact {
                op: "ui-window-open".to_string(),
                resource_kind: "ui".to_string(),
                confidence: "static".to_string(),
                provenance: "direct".to_string(),
                via: "self".to_string(),
                resource_arg_source: None,
                witness_operation_id: None,
                witness_callsite_id: Some(cs.id.clone()),
                extra: None,
            });
        }
    }

    (facts, vec![])
}

// ─── Client dialogs (G26) ────────────────────────────────────────────────────

/// The client dialog a CALL SITE opens (G26), as its capability op:
/// - `ui-file-dialog`: `UploadIntoStream`, `DownloadFromStream`, `Upload`,
///   `Download`, bare or on `File` (the keyword or a `File` variable): the
///   client's file picker or download prompt.
/// - `ui-progress`: `Open` / `Update` on a `Dialog` variable: a progress window.
///
/// Both are interactive UI that needs a client, which is what `prove …
/// reaches-ui` asks. Other `File` members (`Open`, `Read`, …) and anything
/// else: `None`.
pub(crate) fn ui_dialog_call(
    callee: &PCallee,
    receiver_type_of: &dyn Fn(&str) -> String,
) -> Option<&'static str> {
    let is_file_dialog = |name: &str| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "uploadintostream" | "downloadfromstream" | "upload" | "download"
        )
    };
    match callee {
        PCallee::Bare { name } if is_file_dialog(name) => Some("ui-file-dialog"),
        PCallee::Member { receiver, method } => {
            let r = receiver.to_ascii_lowercase();
            let declared = receiver_type_of(&r).to_ascii_lowercase();
            if is_file_dialog(method) && (r == "file" || declared == "file") {
                Some("ui-file-dialog")
            } else if (method.eq_ignore_ascii_case("open") || method.eq_ignore_ascii_case("update"))
                && declared == "dialog"
            {
                Some("ui-progress")
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn extract_ui_dialog(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let receiver_type_of = |r: &str| ctx.receiver_type_of(r);
    let facts = ctx
        .features
        .call_sites
        .iter()
        .filter_map(|cs| {
            let op = ui_dialog_call(&cs.callee, &receiver_type_of)?;
            Some(CapabilityFact {
                op: op.to_string(),
                resource_kind: "ui".to_string(),
                confidence: "static".to_string(),
                provenance: "direct".to_string(),
                via: "self".to_string(),
                resource_arg_source: None,
                witness_operation_id: None,
                witness_callsite_id: Some(cs.id.clone()),
                extra: None,
            })
        })
        .collect();
    (facts, vec![])
}

// ─── Events (SUBSCRIBE only) ─────────────────────────────────────────────────

pub fn extract_events(
    _ctx: &ExtractionContext,
    routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    // findAttribute(attributesParsed, "EventSubscriber") — case-insensitive name.
    let has_subscriber = routine.attributes_parsed.iter().any(|a| {
        a.get("name")
            .and_then(|n| n.as_str())
            .map(|n| n.eq_ignore_ascii_case("EventSubscriber"))
            .unwrap_or(false)
    });

    if !has_subscriber {
        return (vec![], vec![]);
    }

    let fact = CapabilityFact {
        op: "subscribe".to_string(),
        resource_kind: "event".to_string(),
        confidence: "static".to_string(),
        provenance: "direct".to_string(),
        via: "self".to_string(),
        resource_arg_source: None,
        witness_operation_id: None,
        witness_callsite_id: None,
        extra: Some(CapabilityExtra::Event(EventExtra {
            kind: "event",
            event_class: "Integration".to_string(),
            include_sender: None,
        })),
    };

    (vec![fact], vec![])
}

// ─── Error ───────────────────────────────────────────────────────────────────

/// The implicit runtime error a RECORD OPERATION may raise (G20), named for the
/// fact: `Get` / `FindFirst` / `FindLast` / `FindSet` when the op is a statement
/// (its Boolean discarded, so a missing record raises an error), `TestField`
/// always. Record operations are their own list (`PRecordOperation`), not call
/// sites. With [`implicit_error_call`], the ONE rule: the body extractor and
/// the L4 cone both call them.
pub(crate) fn implicit_error_record_op(
    op: &str,
    in_statement_position: bool,
) -> Option<&'static str> {
    match op {
        "Get" if in_statement_position => Some("Get"),
        "FindFirst" if in_statement_position => Some("FindFirst"),
        "FindLast" if in_statement_position => Some("FindLast"),
        "FindSet" if in_statement_position => Some("FindSet"),
        "TestField" => Some("TestField"),
        _ => None,
    }
}

/// The implicit runtime error a CALL SITE may raise (G20): `FieldError`, which
/// is no record operation, on a record (a variable declared `Record …`, by the
/// extractor's own `receiver_type_of`, or the implicit `Rec` / `xRec`) or bare
/// in table or page code. G50: a call whose optional Boolean "ok" result is
/// discarded raises an error when it fails, as `Get` does: `Evaluate(X, Text)`
/// as a statement (Continia DO's `CDO Functions.GetCDOVerAsInteger`), and
/// `ReadFrom` on a JSON or XML value. Anything else, an `HttpClient.Get`
/// included: `None`.
///
/// G52: a statement `Evaluate` that provably cannot fail is no error. See
/// [`evaluate_cannot_fail`]; `var_facts` gives, for a variable name, its
/// declared type and its constant text ([`VarFacts`]).
pub(crate) fn implicit_error_call(
    cs: &PCallSite,
    receiver_type_of: &dyn Fn(&str) -> String,
    var_facts: &VarFacts,
) -> Option<&'static str> {
    let in_statement_position = cs.in_statement_position;
    match &cs.callee {
        PCallee::Bare { name }
            if in_statement_position && name.eq_ignore_ascii_case("evaluate") =>
        {
            (!evaluate_cannot_fail(&cs.argument_infos, var_facts)).then_some("Evaluate")
        }
        PCallee::Member { receiver, method }
            if in_statement_position && method.eq_ignore_ascii_case("readfrom") && {
                let t = receiver_type_of(&receiver.to_ascii_lowercase()).to_ascii_lowercase();
                t.starts_with("json") || t.starts_with("xml")
            } =>
        {
            Some("ReadFrom")
        }
        PCallee::Member { receiver, method } if method.eq_ignore_ascii_case("fielderror") => {
            let r = receiver.to_ascii_lowercase();
            let is_record = r == "rec"
                || r == "xrec"
                || receiver_type_of(&r)
                    .to_ascii_lowercase()
                    .starts_with("record");
            is_record.then_some("FieldError")
        }
        PCallee::Bare { name } if name.eq_ignore_ascii_case("fielderror") => Some("FieldError"),
        _ => None,
    }
}

/// For a variable name (lowercased): its declared type and the constant text it
/// holds (a locked label's), known only for a FIXED local
/// ([`crate::program::body::features::PVariableSymbol::fixed_local`]): one
/// declaration, no parameter or global of that name. `None` for any other name.
/// The IR keeps both arms of an `#if`, so a global, or a local declared twice,
/// may have another type or text in the arm that is compiled; and a local
/// declared once inside an `#if` may give way to a global of its name in the
/// other arm.
pub(crate) type VarFacts<'a> = dyn Fn(&str) -> Option<(String, Option<String>)> + 'a;

/// [`VarFacts`] over a flat variable list of `(name, declared type, fixed local)`.
/// Every entry of the name must be fixed (there is then exactly one).
pub(crate) fn agreed_var_facts<'v>(
    vars: impl Iterator<Item = (&'v str, &'v str, Option<&'v FixedLocal>)>,
    name: &str,
) -> Option<(String, Option<String>)> {
    let mut found = None;
    for (n, ty, fixed) in vars {
        if !n.eq_ignore_ascii_case(name) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some((ty.to_string(), fixed?.constant.clone()));
    }
    found
}

/// G52: whether `Evaluate(Target, Source)` (two arguments, no format number)
/// can be proved never to fail. It can when `Source` is a constant whose text
/// no translation changes (a string literal, or a locked label) and that text
/// parses for `Target`'s declared type: a `Guid` in its braced form
/// (`{8-4-4-4-12}` hex digits), an `Integer` of plain decimal digits with an
/// optional leading `-` in the 32-bit range. Anything else, or any doubt (a
/// target that is no plain variable, an unlocked label, a computed text, another
/// type), is `false`: a wrongly exempted `Evaluate` would answer a false "no".
pub(crate) fn evaluate_cannot_fail(args: &[PExpressionInfo], var_facts: &VarFacts) -> bool {
    let [target, source] = args else {
        return false;
    };
    let var_name = |info: &PExpressionInfo| match info.kind.as_str() {
        "identifier" | "quoted_identifier" => info.value.as_deref().map(str::to_ascii_lowercase),
        _ => None,
    };
    let Some((target_type, _)) = var_name(target).and_then(|n| var_facts(&n)) else {
        return false;
    };
    let text = match source.kind.as_str() {
        "string_literal" => al_syntax::ir::al_string_literal_text(&source.text),
        _ => var_name(source)
            .and_then(|n| var_facts(&n))
            .and_then(|(_, constant)| constant),
    };
    let Some(text) = text else {
        return false;
    };
    match target_type.to_ascii_lowercase().as_str() {
        "guid" => is_braced_guid(&text),
        "integer" => is_plain_integer(&text),
        _ => false,
    }
}

/// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`, hex digits of either case.
fn is_braced_guid(s: &str) -> bool {
    let Some(inner) = s.strip_prefix('{').and_then(|r| r.strip_suffix('}')) else {
        return false;
    };
    let groups: Vec<&str> = inner.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Decimal digits, an optional leading `-`, within AL's Integer range
/// (±2,147,483,647: unlike `i32`, `-2147483648` is outside it).
fn is_plain_integer(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<i32>().is_ok_and(|n| n != i32::MIN)
}

pub fn extract_error(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for op in &ctx.features.operation_sites {
        if op.kind == "error-call" {
            facts.push(CapabilityFact {
                op: "error-throw".to_string(),
                resource_kind: "error".to_string(),
                confidence: "static".to_string(),
                provenance: "direct".to_string(),
                via: "self".to_string(),
                resource_arg_source: None,
                witness_operation_id: Some(op.id.clone()),
                witness_callsite_id: None,
                extra: None,
            });
        }
    }
    // Implicit runtime errors (G20): their own op, as in the L4 cone.
    let implicit = |witness_operation_id: Option<String>, witness_callsite_id: Option<String>| {
        CapabilityFact {
            op: "error-implicit".to_string(),
            resource_kind: "error".to_string(),
            confidence: "static".to_string(),
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: None,
            witness_operation_id,
            witness_callsite_id,
            extra: None,
        }
    };
    for op in &ctx.features.record_operations {
        if implicit_error_record_op(&op.op, op.in_statement_position).is_some() {
            facts.push(implicit(Some(op.id.clone()), None));
        }
    }
    let type_of = |name: &str| ctx.receiver_type_of(name);
    let var_facts = |name: &str| {
        agreed_var_facts(
            ctx.features.variables.iter().map(|v| {
                (
                    v.name.as_str(),
                    v.declared_type.as_str(),
                    v.fixed_local.as_ref(),
                )
            }),
            name,
        )
    };
    for cs in &ctx.features.call_sites {
        if implicit_error_call(cs, &type_of, &var_facts).is_some() {
            facts.push(implicit(None, Some(cs.id.clone())));
        }
    }

    (facts, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(kind: &str, text: &str, value: Option<&str>) -> PExpressionInfo {
        PExpressionInfo {
            kind: kind.to_string(),
            text: text.to_string(),
            value: value.map(str::to_string),
            qualifier: None,
            member: None,
        }
    }
    fn ident(name: &str) -> PExpressionInfo {
        info("identifier", name, Some(name))
    }

    #[test]
    fn evaluate_cannot_fail_only_on_a_constant_that_parses() {
        const G: &str = "{a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2}";
        let fixed = |c: Option<&str>| {
            Some(FixedLocal {
                constant: c.map(str::to_string),
            })
        };
        let vars: Vec<(&str, &str, Option<FixedLocal>)> = vec![
            ("id", "Guid", fixed(None)),
            ("n", "Integer", fixed(None)),
            ("d", "Date", fixed(None)),
            ("tok", "Label", fixed(Some(G))),
            ("txt", "Label", fixed(None)),
            ("seven", "Label", fixed(Some("7"))),
            // A global (or a local with a second `#if`-arm declaration):
            // nothing fixed, whatever text one declaration shows.
            ("gtok", "Label", None),
            ("gid", "Guid", None),
            // Two entries of one name: never answered.
            ("twice", "Label", fixed(Some(G))),
            ("twice", "Label", fixed(Some(G))),
        ];
        let facts =
            |n: &str| agreed_var_facts(vars.iter().map(|(n, t, f)| (*n, *t, f.as_ref())), n);
        let ok = |args: &[PExpressionInfo]| evaluate_cannot_fail(args, &facts);
        let lit = |s: &str| info("string_literal", s, None);

        assert!(ok(&[ident("Id"), ident("Tok")]));
        assert!(ok(&[
            ident("Id"),
            lit("'{A7C3E1B4-9D2F-4A8E-B5C6-D7E8F9A0B1C2}'")
        ]));
        assert!(ok(&[ident("N"), ident("Seven")]));
        assert!(ok(&[ident("N"), lit("'-2147483647'")]));
        assert!(ok(&[ident("N"), lit("'2147483647'")]));

        assert!(
            !ok(&[ident("N"), lit("'-2147483648'")]),
            "outside AL's Integer"
        );
        assert!(!ok(&[ident("Id"), ident("Txt")]), "unlocked label");
        assert!(!ok(&[ident("Id"), ident("Twice")]), "two declarations");
        assert!(!ok(&[ident("Id"), ident("GTok")]), "global label");
        assert!(!ok(&[ident("GId"), ident("Tok")]), "global target");
        assert!(!ok(&[ident("Id"), ident("Seven")]), "7 is no Guid");
        assert!(!ok(&[ident("N"), ident("Tok")]), "a Guid is no Integer");
        assert!(!ok(&[ident("N"), lit("'2147483648'")]), "out of range");
        assert!(!ok(&[ident("N"), lit("'+1'")]));
        assert!(!ok(&[ident("N"), lit("' 1'")]));
        assert!(!ok(&[ident("N"), lit("''")]));
        assert!(!ok(&[
            ident("Id"),
            lit("'a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1c2'")
        ]));
        assert!(!ok(&[
            ident("Id"),
            lit("'{a7c3e1b4-9d2f-4a8e-b5c6-d7e8f9a0b1cg}'")
        ]));
        assert!(!ok(&[ident("D"), lit("'2020-01-01'")]), "Date unsupported");
        assert!(!ok(&[ident("Unknown"), ident("Tok")]), "no such variable");
        assert!(!ok(&[ident("Id"), ident("Unknown")]));
        assert!(!ok(&[info("member_expression", "R.F", None), ident("Tok")]));
        assert!(!ok(&[
            ident("N"),
            ident("Seven"),
            info("integer", "9", Some("9"))
        ]));
    }
}
