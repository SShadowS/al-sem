import re
from pathlib import Path

from agentflow.protect import (GATE_INERT_DIR, GATE_INERT_EXACT, cargo_version_changed, check_diff,
                               forbidden_additions, gate_inert, protected_violations)

REPO = Path(__file__).resolve().parents[3]
GATE_SCRIPTS = ("scripts/ci-steps", "scripts/check-goldens", "scripts/cdo-gate")
# `docs/…`, a `"docs"` path segment (`.join("docs")`), or `CHANGELOG`.
_INERT_REF = re.compile("|".join([re.escape(GATE_INERT_DIR), r'"docs"', *(re.escape(s.split(".")[0])
                                                                        for s in GATE_INERT_EXACT)]))
# Lines that NAME an inert path without reading it, each with its reason. An entry
# exempts every line of its file that contains its text, so each names the exact
# expression (a fixture repo's join, an expected message), never the bare file name.
_FIXTURE = "a tempdir fixture repo's CHANGELOG.md"
_MESSAGE = "an expected message naming the file"
_MENTIONS_ONLY = {
    ("src/server.rs", "docs/OUTSTANDING.md"): "a warn! message points the reader at the doc",
    ("scripts/ci-steps", "docs/*|*.md) ;;"): "task-affected's path map SKIPS prose paths; it reads none",
    ("scripts/cdo-gate", "| CHANGELOG.md) ;;"): "product_paths SKIPS prose paths; it reads none",
    ("tests/cli/gate_scripts.rs", '"docs/superpowers/notes/n.md",'): "a path string fed to product_paths",
    ("tests/cli/gate_scripts.rs", '"CHANGELOG.md",'): "a path string fed to product_paths",
    ("tests/cli/land_script.rs", 'main.join("CHANGELOG.md")'): _FIXTURE,
    ("tests/cli/land_script.rs", 'gate.join("CHANGELOG.md")'): _FIXTURE,
    ("tests/cli/land_script.rs", '("CHANGELOG.md", '): "a file in a fixture commit",
    ("tests/cli/land_script.rs", '"rm", "-q", "CHANGELOG.md"'): "git rm in a fixture repo",
    ("tests/cli/land_script.rs", "CHANGELOG.md conflicts: would keep"): _MESSAGE,
    ("tests/cli/land_script.rs", '"CHANGELOG.md is missing on one side"'): _MESSAGE,
    ("tests/cli/land_script.rs", '"cannot merge CHANGELOG.md"'): _MESSAGE,
    # TLS-87: prose files in a fixture branch (land must not ask them for a record).
    ("tests/cli/land_script.rs", '("docs/n.md", b"x\\n")'): "a file in a fixture commit",
    ("tests/cli/land_script.rs", '("docs/café.md", b"x\\n")'): "a file in a fixture commit",
    ("tests/cli/land_script.rs", '("docs/tab\\there.md", b"x\\n")'): "a file in a fixture commit",
    ("tests/cli/land_script.rs", '("docs/late-ok.md", b"x\\n")'): "a file in a fixture commit",
    ("tests/cli/changelog_merge.rs", 'p.join("CHANGELOG.md")'): _FIXTURE,
    ("tests/cli/changelog_merge.rs", 'dir.path().join("CHANGELOG.md")'): _FIXTURE,
    ("tests/cli/changelog_merge.rs", '"CHANGELOG.md merge=changelog\\n"'): "a fixture repo's .gitattributes",
    ("tests/cli/changelog_merge.rs", '"check-attr", "merge", "CHANGELOG.md"'): (
        "git check-attr on the real repo reads .gitattributes, not the file"),
    ("tests/cli/changelog_merge.rs", '"CHANGELOG.md: merge: changelog"'): "check-attr's expected output",
}


