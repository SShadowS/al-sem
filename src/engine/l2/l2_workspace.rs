//! Workspace-level L2 feature emitter (R1a Task 3).
//!
//! Drives the owned AL syntax IR (`al_syntax::parse` → [`ir_walk`]) over an entire
//! AL workspace and produces the ALLOWLISTED L2 projection
//! ([`features::L2Projection`]) — objects + routines with metadata + per-routine
//! `features`. This is the producer behind `aldump --l2`; it mirrors the golden
//! shape of `scripts/r1a-goldens/<fixture>.l2.golden.json` EXACTLY.
//!
//! The emitter no longer walks the tree-sitter CST: objects, routines, metadata,
//! parameters and per-routine `features` all come from the owned IR (the legacy
//! `project_routine_features` body-walker survives only as the dual_run validation
//! oracle, not on this production path). See `tests/ir_robustness.rs` for the
//! edge-case anti-regression suite that backs the cutover.
//!
//! Discovery + fail-closed layout detection + BOM strip + `.al` sort reproduce
//! R0's `snapshot_workspace` (`engine::snapshot`): a sound workspace is exactly
//! ONE AL app (a readable root `app.json` with a non-empty string `id`, deps
//! under skipped dirs). An unsound layout yields an EMPTY projection.
//!
//! Metadata derivation mirrors al-sem EXACTLY:
//!   - routine `attributes` / `attributesParsed` / `accessModifier` /
//!     `bodyAvailable` / `parseIncomplete`: `src/index/routine-indexer.ts`
//!     (`classifyAndCollectAttributes`, `classifyAccessModifier`) +
//!     `src/index/attribute-from-node.ts` (`attributeInfoFromNode`).
//!   - object `objectSubtype` / `pageType` / `sourceTableName` /
//!     `inherentCommitBehavior`: `src/index/object-indexer.ts` (`indexObjects`,
//!     `readObjectProperty`).
//!
//! R1b/R1c: `controlContext` + `order` + `scopeFrames` ARE now emitted (absent
//! when the CFN walker assigned none; scopeFrames present-with-root when a body
//! tree exists, omitted for TryFunction / no body). FORBIDDEN fields (capability /
//! resourceId / tableId / calleeParameterIsVar / bindingResolution /
//! sourceTableId) remain STRUCTURALLY ABSENT from the serde projection types
//! (`features.rs`), so they can never appear in this output.
//!
//! Output discipline: ONLY JSON goes to stdout (the binary prints it); all
//! logs/warnings go to stderr. The projection carries no absolute paths.

use super::features::{L2Projection, PFeatures, PObject, PRoutine};
use crate::engine::ids::{
    ParamSpec, encode_object_id, normalized_signature_hash, to_stable_object_id,
    to_stable_routine_id_from_parts,
};
use crate::engine::l2::scope;
use std::path::Path;

/// The intentional stable corpus/model-instance label (matches the golden's id
/// prefixes `r0/…`). It does not enter the R1a stable comparison subset.
const MODEL_INSTANCE_ID: &str = "r0";

/// A discovered workspace `.al` file (the shared walk's type).
pub(crate) use crate::source_text::AlFile;
use crate::source_text::NestedApps;

/// Every workspace `.al` file, nested apps included: the program engine's
/// file set (`crate::source_text::discover_al_files`).
pub(crate) fn discover_al_files(workspace: &Path) -> std::io::Result<Vec<AlFile>> {
    crate::source_text::discover_al_files(workspace, NestedApps::Walk)
}

/// Like [`discover_al_files`] but scoped to ONE app: a child directory that carries
/// its own `app.json` is a separate AL project (the AL compiler treats each
/// `app.json` as a project root), so discovery does NOT descend into it. The
/// `workspace` root's own `app.json` does not stop the walk (it IS this app). This
/// lets a root app whose tree contains nested sub-apps (a monorepo / `Modules/`
/// layout) be analyzed in isolation, and lets each nested app be analyzed by
/// pointing at its own root. Otherwise the same walk as [`discover_al_files`].
pub(crate) fn discover_al_files_app_scoped(workspace: &Path) -> std::io::Result<Vec<AlFile>> {
    crate::source_text::discover_al_files(workspace, NestedApps::Skip)
}

