"""The per-issue leases, their heartbeat, the run-id fence, and the HALT kill switch.

Labels on GitHub are a mirror for humans. The lease files are the lock. Each
claimed issue has ONE lease, `.agent/locks/issue-<N>.json`, naming the run
that owns it; create-exclusive on that file makes "one run per issue" true by
construction. A lease is stale when its HEARTBEAT (not its start) is older
than STALE_SECONDS of awake time (`is_stale`); a live run refreshes it via
`beat` (the supervisor does this every 60 s while a gate runs). Every
mutation checks the fence: the run's OWN lease must exist, so a superseded
run cannot act after losing it.

`.agent/lock.json` is the LEGACY lease (one lock for the whole checkout,
before the parallel-issues design). It is read as one more lease, and
`beat`/`release`/`check_fence` act on it in place when it names the caller,
so a run that claimed under the old code finishes under the new code. New
leases are never written there.

Mutex `registry` covers every check-then-act on the lease set (`acquire`).
Only the public entry points here take it; helpers assume it is held.
"""
from __future__ import annotations

import json
import shutil
from dataclasses import asdict, dataclass, fields
from pathlib import Path

from . import budget
from .state import (Ctx, MutexTimeout, append_audit, awake_elapsed, config, is_stamp, meta_sum,
                    mutex, read_json, read_optional, write_json)

STALE_SECONDS = 30 * 60

# The refusal codes the conductor BRANCHES on (`orchestrate.md`, `issue.md`).
# Named once here so `tests/test_docs_drift.py` can ask the documents for
# exactly these strings.
ISSUE_HELD, SLOTS_FULL, DISK_LOW, LOOP_EXHAUSTED = "issue-held", "slots-full", "disk-low", "loop-exhausted"
# Lane states that refuse a merge; the code is `merge-lane-<state>`.
LANE_REFUSED = ("busy", "orphaned")


class LockHeld(RuntimeError):
    """`acquire` or `steal_for_recovery` refused. `.code` is the
    machine-readable reason: `issue-held:<run>` (the issue has a lease, live
    or stale), `slots-full` (as many leases as `slots`), `run-holds-a-lock`
    (this run id already owns a lease: one run, one issue),
    `disk-low:<free>GB<<need>GB` (see `disk_required_gb`), `loop-exhausted`
    (`claim --loop-max`: the shared loop budget is spent), `lock-live:<run>`
    (the lease to recover is live again: another session stole it first) or
    `lock-gone` (the lease to recover no longer names the stale run)."""

    def __init__(self, reason: str, existing: "Lock | None" = None):
        self.reason, self.existing = reason, existing
        named = reason in (ISSUE_HELD, "lock-live") and existing
        self.code = f"{reason}:{existing.run_id}" if named else reason
        held = f" (run {existing.run_id} holds issue {existing.issue})" if existing else ""
        super().__init__(f"{self.code}{held}")


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
    # The run recovering this lease (`steal_for_recovery`). A legacy
    # `lock.json` has no such key, hence the default.
    recovering_by: str | None = None
    # `[clock id, seconds]` of the awake clock at `heartbeat` (TLS-122,
    # `state.host_awake`), or None. IN MEMORY ONLY: it is kept in the sidecar
    # `locks/awake/issue-<N>.json`, never in the lease file, because the
    # executor before TLS-122 builds `Lock(**data)` and fails on any key it does
    # not know. Old and new executors can share one `.agent/` this way.
    beat_awake: list | None = None
    # IN MEMORY ONLY, set only on a read under mutex `registry` (sol r4): the
    # sidecar names this run with an OLDER heartbeat than the lease. A beat
    # writes both under `registry`, so seen under it this is a beat that died
    # between its two writes: the deciding commands treat the lease as live.
    beat_pending: bool = False


_LOCK_FIELDS = {f.name for f in fields(Lock)} - {"beat_awake", "beat_pending"}


def _lease_dict(lk: Lock) -> dict:
    """The lease file's content: the pre-TLS-122 schema exactly."""
    d = asdict(lk)
    del d["beat_awake"], d["beat_pending"]
    return d


def _awake_path(ctx: Ctx, issue: int) -> Path:
    # A subfolder: the lease glob `locks/issue-*.json` (old code's too) never sees it.
    return ctx.paths.locks_dir / "awake" / f"issue-{issue}.json"


def _stamp(ctx: Ctx, lk: Lock, now: float) -> None:
    """Set the heartbeat, on both clocks (the lease write stores both)."""
    aw = ctx.awake_now()
    lk.heartbeat, lk.beat_awake = now, list(aw) if aw else None


