//! `scripts/cdo-gate`'s coverage guard, the static half (2026-11-09).
//!
//! The gate runs only the lib tests whose path contains `cdo` (and named
//! integration members). The runtime half lives in the shared gate itself
//! (`tests/common/cdo.rs`, `cdo_ws_or_enforce`): every call checks that the
//! calling test is one the gate runs, and the debug suites call it for every
//! CDO test (without `CDO_WS`), so a CDO test outside the filters fails there.
//! That holds only while every test reads the workspace THROUGH the gate: a
//! test reading the env var itself bypasses the check (and skips silently
//! under `ENFORCE_CDO_WS=1`). This test keeps it so.

use std::path::Path;

/// The source files allowed to read the env var directly: the gate, and
/// `mint-goldens` (a binary, not a test; it defaults its workspace to it).
const ALLOWED: &[&str] = &["tests/common/cdo.rs", "src/bin/mint-goldens.rs"];

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Every place in the crate's sources that reads the CDO workspace env var
/// directly, outside [`ALLOWED`]: `path:line`.
fn direct_reads(root: &Path) -> Vec<String> {
    // Spelled in pieces so this file does not match itself.
    let var = concat!("\"CDO", "_WS\"");
    let needles = [
        format!("var_os({var})"),
        format!("var({var})"),
        format!("env!({var})"),
    ];
    let mut files = Vec::new();
    for dir in ["src", "tests", "crates", "benches"] {
        rust_files(&root.join(dir), &mut files);
    }
    files.sort();
    let mut found = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if needles.iter().any(|n| line.contains(n.as_str())) {
                found.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    found
}

/// Every test reads the CDO workspace through `crate::cdo::cdo_ws_or_enforce`
/// (or the integration targets' copy), never the env var itself.
///
/// DISCRIMINATION (2026-11-09): putting a direct `std::env::var_os(..)` read
/// of the variable back into `snapshot.rs`'s `builds_snapshot_over_cdo_workspace`
/// fails here naming `src/snapshot/snapshot.rs:<line>`; restored, passes.
#[test]
fn cdo_ws_is_read_only_through_the_gate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let found = direct_reads(root);
    assert!(
        found.is_empty(),
        "these read the CDO workspace env var directly, bypassing the gate's coverage check \
         and ENFORCE_CDO_WS; use `crate::cdo::cdo_ws_or_enforce()` (lib) or `crate::cdo::` \
         (integration targets):\n{}",
        found.join("\n")
    );
}

/// The runtime half's predicate is the script's phases: the lib by the
/// `cdo` substring, `cli` by `output_cdo::`, `lsp` by its two members, two
/// whole targets, nothing else.
#[test]
fn cdo_gate_run_set_matches_the_script() {
    use crate::cdo::on_the_cdo_gate;
    assert!(on_the_cdo_gate(
        "al_sem",
        "engine::l4::pd_solve::tests::parallel_solve_equals_the_serial_walk_on_cdo"
    ));
    assert!(!on_the_cdo_gate(
        "al_sem",
        "snapshot::snapshot::tests::workspace_is_not_its_own_dependency"
    ));
    assert!(on_the_cdo_gate("cli", "output_cdo::cdo_rendering_goldens"));
    assert!(!on_the_cdo_gate("cli", "cli_impact::cdo_impact_ratchets"));
    assert!(on_the_cdo_gate(
        "lsp",
        "program_graph::cdo_program_graph_is_app_qualified_and_panic_free"
    ));
    assert!(!on_the_cdo_gate("lsp", "handlers::anything"));
    assert!(on_the_cdo_gate("program_resolve_harness", "any"));
    assert!(on_the_cdo_gate("l4_summary_differential", "any"));
    assert!(!on_the_cdo_gate("differential", "any"));
    // The script states the same filters (a renamed phase must move both).
    let script =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cdo-gate"))
            .expect("scripts/cdo-gate");
    for phase in [
        "--test program_resolve_harness -- ",
        "--test lsp -- program_graph:: snapshot_robustness::\n",
        "--test l4_summary_differential\n",
        "--test cli output_cdo::\n",
        "-p al-sem --lib cdo -- ",
    ] {
        assert!(
            script.contains(phase),
            "scripts/cdo-gate no longer runs `{phase}`: change `on_the_cdo_gate` with it"
        );
    }
}
