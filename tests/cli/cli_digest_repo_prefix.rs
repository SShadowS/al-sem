//! #77: `alsem digest --diff`, `--file` and the `--changed` file lists accept a
//! path written relative to a directory that contains the workspace (a `git diff`
//! run in `DO/` for the workspace `DO/Cloud`). Discovered: a path that names a
//! workspace file as written wins; otherwise each repository prefix of the
//! workspace (every `.git` at or above it, innermost first) is stripped. With
//! `--diff-prefix`, every path is repository-relative and only that prefix is
//! stripped.
//!
//! Every case spawns `CARGO_BIN_EXE_alsem digest`. Unless it says otherwise, the
//! workspace is a copy of `ws-d8-commit-in-tx` at `<tmp>/repo/Cloud` with
//! `<tmp>/repo/.git/` created. Every exit-2 case parses stdout as a digest
//! document first, so a clap usage error (also exit 2) cannot pass one.
//! Ledger: `.agent/issue-77/ledger.md` (Design).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const VERSION_OVERRIDE: &str = "cli-repo-prefix-v1";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Copy `ws-d8-commit-in-tx` into `ws` (LF).
fn d8_copy(ws: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d8-commit-in-tx");
    for rel in ["app.json", "src/posting.al", "src/subscriber.al"] {
        let text = std::fs::read_to_string(src.join(rel))
            .unwrap()
            .replace('\r', "");
        write(&ws.join(rel), &text);
    }
}

/// `<tmp>/repo/Cloud` holding the d8 copy, with `<tmp>/repo/.git/`.
fn d8_repo(tmp: &Path) -> PathBuf {
    std::fs::create_dir_all(tmp.join("repo/.git")).unwrap();
    let ws = tmp.join("repo/Cloud");
    d8_copy(&ws);
    ws
}

/// Is there a `.git` entry at or above `dir`?
fn git_at_or_above(dir: &Path) -> Option<PathBuf> {
    let canon = std::fs::canonicalize(dir).unwrap();
    canon
        .ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

/// A second codeunit for `<ws>/Cloud/src/posting.al`: its own number, name and
/// procedure, which spans line 8 (lines 3-10).
const INNER_POSTING: &str = "codeunit 50190 InnerCloudPosting
{
    procedure InnerPost()
    var
        X: Integer;
    begin
        X := 1;
        X := 2;
        X := 3;
    end;
}
";

/// `alsem digest <ws> <args> --deterministic --format json` -> (doc, exit code).
/// A `diff` is written next to the workspace and passed as `--diff <path>`.
fn digest(ws: &Path, args: &[&str], diff: Option<&str>) -> (Value, i32) {
    let out = run(ws, args, diff);
    let stdout = String::from_utf8_lossy(&out.stdout).replace('\r', "");
    let doc: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: stdout={stdout} stderr={}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (doc, out.status.code().unwrap_or(-1))
}

fn run(ws: &Path, args: &[&str], diff: Option<&str>) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.arg("digest").arg(ws).args(args);
    let diff_dir = tempfile::tempdir().unwrap();
    if let Some(d) = diff {
        let p = diff_dir.path().join("change.diff");
        std::fs::write(&p, d).unwrap();
        cmd.arg("--diff").arg(&p);
    }
    cmd.args(["--deterministic", "--format", "json"])
        .env("ALCH_DRIVER_VERSION_OVERRIDE", VERSION_OVERRIDE)
        .output()
        .expect("spawn alsem")
}

/// The sorted display names of the digest entries (one per root).
fn roots(doc: &Value) -> Vec<String> {
    let mut r: Vec<String> = doc["payload"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["routine"]["display"].as_str().unwrap().to_string())
        .collect();
    r.sort();
    r
}

