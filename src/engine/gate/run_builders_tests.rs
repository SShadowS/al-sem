//! #277 T16 `single_app_builders_agree` (spec §5, P-N10's runtime half): the three
//! single-app model builders give the same model, so one builder per view (D4.1)
//! changes no output.

use std::path::Path;
use std::sync::Arc;

use crate::engine::gate::run::{AnalysisTarget, build_analysis_model};
use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
use crate::program::model::program_calls::assemble_and_resolve_workspace_program;
use crate::program::model::workspace::{AssemblyNeeds, Model};

/// The model's rows as text: every field `Debug`-printed, the one hash map
/// (`upgraded_bindings`) sorted by key.
fn rows(m: &Model) -> Vec<String> {
    let mut bindings: Vec<_> = m.calls.upgraded_bindings.iter().collect();
    bindings.sort_by(|a, b| a.0.cmp(b.0));
    vec![
        format!("{:?}", m.workspace),
        format!("{:?}", m.root_classifications),
        format!("{:?}", m.primary_app),
        format!("{:?}", m.infra_diagnostics),
        format!("{:?}", m.workspace_diagnostics),
        format!("{:?}", m.dependency_diagnostics),
        format!("{:?}", m.calls.edges),
        format!("{bindings:?}"),
        format!("{:?}", m.calls.diagnostics),
        format!("{:?}", m.calls.external_targets),
        format!("{:?}", m.events),
    ]
}

/// `inputs` without its roots config: how a builder that takes no
/// `--no-roots-config` flag sees "no roots config".
fn without_roots_config(ws: &Path) -> Arc<WorkspaceInputs> {
    let mut set = Arc::try_unwrap(WorkspaceInputs::read(ws, &WorkspaceNeeds::default()))
        .unwrap_or_else(|_| panic!("a fresh read has one owner"));
    set.roots_config = None;
    Arc::new(set)
}

/// The three builders' rows on `ws`; `None` where a builder built no model.
fn three(ws: &Path, no_roots_config: bool) -> [Option<Vec<String>>; 3] {
    let full = WorkspaceInputs::read(ws, &WorkspaceNeeds::default());
    let other = if no_roots_config {
        without_roots_config(ws)
    } else {
        Arc::clone(&full)
    };
    let program = full.model_instance_id.as_deref().and_then(|id| {
        assemble_and_resolve_workspace_program(&full, id, no_roots_config, AssemblyNeeds::BASE)
            .map(|m| rows(&m))
    });
    let target = AnalysisTarget::build(&other, true, "t16")
        .ok()
        .map(|t| rows(t.resolved()));
    let analysis = build_analysis_model(&other, true)
        .model
        .ok()
        .map(|t| rows(t.resolved()));
    [program, target, analysis]
}

#[test]
fn single_app_builders_agree() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus");
    let mut dirs: Vec<_> = std::fs::read_dir(&corpus)
        .expect("read r0-corpus")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("ws-"))
        })
        .collect();
    dirs.sort();
    // Precondition: a dir whose roots config changes the model, so a builder that
    // ignored (or wrongly applied) `--no-roots-config` would show here.
    let with_config: Vec<_> = dirs
        .iter()
        .filter(|d| d.join("roots.config.json").is_file())
        .collect();
    assert!(!with_config.is_empty(), "no r0 dir has a roots.config.json");
    let mut built = 0;
    for ws in &dirs {
        let by_flag = [false, true].map(|no_roots_config| {
            let [program, target, analysis] = three(ws, no_roots_config);
            let name = ws.display();
            assert_eq!(
                program, target,
                "{name} (no_roots_config={no_roots_config}): AnalysisTarget::build differs"
            );
            assert_eq!(
                program, analysis,
                "{name} (no_roots_config={no_roots_config}): build_analysis_model differs"
            );
            program
        });
        if by_flag[0].is_some() {
            built += 1;
        }
        if ws.join("roots.config.json").is_file() {
            assert_ne!(
                by_flag[0],
                by_flag[1],
                "{}: the roots config does not change the model",
                ws.display()
            );
        }
    }
    assert!(
        built > dirs.len() / 2,
        "only {built} of {} dirs built a model",
        dirs.len()
    );
}