/// The shared `.al` decoder: L2/L3 must see the same text as the program engine.
pub(crate) use crate::source_text::read_al_source;

/// Read the workspace ROOT's `app.json` `id` field VERBATIM when it is a
/// non-empty string. Mirrors `providers/workspace.ts` (GAP 2).
pub(crate) fn read_root_app_guid(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(workspace.join("app.json")).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let id = value.get("id")?.as_str()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Count `app.json` files anywhere under `workspace`, excluding the shared
/// skip folders (`crate::source_text::SKIP_DIRS`, any case).
pub(crate) fn count_app_json(workspace: &Path) -> usize {
    count_app_json_paths(workspace).len()
}

/// Collect the absolute paths of every `app.json` anywhere under `workspace`,
/// excluding the shared skip folders (`crate::source_text::SKIP_DIRS`, any
/// case). Used by the gate's `workspace_diagnostics` to reproduce the
/// provider's multi-app fail-closed message (which sorts these paths).
pub(crate) fn count_app_json_paths(workspace: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
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
                if crate::source_text::is_skipped_dir_name(&entry.file_name()) {
                    continue;
                }
                stack.push(entry.path());
            } else if ftype.is_file()
                && entry.file_name().to_string_lossy().to_lowercase() == "app.json"
            {
                paths.push(entry.path());
            }
        }
    }
    paths
}

/// Build one fully-populated [`PRoutine`] from an IR routine (features →
/// control-context → operation-order → capabilities), the single shared per-routine
/// path for BOTH [`project_file`] and [`project_named_routine`] so they cannot drift.
/// Returns `None` for a nameless routine.
#[allow(clippy::too_many_arguments)]
fn build_proutine(
    ir_file: &al_syntax::ir::AlFile,
    oi: usize,
    ir_routine: &al_syntax::ir::RoutineDecl,
    object_type: &str,
    object_number: i64,
    stable_object_id: &str,
    source_table_name: Option<&str>,
    app_guid: &str,
    source: &str,
    source_unit_id: &str,
) -> Option<PRoutine> {
    use crate::engine::l2::ir_walk;

    let rname = ir_routine.name.clone();
    if rname.is_empty() {
        return None;
    }

    let body_available = ir_routine.body.is_some();
    let parse_incomplete = ir_routine.parse_incomplete;
    let kind = ir_walk::ir_routine_kind(ir_routine);
    let (attributes, attributes_parsed) = ir_walk::ir_attributes(ir_routine, ir_file, source);
    let access_modifier = ir_routine.access_modifier.clone();

    // Stable routine id — its normalizedSignatureHash is the canonical
    // (return-type-aware) signature hash, identical on the ABI side.
    let parameters = ir_walk::ir_parameter_symbols(ir_routine);
    let param_specs: Vec<ParamSpec> = parameters
        .iter()
        .map(|p| ParamSpec {
            type_text: p.type_text.clone(),
            is_var: p.is_var,
        })
        .collect();
    let return_type_text = ir_routine.return_type.clone();
    let norm_hash = normalized_signature_hash(&rname, &param_specs, return_type_text.as_deref());

    // The member-trigger discriminator: without it the N `trigger OnAction()`
    // bodies of one page all hash to ONE internal id (see
    // `ids::encode_canonical_routine_key`) AND to one stable id (see
    // `ids::to_stable_routine_id_from_parts`). `None` for procedures /
    // object-level triggers, which keep byte-identical ids. Computed ONCE, from
    // `ir_enclosing_member` — the single normalization — and fed to BOTH ids.
    let enclosing_member = ir_walk::ir_enclosing_member(ir_routine);
    let stable_routine_id =
        to_stable_routine_id_from_parts(stable_object_id, &norm_hash, enclosing_member.as_deref());
    let routine_id = scope::compute_routine_id(
        app_guid,
        object_type,
        object_number,
        kind,
        &rname,
        enclosing_member.as_deref(),
        &parameters,
        return_type_text.as_deref(),
        MODEL_INSTANCE_ID,
    );
    let mut features: PFeatures = ir_walk::project_routine_features_ir(
        ir_file,
        oi,
        ir_routine,
        &routine_id,
        source,
        source_unit_id,
        source_table_name,
    );

    // R1b: control-context lattice over the CFN skeleton (+ metadata). Populates
    // `controlContext` on each op/callsite (absent when none), including the
    // error-call source-range post-pass. `attributesParsed` names drive the
    // TryFunction guard; `parameters` the by-var Boolean IsHandled eligibility.
    let attr_names_lc: Vec<String> = attributes_parsed
        .iter()
        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
        .map(|n| n.to_lowercase())
        .collect();
    crate::engine::l2::control_context::apply_control_contexts(
        &mut features,
        &attr_names_lc,
        &parameters,
    );

    // R1c: operation-order index over the CFN skeleton (+ TryFunction guard).
    // Populates `order` on each op/callsite (absent when the walk produced none) —
    // including the error-call source-range post-pass over the op/callsite records —
    // and the routine's `scopeFrames`.
    crate::engine::l2::operation_order::apply_operation_order(&mut features, &attr_names_lc);

    let mut routine = PRoutine {
        stable_routine_id,
        name: rname,
        kind: kind.to_string(),
        attributes,
        attributes_parsed,
        access_modifier,
        body_available,
        parse_incomplete,
        features,
        capability_facts_direct: Vec::new(),
        capability_status: crate::engine::l2::capability::CoverageStatus::Complete,
        capability_reasons: Vec::new(),
        capability_diagnostics: Vec::new(),
    };

    // R1d: direct capability facts. MUST run AFTER controlContext is set (the
    // unreachable filter in `extract_capabilities` reads it).
    apply_capabilities(&mut routine);

    Some(routine)
}

