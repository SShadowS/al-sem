"""Protected paths and forbidden additions. A hit anywhere blocks the issue.

The flow must not be able to change its own gates, commands, CI, or doctrine,
and must not get green by ignoring tests, skipping hooks, or silencing lints.
Markdown and other prose files may MENTION these strings; only code-like
files are scanned for additions, and a line comment in one may mention them too.
"""
from __future__ import annotations

import re

PROTECTED_PREFIXES = (".github/", "scripts/", ".claude/", ".agent/")
PROTECTED_EXACT = ("CLAUDE.md", ".gitignore", "tree-sitter-al")
CODE_SUFFIXES = (".rs", ".toml", ".sh", ".py", ".yml", ".yaml", ".js", ".json")
FORBIDDEN = (
    (re.compile(r"#\[ignore"), "ignore-attribute"),
    (re.compile(r"--no-verify"), "no-verify"),
    (re.compile(r"#!?\[allow\("), "allow-attribute"),
)
_VERSION = re.compile(r'^\s*version\s*=\s*"')


def evidence_files(issue: int) -> set[str]:
    return {f".agent/issue-{issue}/ledger.md", f".agent/issue-{issue}/findings.json"}


# Paths no gate reads: prose the build, the tests, the goldens and the gate
# scripts never open. A change to one after `H` cannot change what the gates
# verified, so it is not a new head (`mergeops.freeze_violations`). On #160 step
# 2, CHANGELOG and numbers-file wording fixes from the final panel moved `H`
# twice and cost two full gate runs. `tests/**/*.md` is NOT inert: the
# prsummary goldens are Markdown the tests read byte for byte.
# test_protect.test_no_gate_reads_a_gate_inert_path keeps this list true.
GATE_INERT_EXACT = ("CHANGELOG.md",)
GATE_INERT_DIR, GATE_INERT_SUFFIX = "docs/", ".md"
# Prose under `docs/` that a gate DOES read: `ci-steps all` runs the agentflow suite
# (TLS-71), whose `test_docs_drift` checks this spec's strings against the code.
GATE_READ_DOCS = ("docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md",)


def gate_inert(path: str) -> bool:
    path = path.replace("\\", "/")
    if path in GATE_READ_DOCS:
        return False
    return path in GATE_INERT_EXACT or (path.startswith(GATE_INERT_DIR) and path.endswith(GATE_INERT_SUFFIX))


def protected_violations(changed_files: list[str], issue: int) -> list[str]:
    allowed = evidence_files(issue)
    out = []
    for f in changed_files:
        f = f.replace("\\", "/")
        if f in allowed:
            continue
        if f in PROTECTED_EXACT or f.startswith(PROTECTED_PREFIXES):
            out.append(f)
    return out


def _added_lines(diffs: dict[str, str]):
    """(path, added text) for each file's own patch (`Git.file_diffs`, TLS-114).
    The path is the key, never read from a `+++ b/` header: git quotes a
    header whose name holds a non-ASCII byte, a CR, an LF, a `"` or a
    backslash, and a header parser then charged that file's lines to the file
    before it. Lines split at LF only, so a CR inside an added line does not
    start a new one. Everything before the first `@@` is the patch's header,
    so an added line whose text starts with `++` is still read."""
    for path, text in diffs.items():
        in_hunks = False
        for line in text.split("\n"):
            if line.startswith("@@"):
                in_hunks = True
            elif in_hunks and line.startswith("+"):
                yield path, line[1:]


_SLASH_COMMENT = (".rs", ".js")
_HASH_COMMENT = (".py", ".sh", ".toml", ".yml", ".yaml")


def _code_part(path: str, text: str) -> str:
    """`text` with its trailing line comment removed, so a comment that NAMES a
    forbidden pattern is not read as using it (#46). A marker inside a
    double-quoted string (`"https://..."`) is not a comment. A `#` comment
    must start the line or follow whitespace, so `${#x}` is not one.
    ponytail: line comments only; a line inside a /* */ block is still scanned."""
    marker = "//" if path.endswith(_SLASH_COMMENT) else "#" if path.endswith(_HASH_COMMENT) else None
    if marker is None:
        return text
    in_str, i = False, 0
    while i < len(text):
        c = text[i]
        if in_str and c == "\\":
            i += 2
            continue
        if c == '"':
            in_str = not in_str
        elif not in_str and text.startswith(marker, i) and (
                marker == "//" or i == 0 or text[i - 1].isspace()):
            return text[:i]
        i += 1
    return text


def forbidden_additions(diffs: dict[str, str]) -> list[tuple[str, str, str]]:
    out = []
    for f, text in _added_lines(diffs):
        if not f.endswith(CODE_SUFFIXES):
            continue
        code = _code_part(f, text)
        for rx, kind in FORBIDDEN:
            if rx.search(code):
                out.append((f, kind, text.strip()))
    return out


def cargo_version_changed(diffs: dict[str, str]) -> bool:
    return any(f.endswith("Cargo.toml") and _VERSION.match(text) for f, text in _added_lines(diffs))


def check_diff(changed_files: list[str], diffs: dict[str, str], issue: int) -> list[str]:
    """`diffs`: each changed file's own patch, by path (`Git.file_diffs`)."""
    reasons = [f"protected-path:{f}" for f in protected_violations(changed_files, issue)]
    reasons += [f"forbidden:{kind}:{f}:{text}" for f, kind, text in forbidden_additions(diffs)]
    if cargo_version_changed(diffs):
        reasons.append("cargo-version-changed")
    return reasons
