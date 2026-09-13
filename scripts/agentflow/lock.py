"""The local lock, its heartbeat, the run-id fence, and the HALT kill switch.

Labels on GitHub are a mirror for humans. This file is the lock. A lock is
stale when its HEARTBEAT (not its start) is older than STALE_SECONDS; a live
run refreshes the heartbeat via `beat` (the supervisor does this every 60 s
while a gate runs). Every mutation checks the fence: the lock must name the
caller's run id, so a superseded run cannot act after losing the lock.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass

from .state import Ctx, read_json

STALE_SECONDS = 30 * 60


class LockHeld(RuntimeError):
    def __init__(self, existing: "Lock"):
        super().__init__(f"lock held by run {existing.run_id} for issue {existing.issue}")
        self.existing = existing


class FenceError(RuntimeError):
    """The lock names a different run id (or no lock exists)."""


class HaltError(RuntimeError):
    """`.agent/HALT` is present."""


@dataclass
class Lock:
    issue: int
    run_id: str
    session: str
    started: float
    heartbeat: float
    attempt: int


def read(ctx: Ctx) -> Lock | None:
    data = read_json(ctx.paths.lock)
    return Lock(**data) if data else None


def _write(ctx: Ctx, lk: Lock) -> None:
    ctx.write_guard("write lock")
    ctx.paths.lock.write_text(json.dumps(asdict(lk), indent=2), encoding="utf-8")


def acquire(ctx: Ctx, issue: int, session: str, attempt: int) -> Lock:
    ctx.write_guard("acquire lock")
    if not ctx.run_id:
        raise RuntimeError("acquire needs a run_id")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    now = ctx.now()
    lk = Lock(issue=issue, run_id=ctx.run_id, session=session, started=now, heartbeat=now, attempt=attempt)
    try:
        with open(ctx.paths.lock, "x", encoding="utf-8") as f:
            f.write(json.dumps(asdict(lk), indent=2))
    except FileExistsError:
        existing = read(ctx)
        if existing is None:  # raced with a release; retry once
            return acquire(ctx, issue, session, attempt)
        raise LockHeld(existing)
    return lk


def check_fence(ctx: Ctx) -> Lock:
    lk = read(ctx)
    if lk is None or lk.run_id != ctx.run_id:
        raise FenceError(f"lock is {lk.run_id if lk else 'absent'}, context is {ctx.run_id}")
    return lk


def beat(ctx: Ctx) -> None:
    ctx.write_guard("beat")
    lk = check_fence(ctx)
    lk.heartbeat = ctx.now()
    _write(ctx, lk)


def release(ctx: Ctx) -> None:
    ctx.write_guard("release lock")
    check_fence(ctx)
    ctx.paths.lock.unlink()


def is_stale(lk: Lock, now: float) -> bool:
    return (now - lk.heartbeat) > STALE_SECONDS


def halted(ctx: Ctx) -> str | None:
    try:
        return ctx.paths.halt.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return None


def set_halt(ctx: Ctx, reason: str) -> None:
    ctx.write_guard("set HALT")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    ctx.paths.halt.write_text(reason + "\n", encoding="utf-8")


def require_not_halted(ctx: Ctx, terminal: bool = False) -> None:
    reason = halted(ctx)
    if reason is not None and not terminal:
        raise HaltError(reason)