def _write_awake(ctx: Ctx, lk: Lock) -> None:
    """The sidecar `{run_id, heartbeat, awake, checksum}`, written AFTER its
    lease. It counts only while it names the lease's run and heartbeat, and its
    checksum matches (a hand-edited stamp would make a fresh lease look stale
    or a dead one live). A beat by an executor that writes none (one from
    before TLS-122) leaves it unmatched: the lease ages on the wall clock.

    BEST-EFFORT (sol r2): it is optional metadata, so a failed write never
    interrupts the lease or merge-lane change around it; the old sidecar left
    behind names an older heartbeat, and the lease ages on the wall clock."""
    p = _awake_path(ctx, lk.issue)
    try:
        if lk.beat_awake:
            body = {"run_id": lk.run_id, "heartbeat": lk.heartbeat, "awake": lk.beat_awake}
            write_json(ctx, p, {**body, "checksum": meta_sum(body)})
        else:
            p.unlink(missing_ok=True)
    except OSError:
        pass


def _read_awake(ctx: Ctx, lk: Lock) -> list | None:
    s = read_optional(_awake_path(ctx, lk.issue))
    if not (isinstance(s, dict) and s.get("run_id") == lk.run_id and s.get("heartbeat") == lk.heartbeat
            and is_stamp(s.get("awake"))):            # the shape first: only a flat stamp is summed
        return None
    body = {"run_id": s["run_id"], "heartbeat": s["heartbeat"], "awake": s["awake"]}
    return body["awake"] if s.get("checksum") == meta_sum(body) else None


def _read_lease(path: Path) -> Lock | None:
    data = read_json(path)
    # Only the fields this code knows: a lease written by newer code with a
    # field added must still load here (forward compatibility, TLS-122 r1).
    return Lock(**{k: v for k, v in data.items() if k in _LOCK_FIELDS}) if data else None


def _sidecar_is_older(ctx: Ctx, lk: Lock) -> bool:
    """The sidecar names this lease's run with an older heartbeat."""
    s = read_optional(_awake_path(ctx, lk.issue))
    hb = s.get("heartbeat") if isinstance(s, dict) else None
    return (isinstance(s, dict) and s.get("run_id") == lk.run_id and isinstance(hb, (int, float))
            and not isinstance(hb, bool) and hb < lk.heartbeat)


def _entries(ctx: Ctx, held: bool = False) -> list[tuple[Path, Lock]]:
    """Every lease with its file: the legacy `lock.json`, then `locks/`.

    `held`: the caller holds mutex `registry`. Every writer of a lease and its
    sidecar holds it too, so each pair read is complete; a sidecar older than
    its lease then means a beat died between its writes (`beat_pending`).

    Unlocked (reporting: `status`, `preflight`), a beat can land between the
    two reads. On a mismatch the lease is read once more (sol r3): a lease
    that has gone is left out, and a changed one replaces the first read. This
    is best-effort only (a beat suspended between its own writes still reads
    as a mismatch, sol r4), so every command that ACTS on liveness decides
    under `registry` instead."""
    out = []
    for p in [ctx.paths.lock, *ctx.paths.locks_dir.glob("issue-*.json")]:
        lk = _read_lease(p)
        if lk is None:
            continue
        lk.beat_awake = _read_awake(ctx, lk)
        if lk.beat_awake is None and held:
            lk.beat_pending = _sidecar_is_older(ctx, lk)
        elif lk.beat_awake is None:
            again = _read_lease(p)
            if again is None:
                continue                         # gone: no coherent snapshot of it
            if (again.run_id, again.heartbeat) != (lk.run_id, lk.heartbeat):
                lk = again
                lk.beat_awake = _read_awake(ctx, lk)
        out.append((p, lk))
    return sorted(out, key=lambda e: e[1].issue)


def all_locks(ctx: Ctx) -> list[Lock]:
    return [lk for _, lk in _entries(ctx)]


def _own_entry(ctx: Ctx) -> tuple[Path, Lock] | None:
    if not ctx.run_id:
        return None
    return next((e for e in _entries(ctx) if e[1].run_id == ctx.run_id), None)


def own(ctx: Ctx) -> Lock | None:
    """The lease this run owns, or None."""
    e = _own_entry(ctx)
    return e[1] if e else None


def for_issue(ctx: Ctx, issue: int) -> Lock | None:
    return next((lk for lk in all_locks(ctx) if lk.issue == issue), None)


