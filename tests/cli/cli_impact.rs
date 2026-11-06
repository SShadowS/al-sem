//! `alsem impact` (agent-tasks design §8, milestone B1), through the SHIPPED
//! binary: input accounting first, the comparison tuple, per changed routine
//! its callers, entries, current effects and tests.
//!
//! Fixture: `tests/fixtures/test-universe` (a production app with a sibling
//! test app), copied into a temporary folder holding a `.git` marker so test
//! discovery searches only the fixture's apps (and, for `--base`, a real git
//! repository made here). Expected values are hand-stated from the fixture
//! text: `TU Core::Post` calls `Calc` and `Leaf` and modifies the item;
//! `PostUpdatesQty` (in `TU Core Tests`) calls `Post`.
//!
//! Byte goldens: `tests/cli-output-goldens/impact/`. Refresh:
//! `REGEN_TEMP_GOLDENS=1 cargo test --test cli cli_impact::` — then INSPECT
//! the diff.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::regen;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

/// The `test-universe` fixture in a temporary folder with a `.git` marker.
fn universe() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&repo_root().join("tests/fixtures/test-universe"), t.path());
    std::fs::create_dir(t.path().join(".git")).unwrap();
    t
}

/// `alsem impact <ws> <args>` with `stdin`: (stdout, stderr, exit).
fn impact(ws: &Path, args: &[&str], stdin: Option<&str>) -> (String, String, i32) {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .arg("impact")
        .arg(ws)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn alsem");
    {
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8(out.stdout).expect("utf8"),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn json(ws: &Path, args: &[&str], stdin: Option<&str>) -> Value {
    let mut a = args.to_vec();
    a.extend(["--format", "json"]);
    let (out, err, code) = impact(ws, &a, stdin);
    assert_eq!(code, 0, "{out}{err}");
    serde_json::from_str(&out).expect("al-sem.impact JSON")
}

/// The golden check: `text` with the temporary root spelled `<root>` and
/// test-universe keys spelled `<key>`.
fn golden(name: &str, text: &str, root: &Path) {
    let mut text = text.replace(&root.to_string_lossy().to_string(), "<root>");
    let mut from = 0;
    while let Some(i) = text[from..].find("\"tests:").map(|i| from + i + 7) {
        let hex = text[i..]
            .chars()
            .take_while(char::is_ascii_hexdigit)
            .count();
        if hex == 16 {
            text.replace_range(i..i + 16, "<key>");
        }
        from = i;
    }
    let path = repo_root()
        .join("tests/cli-output-goldens/impact")
        .join(name);
    if regen::regen_mode() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        regen::write_golden(&path, &text).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden {}: {e} (REGEN_TEMP_GOLDENS=1 mints it)",
            path.display()
        )
    });
    assert_eq!(text, want, "golden {name} moved");
}

/// A diff of `App/src/Core.al` (git-top-relative): the `Qty` field's line,
/// `Calc`'s body, and a non-AL file. Its context matches the fixture.
const DIFF: &str = "diff --git a/App/src/Core.al b/App/src/Core.al
--- a/App/src/Core.al
+++ b/App/src/Core.al
@@ -8,3 +8,3 @@
         field(2; Qty; Decimal)
