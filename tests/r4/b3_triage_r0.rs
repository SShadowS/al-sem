//! B3 Phase A, Task 4: the detector difference harness
//! (`al_sem::engine::l3::b3_diff`) over every `tests/r0-corpus` fixture with
//! an `app.json`. It must run, and its triage table must equal the committed
//! `docs/b3-triage/r0-corpus.md` with the verdict cells ignored, so Task 5's
//! verdicts do not break it. `REGEN_TEMP_GOLDENS=1` rewrites the file and
//! keeps the verdicts of rows whose text did not change.
//!
//! Task 6: the same for the stage-2 table (`--b3-deps`: the adapter without
//! dependency-callee bindings against the adapter with them),
//! `docs/b3-triage/r0-corpus-deps.md`.

use std::path::Path;

use al_sem::engine::l3::b3_diff::{
    carry_verdicts, detector_diff_for_workspace, strip_verdicts, triage_markdown,
};

use crate::regen;

fn r0_corpus_markdown(root: &Path, deps: bool) -> String {
    let mut dirs: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("app.json").is_file())
        .collect();
    dirs.sort();
    let (mut ok, mut errors) = (Vec::new(), Vec::new());
    for dir in &dirs {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        match detector_diff_for_workspace(dir, deps) {
            Ok(d) => ok.push((name, d)),
            Err(e) => errors.push((name, e)),
        }
    }
    triage_markdown("tests/r0-corpus", &ok, &errors)
}

fn check(deps: bool, file: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let md = r0_corpus_markdown(&root.join("tests/r0-corpus"), deps);
    let path = root.join("docs/b3-triage").join(file);
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    if regen::regen_mode() {
        std::fs::write(&path, carry_verdicts(&md, &committed)).unwrap();
        return;
    }
    assert_eq!(
        strip_verdicts(&md),
        strip_verdicts(&committed),
        "docs/b3-triage/{file} is stale; regenerate with REGEN_TEMP_GOLDENS=1"
    );
}

#[test]
fn r0_corpus_triage_table_is_stable() {
    check(false, "r0-corpus.md");
}

#[test]
fn r0_corpus_deps_triage_table_is_stable() {
    check(true, "r0-corpus-deps.md");
}