/// The changed-roots diagnostics as `(kind, file)`.
fn diags(doc: &Value) -> Vec<(String, String)> {
    doc["payload"]["changedRootsDiagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["kind"].as_str().unwrap().to_string(),
                d["file"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn has_diag(doc: &Value, kind: &str, file: &str) -> bool {
    diags(doc).iter().any(|(k, f)| k == kind && f == file)
}

/// A `diff --git` record changing line 8 of `path`.
fn line8(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -8,1 +8,1 @@\n-        Header.Get('002');\n+        Header.Get('001');\n"
    )
}

/// A `diff --git` record of `path` whose hunk header is malformed (#72).
fn malformed(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ garbage @@\n-        Header.Get('002');\n+        Header.Get('001');\n"
    )
}

/// T1. A diff made at the enclosing repo's top level resolves like the same
/// diff written workspace-relative.
/// Mutation: drop step 2 (no prefix stripping) -> exit 2, unmatched.
#[test]
fn diff_from_the_repo_root_resolves_like_a_workspace_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    let (base, code) = digest(&ws, &[], Some(&line8("src/posting.al")));
    assert_eq!(code, 0, "precondition: {base}");
    assert_eq!(roots(&base), vec!["PostSalesDoc"], "precondition: {base}");
    // A workspace-relative path uses no prefix: the field is omitted.
    assert!(
        base["payload"]["changed"].get("pathPrefixesUsed").is_none(),
        "{base}"
    );

    let (doc, code) = digest(&ws, &[], Some(&line8("Cloud/src/posting.al")));
    assert!(
        !diags(&doc).iter().any(|(k, _)| k == "diff-file-unmatched"),
        "{doc}"
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), roots(&base), "{doc}");
    assert_eq!(
        doc["payload"]["changed"]["pathPrefixesUsed"],
        serde_json::json!(["cloud/"]),
        "{doc}"
    );
}

/// T1b. The literal reproducer: a copied workspace with no `.git` above it,
/// `--diff-prefix Cloud` and headers `Cloud/src/posting.al`.
/// Mutation: ignore the flag -> exit 2.
#[test]
fn diff_prefix_flag_resolves_a_copied_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("x/ws");
    d8_copy(&ws);
    if let Some(g) = git_at_or_above(&ws) {
        eprintln!("SKIP: {} has a .git entry above the temp dir", g.display());
        return;
    }

    let (doc, code) = digest(
        &ws,
        &["--diff-prefix", "Cloud"],
        Some(&line8("Cloud/src/posting.al")),
    );
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert!(diags(&doc).is_empty(), "{doc}");
}

/// T2. `--file` and the `--changed` comma list resolve a repo-root path too.
/// `Cloud/src/posting.al,x.al` is not an existing path, so it is a file list.
/// Mutation: step 2 only for diffs -> exit 2, `file-unmatched`.
#[test]
fn file_inputs_from_the_repo_root_resolve() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    let (base, code) = digest(&ws, &["--file", "src/posting.al"], None);
    assert_eq!(code, 0, "precondition: {base}");
    assert!(!roots(&base).is_empty(), "precondition: {base}");

    let (doc, code) = digest(&ws, &["--file", "Cloud/src/posting.al"], None);
    assert_eq!(code, 0, "--file: {doc}");
    assert_eq!(roots(&doc), roots(&base), "--file: {doc}");
    assert!(diags(&doc).is_empty(), "--file: {doc}");

    assert!(!Path::new("Cloud/src/posting.al,x.al").exists());
    let (doc, code) = digest(&ws, &["--changed", "Cloud/src/posting.al,x.al"], None);
    assert_eq!(code, 0, "--changed: {doc}");
    assert_eq!(roots(&doc), roots(&base), "--changed: {doc}");
    assert_eq!(
        diags(&doc),
        vec![("file-unmatched".to_string(), "x.al".to_string())],
        "--changed: {doc}"
    );
}

/// T3. A path that names a workspace file as written wins over the stripped one.
/// Mutation: try the stripped path first -> `PostSalesDoc`.
#[test]
fn a_path_in_the_workspace_wins_over_the_stripped_one() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());
    write(&ws.join("Cloud/src/posting.al"), INNER_POSTING);

    let (doc, code) = digest(&ws, &[], Some(&line8("Cloud/src/posting.al")));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["InnerPost"], "{doc}");
    assert!(!roots(&doc).contains(&"PostSalesDoc".to_string()), "{doc}");
}

