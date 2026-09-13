"""Scan text that is about to leave the machine (or enter git history).

Rejects: any path under CDO_WS, any `.alpackages/` path, and token-shaped
strings. LIMIT: dependency-source excerpts are detected only by path
attribution; the scanner does not recognise AL source text on its own.
`.dependencies/` folders are ordinary source and are never flagged.
"""
from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

TOKEN_PATTERNS = (
    re.compile(r"\bgh[pousr]_[A-Za-z0-9]{20,}"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}"),
    re.compile(r"\bAKIA[0-9A-Z]{16}\b"),
    re.compile(r"(?i)\bbearer\s+[A-Za-z0-9\-_.]{20,}"),
    re.compile(r"\bsk-[A-Za-z0-9]{20,}"),
)


@dataclass(frozen=True)
class Violation:
    kind: str
    line_no: int
    excerpt: str


def norm_path(s: str) -> str:
    return s.replace("\\", "/").lower().rstrip("/")


def scan(text: str, cdo_ws: str | None) -> list[Violation]:
    cdo = norm_path(cdo_ws) if cdo_ws else None
    out: list[Violation] = []
    for i, line in enumerate(text.splitlines(), start=1):
        n = norm_path(line)
        if cdo and cdo in n:
            out.append(Violation("cdo-path", i, line.strip()[:120]))
            continue
        if ".alpackages/" in n:
            out.append(Violation("alpackages-path", i, line.strip()[:120]))
            continue
        if any(rx.search(line) for rx in TOKEN_PATTERNS):
            out.append(Violation("token", i, line.strip()[:40] + "…"))
    return out


def scan_file(path: Path, cdo_ws: str | None) -> list[Violation]:
    return scan(path.read_text(encoding="utf-8", errors="replace"), cdo_ws)
