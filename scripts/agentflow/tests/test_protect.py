from agentflow.protect import cargo_version_changed, check_diff, forbidden_additions, protected_violations

DIFF = """\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,4 @@
 fn a() {}
+#[ignore]
+#[allow(dead_code)]
+fn b() {}
diff --git a/docs/x.md b/docs/x.md
--- a/docs/x.md
+++ b/docs/x.md
@@ -1 +1,2 @@
 text
+never use --no-verify
diff --git a/Cargo.toml b/Cargo.toml
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1,3 +1,3 @@
 [package]
-version = "1.2.0"
+version = "1.3.0"
"""


def test_protected_paths_block_but_own_evidence_allowed():
    files = [".github/workflows/ci.yml", "scripts/x.sh", ".claude/commands/issue.md", "CLAUDE.md", ".gitignore",
             "tree-sitter-al", ".agent/issue-8/ledger.md", ".agent/issue-8/findings.json", ".agent/issue-9/ledger.md",
             ".agent/lock.json", "src/ok.rs"]
    v = protected_violations(files, issue=8)
    assert set(v) == {".github/workflows/ci.yml", "scripts/x.sh", ".claude/commands/issue.md", "CLAUDE.md",
                      ".gitignore", "tree-sitter-al", ".agent/issue-9/ledger.md", ".agent/lock.json"}


def test_forbidden_additions_only_in_code_files():
    got = forbidden_additions(DIFF)
    assert ("src/lib.rs", "ignore-attribute", "#[ignore]") in got
    assert ("src/lib.rs", "allow-attribute", "#[allow(dead_code)]") in got
    assert not any(f == "docs/x.md" for f, _, _ in got)


def test_cargo_version_change_detected():
    assert cargo_version_changed(DIFF)
    assert not cargo_version_changed(DIFF.replace('+version = "1.3.0"', '+name = "x"'))


def test_check_diff_reasons():
    reasons = check_diff(["src/lib.rs", "Cargo.toml", "scripts/x"], DIFF, issue=8)
    assert any(r.startswith("protected-path:scripts/x") for r in reasons)
    assert any(r.startswith("forbidden:ignore-attribute") for r in reasons)
    assert "cargo-version-changed" in reasons
    assert check_diff(["src/lib.rs"], "diff --git a/src/lib.rs b/src/lib.rs\n+fn ok() {}\n", issue=8) == []


def _one_file_diff(path, *added):
    body = "\n".join("+" + a for a in added)
    return f"diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1 +1,{len(added)} @@\n{body}\n"


def test_forbidden_patterns_in_comments_pass_but_code_still_fails():
    # #46: the guard must see code, not prose. A comment naming a pattern passes;
    # the same pattern as real code -- including code with a trailing comment -- fails.
    prose = [
        ("src/a.rs", "// tests/common/regen.rs carries #[allow(dead_code)] for this reason"),
        ("src/a.rs", "    /// a probe marked #[ignore] would hide a failure"),
        ("src/a.rs", "//! never commit with --no-verify"),
        ("scripts/x.py", "    # no #[ignore] here, and never --no-verify"),
        ("scripts/x.sh", "# git commit --no-verify is forbidden"),
    ]
    for path, line in prose:
        assert forbidden_additions(_one_file_diff(path, line)) == [], (path, line)
    code = [
        ("src/a.rs", "#[allow(dead_code)]", "allow-attribute"),
        ("src/a.rs", "    #[ignore] // flaky on CI", "ignore-attribute"),
        ("src/a.rs", '    let url = "https://x"; #![allow(unused)]', "allow-attribute"),
        ("scripts/x.sh", "git commit --no-verify  # just this once", "no-verify"),
        ("scripts/x.py", 'run(["git", "commit", "--no-verify"])', "no-verify"),
    ]
    for path, line, kind in code:
        assert [k for _, k, _ in forbidden_additions(_one_file_diff(path, line))] == [kind], (path, line)