/// T3b. A workspace file with no routine (a table only; comments only, no
/// object) is still a workspace file: it is never stripped, and keeps today's
/// diagnostic, through `--diff` and `--file`.
/// Mutation: decide "workspace file" by spans -> the routine-less file strips
/// to `src/posting.al` -> `PostSalesDoc` (the table variant already). Object
/// anchors would still miss the comments-only variant.
#[test]
fn a_routine_less_workspace_file_is_not_stripped() {
    let variants = [
        (
            "table",
            "table 50191 \"Inner Tab\"\n{\n    fields\n    {\n        field(1; \"A\"; Integer) { }\n    }\n}\n",
        ),
        ("comments", "// no object here\n// line 2\n"),
    ];
    for (name, text) in variants {
        let tmp = tempfile::tempdir().unwrap();
        let ws = d8_repo(tmp.path());
        write(&ws.join("Cloud/src/posting.al"), text);

        let (doc, code) = digest(&ws, &[], Some(&line8("Cloud/src/posting.al")));
        assert!(
            !roots(&doc).contains(&"PostSalesDoc".to_string()),
            "{name} --diff: {doc}"
        );
        assert!(
            has_diag(&doc, "diff-file-unmatched", "Cloud/src/posting.al"),
            "{name} --diff: {doc}"
        );
        assert_eq!(code, 2, "{name} --diff: {doc}");

        let (doc, code) = digest(&ws, &["--file", "Cloud/src/posting.al"], None);
        assert!(
            !roots(&doc).contains(&"PostSalesDoc".to_string()),
            "{name} --file: {doc}"
        );
        assert!(
            has_diag(&doc, "file-unmatched", "Cloud/src/posting.al"),
            "{name} --file: {doc}"
        );
        assert_eq!(code, 2, "{name} --file: {doc}");
    }
}

/// T4. The workspace has the same name but no `.git` at or above it: no prefix.
/// Passes on base by design.
/// Mutation: a basename heuristic (strip the workspace's own name) -> exit 0.
#[test]
fn no_git_top_level_means_no_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("x/Cloud");
    d8_copy(&ws);
    if let Some(g) = git_at_or_above(&ws) {
        eprintln!("SKIP: {} has a .git entry above the temp dir", g.display());
        return;
    }

    let (doc, code) = digest(&ws, &[], Some(&line8("Cloud/src/posting.al")));
    assert!(
        has_diag(&doc, "diff-file-unmatched", "Cloud/src/posting.al"),
        "{doc}"
    );
    assert!(roots(&doc).is_empty(), "{doc}");
    assert_eq!(code, 2, "{doc}");
}

/// T5. A repo file outside the workspace stays unmatched.
/// Mutation: strip any first segment (no prefix-match requirement) ->
/// `OnPrem/src/posting.al` resolves to `src/posting.al`, no longer reported.
#[test]
fn a_repo_file_outside_the_workspace_stays_unmatched() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    let diff = format!(
        "{}{}",
        line8("Cloud/src/posting.al"),
        line8("OnPrem/src/posting.al")
    );
    let (doc, code) = digest(&ws, &[], Some(&diff));
    assert_eq!(code, 0, "{doc}");
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert_eq!(
        diags(&doc),
        vec![(
            "diff-file-unmatched".to_string(),
            "OnPrem/src/posting.al".to_string()
        )],
        "{doc}"
    );
}

/// The one `diff-parse-error` of `doc`, by file.
fn parse_error_files(doc: &Value) -> Vec<String> {
    diags(doc)
        .into_iter()
        .filter(|(k, _)| k == "diff-parse-error")
        .map(|(_, f)| f)
        .collect()
}

/// T6 (#72 through the prefix). A file poisoned by a malformed hunk header gives
/// no roots, whatever spelling (root-relative or workspace-relative) each record
/// uses. Then the collision control: with a real inner `Cloud/src/posting.al`, a
/// malformed header there poisons only the inner file.
/// Mutations: build `poisoned` from the raw normalised path -> roots appear;
/// poison every candidate key (raw and stripped) -> `PostSalesDoc` is lost.
#[test]
fn a_poisoned_root_relative_file_gives_no_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    let cases = [
        (
            "bad-first",
            format!(
                "{}{}",
                malformed("Cloud/src/posting.al"),
                line8("Cloud/src/posting.al")
            ),
        ),
        (
            "bad-last",
            format!(
                "{}{}",
                line8("Cloud/src/posting.al"),
                malformed("Cloud/src/posting.al")
            ),
        ),
        (
            "bad-root-healthy-ws",
            format!(
                "{}{}",
                malformed("Cloud/src/posting.al"),
                line8("src/posting.al")
            ),
        ),
    ];
    for (name, diff) in cases {
        let (doc, code) = digest(&ws, &[], Some(&diff));
        assert!(roots(&doc).is_empty(), "{name}: {doc}");
        assert_eq!(
            parse_error_files(&doc),
            vec!["Cloud/src/posting.al"],
            "{name}: {doc}"
        );
        assert_eq!(code, 2, "{name}: {doc}");
    }

    // Collision control: T3's workspace (both files exist).
    write(&ws.join("Cloud/src/posting.al"), INNER_POSTING);
    let diff = format!(
        "{}{}",
        malformed("Cloud/src/posting.al"),
        line8("src/posting.al")
    );
    let (doc, code) = digest(&ws, &[], Some(&diff));
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "collision: {doc}");
    assert_eq!(
        parse_error_files(&doc),
        vec!["Cloud/src/posting.al"],
        "collision: {doc}"
    );
    assert_eq!(code, 0, "collision: {doc}");
}

