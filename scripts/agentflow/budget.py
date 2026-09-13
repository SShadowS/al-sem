"""Per-issue caps and the loop counter.

Ownership convention, not isolation: only the executor writes budget.json, and
a value that does not match the executor's own history is rejected (the file
carries a running checksum). The ledger mirrors the counters for reading.
"""
from __future__ import annotations

import hashlib
import json

from .state import Ctx, read_json, write_json

CAPS: dict[str, int] = {
    "spec_rounds": 3,
    "plan_tasks": 12,
    "task_attempts": 3,
    "final_rounds": 3,
    "rebase_regate": 1,
    "ci_fix": 1,
    "discoveries": 5,
    "subagents": 60,
    "pi_calls": 14,
}
WALL_CLOCK_SECONDS = 4 * 3600


class BudgetExceeded(RuntimeError):
    def __init__(self, key: str):
        super().__init__(f"budget exhausted: {key}")
        self.key = key


class DeadlineExceeded(RuntimeError):
    pass


def _path(ctx: Ctx):
    return ctx.run_dir / "budget.json"


def _checksum(counts: dict, claimed_at: float) -> str:
    raw = json.dumps({"counts": counts, "claimed_at": claimed_at}, sort_keys=True)
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def _load(ctx: Ctx) -> dict:
    data = read_json(_path(ctx))
    if not data:
        raise RuntimeError("budget not initialised for this run")
    if data.get("checksum") != _checksum(data["counts"], data["claimed_at"]):
        raise RuntimeError("budget.json does not match executor history")
    return data


def _save(ctx: Ctx, data: dict) -> None:
    data["checksum"] = _checksum(data["counts"], data["claimed_at"])
    write_json(ctx, _path(ctx), data)


def init(ctx: Ctx, claimed_at: float) -> None:
    _save(ctx, {"counts": {}, "claimed_at": claimed_at})


def check_deadline(ctx: Ctx) -> None:
    data = _load(ctx)
    if ctx.now() - data["claimed_at"] > WALL_CLOCK_SECONDS:
        raise DeadlineExceeded("4 h wall-clock cap reached")


def charge(ctx: Ctx, key: str, n: int = 1, sub: str | None = None) -> int:
    if key not in CAPS:
        raise KeyError(key)
    data = _load(ctx)
    if ctx.now() - data["claimed_at"] > WALL_CLOCK_SECONDS:
        raise DeadlineExceeded("4 h wall-clock cap reached")
    slot = f"{key}:{sub}" if sub else key
    used = data["counts"].get(slot, 0) + n
    if used > CAPS[key]:
        raise BudgetExceeded(slot)
    data["counts"][slot] = used
    _save(ctx, data)
    return CAPS[key] - used


def snapshot(ctx: Ctx) -> dict:
    data = _load(ctx)
    return {"counts": data["counts"], "claimed_at": data["claimed_at"], "caps": CAPS}


def loop_reset(ctx: Ctx) -> None:
    write_json(ctx, ctx.paths.loop, {"issues_done": 0})


def loop_tick(ctx: Ctx, max_issues: int) -> int:
    data = read_json(ctx.paths.loop, default={"issues_done": 0})
    done = data["issues_done"] + 1
    if done > max_issues:
        raise BudgetExceeded("loop")
    write_json(ctx, ctx.paths.loop, {"issues_done": done})
    return max_issues - done
