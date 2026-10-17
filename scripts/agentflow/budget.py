"""Per-issue caps and the loop counter.

Ownership convention, not isolation: only the executor writes budget.json, and
a value that does not match the executor's own history is rejected (the file
carries a running checksum). The ledger mirrors the counters for reading.
"""
from __future__ import annotations

import hashlib
import json

from .state import Ctx, read_json, update_json, write_json

CAPS: dict[str, int] = {
    "spec_rounds": 5,
    "plan_tasks": 12,
    "task_attempts": 3,
    "final_rounds": 5,
    "rebase_regate": 1,
    "ci_fix": 1,
    "discoveries": 5,
    "subagents": 60,
    "pi_calls": 20,
}
WALL_CLOCK_SECONDS = 10 * 3600
_DEADLINE_MSG = f"{WALL_CLOCK_SECONDS // 3600} h wall-clock cap reached"


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
        raise DeadlineExceeded(_DEADLINE_MSG)


def charge(ctx: Ctx, key: str, n: int = 1, sub: str | None = None) -> int:
    if key not in CAPS:
        raise KeyError(key)
    data = _load(ctx)
    if ctx.now() - data["claimed_at"] > WALL_CLOCK_SECONDS:
        raise DeadlineExceeded(_DEADLINE_MSG)
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


# `loop.json` is shared by every session, so its read-modify-write holds mutex
# `registry`: two ticks at once must not both read 2 and both write 3.
# `--max-issues` is therefore a total across sessions, not per session.
_LOOP_DEFAULT = {"issues_done": 0}


def loop_reset(ctx: Ctx) -> None:
    update_json(ctx, ctx.paths.loop, lambda _: {"issues_done": 0}, mutex_name="registry")


def _loop_next(data: dict, max_issues: int) -> dict:
    done = data["issues_done"] + 1
    if done > max_issues:
        raise BudgetExceeded("loop")
    return {"issues_done": done}


def loop_tick(ctx: Ctx, max_issues: int) -> int:
    """The old, separate budget step (kept one release for old prompts)."""
    data = update_json(ctx, ctx.paths.loop, lambda d: _loop_next(d, max_issues),
                       mutex_name="registry", default=_LOOP_DEFAULT)
    return max_issues - data["issues_done"]


def loop_next_held(ctx: Ctx, max_issues: int) -> dict:
    """What `loop.json` becomes if one more issue is claimed, or
    `BudgetExceeded`. Writes nothing. ASSUMES mutex `registry` is held: it is
    `lock.acquire`'s helper for `claim --loop-max`, which charges the budget
    in the same critical section as the lease (calling `loop_tick` there
    would take `registry` twice and time out)."""
    return _loop_next(read_json(ctx.paths.loop, _LOOP_DEFAULT), max_issues)


def loop_refund(ctx: Ctx) -> None:
    """Give back the unit a rolled-back `claim --loop-max` spent. Takes
    `registry`; never below 0 (a `loop-reset` may have run meanwhile)."""
    update_json(ctx, ctx.paths.loop, lambda d: {"issues_done": max(0, d["issues_done"] - 1)},
                mutex_name="registry", default=_LOOP_DEFAULT)