/// Project the L2 `PFeatures` (+ parameters + lowercased attribute names) for a
/// single NAMED routine in a one-file source, driven by the owned IR. Shared by the
/// control-context / operation-order single-routine analyzers (and their vector
/// tests). Mirrors [`build_proutine`]'s per-routine feature setup; returns `None`
/// when the routine isn't found.
pub fn ir_features_for_named_routine(
    source: &str,
    routine_name: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
) -> Option<(PFeatures, Vec<scope::ParameterSymbol>, Vec<String>)> {
    use crate::engine::l2::ir_walk;
    let ir_file = al_syntax::parse(source);
    for (oi, o) in ir_file.objects.iter().enumerate() {
        let Some(object_type) = ir_walk::ir_object_type(&o.kind) else {
            continue;
        };
        let object_number = o.id.unwrap_or(0);
        let (_subtype, _page_type, source_table_name, _icb) =
            ir_walk::ir_object_metadata(o, object_type);
        for r in &o.routines {
            if r.name != routine_name {
                continue;
            }
            let kind = ir_walk::ir_routine_kind(r);
            let parameters = ir_walk::ir_parameter_symbols(r);
            let enclosing_member = ir_walk::ir_enclosing_member(r);
            let routine_id = scope::compute_routine_id(
                app_guid,
                object_type,
                object_number,
                kind,
                &r.name,
                enclosing_member.as_deref(),
                &parameters,
                r.return_type.as_deref(),
                model_instance_id,
            );
            let features = ir_walk::project_routine_features_ir(
                &ir_file,
                oi,
                r,
                &routine_id,
                source,
                source_unit_id,
                source_table_name.as_deref(),
            );
            // `RoutineDecl.attributes` is ALREADY lowercased by the lowerer, built from
            // the same `attributes_parsed` items — so this equals `build_proutine`'s
            // `attributes_parsed → name → to_lowercase()` set (drives the TryFunction guard).
            let attr_names_lc = r.attributes.clone();
            return Some((features, parameters, attr_names_lc));
        }
    }
    None
}