def read(ctx: Ctx) -> Lock | None:
    """Compatibility shim for callers written when there was one lock: the
    run's own lease, else the ONLY lease, else None. With one lease this is
    exactly the old `read`."""
    mine = own(ctx)
    if mine is not None:
        return mine
    every = all_locks(ctx)
    return every[0] if len(every) == 1 else None


def _write(ctx: Ctx, path: Path, lk: Lock) -> None:
    # Atomic: a reader in another process (the conductor, while the
    # supervisor beats) must never see half a lease. Only `acquire`'s FIRST
    # write is not this one, because it must be create-exclusive.
    ctx.write_guard("write lock")
    write_json(ctx, path, _lease_dict(lk))
    _write_awake(ctx, lk)


def disk_free_gb(ctx: Ctx) -> int:
    """Free GB on the volume the issue and verify worktrees live on: the
    root's PARENT, not the root (the same drive here, but the right question)."""
    return shutil.disk_usage(ctx.paths.root.parent).free // 2**30


def disk_required_gb(ctx: Ctx, live_runs: int) -> int:
    """`min_free_gb` plus `reserve_gb_per_run` for each LIVE run: each may
    still grow its worktree `target/` by ~30 GB, and post-merge builds in the
    root's `target/`. A stale run builds nothing more, so it reserves nothing.
    With the defaults: 40 GB for the first claim, 70 GB for the second.
    With a `target_root` the builds are not on this drive, so it keeps only
    the floor and the reserve moves to `target_disk`."""
    cfg = config(ctx)
    return cfg["min_free_gb"] + (0 if cfg["target_root"] else cfg["reserve_gb_per_run"] * live_runs)


def target_disk(ctx: Ctx, live_runs: int) -> tuple[int, int] | None:
    """(free GB, required GB) on the drive holding `target_root`, or None when
    builds stay in the worktrees. Required is `reserve_gb_per_run` per LIVE run."""
    cfg = config(ctx)
    if not cfg["target_root"]:
        return None
    root = Path(cfg["target_root"])
    probe = root if root.exists() else Path(root.anchor or ".")
    return shutil.disk_usage(probe).free // 2**30, cfg["reserve_gb_per_run"] * live_runs


def acquire(ctx: Ctx, issue: int, session: str, attempt: int, *,
            started: float | None = None, count_slot: bool = True,
            loop_max: int | None = None) -> Lock:
    """Take the lease for `issue` under this run's id.

    Under mutex `registry`, in this order: refuse `run-holds-a-lock` if this
    run already owns a lease; refuse `issue-held` if the issue has a lease,
    live OR stale (a stale lease belongs to `recover`, not to a new claim);
    refuse `slots-full` when `count_slot` and the lease count is at least
    `slots` from `.agent/config.json`. Stale leases count, so a crashed run
    cannot be over-subscribed around: `recover` frees its slot. Then refuse
    `disk-low:<free>GB<<need>GB` below `disk_required_gb`. `unblock` passes
    `count_slot=False`, because it does no work, and skips both.

    `loop_max` (`claim --loop-max`): refuse `loop-exhausted` first if one
    more claim would pass the loop budget, and charge `.agent/runs/loop.json`
    only once the lease file exists, all in this one critical section. A
    budget checked here but charged later would let two sessions at the last
    unit both claim; a refused claim spends nothing.

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
    with mutex(ctx, "registry"):
        return _acquire_held(ctx, issue, session, attempt, started, count_slot, loop_max)


def _acquire_held(ctx: Ctx, issue: int, session: str, attempt: int,
                  started: float | None, count_slot: bool, loop_max: int | None) -> Lock:
    """`acquire`'s body. Assumes mutex `registry` is held."""
    entries = [lk for _, lk in _entries(ctx)]
    mine = next((lk for lk in entries if lk.run_id == ctx.run_id), None)
    if mine is not None:
        raise LockHeld("run-holds-a-lock", mine)
    loop_after = None
    if loop_max is not None:
        try:
            loop_after = budget.loop_next_held(ctx, loop_max)
        except budget.BudgetExceeded:
            raise LockHeld(LOOP_EXHAUSTED) from None
    held = next((lk for lk in entries if lk.issue == issue), None)
    if held is not None:
        raise LockHeld(ISSUE_HELD, held)
    if count_slot and len(entries) >= config(ctx)["slots"]:
        raise LockHeld(SLOTS_FULL, entries[0] if entries else None)
    now = ctx.now()
    if count_slot:
        aw = ctx.awake_now()
        live = sum(not is_stale(lk, now, aw) for lk in entries)
        free, need = disk_free_gb(ctx), disk_required_gb(ctx, live)
        if free < need:
            raise LockHeld(f"{DISK_LOW}:{free}GB<{need}GB")
        if (t := target_disk(ctx, live)) is not None and t[0] < t[1]:
            raise LockHeld(f"{DISK_LOW}:targets:{t[0]}GB<{t[1]}GB")
    lk = Lock(issue=issue, run_id=ctx.run_id, session=session,
              started=now if started is None else started, heartbeat=now, attempt=attempt)
    _stamp(ctx, lk, now)
    path = ctx.paths.lease(issue)
    path.parent.mkdir(parents=True, exist_ok=True)
    for retried in (False, True):
        try:
            with open(path, "x", encoding="utf-8") as f:
                f.write(json.dumps(_lease_dict(lk), indent=2))
            if loop_after is not None:
                # Charged only now that the lease exists. A failed charge takes
                # the lease back: no lease without its unit, and no unit spent
                # without a lease.
                try:
                    write_json(ctx, ctx.paths.loop, loop_after)
                except BaseException:
                    path.unlink(missing_ok=True)
                    raise
            _write_awake(ctx, lk)            # best-effort: never fails the claim
            return lk
        except FileExistsError:
            # Only a file the check above could not read gets here (a writer
            # outside the registry mutex, or an empty file). If it is gone by
            # now, retry the create once. `lk` is reused, so the retry keeps
            # the caller's `started` and never reverts to `now`.
            existing = _read_lease(path)
            if existing is not None or retried:
                raise LockHeld(ISSUE_HELD, existing)
    raise AssertionError("unreachable")


