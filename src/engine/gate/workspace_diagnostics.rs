//! `workspace_diagnostics` — port of al-sem `indexWorkspace`'s `workspace.diagnostics`
//! (the FIRST of the six diagnostic sources `analyzeWorkspace` concatenates;
//! al-sem `src/index.ts:157-173`).
//!
//! al-sem assembles `workspace.diagnostics` as:
//!   1. PROVIDER diagnostics (`WorkspaceProvider.collect`, `providers/workspace.ts`)
//!      REMAPPED to `stage: "discover"` — fail-closed errors (multi-app /
//!      id-less / unreadable root `app.json`) and per-file read warnings.
//!   2. INDEX diagnostics (`buildSemanticIndex`, `index/indexer.ts`) — carry their
//!      OWN stage: `index` ("No object declaration found in <rel>"), `parse`
//!      ([PARSER001] native parser unavailable), or `index` (failed-to-index warn).
//!
//! It is computed DETERMINISTICALLY from the command's one read of the workspace
//! (`InputSet`, #160) so BOTH the success path (the model's
//! `workspace_diagnostics`, built in `attach_program_calls`) and the fail-closed
//! path (`empty_output_result`) can thread it into the JSON envelope's
//! `diagnostics` array. The index rows come from the program build's own parse
//! (`FreshCoverage::files_without_object`): there is no second parse, and no
//! index row when the build parsed nothing.
//!
//! Determinism: the provider message for the multi-app case sorts the discovered
//! `app.json` absolute paths (al-sem `appJsonPaths.slice().sort()`); the index
//! "No object declaration found" diagnostics are emitted in rel-posix-sorted file
//! order (the same total order `assemble_workspace` ingests). No HashMap/HashSet
//! iteration leaks into the output order.

use crate::engine::l5::registry::Diagnostic;
use crate::program::inputs::InputSet;

/// Compute al-sem's `workspace.diagnostics` for the workspace `inputs` read:
/// PROVIDER diagnostics (remapped to `stage: "discover"`) concatenated with INDEX
/// diagnostics (their own `index`/`parse` stage), in al-sem order.
///
/// `files_without_object` is the program build's list of workspace files that
/// parsed to no object (`FreshCoverage::files_without_object`); `None` when the
/// build failed, which then emits no index row (#160 §4.2.4: no fallback parse).
///
/// Engine-never-throws: any read/IO failure degrades to fewer diagnostics, never
/// a panic. Returns an empty vec for a clean sound workspace with objects in every
/// file (the source-only corpus's common case — which is why the 40 goldens have
/// `diagnostics: []`).
pub fn compute_workspace_diagnostics(
    inputs: &InputSet,
    files_without_object: Option<&[String]>,
) -> Vec<Diagnostic> {
    assert!(
        !inputs.build_only,
        "InputSet read with build_only: the app.json list and nested flags were not read"
    );
    let workspace = inputs.workspace.as_path();
    let mut out: Vec<Diagnostic> = Vec::new();

    // --- 1. PROVIDER diagnostics (remapped to "discover") ----------------------
    // Mirror `WorkspaceProvider.collect`'s fail-closed layout detection EXACTLY:
    //   isMultiApp = (count app.json > 1);  appGuid = readable root id (non-empty).
    let app_json_paths = &inputs.app_json_paths;
    let is_multi_app = app_json_paths.len() > 1;

    // `readable` is true iff the file parsed as JSON (al-sem sets
    // `rootAppJsonReadable = true` AFTER `JSON.parse`).
    let root_app_json_readable = matches!(inputs.app_json_value, Some(Ok(_)));
    let app_guid = inputs.app_id();

    if is_multi_app || app_guid.is_none() {
        if is_multi_app {
            // al-sem: `appJsonPaths.slice().sort().join(", ")` over ABSOLUTE paths.
            let mut sorted: Vec<String> = app_json_paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            sorted.sort();
            let found_list = sorted.join(", ");
            out.push(Diagnostic {
                severity: "error".to_string(),
                stage: "discover".to_string(),
                message: format!(
                    "multi-app source workspace unsupported — al-sem analyzes one app project per run; point workspaceRoot at the app root. Found {} app.json files: {found_list}",
                    app_json_paths.len()
                ),
            });
        }
        if app_guid.is_none() {
            let root = workspace.to_string_lossy();
            let message = if root_app_json_readable {
                format!(
                    "root app.json at {root} has no string `id` — cannot assign object identity; emitting no source units (fail-closed)"
                )
            } else {
                format!(
                    "no readable root app.json with an `id` at {root} — cannot assign object identity; emitting no source units (fail-closed)"
                )
            };
            out.push(Diagnostic {
                severity: "error".to_string(),
                stage: "discover".to_string(),
                message,
            });
        }
        // Fail-closed: the provider returns NO units, so `buildSemanticIndex` runs
        // over an empty unit list and produces NO index diagnostics. Done.
        return out;
    }

    // --- 1b. PROVIDER per-file read warnings ------------------------------------
    // The set's `.al` walk (`NestedApps::Walk`, rel-posix-sorted): a read error
    // surfaces as a provider `warning` (remapped to "discover"). A failed walk
    // left the list empty and stops here, as the disk walk did.
    if inputs.al_walk.is_err() {
        return out;
    }
    for f in &inputs.al_files {
        if let Err(e) = &f.text {
            out.push(Diagnostic {
                severity: "warning".to_string(),
                stage: "discover".to_string(),
                message: format!("Could not read {}: {e}", f.abs_path.to_string_lossy()),
            });
        }
    }

    // --- 2. INDEX diagnostics (buildSemanticIndex; their own stage) ------------
    // Per file the program build parsed to NO object declaration, in
    // rel-posix-sorted order: an `info`/`index` "No object declaration found in
    // <rel>" — al-sem `indexer.ts:56-63`. The build's own `al_syntax::parse`, so
    // the diagnostic reflects exactly what the engine sees (incl. objects nested
    // under a `namespace`).
    for rel in files_without_object.unwrap_or_default() {
        out.push(Diagnostic {
            severity: "info".to_string(),
            stage: "index".to_string(),
            message: format!("No object declaration found in {rel}"),
        });
    }

    out
}
