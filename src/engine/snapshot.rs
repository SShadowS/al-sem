//! R0 identity-subset extraction — the structural derivation behind `aldump`.
//!
//! Derives the object/routine *identity subset* (stable ids + signature
//! fingerprints + normalizedSignatureHash + canonicalSignatureText) the R0
//! differential harness diffs against the Rust-owned goldens. Sources everything
//! from the owned `al-syntax` IR (`al_syntax::parse`) — no tree-sitter walk; the
//! stable-id / signature algorithms are the shared `engine::ids` ones (the same the
//! L2/L3 pipeline uses), so R0 identity matches production identity.
//!
//! The identity subset carries fields the production v3 snapshot does not (routine
//! sub-kind + `canonicalSignatureText`), so `aldump` emits this shape directly (see
//! [`IdentitySnapshot`]). The object-kind label map omits PermissionSetExtension /
//! Profile / Entitlement and renders XmlPort as "XMLport" (historical al-sem
//! identity shape, now a Rust-owned baseline).

use crate::engine::ids::{
    ParamSpec, canonical_routine_signature, encode_object_id, normalized_signature_hash,
    object_signature_fingerprint, routine_signature_fingerprint, to_stable_object_id,
    to_stable_routine_id_from_parts,
};
use al_syntax::ir::{ObjectKind, RoutineDecl, RoutineKind};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The identity subset of a single AL object declaration, in the golden's shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectIdentity {
    #[serde(rename = "stableObjectId")]
    pub stable_object_id: String,
    pub name: String,
    /// The display object-type string (e.g. "Codeunit", "XMLport").
    pub kind: String,
    #[serde(rename = "signatureFingerprint")]
    pub signature_fingerprint: String,
}

/// The identity subset of a single routine, in the golden's shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutineIdentity {
    #[serde(rename = "stableRoutineId")]
    pub stable_routine_id: String,
    pub name: String,
    /// "procedure" | "trigger" | "event-publisher" | "event-subscriber".
    pub kind: String,
    #[serde(rename = "signatureFingerprint")]
    pub signature_fingerprint: String,
    #[serde(rename = "normalizedSignatureHash")]
    pub normalized_signature_hash: String,
    #[serde(rename = "canonicalSignatureText")]
    pub canonical_signature_text: String,
}

/// The complete identity-subset snapshot for a workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentitySnapshot {
    pub objects: Vec<ObjectIdentity>,
    pub routines: Vec<RoutineIdentity>,
}

/// Map an owned-IR object kind to its display object-type name. Mirrors al-sem's
/// `OBJECT_TYPE_MAP` EXACTLY — note `XmlPort` → "XMLport" and the deliberate
/// OMISSION of PermissionSetExtension / Profile / Entitlement (al-sem skips those
/// object types in the identity snapshot, so we must too).
fn object_type_label_ir(kind: ObjectKind) -> Option<&'static str> {
    use ObjectKind as K;
    Some(match kind {
        K::Codeunit => "Codeunit",
        K::Table => "Table",
        K::TableExtension => "TableExtension",
        K::Page => "Page",
        K::PageExtension => "PageExtension",
        K::Report => "Report",
        K::ReportExtension => "ReportExtension",
        K::Query => "Query",
        K::XmlPort => "XMLport",
        K::Enum => "Enum",
        K::EnumExtension => "EnumExtension",
        K::Interface => "Interface",
        K::ControlAddIn => "ControlAddIn",
        K::PermissionSet => "PermissionSet",
        K::PermissionSetExtension | K::Profile | K::Entitlement | K::Other => return None,
    })
}

/// Routine kind label, mirroring al-sem `classifyAndCollectAttributes` (kind only):
/// base is "trigger" / "procedure"; the FIRST event attribute (closest sibling to
/// the routine — i.e. last in source order) wins: `eventsubscriber` →
/// "event-subscriber", `integrationevent`|`businessevent` → "event-publisher".
/// `internalevent` (and all other attributes) leave the kind unchanged — matching
/// AL-SEM, NOT the LSP parser (which treats InternalEvent as a publisher).
fn classify_kind_ir(r: &RoutineDecl) -> String {
    let mut kind = match r.kind {
        RoutineKind::Trigger => "trigger",
        RoutineKind::Procedure => "procedure",
    };
    // `r.attributes` are lowercased, in source order; closest-to-routine = last.
    for attr in r.attributes.iter().rev() {
        match attr.as_str() {
            "eventsubscriber" => {
                kind = "event-subscriber";
                break;
            }
            "integrationevent" | "businessevent" => {
                kind = "event-publisher";
                break;
            }
            _ => {}
        }
    }
    kind.to_string()
}