def check_fence(ctx: Ctx, issue: int | None = None) -> Lock:
    """The run's own lease, else `FenceError`. With `issue`, also refuse when
    the lease is for another issue: a call naming issue 9 under a lease for 8
    must not label, comment on or release anything."""
    lk = own(ctx)
    if lk is None:
        raise FenceError(f"no lease names run {ctx.run_id}")
    if issue is not None and lk.issue != issue:
        raise FenceError(f"lock is for issue {lk.issue}, call names {issue}")
    return lk


def _fenced_entry(ctx: Ctx) -> tuple[Path, Lock]:
    e = _own_entry(ctx)
    if e is None:
        raise FenceError(f"no lease names run {ctx.run_id}")
    return e


def beat(ctx: Ctx) -> None:
    """Refresh this run's heartbeat. Under mutex `registry` (TLS-122, sol r1):
    unlocked, a beat that read the lease before `steal_for_recovery` rewrote
    it would write its old copy back over the steal."""
    ctx.write_guard("beat")
    with mutex(ctx, "registry"):
        path, lk = _fenced_entry(ctx)
        _stamp(ctx, lk, ctx.now())
        _write(ctx, path, lk)


def release(ctx: Ctx) -> None:
    """Remove this run's lease and its sidecar, under mutex `registry` (sol r2):
    unlocked, a claim between the two unlinks would publish a new lease and
    stamp at the same paths, and this release would delete the new stamp."""
    ctx.write_guard("release lock")
    with mutex(ctx, "registry"):
        path, lk = _fenced_entry(ctx)
        path.unlink()
        try:
            _awake_path(ctx, lk.issue).unlink(missing_ok=True)
        except OSError:
            pass                             # optional metadata: never fails a release


def steal_for_recovery(ctx: Ctx, stale: Lock) -> Lock:
    """Mark `stale`'s lease as being recovered by this run, and make it live.

    Under mutex `registry` the lease is re-read: it must still name
    `stale.run_id` (else `lock-gone`) and still be stale (else `lock-live`:
    another session stole it first). It is rewritten with `heartbeat = now`
    and `recovering_by = ctx.run_id`, and KEEPS `run_id = stale.run_id`.
    Live, a second session's `recover` leaves it alone; with the old run id,
    `recover_stale`'s bookkeeping under the old run's fence still passes.
    `handover` or `drop` ends it. A run that already owns a lease is refused
    `run-holds-a-lock`: one run, one issue.
    """
    ctx.write_guard("steal lease for recovery")
    if not ctx.run_id:
        raise RuntimeError("steal_for_recovery needs a run_id")
    with mutex(ctx, "registry"):
        entries = _entries(ctx, held=True)
        mine = next((lk for _, lk in entries if lk.run_id == ctx.run_id), None)
        if mine is not None:
            raise LockHeld("run-holds-a-lock", mine)
        e = next(((p, lk) for p, lk in entries if lk.issue == stale.issue), None)
        if e is None or e[1].run_id != stale.run_id:
            raise LockHeld("lock-gone", e[1] if e else None)
        path, cur = e
        now = ctx.now()
        # `is_stale`, not `_live_held`: a beat that died between its writes
        # (`beat_pending`) must stay recoverable, or its lease is stuck forever.
        if not is_stale(cur, now, ctx.awake_now()):
            raise LockHeld("lock-live", cur)
        _stamp(ctx, cur, now)
        cur.recovering_by = ctx.run_id
        _write(ctx, path, cur)
        return cur


