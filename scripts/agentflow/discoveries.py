"""Discovery filing: deterministic fingerprints, a crash-safe local index,
a marker in every filed body, and reconciliation before any new creation.

Order for each discovery: index check (only a `filed` entry is skipped; a
`pending` entry is retryable) -> remote marker search, bounded to the
`agent-filed` label (never a free-text search) -> write `pending` -> create
-> charge the discovery budget only after a successful create -> write
`filed`. A crash between create and the index write leaves `pending`; the
next run retries it like a new discovery and the marker search resolves it
without recreating.
"""
from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass

from . import budget
from .gh import Gh
from .state import Ctx, read_json, write_json

MARKER_FMT = "<!-- agentflow-fp: {fp} -->"
_NUM = re.compile(r"\d+")
_WS = re.compile(r"\s+")


@dataclass(frozen=True)
class Discovery:
    subsystem: str
    locator: str
    symptom: str
    kind: str  # "bug" | "enhancement"
    origin_issue: int
    reproducer: str
    pre_existing: bool
    capability: str
    acceptance: str


def _norm(s: str) -> str:
    return _WS.sub(" ", _NUM.sub("#", s.replace("\\", "/").lower())).strip()


def fingerprint(subsystem: str, locator: str, symptom: str) -> str:
    raw = "|".join(_norm(x) for x in (subsystem, locator, symptom))
    return hashlib.sha1(raw.encode()).hexdigest()[:16]


def title(d: Discovery) -> str:
    return f"[agent-discovery][{d.subsystem}] {d.symptom[:80]}"


def render_body(d: Discovery, session_url: str) -> str:
    fp = fingerprint(d.subsystem, d.locator, d.symptom)
    pre = "Reproduces on `master` before the origin issue's change (pre-existing)." if d.pre_existing \
        else "Does not reproduce on `master` at the origin issue's base; introduced or exposed by that work."
    return (
        f"## Capability\n\n{d.capability}\n\n"
        f"## Acceptance\n\n{d.acceptance}\n\n"
        f"## Origin\n\nFound while working on #{d.origin_issue}. {pre}\n\n"
        f"## Reproducer\n\n```\n{d.reproducer}\n```\n\nLocation: `{d.locator}`\n\n"
        f"Session: {session_url}\n\n{MARKER_FMT.format(fp=fp)}\n"
    )


def _load_index(ctx: Ctx) -> dict:
    return read_json(ctx.paths.discoveries_index, default={})


def _save_index(ctx: Ctx, idx: dict) -> None:
    write_json(ctx, ctx.paths.discoveries_index, idx)


def _search_marker(gh: Gh, fp: str) -> list:
    marker = MARKER_FMT.format(fp=fp)
    return [i for i in gh.list_labeled("agent-filed") if marker in i.body]


def reconcile_pending(ctx: Ctx, gh: Gh) -> list[dict]:
    idx = _load_index(ctx)
    report = []
    changed = False
    for fp, entry in idx.items():
        if entry["status"] != "pending":
            continue
        try:
            hits = _search_marker(gh, fp)
        except Exception as e:  # gh failure: stay pending
            report.append({"fp": fp, "status": "pending-search-failed", "error": str(e)})
            continue
        if not hits:
            report.append({"fp": fp, "status": "pending-not-found"})
        elif len(hits) == 1:
            entry.update(status="filed", number=hits[0].number)
            changed = True
            report.append({"fp": fp, "status": "filed", "number": hits[0].number})
        else:
            report.append({"fp": fp, "status": "pending-ambiguous", "hits": [h.number for h in hits]})
    if changed:
        _save_index(ctx, idx)
    return report


def file_all(ctx: Ctx, gh: Gh, discoveries: list[Discovery], session_url: str) -> list[dict]:
    idx = _load_index(ctx)
    out = []
    for d in discoveries:
        fp = fingerprint(d.subsystem, d.locator, d.symptom)
        entry = idx.get(fp)
        if entry is not None and entry["status"] == "filed":
            out.append({"fp": fp, "status": "skipped-index", "number": entry.get("number"), "error": None})
            continue
        try:
            hits = _search_marker(gh, fp)
        except Exception as e:
            out.append({"fp": fp, "status": "search-failed", "number": None, "error": str(e)})
            continue
        if len(hits) == 1:
            idx[fp] = {"status": "filed", "number": hits[0].number, "origin": d.origin_issue}
            _save_index(ctx, idx)
            out.append({"fp": fp, "status": "skipped-remote", "number": hits[0].number, "error": None})
            continue
        if len(hits) > 1:
            # Never silently resolve an ambiguous marker hit (reconcile_pending
            # refuses to for the same reason): leave the index as-is.
            out.append({"fp": fp, "status": "ambiguous", "number": None, "error": f"{len(hits)} hits"})
            continue
        budget.check_deadline(ctx)
        if budget.snapshot(ctx)["counts"].get("discoveries", 0) >= budget.CAPS["discoveries"]:
            out.append({"fp": fp, "status": "over-cap", "number": None, "error": None})
            continue
        idx[fp] = {"status": "pending", "number": None, "origin": d.origin_issue}
        _save_index(ctx, idx)
        try:
            number = gh.create_issue(title(d), render_body(d, session_url), ["agent-filed", d.kind])
        except Exception as e:
            out.append({"fp": fp, "status": "pending", "number": None, "error": str(e)})
            continue
        budget.charge(ctx, "discoveries")
        idx[fp] = {"status": "filed", "number": number, "origin": d.origin_issue}
        _save_index(ctx, idx)
        out.append({"fp": fp, "status": "filed", "number": number, "error": None})
    return out