/// Build the full L2 projection for one source file, driven entirely by the owned
/// AL syntax IR (`al_syntax::parse`). The engine no longer walks the tree-sitter CST
/// here: objects, routines, metadata, parameters and per-routine `features` all come
/// from the IR. Preconditions proven over the r0-corpus before cutover — object set
/// 404/404, routine set 591/591, (type,number,name) 404/404, parse_incomplete 591/591
/// — and feature output is byte-identical to the legacy body_walk on every well-formed
/// routine. Malformed routines (`parse_incomplete`) take the IR's recovery too (it
/// cleanly drops stray ERROR tokens rather than emitting phantom `other` nodes).
fn project_file(
    source: &str,
    app_guid: &str,
    source_unit_id: &str,
    objects: &mut Vec<PObject>,
    routines: &mut Vec<PRoutine>,
) {
    let ir_file = al_syntax::parse(source);

    for (oi, o) in ir_file.objects.iter().enumerate() {
        let Some(object_type) = crate::engine::l2::ir_walk::ir_object_type(&o.kind) else {
            continue;
        };
        let object_number = o.id.unwrap_or(0);
        let name = o.name.clone();

        let internal_object_id = encode_object_id(app_guid, object_type, object_number);
        let stable_object_id = to_stable_object_id(&internal_object_id);

        let (object_subtype, page_type, source_table_name, inherent_commit_behavior) =
            crate::engine::l2::ir_walk::ir_object_metadata(o, object_type);

        objects.push(PObject {
            stable_object_id: stable_object_id.clone(),
            name,
            object_type: object_type.to_string(),
            object_subtype,
            page_type,
            source_table_name: source_table_name.clone(),
            inherent_commit_behavior,
        });

        // Interface / ControlAddIn: al-sem's L2 feature projection never modeled
        // these objects' signature-only members as routines (frozen, differential-
        // gated — `tests/differential.rs`'s R1a harness). See the matching, more
        // detailed comment in `engine::l3::l3_workspace.rs` and `engine::snapshot.rs`
        // (the same shared `al_syntax::lower::collect_routines` fix, receiver-closure
        // plan Task 1, surfaces in every independent IR-consumer that iterates
        // `obj.routines` unconditionally).
        if object_type == "Interface" || object_type == "ControlAddIn" {
            continue;
        }

        for ir_routine in &o.routines {
            if let Some(routine) = build_proutine(
                &ir_file,
                oi,
                ir_routine,
                object_type,
                object_number,
                &stable_object_id,
                source_table_name.as_deref(),
                app_guid,
                source,
                source_unit_id,
            ) {
                routines.push(routine);
            }
        }
    }
}

/// R1d emitter wiring: run `extract_capabilities` on the (control-context-set)
/// routine and populate the four sibling-of-`features` capability fields, ordered
/// to match the al-sem golden projection (`r1a-l2-projection.ts`):
///   - `capabilityFactsDirect`: extraction order (positional) — NO sort.
///   - `capabilityReasons`: dedupe + LEXICOGRAPHIC sort on the kebab string
///     (al-sem `Array.from(new Set(reasons)).sort()` — JS string sort, NOT the
///     `CoverageReason` declaration order).
///   - `capabilityDiagnostics`: sort by `(sourceRef, message)`.
fn apply_capabilities(routine: &mut PRoutine) {
    let result = crate::engine::l2::capability::extract_capabilities(routine);

    let mut reasons = result.reasons;
    // Match al-sem's `.sort()` (lexicographic on the serialized kebab string),
    // not the enum's declaration-order `Ord`.
    reasons.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    let mut diagnostics = result.diagnostics;
    diagnostics.sort_by(|a, b| {
        a.source_ref
            .cmp(&b.source_ref)
            .then_with(|| a.message.cmp(&b.message))
    });

    routine.capability_facts_direct = result.facts;
    routine.capability_status = result.status;
    routine.capability_reasons = reasons;
    routine.capability_diagnostics = diagnostics;
}