def handover(ctx: Ctx, issue: int, *, started: float) -> Lock:
    """Replace the lease this run is recovering with its own follow-through
    lease (`run_id = ctx.run_id`, session `recover`), in one atomic write.

    The issue has a lease at every moment: the old one until `os.replace`,
    the new one after. A legacy `lock.json` lease is replaced by a lease
    under `locks/` written FIRST, then the legacy file goes, so the overlap
    is two leases, never none. `started` is the original claim's date (see
    `acquire`); the heartbeat is `now`.
    """
    ctx.write_guard("hand over recovered lease")
    with mutex(ctx, "registry"):
        e = next(((p, lk) for p, lk in _entries(ctx) if lk.issue == issue), None)
        if e is None or e[1].recovering_by != ctx.run_id:
            raise FenceError(f"the lease for issue {issue} is not being recovered by run {ctx.run_id}")
        path, old = e
        new = Lock(issue=issue, run_id=ctx.run_id, session="recover", started=started,
                   heartbeat=ctx.now(), attempt=old.attempt)
        _stamp(ctx, new, new.heartbeat)
        target = ctx.paths.lease(issue)
        _write(ctx, target, new)
        if path != target:
            path.unlink(missing_ok=True)
        # The merge lane follows the lease: a lane the dead run took for this
        # merge is the follow-through's to verify and release. Already under
        # `registry`, so `_lane_write` takes no mutex.
        lane = read_json(ctx.paths.merge_lane)
        if lane and lane.get("run_id") == old.run_id:
            _lane_write(ctx, {**lane, "run_id": ctx.run_id})
        return new


# ---- the merge lane ---------------------------------------------------------
# `.agent/merge-lane.json` = {run_id, issue, pr, merge_sha, acquired}: one
# merge at a time, from `merge` until its `post-merge` verified it. A LEASE,
# not a mutex, because it spans three CLI calls (`merge`, `post-merge`,
# `finish`). It has no heartbeat of its own: it is held while its owner's
# issue lease is live. The owner lease is found BY ISSUE and must still name
# the lane's run: during a recovery the lease is live but names the dead run.

def _lane_write(ctx: Ctx, lane: dict) -> None:
    ctx.write_guard("write merge lane")
    write_json(ctx, ctx.paths.merge_lane, lane)


def _live_held(lk: Lock, now: float, awake) -> bool:
    """Live, for a command that ACTS on it, read under mutex `registry`: not
    stale, or a beat that died between its two writes (`beat_pending`). Only
    `clear-halt`, `resolve incident`, verification `cleanup` and `take_lane`
    judge this way; recovery does not, or such a lease could never be taken."""
    return lk.beat_pending or not is_stale(lk, now, awake)


def _lane_state_held(ctx: Ctx, held: bool = True) -> tuple[dict | None, str]:
    lane = read_json(ctx.paths.merge_lane)
    if not lane:
        return None, "free"
    owner = next((lk for _, lk in _entries(ctx, held)
                  if lk.issue == lane.get("issue") and lk.run_id == lane.get("run_id")), None)
    if owner is None:
        return lane, "free"          # the owner's lease is gone: nothing holds it
    if lane["run_id"] == ctx.run_id:
        return lane, "mine"
    return lane, "busy" if _live_held(owner, ctx.now(), ctx.awake_now()) else "orphaned"


def lane_state(ctx: Ctx) -> tuple[dict | None, str]:
    """`(lane, state)`, state one of `free`, `mine`, `busy` (another run with
    a live lease holds it) or `orphaned` (its owner's lease is stale: only
    `recover` moves it on). A read for REPORTING; takes no mutex. A command
    that acts on the answer uses `lane_state_locked` under `registry`."""
    return _lane_state_held(ctx, held=False)