/// Parameters in identity-signature form. Mirrors al-sem GAP 1: a parameter with
/// NO semantic name (a parse artifact) is skipped; a quoted-name parameter is KEPT
/// (its type enters the signature). The IR stores a nameless param with `name ==
/// ""`, so the GAP-1 skip is `!name.is_empty()`.
fn params_ir(r: &RoutineDecl) -> Vec<ParamSpec> {
    r.params
        .iter()
        .filter(|p| !p.name.is_empty())
        .map(|p| ParamSpec {
            type_text: p.ty.clone().unwrap_or_default(),
            is_var: p.by_ref,
        })
        .collect()
}

/// Strip surrounding double quotes — matches al-sem's `stripQuotes` exactly:
/// only strips when the text is >= 2 chars AND starts with `"` AND ends with
/// `"`. (A lone `"` or unbalanced quotes are returned verbatim.) This counts
/// length in `char`s, but the predicate is on ASCII `"` so byte vs char length
/// is immaterial for the >= 2 guard except for the degenerate single-char case,
/// which neither side strips.
fn strip_quotes(text: &str) -> &str {
    let mut chars = text.chars();
    let first = chars.next();
    let last = chars.next_back();
    // `chars.next()` then `chars.next_back()` having both yielded a char means
    // the string has >= 2 chars.
    if first == Some('"') && last == Some('"') {
        // Slice off the leading and trailing ASCII quote (1 byte each).
        &text[1..text.len() - 1]
    } else {
        text
    }
}

/// Build the identity subset for one parsed source file's owned IR.
fn extract_from_ir(file: &al_syntax::ir::AlFile, app_guid: &str, out: &mut IdentitySnapshot) {
    for obj in &file.objects {
        let Some(object_type) = object_type_label_ir(obj.kind) else {
            continue;
        };
        let object_number = obj.id.unwrap_or(0);
        // `strip_quotes` is idempotent on an already-unquoted IR name; applying it
        // keeps parity with al-sem's `extractObjectName` regardless.
        let name = strip_quotes(&obj.name).to_string();

        let internal_object_id = encode_object_id(app_guid, object_type, object_number);
        let stable_object_id = to_stable_object_id(&internal_object_id);
        let signature_fingerprint = object_signature_fingerprint(object_type, object_number, &name);

        out.objects.push(ObjectIdentity {
            stable_object_id: stable_object_id.clone(),
            name,
            kind: object_type.to_string(),
            signature_fingerprint,
        });

        // Interface / ControlAddIn: al-sem's R0 identity snapshot never modeled
        // these objects' signature-only members as routines (a frozen, differential-
        // gated historical baseline — `tests/differential.rs`'s R0 harness). See the
        // identical, more-detailed comment in `program::model::workspace.rs`'s routine
        // loop (the same shared `al_syntax::lower::collect_routines` fix — receiver-
        // closure plan, Task 1 — surfaces here too, since this module independently
        // re-derives its own identity snapshot from the raw IR rather than reusing
        // `ModelEntities`).
        if object_type == "Interface" || object_type == "ControlAddIn" {
            continue;
        }

        for r in &obj.routines {
            let rname = strip_quotes(&r.name).to_string();
            if rname.is_empty() {
                continue;
            }
            let kind = classify_kind_ir(r);
            let parameters = params_ir(r);
            let return_type_ref = r.return_type.as_deref();

            let canonical_signature_text =
                canonical_routine_signature(&rname, &parameters, return_type_ref);
            let norm_hash = normalized_signature_hash(&rname, &parameters, return_type_ref);
            let sig_fp = routine_signature_fingerprint(&rname, &parameters, return_type_ref);
            // This module re-derives its own identity snapshot from the raw IR, so it
            // must take the enclosing-member discriminator from the SAME single source
            // (`ir_enclosing_member`) the L2/L3 paths use — otherwise the two would mint
            // different stable ids for the same member trigger.
            let enclosing_member = crate::program::body::ir_walk::ir_enclosing_member(r);
            let stable_routine_id = to_stable_routine_id_from_parts(
                &stable_object_id,
                &norm_hash,
                enclosing_member.as_deref(),
            );

            out.routines.push(RoutineIdentity {
                stable_routine_id,
                name: rname,
                kind,
                signature_fingerprint: sig_fp,
                normalized_signature_hash: norm_hash,
                canonical_signature_text,
            });
        }
    }
}

/// A discovered AL source file: its normalized workspace-relative POSIX path
/// (used for deterministic sort) and absolute path on disk.
#[derive(Debug, Clone)]
struct AlFile {
    /// Lowercased, forward-slash, workspace-relative path — the sort key.
    sort_key: String,
    abs_path: std::path::PathBuf,
}

