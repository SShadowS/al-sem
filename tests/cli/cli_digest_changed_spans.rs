//! #71: `alsem digest --file` / `--diff` resolve changed roots by routine
//! DECLARATION SPAN (the attribute lines, the header and the whole body).
//!
//! This file grows with the #71 plan: Task 3 adds the model's declaration start;
//! the digest acceptance tests join it in Task 5.
//! Ledger: `.agent/issue-71/ledger.md` (Design, Acceptance matrix).

use std::path::Path;

use al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id;
use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn corpus(fixture: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(fixture)
}

/// The d8 fixture's `src/posting.al`, LF, as committed. `PostSalesDoc` is lines
/// 3-16, `[IntegrationEvent]` is line 18, `OnAfterPostSalesDoc` lines 19-21.
fn d8_posting() -> String {
    std::fs::read_to_string(corpus("ws-d8-commit-in-tx").join("src/posting.al"))
        .unwrap()
        .replace('\r', "")
}

/// Copy `ws-d8-commit-in-tx` into `dir`, with `posting.al` replaced by `posting`
/// (the post-change file text).
fn d8_workspace(dir: &Path, posting: &str) {
    let src = corpus("ws-d8-commit-in-tx");
    write(
        &dir.join("app.json"),
        &std::fs::read_to_string(src.join("app.json")).unwrap(),
    );
    write(
        &dir.join("src/subscriber.al"),
        &std::fs::read_to_string(src.join("src/subscriber.al"))
            .unwrap()
            .replace('\r', ""),
    );
    write(&dir.join("src/posting.al"), posting);
}

/// Plan Task 3: a routine's declaration starts at its first attached attribute;
/// `source_anchor` still starts at the keyword.
#[test]
fn model_routine_declaration_start_includes_attributes() {
    let dir = tempfile::tempdir().unwrap();
    d8_workspace(dir.path(), &d8_posting());
    let model_id = compute_gate_model_instance_id(dir.path()).unwrap();
    let model = assemble_and_resolve_workspace_program(dir.path(), &model_id, false).unwrap();
    let r = |name: &str| {
        model
            .workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .unwrap()
    };
    // 0-based: `[IntegrationEvent]` row 17, `procedure OnAfterPostSalesDoc` row 18.
    let p = r("OnAfterPostSalesDoc");
    assert_eq!(
        (p.declaration_start_line, p.source_anchor.start_line),
        (17, 18)
    );
    let q = r("PostSalesDoc");
    assert_eq!(
        (q.declaration_start_line, q.source_anchor.start_line),
        (2, 2)
    );
}