def _inert_hits(rel, text, script=False):
    """The lines of `text` (file `rel`) that name a gate-inert path in code, less `_MENTIONS_ONLY`."""
    hits = []
    for n, line in enumerate(text.splitlines(), 1):
        if script:
            code = "" if line.lstrip().startswith("#") else line
        else:
            code = line.split("//", 1)[0]
        if _INERT_REF.search(code) and not any(f == rel and s in code for f, s in _MENTIONS_ONLY):
            hits.append(f"{rel}:{n}: {line.strip()}")
    return hits


def _scanned():
    """(rel, text, script) for every file the gate-inert scan reads."""
    sources = [p for d in ("src", "tests", "benches", "crates", "tools")
               for p in (REPO / d).rglob("*.rs") if "target" not in p.parts]
    assert len(sources) > 100, "precondition: the scan found the Rust sources"
    out = [(p.relative_to(REPO).as_posix(), p.read_text(encoding="utf-8", errors="replace"), False)
           for p in sources]
    return out + [(s, (REPO / s).read_text(encoding="utf-8"), True) for s in GATE_SCRIPTS]


def test_gate_inert_is_prose_no_gate_reads():
    assert gate_inert("CHANGELOG.md") and gate_inert("docs/superpowers/specs/x.md")
    assert gate_inert("docs\\measurements\\n.md")
    for p in ("docs/tool.py", "README.md", "CLAUDE.md", "tests/cli-a-goldens/a.prsummary.md", "docs.md",
              "src/docs/x.md", ".agent/issue-8/ledger.md"):
        assert not gate_inert(p), p


def test_no_gate_reads_a_gate_inert_path():
    """`gate_inert` paths may change after the gates ran (`freeze_violations`),
    which is sound only while nothing a gate runs opens them. Every Rust source
    the gates build (the crate, its tests, benches, workspace crates and tools,
    build scripts included) and the gate scripts themselves are scanned for a
    non-comment line naming one. A hit means a gate may read prose: take the
    path out of `GATE_INERT_*` or stop reading it."""
    hits = [h for rel, text, script in _scanned() for h in _inert_hits(rel, text, script)]
    assert hits == []


def test_the_prose_the_agentflow_suite_reads_is_not_inert():
    """`ci-steps all` runs `test_docs_drift`, which reads these documents: a change
    to one after the gates can turn the gate red, so it must move the head.
    Limit: the Rust scan above does not read the pytest files; this pins only the
    drift test's own list."""
    from agentflow.tests.test_docs_drift import DOCS, ISSUE
    for p in (*DOCS, ISSUE):
        rel = p.relative_to(REPO).as_posix()
        assert not gate_inert(rel), rel


def test_mentions_only_exempts_its_shape_and_no_more():
    """The exemptions are per file and per text: a real read of prose, in a scanned
    file or in an exempted file under another spelling, is still a hit."""
    read = 'let s = std::fs::read_to_string(root.join("CHANGELOG.md")).unwrap();'
    assert _inert_hits("tests/cli/other.rs", read) != []
    assert _inert_hits("tests/cli/land_script.rs", read) != []
    assert _inert_hits("tests/cli/land_script.rs", 'std::fs::write(r.main.join("CHANGELOG.md"), x)') == []
    assert _inert_hits("tests/cli/land_script.rs", 'read_to_string(root.join("docs/x.md"))') != []
    assert _inert_hits("tests/cli/gate_scripts.rs", read) != []
    assert _inert_hits("scripts/cdo-gate", 'cat CHANGELOG.md', script=True) != []
    assert _inert_hits("scripts/cdo-gate", '# cat CHANGELOG.md', script=True) == []
    assert _inert_hits("src/a.rs", '// see docs/x.md') == []


def test_every_mentions_only_entry_still_matches_a_line():
    """A stale exemption (its line gone) is removed, not kept to hide a later read."""
    texts = {rel: text for rel, text, _ in _scanned()}
    stale = [(f, s) for f, s in _MENTIONS_ONLY if s not in texts.get(f, "")]
    assert stale == []

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