/// Recursively discover `*.al` files under `workspace`, excluding dependency
/// dirs (`.alpackages`, `.app` payloads). R0 is source-symbol parity, so only
/// the workspace's own AL source is in scope.
fn discover_al_files(workspace: &Path) -> std::io::Result<Vec<AlFile>> {
    let mut files = Vec::new();
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                let dname = entry.file_name();
                let dname_lc = dname.to_string_lossy().to_lowercase();
                // Skip dependency / package dirs.
                if dname_lc == ".alpackages" || dname_lc == ".git" {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                let is_al = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_lowercase() == "al")
                    .unwrap_or(false);
                if is_al {
                    let rel = path.strip_prefix(workspace).unwrap_or(&path);
                    let sort_key = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
                        .collect::<Vec<_>>()
                        .join("/");
                    files.push(AlFile {
                        sort_key,
                        abs_path: path,
                    });
                }
            }
        }
    }
    // Deterministic order — identity doesn't depend on file order, but
    // determinism is the contract.
    files.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
    Ok(files)
}

use crate::source_text::read_al_source;

/// Read the workspace ROOT's `app.json` and return its `id` field VERBATIM (no
/// case change) when present as a non-empty string. al-sem uses this as the
/// StableObjectId prefix (appGuid).
///
/// ORACLE BEHAVIOR (providers/workspace.ts:62-77): the appGuid is read ONLY from
/// `<root>/app.json`, and only when `id` is a non-empty string. Missing file,
/// unparseable JSON, or a missing/empty/non-string `id` → `None`. The fail-closed
/// guard in `snapshot_workspace` turns a `None` here into an EMPTY snapshot.
fn read_root_app_guid(workspace: &Path) -> Option<String> {
    let app_json_path = workspace.join("app.json");
    let text = std::fs::read_to_string(&app_json_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let id = value.get("id")?.as_str()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Count `app.json` files anywhere under `workspace`, EXCLUDING dirs named
/// `node_modules` and `.alpackages` (case-insensitive). Mirrors al-sem's
/// `WorkspaceProvider.collect` layout check: `walk` skips `SKIP_DIR_EXACT`
/// (`node_modules`, `.alpackages`) and counts every remaining file whose basename
/// lowercases to `app.json`. More than one ⇒ multi-app source tree (unsound).
fn count_app_json(workspace: &Path) -> usize {
    let mut count = 0usize;
    let mut stack = vec![workspace.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ftype) = entry.file_type() else {
                continue;
            };
            if ftype.is_dir() {
                let dname_lc = entry.file_name().to_string_lossy().to_lowercase();
                // Mirror al-sem SKIP_DIR_EXACT (node_modules / .alpackages).
                if dname_lc == "node_modules" || dname_lc == ".alpackages" {
                    continue;
                }
                stack.push(entry.path());
            } else if ftype.is_file()
                && entry.file_name().to_string_lossy().to_lowercase() == "app.json"
            {
                count += 1;
            }
        }
    }
    count
}