/// T7. A submodule (`<tmp>/repo/Cloud/.git`, a file) inside an outer repo
/// (`<tmp>/repo/.git`): a `Cloud/`-prefixed diff and an unprefixed one both
/// resolve.
/// Mutation: stop at the first `.git` ancestor -> the `Cloud/` diff is unmatched.
#[test]
fn nested_git_entries_give_innermost_first_prefixes() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());
    write(&ws.join(".git"), "gitdir: ../.git/modules/Cloud\n");

    for path in ["Cloud/src/posting.al", "src/posting.al"] {
        let (doc, code) = digest(&ws, &[], Some(&line8(path)));
        assert_eq!(code, 0, "{path}: {doc}");
        assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{path}: {doc}");
        assert!(diags(&doc).is_empty(), "{path}: {doc}");
    }
}

/// T9. An explicit `--diff-prefix` states the coordinate system: every path is
/// repository-relative, so `src/posting.al` (outside `Cloud/`) is unmatched
/// even though it names a workspace file as written.
/// Mutation: keep the workspace-first step with the flag -> `PostSalesDoc`.
#[test]
fn explicit_diff_prefix_never_reads_a_path_as_workspace_relative() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("x/ws");
    d8_copy(&ws);
    if let Some(g) = git_at_or_above(&ws) {
        eprintln!("SKIP: {} has a .git entry above the temp dir", g.display());
        return;
    }
    let flag = ["--diff-prefix", "Cloud"];

    let (doc, code) = digest(&ws, &flag, Some(&line8("src/posting.al")));
    assert!(
        !roots(&doc).contains(&"PostSalesDoc".to_string()),
        "workspace-relative: {doc}"
    );
    assert!(
        has_diag(&doc, "diff-file-unmatched", "src/posting.al"),
        "workspace-relative: {doc}"
    );
    assert_eq!(code, 2, "workspace-relative: {doc}");

    let (doc, code) = digest(&ws, &flag, Some(&line8("Cloud/src/posting.al")));
    assert_eq!(
        roots(&doc),
        vec!["PostSalesDoc"],
        "repository-relative: {doc}"
    );
    assert_eq!(code, 0, "repository-relative: {doc}");
}

/// A `diff --git` record deleting `path`.
fn deleted(path: &str) -> String {
    format!(
        "diff --git a/{path} b/{path}\ndeleted file mode 100644\n--- a/{path}\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-x\n"
    )
}

/// T10. Under `--diff-prefix Cloud`, a deleted record outside `Cloud/` is not the
/// workspace's (`diff-file-unmatched`); one inside it is `diff-file-deleted`.
/// Mutation: skip resolution for deleted records -> `OnPrem` is
/// `diff-file-deleted`.
#[test]
fn deleted_records_outside_an_explicit_prefix_are_unmatched() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    let diff = format!(
        "{}{}{}",
        deleted("OnPrem/src/Old.al"),
        deleted("Cloud/src/gone.al"),
        line8("Cloud/src/posting.al")
    );
    let (doc, code) = digest(&ws, &["--diff-prefix", "Cloud"], Some(&diff));
    assert_eq!(
        diags(&doc),
        vec![
            (
                "diff-file-unmatched".to_string(),
                "OnPrem/src/Old.al".to_string()
            ),
            (
                "diff-file-deleted".to_string(),
                "Cloud/src/gone.al".to_string()
            ),
        ],
        "{doc}"
    );
    assert_eq!(roots(&doc), vec!["PostSalesDoc"], "{doc}");
    assert_eq!(code, 0, "{doc}");
}

/// T8. `--diff-prefix` rejects a rooted, empty or `..` value: exit 1 with the
/// message.
/// Mutation: accept them -> exit 0/2 with a document.
#[test]
fn diff_prefix_rejects_rooted_empty_and_dotdot() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = d8_repo(tmp.path());

    for bad in ["/x", ".", "", "a/../b", "C:/x"] {
        let out = run(
            &ws,
            &["--diff-prefix", bad],
            Some(&line8("Cloud/src/posting.al")),
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{bad:?}: {stderr}");
        assert!(
            stderr.contains("digest: --diff-prefix must be a relative path inside the repository"),
            "{bad:?}: {stderr}"
        );
    }
}
