//! Port of al-sem `src/index/capability/ui.ts`, `ui-window-open.ts`,
//! `events.ts` (SUBSCRIBE only), `error.ts`.
//!
//! - `extract_ui` — bare `Confirm`/`Message`/`Error` → ui-confirm/ui-message/
//!   ui-error (presence facts, static, no resourceArgSource).
//! - `extract_ui_window_open` — `StrMenu` (bare), `Page.Run`/`Report.Run`
//!   (object-run), `Page.RunModal`/`Report.RunModal` (static keyword OR a
//!   Page/Report-typed variable) → ui-window-open.
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
                if method_lc == "runmodal" {
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

    (facts, vec![])
}
