# Issue #68: one default detector set — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** digest, prove, fingerprint, events and policy run the same detector SET that
`alsem analyze` runs by default — the registry's `default_detectors()` — instead of the 14
names that still match a 34-name list copied at port time.

**Architecture:** The registry owns membership: `registered_detectors()` is split into
`default_detectors()` (40) and `opt_in_detectors()` (14). Both hand name lists in
`gate/presets.rs` are deleted; presets become one `PRESETS` table. `gate/run.rs` gets one
shared entry point per mode (`compute_analyzer_diagnostics`, `compute_analyzer_diagnostics_cross_app`)
that picks the default set itself, so no caller outside `run.rs` can pass a different set.

**Tech Stack:** Rust 2024, `cargo test`, the `cli` umbrella test crate, `ALSEM_TRACE` Chrome traces.

**Spec:** `.agent/issue-68/ledger.md` (Design 1-10, Acceptance matrix, Golden license) and
`.agent/issue-68/findings.json` (19 accepted findings). Read both before Task 1.

## Global Constraints

- Every cargo command: `export TREE_SITTER_AL_PATH=U:/Git/al-call-hierarchy/tree-sitter-al` (this is a worktree; it has no grammar clone).
- Edit files with Edit/Write only. No `sed -i`, no python rewrites. Discrimination breaks are made and reverted with Edit too.
- Format with `rustfmt --edition 2024 <file>`, never `cargo fmt`. Stage named paths only, never `git add -A`.
- Protected paths no task touches: `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, the `Cargo.toml` version, `tree-sitter-al`, and `.agent/` except `.agent/issue-68/ledger.md` and `.agent/issue-68/findings.json`.
- Golden license: ZERO bytes move in any of the 8 `scripts/check-goldens` targets. `scripts/check-goldens` must pass WITHOUT `--regen`. A moved golden STOPS the task for triage; it is never blessed under this issue.
- Each task ends with `scripts/ci-steps task` green before its commit. Do not pipe a gate through `| tail`; redirect to a log and grep it.
- Record every discrimination proof (mutation, fail output, pass output) in the ledger's "Discrimination proofs" table.
- The `--preset` and `--detector` branches of `resolve_analyze_detectors` stay unchanged (explicit selection is analyze-only).
- Out of scope, do not touch: the cli-a "default" slot (`tests/cli/cli_a_*_differential.rs`, Discovery D2), `.claude/commands/triage-wave.md` (Discovery D4, an owner action), dated plans under `docs/superpowers/plans/`.

## Review Focus

1. An old-name copy surviving somewhere: after Task 3, `grep -rn "DEFAULT_DETECTOR_NAMES\|OPT_IN_DETECTOR_NAMES\|PRESET_NAMES_LIST" src tests` must hit only `tests/cli/cli_a_*` (D2). Task 4 Step 1 runs it.
2. A detector run twice in one command (e.g. digest cross-app calling both runners): T3 assertion (4) "exactly once" covers it (Task 3).
3. A command that exits non-zero after the change (a newly running detector panics on a fixture): T3 assertion (1) exit 0, and `scripts/check-goldens` (Task 4).
4. A golden that moves although P8/P9 predict zero: Task 4 runs `scripts/check-goldens` without regen; any diff stops the work.
5. Cost on a real workspace: Task 4 re-measures the six CDO invocations (Design 9) and reports them; no threshold, but a regression beyond P12's noise band goes to the owner.

---

### Task 1: Split the registry (Design 1) + T1b

**Files:**
- Modify: `src/engine/l5/detectors/mod.rs:1212-1554` (`registered_detectors`) and add a `#[cfg(test)] mod tests` after it.

**Interfaces:**
- Produces: `pub fn default_detectors() -> Vec<Detector>` (40, today's first 40 entries, same order), `pub fn opt_in_detectors() -> Vec<Detector>` (14, today's last 14), and `registered_detectors()` = default then opt-in (order unchanged, so every `detectorStats` order is unchanged).