def lane_state_locked(ctx: Ctx) -> tuple[dict | None, str]:
    """`lane_state` for a caller that HOLDS mutex `registry` and acts on the
    answer before releasing it (verification `cleanup`, sol r4)."""
    return _lane_state_held(ctx, held=True)


def take_lane(ctx: Ctx, issue: int, pr: int | None, merge_sha: str | None = None) -> dict:
    """Hold the lane for this run, under `registry`. Free or already ours:
    (re)written. Otherwise `LockHeld` `merge-lane-busy` / `merge-lane-orphaned`."""
    ctx.write_guard("take merge lane")
    with mutex(ctx, "registry"):
        lane, state = _lane_state_held(ctx)
        if state in LANE_REFUSED:
            raise LockHeld(f"merge-lane-{state}")
        new = {"run_id": ctx.run_id, "issue": issue, "pr": pr, "merge_sha": merge_sha,
               "acquired": lane["acquired"] if state == "mine" else ctx.now()}
        _lane_write(ctx, new)
        return new


def release_lane(ctx: Ctx, run_id: str | None = None) -> bool:
    """Free the lane if it names `run_id` (default: this run). True if freed."""
    ctx.write_guard("release merge lane")
    with mutex(ctx, "registry"):
        lane = read_json(ctx.paths.merge_lane)
        if not lane or lane.get("run_id") != (run_id or ctx.run_id):
            return False
        ctx.paths.merge_lane.unlink(missing_ok=True)
        return True


def drop(ctx: Ctx, issue: int, expect_run_id: str) -> bool:
    """Delete issue `issue`'s lease only if it still names `expect_run_id`.
    For `recover`, which removes a lease it does not own. True if removed."""
    ctx.write_guard("drop lease")
    with mutex(ctx, "registry"):
        for path, lk in _entries(ctx):
            if lk.issue == issue and lk.run_id == expect_run_id:
                path.unlink(missing_ok=True)
                try:
                    _awake_path(ctx, issue).unlink(missing_ok=True)
                except OSError:
                    pass                     # optional metadata
                return True
    return False


def is_stale(lk: Lock, now: float, awake: tuple[str, float] | None) -> bool:
    """No heartbeat for STALE_SECONDS of AWAKE time (TLS-122). The wall clock
    counts a host suspend, so after a long one a live run's lease read as stale
    and another session could recover it before the run's next beat. With
    `awake` (`Ctx.awake_now()`) and a lease stamped on the same boot, the age is
    the awake clock's; otherwise (a lease from before this rule, from before a
    reboot, or no awake clock) it is the wall clock's, as before.

    Why not "stale only when the holder process is dead": a run is a series of
    short CLI calls from a conductor session, so between calls no process
    holds the lease, and a session that stopped working must still lose it."""
    return awake_elapsed(lk.beat_awake, awake, now - lk.heartbeat)[0] > STALE_SECONDS


def require_human(ctx: Ctx, what: str) -> None:
    """Refusal (1) of `require_operator` alone: a run id means automation.
    `grant` uses only this half, because the run it extends is live by design."""
    if ctx.run_id:
        raise OperatorOnly(
            f"{what} is operator-only; this call carries run id {ctx.run_id!r}. "
            f"Run it with no --run-id and with AGENTFLOW_RUN_ID unset.")


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
    counts only live leases toward `slots-full`.

    ANY live lease refuses, not only "the" lock: with several runs, one live
    run is enough to make a clear unsafe.
    """
    require_human(ctx, what)
    with mutex(ctx, "registry"):
        _refuse_live_held(ctx, what)


def _refuse_live_held(ctx: Ctx, what: str) -> None:
    """`require_operator`'s live-run half. ASSUMES mutex `registry` is held
    (sol r4): an unlocked read can see a beat half-written and take a live
    run for a stale one; under `registry` no beat is half-written."""
    now, aw = ctx.now(), ctx.awake_now()
    lk = next((x for _, x in _entries(ctx, held=True) if _live_held(x, now, aw)), None)
    if lk is not None:
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
    except (OSError, MutexTimeout):
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

    Steps 2-5 run under mutex `registry` (sol r4): the live-run check reads
    complete lease/sidecar pairs, and no run can change its lease between the
    check and the unlink.
    """
    ctx.write_guard("clear HALT")
    require_human(ctx, "clear HALT")
    with mutex(ctx, "registry"):
        _refuse_live_held(ctx, "clear HALT")
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