/// Build the identity-subset snapshot for a workspace directory.
///
/// Errors (non-throwing-contract note): this is a CLI helper, so a missing /
/// unreadable workspace or app.json surfaces as an `Err` for a clean non-zero
/// exit. The al-sem engine itself never throws; that contract lives in the
/// pipeline, not in this thin extraction CLI.
pub fn snapshot_workspace(workspace: &Path) -> anyhow::Result<IdentitySnapshot> {
    if !workspace.is_dir() {
        anyhow::bail!("workspace is not a directory: {}", workspace.display());
    }

    // --- fail-closed layout detection (identity soundness, al-sem GAP 2) -------
    // A sound workspace is exactly ONE AL app: a readable root app.json with a
    // non-empty string `id`, plus deps under skipped dirs. Anything else mints
    // colliding object identities (one appGuid stamped onto files belonging to
    // distinct apps), so al-sem's WorkspaceProvider.collect emits NO source units.
    // We mirror that: an unsound layout yields an EMPTY identity snapshot.
    //   (a) no readable <root>/app.json with a string `id`, OR
    //   (b) MORE THAN ONE app.json under <root> (excl. node_modules/.alpackages).
    let app_guid = match read_root_app_guid(workspace) {
        Some(g) => g,
        None => {
            eprintln!(
                "fail-closed: no readable root app.json with a string `id` at {} — emitting empty snapshot",
                workspace.display()
            );
            return Ok(IdentitySnapshot {
                objects: Vec::new(),
                routines: Vec::new(),
            });
        }
    };
    let app_json_count = count_app_json(workspace);
    if app_json_count > 1 {
        eprintln!(
            "fail-closed: multi-app source workspace at {} ({app_json_count} app.json files, excl. node_modules/.alpackages) — emitting empty snapshot",
            workspace.display()
        );
        return Ok(IdentitySnapshot {
            objects: Vec::new(),
            routines: Vec::new(),
        });
    }

    let files = discover_al_files(workspace)
        .map_err(|e| anyhow::anyhow!("failed to discover .al files: {e}"))?;

    let mut snapshot = IdentitySnapshot {
        objects: Vec::new(),
        routines: Vec::new(),
    };

    // Sequential parse loop, run on a big-stack thread (T2.1): this CLI path
    // (`aldump`/`alsem`) runs on the process main thread, which has no
    // guaranteed-generous stack — see `big_stack`'s doc. One big-stack thread
    // for the WHOLE loop, not per-file (thousands of files in a real
    // workspace; per-file spawn/join overhead would be wasteful).
    crate::big_stack::run_with_big_stack(|| {
        for file in &files {
            let source = match read_al_source(&file.abs_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("warning: skipping {} (read error: {e})", file.sort_key);
                    continue;
                }
            };
            extract_from_ir(&al_syntax::parse(&source), &app_guid, &mut snapshot);
        }
    });

    // `objects` sorted by stableObjectId; `routines` by stableRoutineId. Plain
    // lexicographic (byte) sort — for these ASCII strings it matches the JS
    // default sort used to produce the goldens.
    snapshot
        .objects
        .sort_by(|a, b| a.stable_object_id.cmp(&b.stable_object_id));
    snapshot
        .routines
        .sort_by(|a, b| a.stable_routine_id.cmp(&b.stable_routine_id));

    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_quotes_matches_al_sem_semantics() {
        assert_eq!(strip_quotes("\"D2 Publisher\""), "D2 Publisher");
        assert_eq!(strip_quotes("Customer"), "Customer");
        assert_eq!(strip_quotes("\""), "\""); // single char — not stripped
        assert_eq!(strip_quotes(""), "");
        assert_eq!(strip_quotes("\"unbalanced"), "\"unbalanced");
        assert_eq!(strip_quotes("unbalanced\""), "unbalanced\"");
    }

    /// Root app.json identity: read the `id` only when it is a non-empty string;
    /// any other state (missing file, unparseable, missing/empty/non-string `id`)
    /// yields `None`, which the fail-closed guard turns into an empty snapshot.
    /// Mirrors providers/workspace.ts GAP 2.
    #[test]
    fn read_root_app_guid_requires_nonempty_string_id() {
        let tmp = std::env::temp_dir().join(format!("alch-r0-noappjson-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        // No root app.json → None.
        assert_eq!(read_root_app_guid(&tmp), None);
        // Unparseable app.json → None.
        std::fs::write(tmp.join("app.json"), "{ not json").unwrap();
        assert_eq!(read_root_app_guid(&tmp), None);
        // Valid JSON but no string `id` → None.
        std::fs::write(tmp.join("app.json"), "{\"name\":\"x\"}").unwrap();
        assert_eq!(read_root_app_guid(&tmp), None);
        // Empty `id` string → None.
        std::fs::write(tmp.join("app.json"), "{\"id\":\"\"}").unwrap();
        assert_eq!(read_root_app_guid(&tmp), None);
        // Valid non-empty `id` → verbatim.
        std::fs::write(tmp.join("app.json"), "{\"id\":\"ABC-123\"}").unwrap();
        assert_eq!(read_root_app_guid(&tmp), Some("ABC-123".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Layout count: `count_app_json` counts app.json anywhere under the root but
    /// skips `node_modules` and `.alpackages` (mirrors al-sem SKIP_DIR_EXACT). A
    /// two-app tree (`a/app.json` + `b/app.json`) counts 2 ⇒ multi-app (unsound).
    #[test]
    fn count_app_json_skips_node_modules_and_alpackages() {
        let tmp = std::env::temp_dir().join(format!("alch-r0-countapp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("a")).unwrap();
        std::fs::create_dir_all(tmp.join("b")).unwrap();
        std::fs::create_dir_all(tmp.join(".alpackages")).unwrap();
        std::fs::create_dir_all(tmp.join("node_modules").join("pkg")).unwrap();
        std::fs::write(tmp.join("a").join("app.json"), "{\"id\":\"a\"}").unwrap();
        std::fs::write(tmp.join("b").join("app.json"), "{\"id\":\"b\"}").unwrap();
        // These two must NOT be counted (under skipped dirs).
        std::fs::write(tmp.join(".alpackages").join("app.json"), "{}").unwrap();
        std::fs::write(tmp.join("node_modules").join("pkg").join("app.json"), "{}").unwrap();
        assert_eq!(count_app_json(&tmp), 2);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
