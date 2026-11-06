//! Port of al-sem `src/index/capability/http.ts`, `telemetry.ts`,
//! `isolated-storage.ts`, `hyperlink.ts`, `file-blob.ts`.
//!
//! All five are IO-family extractors that scan `callSites`:
//!   - HTTP: `HttpClient` member methods (Send/Get/Post/Put/Delete/Patch) →
//!     `send`; url = 1st arg (or none for `.Send`), body captured in `HttpExtra`.
//!   - Telemetry: `Session.LogMessage` / bare `LogMessage` → `log`; eventId arg0.
//!   - IsolatedStorage: `IsolatedStorage.{Get,GetEncrypted,Contains,Set,
//!     SetEncrypted,Delete}` → store-read/write/delete; key arg0, value arg1,
//!     scope arg2 (write only).
//!   - Hyperlink: bare `Hyperlink(url)` → `open`, resourceKind=ui.
//!   - File: `File.{Create,WriteAllText,Copy}` on a `File` receiver →
//!     `write-blob`, resourceKind=file ([`is_file_write_call`]; an in-memory
//!     `Temp Blob` stream is not file I/O).

use super::super::features::PCallee;
use super::super::features::PRoutine;
use super::value_source::classify_value_source;
use super::{
    CapabilityExtra, CapabilityFact, CoverageReason, ExtractionContext, HttpExtra, StorageExtra,
    ValueSource, confidence_from_source,
};

// ─── HTTP ────────────────────────────────────────────────────────────────────

fn is_http_method(v: &str) -> bool {
    matches!(v, "Send" | "Get" | "Post" | "Put" | "Delete" | "Patch")
}

pub fn extract_http(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        if ctx.receiver_type_of(receiver) != "HttpClient" {
            continue;
        }
        if !is_http_method(method) {
            continue;
        }

        // .Send(Request, Response): no URL; arg0 is the body.
        // .Post/.Put/.Patch(Url, Request, Response): arg0 URL, arg1 body.
        let is_send = method == "Send";
        let url_source = if is_send {
            ValueSource::Unknown
        } else {
            classify_value_source(cs.argument_infos.first(), ctx)
        };
        let body_info_idx = if is_send { 0 } else { 1 };
        let body_arg_source = cs
            .argument_infos
            .get(body_info_idx)
            .map(|info| classify_value_source(Some(info), ctx));

        let confidence = confidence_from_source(&url_source).to_string();

        facts.push(CapabilityFact {
            op: "send".to_string(),
            resource_kind: "http".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(url_source),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: Some(CapabilityExtra::Http(HttpExtra {
                kind: "http",
                method: method.clone(),
                body_arg_source,
            })),
        });
    }

    (facts, vec![])
}

// ─── Telemetry ───────────────────────────────────────────────────────────────

pub fn extract_telemetry(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let matches = match &cs.callee {
            PCallee::Member { receiver, method } => {
                receiver.to_lowercase() == "session" && method.to_lowercase() == "logmessage"
            }
            PCallee::Bare { name } => name.to_lowercase() == "logmessage",
            _ => false,
        };
        if !matches {
            continue;
        }

        let event_id_source = classify_value_source(cs.argument_infos.first(), ctx);
        let confidence = confidence_from_source(&event_id_source).to_string();

        facts.push(CapabilityFact {
            op: "log".to_string(),
            resource_kind: "telemetry".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(event_id_source),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: None,
        });
    }

    (facts, vec![])
}

// ─── Isolated storage ────────────────────────────────────────────────────────

fn isolated_storage_op(method_lc: &str) -> Option<&'static str> {
    match method_lc {
        "get" | "getencrypted" | "contains" => Some("store-read"),
        "set" | "setencrypted" => Some("store-write"),
        "delete" => Some("store-delete"),
        _ => None,
    }
}

fn parse_data_scope(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("::company") {
        "Company"
    } else if lower.contains("::user") {
        "User"
    } else if lower.contains("::module") {
        "Module"
    } else {
        "unknown"
    }
}

