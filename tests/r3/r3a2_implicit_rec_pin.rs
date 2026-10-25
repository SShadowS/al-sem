//! #272 A8: the implicit-`Rec` binding change leaves the L4 summary core (db effects,
//! uncertainties, parameter roles) unchanged. An implicit `Rec` has no parameter
//! id, name match or `source_parameter_index`, so no parameter role can move.
//!
//! The expected files are the R3a-2 summary-core projection (`aldump
//! --r3a2-summary-core`, the same `project_r3a2` as `r3a2_differential`) of the
//! two fixtures on master BEFORE #272 (`a6d94a91`); the design's rev3 prototype gave
//! byte-identical output. This is a pin: it passes before and after, and it has no
//! regen path, so an intended change to these summaries is an explicit edit.

use std::path::{Path, PathBuf};

use al_sem::engine::l4::summary::project_r3a2;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_with_program_calls;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issue-272")
}

fn assert_pinned(fixture: &str) {
    let ws = dir().join(fixture);
    let model = assemble_and_resolve_workspace_with_program_calls(&ws)
        .unwrap_or_else(|| panic!("model of {fixture}"));
    let mut got = serde_json::to_string_pretty(&project_r3a2(&model)).expect("serialize");
    got.push('\n');
    let path = dir().join(format!("{fixture}.r3a2.json"));
    let want =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        got == want,
        "R3a-2 summary core of {fixture} moved; expected {}:\n{got}",
        path.display()
    );
}

#[test]
fn shapes_summary_core_is_unchanged() {
    assert_pinned("ws-shapes");
}

#[test]
fn readers_summary_core_is_unchanged() {
    assert_pinned("ws-readers");
}
