//! `--strict` on `events fanout`, `events chains` and `policy check` (#160 Task 26
//! review, minor 7): the strict gate moved into `events_view` and policy's
//! `View` path, so these pin it. The error workspace is `cli_diff_strict`'s: a
//! clean app plus a nested `sub/app.json`, which yields the `multi-app source
//! workspace unsupported` error diagnostic while the root app still loads.
//!
//! Each case first runs WITHOUT `--strict` (the precondition: exit 0, the error
//! line is printed), then with it: exit 1, no output text, and stderr is exactly
//! the analyzer diagnostic lines, as the code prints them.

use std::path::Path;
use std::sync::Arc;

use al_sem::engine::gate::events::{
    EventsChainsOptions, EventsFanoutOptions, EventsRunResult, run_events_chains, run_events_fanout,
};
use al_sem::engine::gate::policy::pipeline::{
    PolicyCheckOptions, policy_request, run_policy_check,
};
use al_sem::engine::l5::event_flow::Scope;
use al_sem::program::inputs::{RequestInputs, WorkspaceInputs};

use crate::cli_diff_strict::{MULTI_APP_ERROR, error_ws};

/// The one multi-app error line `workspace_diagnostics` prints for `ws`.
fn expected_lines(ws: &Path) -> Vec<String> {
    let mut paths = [ws.join("app.json"), ws.join("sub").join("app.json")]
        .map(|p| p.to_string_lossy().into_owned());
    paths.sort();
    vec![format!(
        "{MULTI_APP_ERROR} — al-sem analyzes one app project per run; point workspaceRoot at the app root. Found 2 app.json files: {}",
        paths.join(", ")
    )]
}

fn fanout(inputs: &Arc<WorkspaceInputs>, strict: bool) -> EventsRunResult {
    run_events_fanout(&EventsFanoutOptions {
        inputs,
        format: "json",
        scope: Scope::Primary,
        coverage_policy: "warn",
        driver_version: "strict-v1",
        deterministic: true,
        strict,
        single_app: false,
    })
}

fn chains(inputs: &Arc<WorkspaceInputs>, strict: bool) -> EventsRunResult {
    run_events_chains(&EventsChainsOptions {
        inputs,
        format: "json",
        scope: Scope::Primary,
        coverage_policy: "warn",
        max_depth: None,
        max_nodes: None,
        driver_version: "strict-v1",
        deterministic: true,
        strict,
        single_app: false,
    })
}

#[test]
fn events_strict_exits_1_on_an_error_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = error_ws(tmp.path(), "ws");
    let inputs = WorkspaceInputs::read(&ws, &Default::default());
    let want = expected_lines(&ws);
    for (cmd, run) in [
        (
            "fanout",
            fanout as fn(&Arc<WorkspaceInputs>, bool) -> EventsRunResult,
        ),
        ("chains", chains),
    ] {
        let lax = run(&inputs, false);
        assert_eq!(
            lax.exit_code, 0,
            "{cmd}: precondition, exit 0 without --strict"
        );
        assert!(
            want.iter().all(|l| lax.stderr_lines.contains(l)),
            "{cmd}: precondition, the error line; stderr={:?}",
            lax.stderr_lines
        );
        let strict = run(&inputs, true);
        assert_eq!(strict.exit_code, 1, "{cmd}: exit 1 under --strict");
        assert!(
            strict.text.is_empty(),
            "{cmd}: no stdout; got {:?}",
            strict.text
        );
        assert_eq!(strict.stderr_lines, want, "{cmd}: stderr");
    }
}

#[test]
fn policy_check_strict_exits_1_on_an_error_diagnostic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = error_ws(tmp.path(), "ws");
    let inputs = WorkspaceInputs::read(&ws, &Default::default());
    let request = RequestInputs::read(&ws, policy_request(None, false), None);
    let run = |strict| {
        run_policy_check(&PolicyCheckOptions {
            inputs: &inputs,
            request: &request,
            format: "json",
            out: None,
            deterministic: true,
            strict,
            driver_version: "strict-v1",
            single_app: false,
        })
    };
    let want = expected_lines(&ws);
    let lax = run(false);
    assert_eq!(lax.exit_code, 0, "precondition: exit 0 without --strict");
    assert!(
        want.iter().all(|l| lax.stderr_lines.contains(l)),
        "precondition: the error line; stderr={:?}",
        lax.stderr_lines
    );
    let strict = run(true);
    assert_eq!(strict.exit_code, 1, "exit 1 under --strict");
    assert_eq!(strict.text, None, "no output text");
    assert_eq!(strict.stderr_lines, want, "stderr");
}
