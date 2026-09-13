"""Protected paths and forbidden additions. A hit anywhere blocks the issue.

The flow must not be able to change its own gates, commands, CI, or doctrine,
and must not get green by ignoring tests, skipping hooks, or silencing lints.
Markdown and other prose files may MENTION these strings; only code-like
files are scanned for additions.
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
_FILE_HDR = re.compile(r"^\+\+\+ b/(.+)$")
_VERSION = re.compile(r'^\+\s*version\s*=\s*"')


def evidence_files(issue: int) -> set[str]:
    return {f".agent/issue-{issue}/ledger.md", f".agent/issue-{issue}/findings.json"}


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


def _added_lines(diff_text: str):
    current = None
    for line in diff_text.splitlines():
        m = _FILE_HDR.match(line)
        if m:
            current = m.group(1)
            continue
        if line.startswith("+") and not line.startswith("+++") and current:
            yield current, line[1:]


def forbidden_additions(diff_text: str) -> list[tuple[str, str, str]]:
    out = []
    for f, text in _added_lines(diff_text):
        if not f.endswith(CODE_SUFFIXES):
            continue
        for rx, kind in FORBIDDEN:
            if rx.search(text):
                out.append((f, kind, text.strip()))
    return out


def cargo_version_changed(diff_text: str) -> bool:
    current = None
    for line in diff_text.splitlines():
        m = _FILE_HDR.match(line)
        if m:
            current = m.group(1)
            continue
        if current and current.endswith("Cargo.toml") and _VERSION.match(line):
            return True
    return False


def check_diff(changed_files: list[str], diff_text: str, issue: int) -> list[str]:
    reasons = [f"protected-path:{f}" for f in protected_violations(changed_files, issue)]
    reasons += [f"forbidden:{kind}:{f}:{text}" for f, kind, text in forbidden_additions(diff_text)]
    if cargo_version_changed(diff_text):
        reasons.append("cargo-version-changed")
    return reasons