- [ ] **Step 1: Write T1b** at the end of the registry section, before `mod issue21_predicate_tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn names(ds: Vec<Detector>) -> Vec<String> {
        ds.into_iter().map(|d| d.name).collect()
    }

    /// Issue #68 (T1b): the default/opt-in split is pinned independently of
    /// the code that consumes it. The opt-in literal is HAND-STATED (each
    /// entry carries an `OPT-IN` comment above; d17, d8 and d45 state the
    /// S8/S8.5 owner decision), so a reclassification must edit it on purpose.
    #[test]
    fn default_and_opt_in_membership() {
        let default = names(default_detectors());
        let opt_in = names(opt_in_detectors());
        let all = names(registered_detectors());

        // (1) the registry is the default set, then the opt-in set, in order.
        let chained: Vec<String> = default.iter().chain(opt_in.iter()).cloned().collect();
        assert_eq!(all, chained, "registered_detectors() != default ++ opt_in");

        // (2) no name twice, inside one set or across the two.
        let unique: std::collections::HashSet<&String> = all.iter().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "duplicate detector name in {all:?}"
        );

        // (3) the opt-in set is exactly the owner-decided 14.
        let expected_opt_in = [
            "d40-transitive-load-missing",
            "d46-commit-in-lifecycle",
            "d47-io-unsafe-txn",
            "d48-io-in-loop",
            "d49-uncommitted-write-before-ui",
            "d50-checked-run-implicit-commit",
            "d51-retry-side-effect-duplication",
            "d17-min-version-drift",
            "d8-commit-in-transaction",
            "d45-event-transitive-table-exposure",
            "d61-ishandled-bypasses-critical-write",
            "d62-telemetry-before-success",
            "d63-html-concat-injection",
            "d64-api-page-write-surface",
        ];
        assert_eq!(opt_in, expected_opt_in, "opt-in set changed");

        // (4) counts: a default deleted outright passes (1)-(3).
        assert_eq!(default.len(), 40, "default_detectors() count");
        assert_eq!(all.len(), 54, "registered_detectors() count");
    }
}
```

- [ ] **Step 2: Run, expect a compile failure** (red): `cargo test -p al-sem --lib default_and_opt_in_membership` → `cannot find function default_detectors` / `opt_in_detectors`.

