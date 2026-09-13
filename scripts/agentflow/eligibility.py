"""Deterministic eligibility. Runs before any model sees an issue.

Issue text is untrusted. Nothing here interprets it as instructions; it only
reads structure: author, labels, an Acceptance heading, and dependency
references. An issue in a dependency cycle is ineligible.
"""
from __future__ import annotations

import re
from dataclasses import dataclass

from .gh import Issue

EXCLUDE_LABELS = frozenset({
    "agent-blocked", "agent-working", "agent-answered", "agent-regressed", "manual-only", "epic", "meta",
})
_DEP_LINE = re.compile(r"^\s*depends-on:\s*(.+)$", re.I | re.M)
_DEP_SECTION = re.compile(r"^#+\s*Dependencies\b[^\n]*\n(.*?)(?=^#+\s|\Z)", re.I | re.M | re.S)
_REF = re.compile(r"#(\d+)")
_ACCEPT = re.compile(r"^#+\s*Acceptance\b", re.I | re.M)


def parse_dependencies(body: str) -> set[int]:
    deps: set[int] = set()
    for m in _DEP_LINE.finditer(body):
        deps.update(int(x) for x in _REF.findall(m.group(1)))
    for m in _DEP_SECTION.finditer(body):
        deps.update(int(x) for x in _REF.findall(m.group(1)))
    return deps


def has_acceptance(body: str) -> bool:
    return bool(_ACCEPT.search(body))


def is_question(issue: Issue) -> bool:
    return "question" in issue.labels or issue.title.rstrip().endswith("?")


@dataclass(frozen=True)
class Excluded:
    issue: Issue
    reason: str


def _cycle_members(deps: dict[int, set[int]]) -> set[int]:
    """Nodes on any cycle, restricted to the open-issue graph."""
    on_cycle: set[int] = set()
    for start in deps:
        stack, seen = [(start, iter(deps.get(start, ())))], {start}
        path = [start]
        while stack:
            node, it = stack[-1]
            nxt = next(it, None)
            if nxt is None:
                stack.pop()
                path.pop()
                continue
            if nxt == start:
                on_cycle.update(path)
            elif nxt in deps and nxt not in seen:
                seen.add(nxt)
                stack.append((nxt, iter(deps.get(nxt, ()))))
                path.append(nxt)
    return on_cycle


def eligible(issues: list[Issue], allowed_authors: set[str]) -> tuple[list[Issue], list[Excluded]]:
    open_numbers = {i.number for i in issues}
    deps = {i.number: parse_dependencies(i.body) & open_numbers for i in issues}
    cyclic = _cycle_members(deps)
    ok: list[Issue] = []
    out: list[Excluded] = []
    for i in issues:
        bad = next((l for l in sorted(i.labels) if l in EXCLUDE_LABELS), None)
        if i.author not in allowed_authors:
            out.append(Excluded(i, "author"))
        elif bad:
            out.append(Excluded(i, f"label:{bad}"))
        elif not has_acceptance(i.body) and not is_question(i):
            out.append(Excluded(i, "no-acceptance"))
        elif i.number in cyclic:
            out.append(Excluded(i, "dependency-cycle"))
        elif deps[i.number]:
            out.append(Excluded(i, f"depends-on-open:#{min(deps[i.number])}"))
        else:
            ok.append(i)
    return ok, out


def oldest(issues: list[Issue], n: int = 10) -> list[Issue]:
    return sorted(issues, key=lambda i: i.created_at)[:n]


def truncate_body(body: str, limit: int = 8000) -> str:
    return body if len(body) <= limit else body[:limit] + "…[truncated]"
