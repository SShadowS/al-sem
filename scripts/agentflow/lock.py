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

from .state import Ctx, append_audit, read_json

STALE_SECONDS = 30 * 60


class LockHeld(RuntimeError):
    def __init__(self, existing: "Lock"):
        super().__init__(f"lock held by run {existing.run_id} for issue {existing.issue}")
        self.existing = existing


class FenceError(RuntimeError):
    """The lock names a different run id (or no lock exists)."""


class HaltError(RuntimeError):
    """`.agent/HALT` is present."""


class OperatorOnly(RuntimeError):
    """An operator-only command was invoked by (or alongside) automation."""


class HaltClearRefused(RuntimeError):
    """`clear_halt` refused: `.reason` says which of its checks said no."""

    def __init__(self, reason: str, detail: str = ""):
        super().__init__(f"{reason}: {detail}" if detail else reason)
        self.reason, self.detail = reason, detail


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


def acquire(ctx: Ctx, issue: int, session: str, attempt: int, *,
            started: float | None = None) -> Lock:
    """Take the lock for `issue` under this run's id.

    `started` names WHEN THE WORK WAS CLAIMED, and it is a parameter because
    one caller is not claiming new work: `cli.cmd_recover` mints a
    follow-through lock over a merge that already happened, and passes the
    STALE lock's own `started` so the claim keeps its original date. That
    date is read by `recovery.recover_stale`'s recency axis ("a merge GitHub
    dates before this work was claimed cannot be the merge of this work"). A
    follow-through lock stamped `now` is always LATER than the merge it is
    following through on, so that axis would refuse its own work by
    construction and a post-merge killed mid-gates could never be recovered a
    second time.

    HEARTBEAT IS ALWAYS `now`, never carried back, and the asymmetry is the
    whole point: `is_stale` reads the heartbeat alone, so a lock minted with
    an old heartbeat would be born stale and any concurrent tick could steal
    a follow-through that is still running. `started` dates the CLAIM;
    `heartbeat` says the holder is alive.
    """
    ctx.write_guard("acquire lock")
    if not ctx.run_id:
        raise RuntimeError("acquire needs a run_id")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    now = ctx.now()
    lk = Lock(issue=issue, run_id=ctx.run_id, session=session,
              started=now if started is None else started, heartbeat=now, attempt=attempt)
    try:
        with open(ctx.paths.lock, "x", encoding="utf-8") as f:
            f.write(json.dumps(asdict(lk), indent=2))
    except FileExistsError:
        existing = read(ctx)
        if existing is None:  # raced with a release; retry once
            # `started=started` FORWARDED: without it the retry -- the one
            # path no fixture drives -- silently reverts to `now` and the
            # recency axis starts measuring against the wrong claim again.
            return acquire(ctx, issue, session, attempt, started=started)
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


def require_operator(ctx: Ctx, what: str) -> None:
    """Refuse `what` unless a HUMAN is asking, on a checkout no live run owns.

    Two refusals, and the ASYMMETRY between them is deliberate.

    (1) A run id in the context means automation. Every conductor call carries
    one -- `--run-id`, or the parser's `AGENTFLOW_RUN_ID` default -- so this is
    the check that keeps the kill switch out of the loop's own reach.
    Documenting "the conductor must never call clear-halt" is not what stops
    it; this is. It fails closed, which does mean a human whose shell still
    exports `AGENTFLOW_RUN_ID` from an earlier tick is refused -- the message
    names the escape.

    (2) A LIVE lock means a run is still acting on this checkout, so clearing
    HALT under it would resume a loop mid-flight. A STALE lock is deliberately
    ALLOWED through: `cmd_recover` is the only thing that clears one and it is
    itself reachable under HALT, so refusing on staleness would deadlock the
    operator for the full 30 minutes. This mirrors `cmd_preflight`, which
    fails on `lock-live:` and not on staleness.
    """
    if ctx.run_id:
        raise OperatorOnly(
            f"{what} is operator-only; this call carries run id {ctx.run_id!r}. "
            f"Run it with no --run-id and with AGENTFLOW_RUN_ID unset.")
    lk = read(ctx)
    if lk is not None and not is_stale(lk, ctx.now()):
        raise OperatorOnly(
            f"{what} is refused while run {lk.run_id} still holds the lock for issue "
            f"{lk.issue} (heartbeat {lk.heartbeat}); a live run is acting on this checkout.")


def halted(ctx: Ctx) -> str | None:
    try:
        return ctx.paths.halt.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        return None


def set_halt(ctx: Ctx, reason: str) -> None:
    """Write the kill switch, then record that it was written.

    The audit line is BEST-EFFORT and the asymmetry with `clear_halt` is the
    whole point: HALT itself must land even when its record cannot, while a
    clear must never happen without one. Only `OSError` is caught -- a
    `DryRunViolation` must still escape, and cannot occur here anyway because
    `write_guard` already ran on the line above.

    `post_merge_failure` calls this TWICE per incident (once with a SHA-only
    reason before any git call that can raise, once with the commit subject
    after the fetch), so two `halt-set` lines per incident is expected rather
    than a duplicate to tidy away; the second supersedes.
    """
    ctx.write_guard("set HALT")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    ctx.paths.halt.write_text(reason + "\n", encoding="utf-8")
    try:
        append_audit(ctx, {"action": "halt-set", "reason": reason})
    except OSError:
        pass


def clear_halt(ctx: Ctx, expected: str, by: str) -> str:
    """Remove `.agent/HALT`. The ONLY supported way to do it.

    The order below is load-bearing, top to bottom:

    1. `write_guard` FIRST, so `--dry-run` refuses before anything is even
       read. That is also what makes the dry-run test's expected exception a
       `DryRunViolation` rather than a `HaltClearRefused`/`OperatorOnly`.
    2. the operator gate (see `require_operator`).
    3. refuse when nothing is halted -- a no-op clear must never report
       success, or a typo'd invocation reads exactly like a real one.
    4. refuse unless the CURRENT text is the text the operator quoted. A HALT
       that a NEWER incident overwrote while they were reading the old one
       must not be cleared with the old incident's reason.
    5. the audit line lands BEFORE the unlink, so a clear that dies half-way
       is still on the record. A clear with no record is the one outcome that
       must be impossible; this append is NOT best-effort.
    """
    ctx.write_guard("clear HALT")
    require_operator(ctx, "clear HALT")
    current = halted(ctx)
    if current is None:
        raise HaltClearRefused("not-halted", "there is no .agent/HALT to clear")
    if current != expected.strip():
        raise HaltClearRefused("reason-mismatch",
                               f"HALT reads {current!r}, not the {expected.strip()!r} you named")
    append_audit(ctx, {"action": "halt-clear", "reason": current, "by": by})
    ctx.paths.halt.unlink()
    return current


def require_not_halted(ctx: Ctx, terminal: bool = False) -> None:
    reason = halted(ctx)
    if reason is not None and not terminal:
        raise HaltError(reason)