pub fn extract_isolated_storage(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        if receiver.to_lowercase() != "isolatedstorage" {
            continue;
        }
        let Some(op) = isolated_storage_op(&method.to_lowercase()) else {
            continue;
        };

        let key_source = classify_value_source(cs.argument_infos.first(), ctx);

        let mut value_arg_source = None;
        let mut scope = None;
        if op == "store-write" {
            if let Some(value_info) = cs.argument_infos.get(1) {
                value_arg_source = Some(classify_value_source(Some(value_info), ctx));
            }
            if let Some(scope_info) = cs.argument_infos.get(2) {
                scope = Some(parse_data_scope(&scope_info.text).to_string());
            }
        }

        let confidence = confidence_from_source(&key_source).to_string();

        facts.push(CapabilityFact {
            op: op.to_string(),
            resource_kind: "isolated-storage".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(key_source.clone()),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: Some(CapabilityExtra::Storage(StorageExtra {
                kind: "storage",
                key_arg_source: Some(key_source),
                value_arg_source,
                scope,
            })),
        });
    }

    (facts, vec![])
}

// ─── Hyperlink ───────────────────────────────────────────────────────────────

pub fn extract_hyperlink(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let PCallee::Bare { name } = &cs.callee else {
            continue;
        };
        if name.to_lowercase() != "hyperlink" {
            continue;
        }

        let url_source = classify_value_source(cs.argument_infos.first(), ctx);
        let confidence = confidence_from_source(&url_source).to_string();

        facts.push(CapabilityFact {
            op: "open".to_string(),
            resource_kind: "ui".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(url_source),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: None,
        });
    }

    (facts, vec![])
}

// ─── File / TempBlob ─────────────────────────────────────────────────────────

/// The one rule for a FILE write fact (`write-blob`, resourceKind `file`): a
/// method of the AL `File` data type that writes the server's file system
/// (`Create`, `Copy`, `WriteAllText`). Shared by this extractor and the capability
/// cone's (`engine::l4::capability_cone`), so the two cannot drift.
///
/// Not file I/O, so no FILE fact:
/// - a `Temp Blob` codeunit (or the old `TempBlob` table), a BLOB field, or an
///   `InStream` / `OutStream`: in-memory streams. `TempBlob.CreateOutStream` was
///   a FILE write here (ported from al-sem's `file-blob.ts`), so `explain`, `digest`
///   and the ordering engine reported in-memory streams as file access (r2 fact
///   check n10, c25);
/// - `UploadIntoStream` / `DownloadFromStream` / `File.UploadIntoStream`: client
///   file dialogs, `UI_FILE_DIALOG` (G26);
/// - a `File Management` call: a call into Base Application, whose body (when
///   loaded) reaches the `File` methods above, and the cone follows it.
pub(crate) fn is_file_write_call(receiver_type: &str, method_lc: &str) -> bool {
    receiver_type == "File" && matches!(method_lc, "create" | "writealltext" | "copy")
}

pub fn extract_file_blob(
    ctx: &ExtractionContext,
    _routine: &PRoutine,
) -> (Vec<CapabilityFact>, Vec<CoverageReason>) {
    let mut facts = Vec::new();

    for cs in &ctx.features.call_sites {
        let PCallee::Member { receiver, method } = &cs.callee else {
            continue;
        };
        if !is_file_write_call(&ctx.receiver_type_of(receiver), &method.to_lowercase()) {
            continue;
        }

        let arg_source = classify_value_source(cs.argument_infos.first(), ctx);
        let confidence = confidence_from_source(&arg_source).to_string();

        facts.push(CapabilityFact {
            op: "write-blob".to_string(),
            resource_kind: "file".to_string(),
            confidence,
            provenance: "direct".to_string(),
            via: "self".to_string(),
            resource_arg_source: Some(arg_source),
            witness_operation_id: None,
            witness_callsite_id: Some(cs.id.clone()),
            extra: None,
        });
    }

    (facts, vec![])
}