- [ ] **Step 3: Split the registry.** Three edits:
  - `registered_detectors()` body becomes:
    ```rust
    pub fn registered_detectors() -> Vec<Detector> {
        let mut all = default_detectors();
        all.extend(opt_in_detectors());
        all
    }

    /// The DEFAULT detector set: what `alsem analyze` runs with no `--detector` or
    /// `--preset`, and what digest, prove, fingerprint, events and policy run.
    /// Which function a detector sits in IS its class: demote one by moving its
    /// entry to [`opt_in_detectors`].
    pub fn default_detectors() -> Vec<Detector> {
        // (the existing `requires` comment block and `vec![` follow unchanged)
    ```
  - After the d60 entry, close the vec and open the second function:
    ```rust
        ]
    }

    /// The OPT-IN detector set: run only when named by `--detector` or a preset.
    pub fn opt_in_detectors() -> Vec<Detector> {
        vec![
            // --- OPT_IN_DETECTORS (14: al-sem opt-in order, then the BCQuality-wave opt-ins) ---
    ```
  - Fix the stale counts: `// --- DEFAULT_DETECTORS (43:` → `(40:`; the OPT_IN header `(11:` → `(14:` (done in the line above).

- [ ] **Step 4: Run, expect PASS:** `cargo test -p al-sem --lib default_and_opt_in_membership` → `test engine::l5::detectors::tests::default_and_opt_in_membership ... ok`. (Verified during step 6 of the issue flow, 2026-10-14.)

- [ ] **Step 5: Discrimination proofs** (Acceptance matrix T1b; each break with Edit, run, revert, run green, record in the ledger):
  - (i) Swap the d8 and d60 entries between the two functions → (3) fails `opt-in set changed`.
  - (ii) Duplicate the d1 entry inside `default_detectors()` → (2) fails `duplicate detector name`.
  - (iii) `registered_detectors()` returns `default_detectors()` only → (1) fails.
  - (iv) Delete the `d19-unused-parameter` entry → only (4) fails (`39` vs `40`).

- [ ] **Step 6: Gate and commit.** `rustfmt --edition 2024 src/engine/l5/detectors/mod.rs`; `scripts/ci-steps task` green (byte-identical refactor: every golden stays green).

```bash
git add src/engine/l5/detectors/mod.rs
git commit -m "refactor(l5): split the detector registry into default_detectors() and opt_in_detectors() (#68)"
```

---

### Task 2: Presets read the registry; one `PRESETS` table (Design 2) + T1 + T2

**Files:**
- Modify: `src/engine/gate/presets.rs` (module doc `:1-8`; delete `DEFAULT_DETECTOR_NAMES` `:16-57` and `OPT_IN_DETECTOR_NAMES` `:61-…`; `PRESET_NAMES_LIST` `:109`; `resolve_preset` `:113-124`; no-flag branch `:182-187`; tests)
- Modify: `src/bin/alsem.rs:69,1531` (`PRESET_NAMES_LIST` → `preset_names()`)
- Modify: `tests/cli/gate_suppress_baseline_differential.rs:28,106` (names from `default_detectors()`)

**Interfaces:**
- Consumes: `crate::engine::l5::detectors::{default_detectors, registered_detectors}` (Task 1).
- Produces: `pub const PRESETS: &[(&str, &[&str])]`, `pub fn preset_names() -> Vec<&'static str>`. `resolve_preset` keeps its signature and its error text `Unknown preset '<x>'. Known: transaction-integrity, bcquality`.

- [ ] **Step 1: Write T1** in `presets.rs`'s `mod tests` (compiles after Task 1; GREEN today, since the old list equals the registry's first 40 — it is a guard, proven by Step 6's mutation):

```rust
    /// Issue #68 (T1, selector): `analyze` with no `--detector`/`--preset`
    /// selects the registry's default set, in registry order. The USE is pinned
    /// end to end by `tests/cli/cli_envelope_detector_set.rs`'s analyze cases.
    #[test]
    fn analyze_default_is_registry_default() {
        let selected: Vec<String> = resolve_analyze_detectors(None, None)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        let registry: Vec<String> = crate::engine::l5::detectors::default_detectors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(selected, registry);
    }
```

- [ ] **Step 2: Write T2** in the same module, and DELETE `default_set_excludes_opt_in` and `every_default_and_opt_in_name_is_registered`; in `preset_resolves_bcquality` delete the `// every member must be registered` loop (T2 covers it; keep `assert_eq!(names, PRESET_BCQUALITY)` and `assert_eq!(names.len(), 13)`):

```rust
    /// Issue #68 (T2): every member of every preset is a registered detector.
    /// Walks `PRESETS`, the only place a preset can be defined, so a new
    /// preset is covered without editing this test.
    #[test]
    fn every_preset_member_is_registered() {
        let registered: std::collections::HashSet<String> =
            registered_detectors().into_iter().map(|d| d.name).collect();
        for (preset, members) in PRESETS {
            for m in *members {
                assert!(
                    registered.contains(*m),
                    "{m} not registered (preset {preset})"
                );
            }
        }
        let keys: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
        assert_eq!(preset_names(), keys);
        for (preset, members) in PRESETS {
            assert_eq!(
                resolve_preset(preset).unwrap(),
                members.to_vec(),
                "resolve_preset({preset}) != PRESETS entry"
            );
        }
    }
```

- [ ] **Step 3: Run, expect a compile failure** (red): `cargo test -p al-sem --lib presets::` → `cannot find value PRESETS` / `cannot find function preset_names`.

- [ ] **Step 4: Implement.**
  - Delete `DEFAULT_DETECTOR_NAMES` and `OPT_IN_DETECTOR_NAMES` (with their docs).
  - Replace `PRESET_NAMES_LIST` and `resolve_preset` with:
    ```rust
    /// Every preset: name -> member detector names. The ONLY place a preset is
    /// defined; `resolve_preset` and `preset_names` both read it.
    pub const PRESETS: &[(&str, &[&str])] = &[
        ("transaction-integrity", PRESET_TRANSACTION_INTEGRITY),
        ("bcquality", PRESET_BCQUALITY),
    ];

    /// Known preset names, in `PRESETS` order (CLI surface + error messages).
    pub fn preset_names() -> Vec<&'static str> {
        PRESETS.iter().map(|(n, _)| *n).collect()
    }

    /// Resolve a preset name to its detector-name list. `Err` on an unknown preset.
    pub fn resolve_preset(name: &str) -> Result<Vec<String>, String> {
        PRESETS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, members)| members.iter().map(|s| s.to_string()).collect())
            .ok_or_else(|| {
                format!(
                    "Unknown preset '{name}'. Known: {}",
                    preset_names().join(", ")
                )
            })
    }
    ```
  - No-flag branch of `resolve_analyze_detectors`: replace the `DEFAULT_DETECTOR_NAMES` block with `Ok(crate::engine::l5::detectors::default_detectors())`; change its doc bullet to "neither → the registry's `default_detectors()`".
  - Module doc `:1-8`: rewrite — presets and `analyze` selection; the default set is the registry's `default_detectors()` (no name list here); presets live in `PRESETS`.
  - `src/bin/alsem.rs`: import `preset_names` instead of `PRESET_NAMES_LIST`; `:1531` → `preset_names().join(", ")`.
  - `tests/cli/gate_suppress_baseline_differential.rs`: import `al_sem::engine::l5::detectors::default_detectors`; `inline_suppress_detectors` builds `names` as `default_detectors().into_iter().map(|d| d.name).collect()` (same 40 names, same order — P7, byte-identical).

- [ ] **Step 5: Run, expect PASS:** `cargo test -p al-sem --lib presets::` (T1, T2 and the kept preset tests green); `cargo test -p al-sem --test cli gate_suppress_baseline_differential::` green.

- [ ] **Step 6: Discrimination proofs:**
  - T1: no-flag branch returns `Ok(default_detectors().into_iter().take(39).collect())` → T1 fails (39 vs 40 names). Revert.
  - T2: add `"d99-fake"` to `PRESET_TRANSACTION_INTEGRITY` → T2 fails `d99-fake not registered (preset transaction-integrity)`, while `preset_resolves_transaction_integrity` stays green. Revert.

- [ ] **Step 7: Gate and commit.** `rustfmt --edition 2024` the three files; `scripts/ci-steps task` green.

```bash
git add src/engine/gate/presets.rs src/bin/alsem.rs tests/cli/gate_suppress_baseline_differential.rs
git commit -m "fix(gate): analyze's default set is the registry's; presets become one PRESETS table (#68)"
```

---

### Task 3: One shared entry point; five commands run the default set (Design 3, 4, 6) + T3

**Files:**
- Create: `tests/cli/cli_envelope_detector_set.rs`; Modify: `tests/cli/main.rs` (add `mod cli_envelope_detector_set;` after `mod cli_digest_temp_state;`)
- Modify: `src/engine/gate/run.rs:1025-1075`
- Modify: `src/engine/l5/digest_cli.rs` (`:21`, `:29`, `:41-81` the list, `:86-123`, `:1430` comment)
- Modify: `src/engine/l5/fingerprint_cli.rs:31-33,549-557`, `src/engine/gate/events.rs:28-31,459-471`, `src/engine/gate/policy/pipeline.rs:30-33,296-304`
- Modify: `tests/cli/cli_b_snapshot_differential.rs:37-38,51-…,85-96`

**Interfaces:**
- Consumes: `default_detectors()` (Task 1).
- Produces: `pub fn compute_analyzer_diagnostics(ws_path: &Path, resolved: &Model) -> Vec<Diagnostic>` (the `detectors` parameter is GONE), `pub fn compute_analyzer_diagnostics_cross_app(ws_path: &Path, base: &crate::engine::l4::capability_cone::R3a5CrossAppBase) -> Vec<Diagnostic>`; `analyzer_diagnostics_of_run` becomes private (`fn`).

- [ ] **Step 1: Write T3** — `tests/cli/cli_envelope_detector_set.rs`, exactly:

```rust
//! Issue #68 (T3): digest, prove, fingerprint, events and policy run the SAME
//! detector set that `alsem analyze` runs with no `--detector`/`--preset` — the
//! registry's `default_detectors()`.
//!
//! The observer is the SHIPPED binary's own Chrome trace (`ALSEM_TRACE=1`,
//! `ALSEM_TRACE_FILE=<per-case file>`, set on the child process only): every
//! detector the process runs opens one `detector.<name>` begin (`"ph":"B"`)
//! span (`src/engine/l5/registry.rs`). So a call site, a selector or a
//! `run.rs` body that runs a different set fails here, whichever layer it is
//! in. `detector.result` is an instant event (`"ph":"i"`) and `detectors.total`
//! does not carry the `detector.` prefix, so neither is counted.
//!
//! Per case: (1) exit 0; (2) the trace is a JSON array; (3) every
//! `detector.*` begin name is a REGISTERED detector; (4) each name occurs
//! exactly once; (5) the name set equals `default_detectors()`'s names.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;

use al_sem::engine::l5::detectors::{default_detectors, registered_detectors};
use serde_json::Value;

fn corpus(ws: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("r0-corpus")
        .join(ws)
        .to_string_lossy()
        .into_owned()
}

/// Run `alsem <args>` with tracing on, assert exit 0, and return the multiset
/// of `detector.*` begin-span names (prefix stripped) -> occurrence count.
fn detector_spans(case: &str, args: &[String]) -> BTreeMap<String, usize> {
    let dir = tempfile::tempdir().expect("tempdir");
    let trace = dir.path().join("trace.json");
    let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(args)
        .env("ALSEM_TRACE", "1")
        .env("ALSEM_TRACE_FILE", &trace)
        .output()
        .expect("spawn alsem");
    assert!(
        out.status.success(),
        "[{case}] alsem {args:?} exited {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&trace)
        .unwrap_or_else(|e| panic!("[{case}] no trace at {}: {e}", trace.display()));
    let events: Vec<Value> = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("[{case}] trace is not a JSON array: {e}"));
    let mut spans: BTreeMap<String, usize> = BTreeMap::new();
    for ev in &events {
        if ev.get("ph").and_then(Value::as_str) != Some("B") {
            continue;
        }
        let Some(name) = ev.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(det) = name.strip_prefix("detector.") {
            *spans.entry(det.to_string()).or_default() += 1;
        }
    }
    spans
}

fn assert_runs_default_set(case: &str, args: &[&str]) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let spans = detector_spans(case, &args);

    let registered: BTreeSet<String> = registered_detectors().into_iter().map(|d| d.name).collect();
    let unknown: Vec<&String> = spans.keys().filter(|n| !registered.contains(*n)).collect();
    assert!(
        unknown.is_empty(),
        "[{case}] ran unregistered detector(s): {unknown:?}"
    );

    let repeated: Vec<(&String, &usize)> = spans.iter().filter(|(_, c)| **c != 1).collect();
    assert!(
        repeated.is_empty(),
        "[{case}] detector(s) not run exactly once: {repeated:?}"
    );

    let expected: BTreeSet<String> = default_detectors().into_iter().map(|d| d.name).collect();
    let actual: BTreeSet<String> = spans.into_keys().collect();
    let missing: Vec<&String> = expected.difference(&actual).collect();
    let extra: Vec<&String> = actual.difference(&expected).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "[{case}] ran {} detectors, the analyze default set has {}\nmissing: {missing:?}\nextra: {extra:?}",
        actual.len(),
        expected.len()
    );
}

const WS: &str = "ws-d1-multi-caller";

#[test]
fn analyze_cross_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set("analyze", &["analyze", &ws, "--format", "json"]);
}

#[test]
fn analyze_single_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "analyze --single-app",
        &["analyze", &ws, "--format", "json", "--single-app"],
    );
}

#[test]
fn digest_cross_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "digest",
        &["digest", &ws, "--file", "src/D1MultiCaller.al", "--deterministic"],
    );
}

#[test]
fn digest_single_app_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "digest --single-app",
        &[
            "digest",
            &ws,
            "--file",
            "src/D1MultiCaller.al",
            "--deterministic",
            "--single-app",
        ],
    );
}

#[test]
fn prove_runs_default_set() {
    let ws = corpus("ws-d34");
    assert_runs_default_set(
        "prove",
        &[
            "prove",
            &ws,
            "DirectCommitInLoop",
            "may-commit",
            "--deterministic",
        ],
    );
}

#[test]
fn fingerprint_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "fingerprint",
        &["fingerprint", &ws, "--format", "json", "--deterministic"],
    );
}

#[test]
fn events_fanout_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "events fanout",
        &["events", "fanout", &ws, "--format", "json", "--deterministic"],
    );
}

#[test]
fn events_chains_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "events chains",
        &["events", "chains", &ws, "--format", "json", "--deterministic"],
    );
}

#[test]
fn policy_check_runs_default_set() {
    let ws = corpus(WS);
    assert_runs_default_set(
        "policy check",
        &["policy", "check", &ws, "--format", "json", "--deterministic"],
    );
}
```

- [ ] **Step 2: Red run against the OLD call sites** (Acceptance mutation (d)): `cargo test -p al-sem --test cli cli_envelope_detector_set:: > t3_red.log 2>&1`; grep it. Expected (measured 2026-10-14 on Task 1's split, ledger Timeline): `2 passed; 7 failed`; both analyze cases `ok`; each of digest, digest --single-app, prove, fingerprint, events fanout, events chains, policy check fails with
  `[<case>] ran 14 detectors, the analyze default set has 40` / `missing: [27 names, "d16-obsolete-routine-call" … "d60-upgrade-loop-should-be-datatransfer"]` / `extra: ["d8-commit-in-transaction"]`.
  Do NOT commit here (the pre-commit hook runs `--test cli`).

- [ ] **Step 3: `run.rs`.** Replace `compute_analyzer_diagnostics` and make the projection private:
  ```rust
  pub fn compute_analyzer_diagnostics(
      ws_path: &Path,
      resolved: &crate::program::model::workspace::Model,
  ) -> Vec<crate::engine::l5::registry::Diagnostic> {
      let run = run_detectors(resolved, &crate::engine::l5::detectors::default_detectors());
      analyzer_diagnostics_of_run(ws_path, resolved, &run)
  }

  /// [`compute_analyzer_diagnostics`] for a cross-app base (`alsem digest`'s
  /// default mode, #66): the default set run by the cross-app runner, the
  /// combination `alsem analyze` runs by default.
  pub fn compute_analyzer_diagnostics_cross_app(
      ws_path: &Path,
      base: &crate::engine::l4::capability_cone::R3a5CrossAppBase,
  ) -> Vec<crate::engine::l5::registry::Diagnostic> {
      let run = crate::engine::l5::registry::run_detectors_cross_app(
          base,
          &crate::engine::l5::detectors::default_detectors(),
          true,
      );
      analyzer_diagnostics_of_run(ws_path, &base.resolved, &run)
  }
  ```
  `pub fn analyzer_diagnostics_of_run` → `fn analyzer_diagnostics_of_run`; update its doc (callers: the two functions above). In `compute_analyzer_diagnostics`' doc, say it runs the registry's default set itself (no detector parameter, so no caller outside `run.rs` can pick another set). `run_analyze_with_exit` (`:452`) is unchanged.

- [ ] **Step 4: Call sites.**
  - `digest_cli.rs`: delete `DEFAULT_DETECTOR_NAMES` (`:41-81` incl. its comment) and the local `fn default_detectors()`; `build_envelope_diagnostics_json` → `diagnostics_json(&compute_analyzer_diagnostics(workspace, resolved))`; `build_envelope_diagnostics_json_cross_app` → `diagnostics_json(&compute_analyzer_diagnostics_cross_app(workspace, base))`; import `{compute_analyzer_diagnostics, compute_analyzer_diagnostics_cross_app}` instead of `analyzer_diagnostics_of_run`; drop `registered_detectors` import if unused. Doc at `:88` "over the DEFAULT_DETECTOR_NAMES set" → "over the registry's default detector set"; comment at `:1430` "same 34-detector set" → "the analyze default detector set".
  - `fingerprint_cli.rs::analyzer_diags_for`, `events.rs::analyze_workspace_diagnostic_lines`, `policy/pipeline.rs::analyze_workspace_diagnostic_lines`: delete the `default_detectors` filter; call `compute_analyzer_diagnostics(workspace, resolved)`; remove the `digest_cli::DEFAULT_DETECTOR_NAMES` and now-unused `registered_detectors` imports.
  - `tests/cli/cli_b_snapshot_differential.rs`: delete its `DEFAULT_DETECTOR_NAMES` copy and the filter in `envelope_diagnostics`; call `compute_analyzer_diagnostics(ws_path, resolved)`; drop `registered_detectors` import if unused.
  - `prove.rs` is unchanged (it calls `build_envelope_diagnostics_json`).

- [ ] **Step 5: Green run:** `cargo test -p al-sem --test cli cli_envelope_detector_set::` → `9 passed`. Also `cargo test -p al-sem --test cli cli_b_` green (snapshot/digest/prove/fingerprint goldens unchanged).

- [ ] **Step 6: Discrimination proofs** (Acceptance matrix T3; break with Edit, run, revert, run):
  - (a) `run_analyze_with_exit`: after `let detectors = resolve_analyze_detectors(...)?;` add `let detectors: Vec<_> = detectors.into_iter().filter(|d| d.name != "d60-upgrade-loop-should-be-datatransfer").collect();` → both analyze cases fail naming d60; `presets::tests::analyze_default_is_registry_default` (T1) stays green.
  - (b) `compute_analyzer_diagnostics` body: run `default_detectors()` minus d60 → prove, fingerprint, events ×2, policy, digest `--single-app` fail; analyze and digest cross-app stay green.
  - (c) `compute_analyzer_diagnostics_cross_app` body: same filter → only digest cross-app fails.
  - (d) is Step 2's red run.

- [ ] **Step 7: Gate and commit.** `rustfmt --edition 2024` every touched file; `scripts/ci-steps task` green (it runs every `check-goldens` target; zero goldens may move).

```bash
git add tests/cli/cli_envelope_detector_set.rs tests/cli/main.rs src/engine/gate/run.rs src/engine/l5/digest_cli.rs src/engine/l5/fingerprint_cli.rs src/engine/gate/events.rs src/engine/gate/policy/pipeline.rs tests/cli/cli_b_snapshot_differential.rs
git commit -m "fix(l5): digest, prove, fingerprint, events and policy run analyze's default detector set (#68)"
```

---

### Task 4: Goldens, CHANGELOG, docs, cost measurement (Design 7, 8, 9, A3)

**Files:**
- Modify: `CHANGELOG.md` (`## [Unreleased]`)
- Modify: `docs/2026-10-04-b3-code-map.md:66`
- Modify: `.agent/issue-68/ledger.md` (Gate results, Discrimination proofs)

- [ ] **Step 1: Leftover-name check:** `grep -rn "DEFAULT_DETECTOR_NAMES\|OPT_IN_DETECTOR_NAMES\|PRESET_NAMES_LIST" src tests` → only `tests/cli/cli_a_*_differential.rs` (Discovery D2, out of scope).

- [ ] **Step 2: Golden license (A3):** `scripts/check-goldens > goldens.log 2>&1` (NO `--regen`); grep for failures. Expected: all 8 targets green, zero bytes moved. Any diff STOPS this task for triage. Record the result in the ledger's Gate results.

- [ ] **Step 3: CHANGELOG** under `## [Unreleased]`:
  - `### Fixed`: "`alsem digest`, `prove`, `fingerprint`, `events` and `policy check` ran 14 detectors (13 of `analyze`'s default set plus opt-in d8), because 20 names in a list copied at port time matched no registered detector. They now run `analyze`'s default detector set from the registry, each in its existing model mode. Diagnostics output is unchanged on every golden (#68)."
  - `### Changed`: "`gate::presets::PRESET_NAMES_LIST` is replaced by `preset_names()`, and presets are one `PRESETS` table. `DEFAULT_DETECTOR_NAMES` and `OPT_IN_DETECTOR_NAMES` (presets and digest_cli) are removed; the default set is `detectors::default_detectors()`. To demote a detector, move its entry from `default_detectors()` to `opt_in_detectors()`, then update T1b's opt-in literal AND its default count (`detectors::tests::default_and_opt_in_membership` asserts 40). `compute_analyzer_diagnostics` no longer takes a detector list; `compute_analyzer_diagnostics_cross_app` is new."

- [ ] **Step 4: Docs:** `docs/2026-10-04-b3-code-map.md:66` — replace `` `DEFAULT_DETECTOR_NAMES` (`presets.rs:16`) `` with `` `default_detectors()` (`src/engine/l5/detectors/mod.rs`) ``.

- [ ] **Step 5: Cost (Design 9):** build `cargo build --profile release-fast --bin alsem`; re-run the P12 invocations with the same `measure.ps1` (scratch `68/measure.ps1`) on `U:/Git/DO-cdo-baseline/Cloud`, 2 runs each, `CDO_WS` unset: digest (cross-app, `--file src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al`), prove (`IssueReminder may-commit`), fingerprint, events fanout, events chains, policy check (json). Record before (P12) and after in Gate results. Not pass/fail; a regression beyond P12's noise band is reported to the owner.

- [ ] **Step 6: Gate and commit.** `scripts/ci-steps task` green.

```bash
git add CHANGELOG.md docs/2026-10-04-b3-code-map.md
git commit -m "docs: CHANGELOG and code map for the registry default detector set (#68)"
```

(The ledger and `findings.json` under `.agent/issue-68/` are updated per the issue flow, not staged here unless the flow says so.)

---

## Not in this plan

- `.claude/commands/triage-wave.md:31-33` (Discovery D4): owner action, the flow may not edit `.claude/`.
- Branch gate (`scripts/ci-steps all`, `scripts/cdo-gate`), panels, PR: later steps of the issue flow.