/// Build the full L2 projection for a workspace directory.
///
/// Errors: a missing / unreadable workspace surfaces as `Err` for a clean
/// non-zero exit (thin CLI helper — the engine pipeline itself never throws).
/// An UNSOUND layout (no root app.json `id`, or multi-app source tree) is NOT an
/// error — it yields an EMPTY projection (fail-closed), matching R0.
pub fn project_workspace(workspace: &Path) -> anyhow::Result<L2Projection> {
    if !workspace.is_dir() {
        anyhow::bail!("workspace is not a directory: {}", workspace.display());
    }

    let empty = || L2Projection {
        objects: Vec::new(),
        routines: Vec::new(),
    };

    // --- fail-closed layout detection (mirrors R0 snapshot_workspace) ----------
    let app_guid = match read_root_app_guid(workspace) {
        Some(g) => g,
        None => {
            eprintln!(
                "fail-closed: no readable root app.json with a string `id` at {} — emitting empty projection",
                workspace.display()
            );
            return Ok(empty());
        }
    };
    let app_json_count = count_app_json(workspace);
    if app_json_count > 1 {
        eprintln!(
            "fail-closed: multi-app source workspace at {} ({app_json_count} app.json files, excl. node_modules/.alpackages) — emitting empty projection",
            workspace.display()
        );
        return Ok(empty());
    }

    let files = discover_al_files(workspace)
        .map_err(|e| anyhow::anyhow!("failed to discover .al files: {e}"))?;

    let mut projection = empty();

    // Sequential parse loop, run on a big-stack thread (T2.1): this CLI path
    // (`aldump`/`alsem`) runs on the process main thread, which has no
    // guaranteed-generous stack — see `big_stack`'s doc. One big-stack thread
    // for the WHOLE loop, not per-file.
    crate::big_stack::run_with_big_stack(|| {
        for file in &files {
            let source = match read_al_source(&file.abs_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("warning: skipping {} (read error: {e})", file.rel_posix);
                    continue;
                }
            };
            let source_unit_id = format!("ws:{}", file.rel_posix);
            project_file(
                &source,
                &app_guid,
                &source_unit_id,
                &mut projection.objects,
                &mut projection.routines,
            );
        }
    });

    // Top-level: objects sorted by StableObjectId, routines by StableRoutineId.
    projection
        .objects
        .sort_by(|a, b| a.stable_object_id.cmp(&b.stable_object_id));
    projection
        .routines
        .sort_by(|a, b| a.stable_routine_id.cmp(&b.stable_routine_id));

    Ok(projection)
}

/// Project a single NAMED routine from a single-file `source` into a full
/// [`PRoutine`] (features + control-context + operation-order applied, plus the
/// routine-level metadata: `attributes`/`attributesParsed`/`accessModifier`/
/// `bodyAvailable`/`parseIncomplete`). Mirrors the per-routine body of
/// [`project_file`] EXACTLY — it is the single-routine entry point used by the
/// R1d capability vector tests (which need a fully-populated `PRoutine`, with
/// `controlContext` set so the unreachable filter fires, plus the internal-id
/// `op*`/`cs*` witness references that the capability facts carry).
///
/// Returns `None` when the named routine isn't found in any object.
///
/// Like [`project_file`] this is now driven entirely by the owned AL syntax IR —
/// no tree-sitter CST walk.
pub fn project_named_routine(
    source: &str,
    routine_name: &str,
    app_guid: &str,
    source_unit_id: &str,
) -> Option<PRoutine> {
    let ir_file = al_syntax::parse(source);

    for (oi, o) in ir_file.objects.iter().enumerate() {
        let Some(object_type) = crate::engine::l2::ir_walk::ir_object_type(&o.kind) else {
            continue;
        };
        let object_number = o.id.unwrap_or(0);
        let internal_object_id = encode_object_id(app_guid, object_type, object_number);
        let stable_object_id = to_stable_object_id(&internal_object_id);

        let (_, _, source_table_name, _) =
            crate::engine::l2::ir_walk::ir_object_metadata(o, object_type);

        for ir_routine in &o.routines {
            if ir_routine.name != routine_name {
                continue;
            }
            if let Some(routine) = build_proutine(
                &ir_file,
                oi,
                ir_routine,
                object_type,
                object_number,
                &stable_object_id,
                source_table_name.as_deref(),
                app_guid,
                source,
                source_unit_id,
            ) {
                return Some(routine);
            }
        }
    }
    None
}
