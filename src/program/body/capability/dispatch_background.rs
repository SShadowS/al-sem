//! Port of al-sem `src/index/capability/dispatch.ts` + `background.ts`.
//!
//! - `extract_dispatch` — object-run callees (`Codeunit.Run` / `Page.Run` /
//!   `Report.Run`, pre-classified by the L2 indexer) + member callees
//!   (`Page.RunModal` → modal, `Report.Execute`). One `execute` fact per match;
//!   the 1st positional arg is the target id `ValueSource`. `resourceId` is L3
//!   (absent at L2).
//! - `extract_background` — `TaskScheduler.CreateTask` (codeunit id = arg0),
//!   `Session.StartSession` / bare `StartSession` (codeunit id = arg1; arg0 is the
//!   OUT session-id var). One `start` fact per match.

use super::super::features::PCallee;
use super::super::features::PRoutine;
use super::value_source::classify_value_source;
use super::{
    CapabilityExtra, CapabilityFact, CoverageReason, DispatchExtra, ExtractionContext, ValueSource,
    confidence_from_source,
};

/// Map an object-run kind string to the dispatch resourceKind.
fn object_type_to_resource_kind(object_type: &str) -> &'static str {
    match object_type {
        "Codeunit" => "codeunit",
        "Page" => "page",
        "Report" => "report",
        _ => "codeunit",
    }
}

fn classify_target_arg(
    cs_args: &[super::super::features::PExpressionInfo],
    idx: usize,
    ctx: &ExtractionContext,
) -> ValueSource {
    classify_value_source(cs_args.get(idx), ctx)
}

pub fn extract_dispatch(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        match &cs.callee {
            PCallee::ObjectRun { object_kind, .. } => {
                let object_type = object_kind.clone();
                let target = classify_target_arg(&cs.argument_infos, 0, ctx);
                let confidence = confidence_from_source(&target).to_string();
                let resource_kind = object_type_to_resource_kind(&object_type).to_string();
                facts.push(CapabilityFact {
                    op: "execute".to_string(),
                    resource_kind,
                    confidence,
                    provenance: "direct".to_string(),
                    via: "self".to_string(),
                    resource_arg_source: Some(target),
                    witness_operation_id: None,
                    witness_callsite_id: Some(cs.id.clone()),
                    extra: Some(CapabilityExtra::Dispatch(DispatchExtra {
                        kind: "dispatch",
                        object_type,
                        modal: None,
                    })),
                });
            }
            PCallee::Member { receiver, method } => {
                // Page.RunModal (modal) / Report.Execute — not pre-classified.
                let key = format!("{}|{}", receiver.to_lowercase(), method.to_lowercase());
                let (object_type, modal): (&str, Option<bool>) = match key.as_str() {
                    "page|runmodal" => ("Page", Some(true)),
                    "report|execute" => ("Report", None),
                    _ => continue,
                };
                let target = classify_target_arg(&cs.argument_infos, 0, ctx);
                let confidence = confidence_from_source(&target).to_string();
                let resource_kind = object_type_to_resource_kind(object_type).to_string();
                facts.push(CapabilityFact {
                    op: "execute".to_string(),
                    resource_kind,
                    confidence,
                    provenance: "direct".to_string(),
                    via: "self".to_string(),
                    resource_arg_source: Some(target),
                    witness_operation_id: None,
                    witness_callsite_id: Some(cs.id.clone()),
                    extra: Some(CapabilityExtra::Dispatch(DispatchExtra {
                        kind: "dispatch",
                        object_type: object_type.to_string(),
                        modal,
                    })),
                });
            }
            _ => {}
        }
    }

    (facts, vec![])
}

pub fn extract_background(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let codeunit_arg_idx: Option<usize> = match &cs.callee {
            PCallee::Member { receiver, method } => {
                let r = receiver.to_lowercase();
                let m = method.to_lowercase();
                if r == "taskscheduler" && m == "createtask" {
                    Some(0)
                } else if r == "session" && m == "startsession" {
                    Some(1)
                } else {
                    None
                }
            }
            PCallee::Bare { name } => {
                if name.to_lowercase() == "startsession" {
                    Some(1)
                } else {
                    None
                }
            }
            _ => None,
        };

        let Some(idx) = codeunit_arg_idx else {
            continue;
        };

        let codeunit_source = classify_value_source(cs.argument_infos.get(idx), ctx);
        let confidence = confidence_from_source(&codeunit_source).to_string();

        facts.push(CapabilityFact {
            op: "start".to_string(),
            resource_kind: "background".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(codeunit_source),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: None,
        });
    }

    (facts, vec![])
}
