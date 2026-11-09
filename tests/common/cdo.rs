//! Shared CDO-workspace gating helper for CDO-env-gated tests.
//!
//! `CDO_WS` points at a real Business Central workspace tree that only
//! exists on machines with access to it (CI cannot reach it). Tests gated on
//! it must skip silently in the common case (no `CDO_WS`, e.g. any CI run or
//! a contributor's machine without the tree) but fail LOUDLY when
//! `ENFORCE_CDO_WS=1` is set and the workspace is missing — that combination
//! marks a gated/scheduled run (see `scripts/cdo-gate`) where losing
//! `CDO_WS` must never silently pass.
//!
//! `cargo test` compiles each `tests/*.rs` file as its own separate
//! binary/crate, so a `mod` defined in one cannot be `use`d from another.
//! This file is included via `#[path = "common/cdo.rs"] mod cdo;` by every
//! test binary that needs the helper (`program_resolve_harness.rs`,
//! `program_graph.rs`, `snapshot_robustness.rs`) so there is exactly one
//! implementation (Task T0.2).
//!
//! It is also the coverage guard of `scripts/cdo-gate` (2026-11-09): every
//! call checks that the calling test is one the gate runs
//! ([`on_the_cdo_gate`]), with or without `CDO_WS`. The debug suites run
//! every test without `CDO_WS`, so a new CDO test the gate's filters miss
//! fails there instead of skipping on every runner.

/// Returns `CDO_WS` as a `PathBuf` when set and pointing at an existing
/// path; otherwise `None` — UNLESS `ENFORCE_CDO_WS=1`, in which case a
/// missing/invalid `CDO_WS` panics (naming the current test via its thread
/// name, which libtest sets to the test's path) instead of returning `None`.
///
/// Callers use the standard skip idiom:
/// ```ignore
/// let Some(ws) = cdo_ws_or_enforce() else { return };
/// ```
/// Under `ENFORCE_CDO_WS=1` with a missing workspace, that `else` arm is
/// unreachable because this function panics first — no fail-open.
pub fn cdo_ws_or_enforce() -> Option<std::path::PathBuf> {
    assert_on_the_cdo_gate();
    let ws = std::env::var_os("CDO_WS")
        .map(std::path::PathBuf::from)
        .filter(|p| p.exists());
    if ws.is_none() {
        assert!(
            std::env::var("ENFORCE_CDO_WS").as_deref() != Ok("1"),
            "ENFORCE_CDO_WS=1 but CDO_WS is unset or does not point at an existing path (test: {})",
            std::thread::current().name().unwrap_or("<unknown test>")
        );
    }
    ws
}

/// The lib tests `scripts/cdo-gate` runs: every one whose path contains this
/// (`cargo test -p al-sem --lib cdo`, libtest's substring filter).
pub const LIB_FILTER: &str = "cdo";

/// Whether `scripts/cdo-gate` runs test `test` (its path, as libtest names
/// it) of the test binary of crate `krate` (`al_sem` for the lib, the
/// umbrella's or file's name for an integration target). This is the
/// script's list of phases, target by target and filter by filter: change
/// the two together.
pub fn on_the_cdo_gate(krate: &str, test: &str) -> bool {
    match krate {
        "al_sem" => test.contains(LIB_FILTER),
        "cli" => test.starts_with("output_cdo::"),
        "lsp" => test.starts_with("program_graph::") || test.starts_with("snapshot_robustness::"),
        "program_resolve_harness" | "l4_summary_differential" => true,
        _ => false,
    }
}

/// The coverage guard: the calling test must be one `scripts/cdo-gate` runs,
/// or it would skip on every runner. libtest names each test's thread after
/// the test's path (under `cargo test` and nextest alike), and this file's
/// `module_path!()` starts with the crate it is included in.
fn assert_on_the_cdo_gate() {
    let krate = module_path!().split("::").next().unwrap_or_default();
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("<unnamed thread>");
    let fix = if krate == "al_sem" {
        format!("put `{LIB_FILTER}` in its name: the gate runs `--lib {LIB_FILTER}`")
    } else {
        "move it into a member the gate runs, or add a phase to the script and to \
         `on_the_cdo_gate`"
            .to_string()
    };
    assert!(
        on_the_cdo_gate(krate, test),
        "test `{test}` of `{krate}` reads the CDO workspace, but scripts/cdo-gate does not run it \
         (tests/common/cdo.rs `on_the_cdo_gate`): {fix}"
    );
}
