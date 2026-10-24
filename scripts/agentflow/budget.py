"""Per-issue caps and the loop counter.

Ownership convention, not isolation: only the executor writes budget.json, and
a value that does not match the executor's own history is rejected (the file
carries a running checksum). The ledger mirrors the counters for reading.
"""
from __future__ import annotations

import hashlib
import json

from .state import Ctx, append_audit, mutex, read_json, update_json, write_json

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
WALL_CLOCK_SECONDS = 24 * 3600
_DEADLINE_MSG = f"{WALL_CLOCK_SECONDS // 3600} h wall-clock cap reached"


class BudgetExceeded(RuntimeError):
    def __init__(self, key: str):
        super().__init__(f"budget exhausted: {key}")
        self.key = key


class DeadlineExceeded(RuntimeError):
    pass


def _path(ctx: Ctx, run_id: str | None = None):
    return (ctx.paths.run_dir(run_id) if run_id else ctx.run_dir) / "budget.json"


def _checksum(counts: dict, claimed_at: float, grants: dict | None = None) -> str:
    payload = {"counts": counts, "claimed_at": claimed_at}
    if grants:  # absent from every budget written before `grant` existed, so theirs still match
        payload["grants"] = grants
    raw = json.dumps(payload, sort_keys=True)
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def _load(ctx: Ctx, run_id: str | None = None) -> dict:
    data = read_json(_path(ctx, run_id))
    if not data:
        raise RuntimeError("budget not initialised for this run")
    if data.get("checksum") != _checksum(data["counts"], data["claimed_at"], data.get("grants")):
        raise RuntimeError("budget.json does not match executor history")
    return data


def _save(ctx: Ctx, data: dict, run_id: str | None = None) -> None:
    data["checksum"] = _checksum(data["counts"], data["claimed_at"], data.get("grants"))
    write_json(ctx, _path(ctx, run_id), data)


def _cap(data: dict, key: str) -> int:
    """The cap plus whatever the operator granted (`grant`)."""
    return CAPS[key] + data.get("grants", {}).get(key, 0)


def init(ctx: Ctx, claimed_at: float) -> None:
    _save(ctx, {"counts": {}, "claimed_at": claimed_at})


def check_deadline(ctx: Ctx) -> None:
    data = _load(ctx)
    if ctx.now() - data["claimed_at"] > WALL_CLOCK_SECONDS:
        raise DeadlineExceeded(_DEADLINE_MSG)


def charge(ctx: Ctx, key: str, n: int = 1, sub: str | None = None) -> int:
    if key not in CAPS:
        raise KeyError(key)
    # Mutex: an operator's `grant` writes the same file; neither may lose the other's write.
    with mutex(ctx, f"budget-{ctx.run_id}"):
        data = _load(ctx)
        if ctx.now() - data["claimed_at"] > WALL_CLOCK_SECONDS:
            raise DeadlineExceeded(_DEADLINE_MSG)
        slot = f"{key}:{sub}" if sub else key
        used = data["counts"].get(slot, 0) + n
        if used > _cap(data, key):
            raise BudgetExceeded(slot)
        data["counts"][slot] = used
        _save(ctx, data)
        return _cap(data, key) - used


# The only caps a grant may raise: panel rounds. A `ci_fix`, `rebase_regate`
# or `pi_calls` past its cap is a different decision, not an extra round.
GRANTABLE = ("spec_rounds", "final_rounds")


def grant(ctx: Ctx, run_id: str, key: str, n: int, by: str, issue: int) -> int:
    """Raise run `run_id`'s cap for `key` (one of `GRANTABLE`) by `n`. Meant for
    the operator; `cli.cmd_grant` refuses a call carrying a run id, but nothing
    in code stops automation calling it without one (the prompt is the guard,
    as for `unblock`). The audit line lands BEFORE the write, so a grant is
    never in effect without its record. Returns the new cap."""
    if key not in GRANTABLE:
        raise KeyError(key)
    if n < 1:
        raise ValueError(f"grant must be at least 1, not {n}")
    with mutex(ctx, f"budget-{run_id}"):
        data = _load(ctx, run_id)
        grants = data.setdefault("grants", {})
        grants[key] = grants.get(key, 0) + n
        append_audit(ctx, {"action": "budget-grant", "issue": issue, "target_run": run_id,
                           "key": key, "n": n, "by": by})
        _save(ctx, data, run_id)
        return _cap(data, key)


def snapshot(ctx: Ctx) -> dict:
    data = _load(ctx)
    return {"counts": data["counts"], "claimed_at": data["claimed_at"], "caps": CAPS,
            "grants": data.get("grants", {})}


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
