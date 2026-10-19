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
_FILE_HDR = re.compile(r"^\+\+\+ b/(.+)$")
_VERSION = re.compile(r'^\+\s*version\s*=\s*"')


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


def gate_inert(path: str) -> bool:
    path = path.replace("\\", "/")
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


def _added_lines(diff_text: str):
    current = None
    for line in diff_text.splitlines():
        m = _FILE_HDR.match(line)
        if m:
            current = m.group(1)
            continue
        if line.startswith("+") and not line.startswith("+++") and current:
            yield current, line[1:]


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


def forbidden_additions(diff_text: str) -> list[tuple[str, str, str]]:
    out = []
    for f, text in _added_lines(diff_text):
        if not f.endswith(CODE_SUFFIXES):
            continue
        code = _code_part(f, text)
        for rx, kind in FORBIDDEN:
            if rx.search(code):
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
