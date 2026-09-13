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
