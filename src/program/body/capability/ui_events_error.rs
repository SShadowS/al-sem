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
pub(crate) fn implicit_error_call(
    callee: &PCallee,
    in_statement_position: bool,
    receiver_type_of: &dyn Fn(&str) -> String,
) -> Option<&'static str> {
    match callee {
        PCallee::Bare { name }
            if in_statement_position && name.eq_ignore_ascii_case("evaluate") =>
        {
            Some("Evaluate")
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
    for cs in &ctx.features.call_sites {
        if implicit_error_call(&cs.callee, cs.in_statement_position, &type_of).is_some() {
            facts.push(implicit(None, Some(cs.id.clone())));
        }
    }

    (facts, vec![])
}