-        {   // quantity
+        {
         }
@@ -24,3 +24,3 @@
     begin
-        Item.Qty := Item.Qty + 2;
+        Item.Qty := Item.Qty + 1;
     end;
diff --git a/App/notes.md b/App/notes.md
--- a/App/notes.md
+++ b/App/notes.md
@@ -1 +1 @@
-a
+b
";

/// B-1 through the binary: the accounting is the line after the
/// comparison, before any routine, with every bucket summing to the changed
/// lines; the field line and the body line each in their bucket; the non-AL
/// file unmapped; `Calc` resolved with its caller `Post`, its tests, and
/// "current effects; no before/after".
#[test]
fn a_diff_is_accounted_first_then_each_routine() {
    let t = universe();
    let ws = t.path().join("App");
    let (human, err, code) = impact(&ws, &["--diff", "-"], Some(DIFF));
    assert_eq!(code, 0, "{err}");
    let lines: Vec<&str> = human.lines().collect();
    assert!(
        lines[0].starts_with("impact of a diff (2 files)"),
        "{human}"
    );
    assert!(
        lines[1].starts_with("input: 1 changed routine resolved"),
        "{human}"
    );
    assert!(
        human.contains("current effects; no before/after"),
        "{human}"
    );
    golden("diff.human.txt", &human, t.path());
    let v = json(&ws, &["--diff", "-"], Some(DIFF));
    let tot = &v["input"]["accounting"]["totals"];
    assert_eq!(tot["total"], 6);
    assert_eq!(tot["routineBody"], 2);
    assert_eq!(tot["objectDeclaration"]["byKind"]["field"], 2);
    assert_eq!(tot["unmapped"]["byReason"]["not-al"], 2);
    let r = &v["roots"][0];
    assert_eq!(r["display"], "Codeunit TU Core::Calc");
    assert_eq!(r["status"], "changed");
    assert_eq!(r["up"]["callers"]["count"], 1);
    assert_eq!(
        r["up"]["callers"]["first"][0]["display"],
        "Codeunit TU Core::Post"
    );
    assert_eq!(r["tests"]["status"], "reached");
    assert!(v["input"]["mismatches"].as_array().unwrap().is_empty());
    let (j, _, _) = impact(&ws, &["--diff", "-", "--format", "json"], Some(DIFF));
    golden("diff.json", &j, t.path());
}

/// Output validation F25: without a base, a routine's status says what the
/// diff shows. The diff has c20's shape for `Audited`: it adds `Untested`'s
/// `end;`, a blank line, `Audited`'s header and `begin`, and aligns
/// `Audited`'s own `end;` with `Untested`'s old one as context (the last
/// context line is the blank line after `Audited`, before `Logged`, which the
/// table-event fixture added; it was `Core`'s closing `}` before). It also
/// rewrites `Leaf`'s header line (a `-` and a `+` line). `Audited` is "new in
/// this diff" (its header is added, none of its lines removed), `Leaf`
/// "signature lines changed in the diff" (its header is added too, but a
/// line is removed), and neither is called "signature changed" (that needs a
/// base).
///
/// Discrimination (2026-11-06): `added_in_diff` always false (the status
/// falls to the signature rule) fails at "Audited is new in this diff";
/// dropping its `removed_near` check fails at the `Leaf` status (the `-`
/// header line sits just before `Leaf`'s span and is counted as removed
/// routine text, so only `removed_near` sees it); restored, passes.
#[test]
fn without_a_base_the_status_says_what_the_diff_shows() {
    let t = universe();
    let ws = t.path().join("App");
    let diff = "diff --git a/App/src/Core.al b/App/src/Core.al
--- a/App/src/Core.al
+++ b/App/src/Core.al
@@ -28,1 +28,1 @@
-    procedure Leaf(X: Integer)
+    procedure Leaf()
@@ -32,4 +32,8 @@
     procedure Untested()
     begin
+    end;
+
+    procedure Audited()
+    begin
     end;

";
    let v = json(&ws, &["--diff", "-", "--no-tests"], Some(diff));
    let status = |name: &str| -> String {
        v["roots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["display"] == format!("Codeunit TU Core::{name}"))
            .unwrap_or_else(|| panic!("no root {name}: {v:#}"))["status"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        status("Audited"),
        "addedInDiff",
        "Audited is new in this diff"
    );
    assert_eq!(status("Leaf"), "signatureLinesChanged");
    let (human, err, code) = impact(&ws, &["--diff", "-", "--no-tests"], Some(diff));
    assert_eq!(code, 0, "{err}");
    assert!(
        human.contains(
            "Codeunit TU Core::Audited (ws:src/Core.al:36) — new in this diff (its header is an added line, none of its lines removed)"
        ),
        "{human}"
    );
    assert!(
        human.contains("— signature lines changed in the diff"),
        "{human}"
    );
    assert!(!human.contains("— signature changed"), "{human}");
}

/// Output validation F24: a diff that changes no routine (only the `Qty`
/// field's line) gets a complete answer, "0 changed routines", and exits 0;
/// a `--routine` selector that names nothing is no answer and exits 2.
///
/// Discrimination (2026-11-06): restoring `exit = if roots.is_empty() { 2 }`
/// in `impact/render.rs` fails at "a complete nothing-changed answer exits
/// 0"; `input_unresolved` given no diagnostic (every empty answer exits 0)
/// fails at "an unresolved selector is no answer"; restored, passes.
#[test]
fn a_change_with_no_routine_is_a_complete_answer() {
    let t = universe();
    let ws = t.path().join("App");
    let field_only = "diff --git a/App/src/Core.al b/App/src/Core.al
--- a/App/src/Core.al
+++ b/App/src/Core.al
@@ -8,3 +8,3 @@
         field(2; Qty; Decimal)
-        {   // quantity
+        {
         }
";
    let (human, err, code) = impact(&ws, &["--diff", "-"], Some(field_only));
    assert!(
        human.contains("input: 0 changed routines resolved"),
        "precondition: no routine changed: {human}{err}"
    );
    assert_eq!(
        code, 0,
        "a complete nothing-changed answer exits 0: {human}{err}"
    );
    let (human, err, code) = impact(&ws, &["--routine", "TU Core.NoSuchRoutine"], None);
    assert_eq!(code, 2, "an unresolved selector is no answer: {human}{err}");
}

/// The main part streams a large effects read in batches
/// (`AL_SEM_IMPACT_ROOT_BATCH=1`: one root per batch, so every one of
/// `Core.al`'s six routines makes a batch, `digest.stream_batches`), and each
/// root's current effects equal the unstreamed read's, which reads them
/// whole (no batch).
///
/// Break: `stream_batch` never streams → no batch (0 against 6).
#[test]
fn a_large_main_part_streams_its_effects_with_the_same_totals() {
    let t = universe();
    let ws = t.path().join("App");
    let ws_s = ws.to_string_lossy().to_string();
    let args = [
        "impact",
        ws_s.as_str(),
        "--file",
        "src/Core.al",
        "--no-tests",
        "--format",
        "json",
    ];
    let run = |env: &[(&str, &str)]| {
        let (out, counters) = crate::cli_build_counters::spawn_traced(&args, env);
        assert!(out.status.success(), "{out:?}");
        let v: Value = serde_json::from_slice(&out.stdout).unwrap();
        let effects: Vec<(String, Value)> = v["roots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["stableId"].as_str().unwrap().into(), r["effects"].clone()))
            .collect();
        (
            effects,
            crate::cli_build_counters::count(&counters, "digest.stream_batches"),
        )
    };
    let (whole, none) = run(&[]);
    // Eight since `TU Core::Logged` and `TU Log Subscribers::OnLogInserted`
    // (the unraised platform table event of C-6) joined the fixture.
    assert_eq!(whole.len(), 8, "precondition: eight routines in Core.al");
    // `Post`'s `Item.Modify()` on its `var` parameter.
    assert!(
        whole
            .iter()
            .any(|(_, e)| e["callerDependentTables"].as_u64() > Some(0)),
        "precondition: a root writes a table: {whole:?}"
    );
    assert_eq!(none, 0, "eight roots are read whole by default");
    let (streamed, batches) = run(&[("AL_SEM_IMPACT_ROOT_BATCH", "1")]);
    assert_eq!(batches, 8, "one batch per root");
    assert_eq!(streamed, whole);
}

/// B-2: a root's direct callers are explain's `callers:` (R0-3's definition,
/// over the same sited graph).
///
/// DISCRIMINATION (2026-11-05): counting every reversed site's `from` as a
/// caller in `callers_json` (instead of `direct_callers`) counts the
/// subscriber's publisher: `OnItemModified` gets 1 caller against explain's
/// 0: fails; restored, passes. (The three `TU Core` roots alone did not
/// discriminate: their only reversed sites are calls.)
#[test]
fn up_callers_equal_explains_callers() {
    let t = universe();
    let ws = t.path().join("App");
    for r in [
        "TU Core::Calc",
        "TU Core::Post",
        "TU Core::Leaf",
        // A subscriber: its publisher dispatches it, it is no caller.
        "TU Subscribers::OnItemModified",
    ] {
        let v = json(&ws, &["--routine", r, "--no-tests"], None);
        let out = Command::new(env!("CARGO_BIN_EXE_alsem"))
            .args(["explain"])
            .arg(&ws)
            .args(["--routine", r, "--format", "json"])
            .output()
            .unwrap();
        let e: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            v["roots"][0]["up"]["callers"]["count"], e["cards"][0]["summary"]["callers"],
            "{r}"
        );
        assert!(
            v["tests"]["notComputed"]
                .as_str()
                .unwrap()
                .contains("not asked"),
            "{v:#}"
        );
    }
}

/// B-3b: a diff whose new side differs from the working tree is reported,
/// and its routines are marked position-uncertain.
///
/// DISCRIMINATION (2026-11-05): `uncertain` always false in `MainJob`
/// leaves `Calc` `changed`: fails; restored, passes.
#[test]
fn a_diff_that_does_not_match_the_working_tree_marks_its_routines() {
    let t = universe();
    let ws = t.path().join("App");
    let stale = DIFF.replace(
        "+        Item.Qty := Item.Qty + 1;",
        "+        Item.Qty := Item.Qty + 3;",
    );
    assert_ne!(stale, DIFF);
    let v = json(&ws, &["--diff", "-", "--no-tests"], Some(&stale));
    assert_eq!(v["input"]["mismatches"][0]["file"], "App/src/Core.al");
    assert_eq!(v["roots"][0]["status"], "positionUncertain");
    let (human, _, _) = impact(&ws, &["--diff", "-", "--no-tests"], Some(&stale));
    assert!(
        human.contains("the diff does not match the working tree: 1 file (App/src/Core.al:25)"),
        "{human}"
    );
}

/// `git <args>` in the test's own repository (`tests/common/git.rs`: never a
/// hook's `GIT_DIR`, never the machine's config).
fn git(dir: &Path, args: &[&str]) {
    crate::git::run(dir, args);
}

/// B-3 through the binary: `--base HEAD` in a real repository: the merge
/// base is HEAD, an untracked `.al` file is listed and its routine resolved,
/// an unstaged edit's routine resolved too.
#[test]
fn base_lists_and_resolves_untracked_and_unstaged() {
    let t = tempfile::tempdir().unwrap();
    copy_dir(&repo_root().join("tests/fixtures/test-universe"), t.path());
    git(t.path(), &["init", "-q", "-b", "main"]);
    git(t.path(), &["add", "."]);
    git(t.path(), &["commit", "-q", "-m", "fixture"]);
    let ws = t.path().join("App");
    std::fs::write(
        ws.join("src/Extra.al"),
        "codeunit 50803 \"TU Extra\"\n{\n    procedure Fresh()\n    begin\n    end;\n}\n",
    )
    .unwrap();
    let core = std::fs::read_to_string(ws.join("src/Core.al")).unwrap();
    std::fs::write(
        ws.join("src/Core.al"),
        core.replace("Item.Qty := Item.Qty + 1;", "Item.Qty := Item.Qty + 2;"),
    )
    .unwrap();
    let v = json(&ws, &["--base", "HEAD", "--no-tests"], None);
    let c = &v["comparison"];
    assert_eq!(c["source"], "base");
    assert_eq!(c["mergeBase"], c["resolvedCommit"]);
    assert_eq!(c["comparisonBase"], c["mergeBase"]);
    assert_eq!(c["layers"]["untracked"][0], "App/src/Extra.al");
    assert_eq!(c["layers"]["unstaged"][0], "App/src/Core.al");
    let names: Vec<&str> = v["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["display"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Codeunit TU Extra::Fresh"), "{names:?}");
    assert!(names.contains(&"Codeunit TU Core::Calc"), "{names:?}");
}

/// Spec §12.1's refused combinations: two modes, `--diff-root` without
/// `--diff`, the base-only switches without `--base`, `--no-delta` on a
/// base-and-diff before/after (`--base` with `--diff` is B2's mode, tested
/// in `cli_impact_base`).
#[test]
fn refused_combinations() {
    let t = universe();
    let ws = t.path().join("App");
    for (args, want) in [
        (
            vec!["--base", "HEAD", "--diff", "-", "--no-delta"],
            "--no-delta drops it",
        ),
        (vec!["--routine", "X", "--no-delta"], "go with --base"),
        (vec!["--routine", "X", "--diff", "-"], "exactly one of"),
        (vec!["--routine", "X", "--file", "y.al"], "exactly one of"),
        (
            vec!["--routine", "X", "--diff-root", "."],
            "--diff-root goes with --diff",
        ),
        (vec![], "exactly one of"),
    ] {
        let (_, err, code) = impact(&ws, &args, Some(""));
        assert_ne!(code, 0, "{args:?}");
        assert!(err.contains(want), "{args:?}: {err}");
    }
}

/// B1's CDO acceptance (`scripts/cdo-gate`), on the pinned baseline:
/// - the 94-file range `bc3ccb18~5..bc3ccb18` (the diff made with `git diff`,
///   read only): 699 routines, 12,395 changed lines in buckets that sum, every
///   one of digest's 93 `hunk-outside-routines` named among the hunks that
///   touch no routine, no diff/working-tree mismatch (the range ends at the
///   checked-out commit; 54 of its files add a byte-order mark), and the
///   human answer bounded under the 40 KiB inline cap with its fold;
/// - `CDO Telemetry::UpdateLastUpload`: its caller `QueueUpload` and the 3
///   `CDO Telemetry Tests` tests, local, 1 hop.
///
/// Run by `output_cdo::cdo_impact_ratchets` (the module `cdo-gate` runs).
pub(crate) fn cdo_impact_ratchets() {
    let Some(ws) = crate::cdo::cdo_ws_or_enforce() else {
        return;
    };
    let top = ws.parent().unwrap().to_path_buf();
    let out = crate::git::reader(&top)
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "bc3ccb18~5",
            "bc3ccb18",
            "--",
            "Cloud",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let diff = String::from_utf8(out.stdout).unwrap();
    let root = top.to_string_lossy().to_string();
    let v = json(&ws, &["--diff", "-", "--diff-root", &root], Some(&diff));
    let i = &v["input"];
    assert_eq!(i["rootsResolved"], 699);
    assert_eq!(i["accounting"]["changedLines"], 12395);
    assert_eq!(i["accounting"]["totals"]["total"], 12395);
    for f in i["accounting"]["files"].as_array().unwrap() {
        assert_eq!(
            f["buckets"]["total"].as_u64().unwrap(),
            f["added"].as_u64().unwrap() + f["removed"].as_u64().unwrap(),
            "{}",
            f["path"]
        );
    }
    let named: std::collections::BTreeSet<(String, i64, i64)> = i["hunksOutsideRoutines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["file"].as_str().unwrap().to_string(),
                h["startLine"].as_i64().unwrap(),
                h["endLine"].as_i64().unwrap(),
            )
        })
        .collect();
    let digest: Vec<&Value> = i["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["kind"] == "hunk-outside-routines")
        .collect();
    assert_eq!(digest.len(), 93);
    for d in digest {
        let k = (
            d["file"].as_str().unwrap().to_string(),
            d["startLine"].as_i64().unwrap(),
            d["endLine"].as_i64().unwrap(),
        );
        assert!(named.contains(&k), "{k:?} not named");
    }
    assert!(
        i["mismatches"].as_array().unwrap().is_empty(),
        "{}",
        i["mismatches"]
    );
    let (human, _, code) = impact(&ws, &["--diff", "-", "--diff-root", &root], Some(&diff));
    assert_eq!(code, 0);
    assert!(human.len() <= 40 * 1024, "{} bytes", human.len());
    // +681 since 2026-11-06 (was +679): each `reached from` list now names
    // its open-only count and non-local witnesses' tiers (round-1 c20), so
    // two fewer routines fit the 40 KiB budget. +682 on the integration with
    // F1 and field-writers (`integrate/outval-r1-fixes`, each alone kept
    // +679): the text before the cut grew, so one more routine falls past
    // the budget.
    assert!(human.contains("  … +682 routines"), "{human}");

    let v = json(&ws, &["--routine", "CDO Telemetry::UpdateLastUpload"], None);
    let r = &v["roots"][0];
    assert_eq!(
        r["up"]["callers"]["first"][0]["display"], "Codeunit CDO Telemetry::QueueUpload",
        "{r:#}"
    );
    assert_eq!(r["tests"]["status"], "reached");
    assert_eq!(r["tests"]["local"], 3);
    assert_eq!(r["tests"]["nearest"]["hops"], 1);
    let names: Vec<&str> = v["tests"]["runFirst"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["codeunit"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["CDO Telemetry Tests"; 3]);
}
