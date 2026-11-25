import hashlib
import json
import os
import time
from contextlib import contextmanager
from dataclasses import asdict, dataclass
from pathlib import Path

import pytest

from agentflow import budget, lock, state
from agentflow.state import Ctx, DryRunViolation, MutexTimeout, Paths, tree_snapshot


def test_acquire_creates_lock_and_second_acquire_loses(ctx):
    """With the default `slots: 1`, a second claim of ANOTHER issue still
    loses, as it did with one lock (`slots-full`), and a second claim of the
    SAME issue loses for its own reason (`issue-held`)."""
    got = lock.acquire(ctx, issue=8, session="s1", attempt=1)
    assert got.issue == 8 and got.run_id == "run-test" and got.attempt == 1
    assert ctx.paths.lease(8).exists() and not ctx.paths.lock.exists()
    other = Ctx(paths=ctx.paths, run_id="run-other", now=ctx.now)
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(other, issue=9, session="s2", attempt=1)
    assert e.value.code == "slots-full" and e.value.existing.run_id == "run-test"
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(other, issue=8, session="s2", attempt=1)
    assert e.value.code == "issue-held:run-test"
    assert [lk.run_id for lk in lock.all_locks(ctx)] == ["run-test"]


def _slots(ctx, n):
    ctx.paths.config.write_text(json.dumps({"slots": n}), encoding="utf-8")


def test_two_slots_take_two_issues_and_a_stale_lease_still_counts(ctx):
    """Design test 7. `slots: 2`: two runs claim two issues; a third is
    `slots-full`. A STALE lease still holds its slot: only `recover` frees it,
    so a crashed run cannot be over-subscribed around."""
    _slots(ctx, 2)
    a = lock.acquire(ctx, 8, "s", 1)
    b = lock.acquire(Ctx(paths=ctx.paths, run_id="run-b", now=ctx.now), 9, "s", 1)
    assert (a.issue, b.issue) == (8, 9)
    later = lambda: 1_000_000.0 + lock.STALE_SECONDS + 1          # noqa: E731 -- both leases stale now
    third = Ctx(paths=ctx.paths, run_id="run-c", now=later)
    assert all(lock.is_stale(lk, later(), None) for lk in lock.all_locks(ctx))
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(third, 10, "s", 1)
    assert e.value.code == "slots-full"
    lock.release(ctx)                                             # one slot frees
    assert lock.acquire(third, 10, "s", 1).issue == 10
    assert sorted(lk.issue for lk in lock.all_locks(ctx)) == [9, 10]


def test_one_run_holds_one_lease(ctx):
    _slots(ctx, 3)
    lock.acquire(ctx, 8, "s", 1)
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(ctx, 9, "s", 1)
    assert e.value.code == "run-holds-a-lock"
    assert not ctx.paths.lease(9).exists()


def test_unblock_style_acquire_does_not_count_a_slot_but_still_respects_the_issue(ctx):
    lock.acquire(ctx, 8, "s", 1)                                  # slots: 1 is now full
    unblock = Ctx(paths=ctx.paths, run_id="unblock-abc123", now=ctx.now)
    assert lock.acquire(unblock, 9, "human", 0, count_slot=False).issue == 9
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(Ctx(paths=ctx.paths, run_id="unblock-def456", now=ctx.now), 8, "human", 0,
                     count_slot=False)
    assert e.value.code == "issue-held:run-test"


def test_a_loop_charge_that_fails_takes_the_new_lease_back(ctx, monkeypatch):
    """`acquire(loop_max=...)` charges `loop.json` after creating the lease.
    If that write fails, the lease goes too: a lease whose budget unit was
    never spent would be a claim the loop never counted."""
    def failing(c, path, data):
        raise OSError("disk full")
    monkeypatch.setattr(lock, "write_json", failing)
    with pytest.raises(OSError):
        lock.acquire(ctx, 8, "s", 1, loop_max=3)
    assert lock.all_locks(ctx) == [] and not ctx.paths.lease(8).exists()


def test_check_fence_with_an_issue_refuses_another_issue(ctx):
    """Design test 12, at the library level: the own lease is not enough when
    the call names a different issue."""
    lock.acquire(ctx, 8, "s", 1)
    assert lock.check_fence(ctx).issue == 8
    assert lock.check_fence(ctx, issue=8).issue == 8
    with pytest.raises(lock.FenceError, match="lock is for issue 8, call names 9"):
        lock.check_fence(ctx, issue=9)


def test_a_sibling_lease_is_not_this_runs_fence(ctx):
    """With two leases, `check_fence`, `beat` and `release` act on the run's
    OWN lease and never on the sibling's."""
    _slots(ctx, 2)
    lock.acquire(ctx, 8, "s", 1)
    sib = Ctx(paths=ctx.paths, run_id="run-sib", now=lambda: 1_000_500.0)
    lock.acquire(sib, 9, "s", 1)
    lock.beat(sib)
    assert lock.for_issue(ctx, 8).heartbeat == 1_000_000.0
    assert lock.for_issue(ctx, 9).heartbeat == 1_000_500.0
    lock.release(sib)
    assert lock.for_issue(ctx, 9) is None and lock.for_issue(ctx, 8).run_id == "run-test"
    with pytest.raises(lock.FenceError):
        lock.check_fence(sib)
    assert lock.read(sib).run_id == "run-test"     # the shim: the ONLY lease


LEGACY = {"issue": 8, "run_id": "run-legacy", "session": "s", "started": 999_000.0,
          "heartbeat": 999_500.0, "attempt": 2}


def test_a_legacy_lock_json_is_one_more_lease(ctx):
    """The lazy migration. A `lock.json` written by the old code (no
    `recovering_by` key) is read, counted and fenced like any lease, and its
    owner beats and releases it IN PLACE. No file moves."""
    ctx.paths.lock.write_text(json.dumps(LEGACY), encoding="utf-8")
    legacy = Ctx(paths=ctx.paths, run_id="run-legacy", now=lambda: 1_000_000.0)
    assert lock.own(legacy).issue == 8 and lock.for_issue(ctx, 8).run_id == "run-legacy"
    assert lock.check_fence(legacy, issue=8).attempt == 2
    with pytest.raises(lock.LockHeld) as e:                       # it blocks its issue
        lock.acquire(ctx, 8, "s", 1)
    assert e.value.code == "issue-held:run-legacy"
    with pytest.raises(lock.LockHeld) as e:                       # and holds a slot
        lock.acquire(ctx, 9, "s", 1)
    assert e.value.code == "slots-full"
    lock.beat(legacy)
    assert json.loads(ctx.paths.lock.read_text(encoding="utf-8"))["heartbeat"] == 1_000_000.0
    assert not ctx.paths.locks_dir.exists()                       # never copied to locks/
    lock.release(legacy)
    assert not ctx.paths.lock.exists() and lock.all_locks(ctx) == []
    assert lock.acquire(ctx, 8, "s", 3).issue == 8                # a new lease, under locks/
    assert ctx.paths.lease(8).exists() and not ctx.paths.lock.exists()


def test_acquire_carries_started_but_never_the_heartbeat(ctx):
    """`started` dates the CLAIM; `heartbeat` says the holder is alive. The
    recovery follow-through carries the first forward and must never carry
    the second, or the lock it mints is born stale and any concurrent tick
    would read a live follow-through as recoverable.

    PRECONDITION HAND-STATED: the two values are written as literals, so this
    pins the field semantics rather than whatever `cmd_recover` happens to
    pass today."""
    lk = lock.acquire(ctx, 8, "recover", 1, started=12.0)
    assert lk.started == 12.0
    assert lk.heartbeat == 1_000_000.0          # `ctx.now()`, not the carried date
    assert not lock.is_stale(lk, 1_000_000.0, None)
    on_disk = json.loads(ctx.paths.lease(8).read_text(encoding="utf-8"))
    assert on_disk["started"] == 12.0 and on_disk["heartbeat"] == 1_000_000.0
    # ...and the default is unchanged: an ordinary claim dates itself `now`.
    ctx.paths.lease(8).unlink()
    assert lock.acquire(ctx, 8, "s", 1).started == 1_000_000.0


def test_acquire_forwards_started_through_the_lost_race_retry(ctx, monkeypatch):
    """THE PATH NO FIXTURE DRIVES. `acquire` retries itself when the `x` open
    loses to an existing file that has since been released, and a parameter
    that is not forwarded there silently reverts to `now` -- on exactly the
    branch nothing else exercises.

    PRECONDITION HAND-STATED: the per-issue lease file is written by this
    test with no lease in it, so the registry check reads nothing there and
    the create-exclusive open fails. The race is staged by the NEXT read of
    that file, which removes it and reports absence -- which is what "the
    holder released between our open and our read" looks like from inside
    `acquire`."""
    lease = ctx.paths.lease(8)
    lease.parent.mkdir(parents=True)
    lease.write_text("{}", encoding="utf-8")            # someone else holds it
    real, seen = lock._read_lease, []

    def racy_read(path):
        if path != lease:
            return real(path)
        seen.append(path)
        if len(seen) == 2:                              # the read after the failed create
            path.unlink()                               # ...and has just released it
            return None
        return real(path)

    monkeypatch.setattr(lock, "_read_lease", racy_read)
    lk = lock.acquire(ctx, 8, "recover", 1, started=12.0)
    assert len(seen) == 2                               # the retry branch really ran
    assert lk.started == 12.0 and lk.heartbeat == 1_000_000.0
    assert json.loads(lease.read_text(encoding="utf-8"))["started"] == 12.0


def test_beat_updates_heartbeat_and_release_removes(ctx):
    lock.acquire(ctx, 8, "s1", 1)
    later = Ctx(paths=ctx.paths, run_id="run-test", now=lambda: 1_000_500.0)
    lock.beat(later)
    assert lock.read(ctx).heartbeat == 1_000_500.0
    lock.release(later)
    assert lock.read(ctx) is None


def test_beat_rewrites_the_lease_through_the_atomic_writer(ctx, monkeypatch):
    """A `beat` that wrote the lease in place let a reader in another process
    see half a file. The supervisor beats while the conductor reads, so this
    already happened with one run. The rewrite must be a temp file plus
    `os.replace` onto the lease."""
    lock.acquire(ctx, 8, "s1", 1)
    real, targets = os.replace, []

    def recording(src, dst):
        targets.append(Path(dst))
        return real(src, dst)

    monkeypatch.setattr(os, "replace", recording)
    lock.beat(Ctx(paths=ctx.paths, run_id="run-test", now=lambda: 1_000_500.0))
    assert targets == [ctx.paths.lease(8)]
    assert lock.read(ctx).heartbeat == 1_000_500.0
    # The legacy twin: a `lock.json` lease is rewritten in place, atomically.
    lock.release(ctx)
    ctx.paths.lock.write_text(json.dumps(LEGACY), encoding="utf-8")
    targets.clear()
    lock.beat(Ctx(paths=ctx.paths, run_id="run-legacy", now=lambda: 1_000_600.0))
    assert targets == [ctx.paths.lock]
    assert lock.for_issue(ctx, 8).heartbeat == 1_000_600.0


def test_fence_refuses_foreign_run(ctx):
    lock.acquire(ctx, 8, "s1", 1)
    foreign = Ctx(paths=ctx.paths, run_id="run-foreign", now=ctx.now)
    with pytest.raises(lock.FenceError):
        lock.beat(foreign)
    with pytest.raises(lock.FenceError):
        lock.release(foreign)
    assert lock.read(ctx).run_id == "run-test"


def test_stale_is_heartbeat_age_not_start_age(ctx):
    got = lock.acquire(ctx, 8, "s1", 1)
    assert not lock.is_stale(got, got.heartbeat + lock.STALE_SECONDS - 1, None)
    assert lock.is_stale(got, got.heartbeat + lock.STALE_SECONDS + 1, None)


# ---- TLS-122: a host suspend is not lease age, nor run time ------------------
# Both clocks are stubbed: `now` (wall) and `awake` ((boot id, seconds), what
# `state.host_awake` reads). A suspend moves the wall clock and not the awake one.

class Clocks:
    def __init__(self):
        self.wall, self.up, self.boot = 1_000_000.0, 500.0, "boot-1"

    def ctx(self, paths, run_id):
        return Ctx(paths=paths, run_id=run_id, now=lambda: self.wall,
                   awake=lambda: (self.boot, self.up))

    def run(self, s):
        self.wall += s
        self.up += s

    def suspend(self, s):
        self.wall += s


def test_a_live_lease_across_a_2h_suspend_is_not_stolen(ctx):
    """The holder's heartbeat is 2 h old by the wall clock, but the host slept
    through it: the lease is live, and `recover`'s steal refuses `lock-live`.
    The same lease after 30 min AWAKE with no beat is stale, and stolen."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-holder"), 8, "s", 1)
    c.run(60)
    c.suspend(2 * 3600)
    rec = c.ctx(ctx.paths, "run-rec")
    lk = lock.for_issue(rec, 8)
    assert not lock.is_stale(lk, rec.now(), rec.awake_now())
    with pytest.raises(lock.LockHeld) as e:
        lock.steal_for_recovery(rec, lk)
    assert e.value.code == "lock-live:run-holder"
    c.run(lock.STALE_SECONDS)
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


def test_beat_refreshes_the_awake_stamp(ctx):
    """A run beating every 20 awake minutes stays live: `beat` must move the
    awake stamp, not only the wall heartbeat."""
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-holder")
    lock.acquire(holder, 8, "s", 1)
    for _ in range(3):
        c.run(20 * 60)
        lock.beat(holder)
    assert not lock.is_stale(lock.for_issue(holder, 8), holder.now(), holder.awake_now())


def test_a_lease_from_another_boot_ages_on_the_wall_clock(ctx):
    """After a reboot (another boot id) the awake readings cannot be compared:
    the wall clock judges, as before TLS-122."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-holder"), 8, "s", 1)
    c.run(60)
    c.boot, c.up = "boot-2", 10.0      # rebooted: the awake clock restarted
    c.wall += lock.STALE_SECONDS
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.is_stale(lock.for_issue(rec, 8), rec.now(), rec.awake_now())


def test_the_budget_leaves_a_3h_suspend_out(ctx):
    """Claimed, then 22 h of work and a 3 h suspend: 25 h of wall time, 22 h of
    run time, so no deadline. 2 h and 1 s more awake pass the 24 h cap."""
    c = Clocks()
    run = c.ctx(ctx.paths, ctx.run_id)
    budget.init(run, claimed_at=run.now())
    c.run(22 * 3600)
    c.suspend(3 * 3600)
    budget.check_deadline(run)
    assert budget.charge(run, "subagents") == budget.CAPS["subagents"] - 1
    snap = budget.snapshot(run)
    assert (snap["elapsed_s"], snap["elapsed_clock"]) == (22 * 3600, "awake")
    c.run(2 * 3600 + 1)
    with pytest.raises(budget.DeadlineExceeded):
        budget.check_deadline(run)


def test_the_awake_claim_stamp_is_checksummed(ctx):
    """Moving the sidecar's claim stamp later would extend the cap: its checksum
    no longer matches, so the run is timed on the wall clock (25 h: past the cap).
    Red proof (TLS-122 sol r1): the checksum test dropped from
    `budget._claimed_awake` failed here (the edited stamp gave 15 h, no deadline)."""
    c = Clocks()
    run = c.ctx(ctx.paths, ctx.run_id)
    budget.init(run, claimed_at=run.now())
    c.run(22 * 3600)
    c.suspend(3 * 3600)
    p = run.run_dir / "budget-awake.json"
    data = json.loads(p.read_text(encoding="utf-8"))
    data["awake"][1] += 7 * 3600
    p.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(budget.DeadlineExceeded):
        budget.check_deadline(run)
    assert budget.snapshot(run)["elapsed_clock"] == "wall"


def _put_stamp(paths, issue, stamp, signed=True):
    """Replace the awake stamp in a lease's sidecar, keeping it matched to the
    lease. `signed`: with a valid checksum (as a buggy writer would leave it),
    so the stamp reaches `awake_elapsed`'s own checks; unsigned is a hand edit,
    which the checksum catches first."""
    p = paths.locks_dir / "awake" / f"issue-{issue}.json"
    s = {**json.loads(p.read_text(encoding="utf-8")), "awake": stamp}
    if signed:
        s["checksum"] = state.meta_sum({"run_id": s["run_id"], "heartbeat": s["heartbeat"], "awake": stamp})
    p.write_text(json.dumps(s), encoding="utf-8")


def test_a_stolen_lease_survives_a_suspend(ctx):
    """`steal_for_recovery` stamps the awake clock too: after a 2 h suspend a
    second recoverer gets `lock-live`, not a second steal.
    Red proof (TLS-122 r1): `_stamp(ctx, cur, now)` in the steal replaced by
    `cur.heartbeat = now` failed here (the second steal succeeded)."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-dead"), 8, "s", 1)
    c.run(lock.STALE_SECONDS + 1)
    first = c.ctx(ctx.paths, "run-rec1")
    lock.steal_for_recovery(first, lock.for_issue(first, 8))
    c.run(60)
    c.suspend(2 * 3600)
    second = c.ctx(ctx.paths, "run-rec2")
    with pytest.raises(lock.LockHeld) as e:
        lock.steal_for_recovery(second, lock.for_issue(second, 8))
    assert e.value.code == "lock-live:run-dead"


def test_a_handed_over_lease_survives_a_suspend(ctx):
    """`handover`'s new lease is stamped on the awake clock: the recovering
    run's follow-through stays live across a 2 h suspend.
    Red proof (TLS-122 r1): `_stamp(ctx, new, new.heartbeat)` in `handover`
    replaced by `pass` failed here (the lease read stale, and was stolen)."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-dead"), 8, "s", 1)
    c.run(lock.STALE_SECONDS + 1)
    rec = c.ctx(ctx.paths, "run-rec")
    lock.steal_for_recovery(rec, lock.for_issue(rec, 8))
    lock.handover(rec, 8, started=1_000_000.0)
    c.run(60)
    c.suspend(2 * 3600)
    other = c.ctx(ctx.paths, "run-other")
    with pytest.raises(lock.LockHeld) as e:
        lock.steal_for_recovery(other, lock.for_issue(other, 8))
    assert e.value.code == "lock-live:run-rec"


def test_a_lease_and_a_budget_with_an_unknown_key_still_load(ctx):
    """Forward compatibility: a field a newer writer added is ignored.
    Red proof (TLS-122 r1): `_read_lease` back to `Lock(**data)` failed here
    (TypeError: unexpected keyword). The budget reader needed no change: it
    keeps the dict and checksums only the keys it knows."""
    lock.acquire(ctx, 8, "s", 1)
    p = ctx.paths.lease(8)
    data = json.loads(p.read_text(encoding="utf-8"))
    p.write_text(json.dumps({**data, "from_the_future": 1}), encoding="utf-8")
    assert lock.for_issue(ctx, 8).run_id == "run-test"
    budget.init(ctx, claimed_at=ctx.now())
    b = ctx.run_dir / "budget.json"
    b.write_text(json.dumps({**json.loads(b.read_text(encoding="utf-8")), "from_the_future": 1}),
                 encoding="utf-8")
    budget.check_deadline(ctx)


@pytest.mark.parametrize("bad", [["boot-1"], ["boot-1", "x"], ["boot-1", True], "boot-1",
                                 ["boot-1", 1, 2], [1, 2], {"boot": "boot-1"},
                                 ["boot-1", float("nan")], ["boot-1", float("-inf")]])
def test_a_malformed_awake_stamp_counts_as_none(ctx, bad):
    """An awake stamp that is not `[str, number]` is no stamp: the lease ages
    on the wall clock, and listing it never crashes.
    Red proof (TLS-122 r1): the shape test dropped from `awake_elapsed`
    failed here for 5 of the 7 shapes (IndexError, TypeError, KeyError, or a
    boolean read as a time); the bare string and `[1, 2]` fell back by chance.
    Red proof (Fable r2 nit): `math.isfinite` dropped failed the NaN shape
    (never stale) and the -inf one (stale at once); restored, passes.
    Re-run on sol r1's code (the stamp in the sidecar): the shape break failed
    the same 5 shapes. The `math.isfinite` break now PASSES: sol r1's range
    check (`0 <= s <= now + tolerance`) is false for NaN and both infinities,
    so `math.isfinite` is a redundant, explicit check. (The stamps here are
    signed, so they reach `awake_elapsed` past the sidecar checksum.)
    Since sol r3 the shape test is `state.is_stamp`, run by `_read_awake`
    before the checksum and by `awake_elapsed`: loosened, it failed the same
    5 shapes."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-holder"), 8, "s", 1)
    _put_stamp(ctx.paths, 8, bad)
    rd = c.ctx(ctx.paths, "run-rd")
    assert not lock.is_stale(lock.for_issue(rd, 8), rd.now(), rd.awake_now())
    c.suspend(lock.STALE_SECONDS + 1)          # wall time only: the wall rule judges
    assert lock.is_stale(lock.for_issue(rd, 8), rd.now(), rd.awake_now())


def test_a_stamped_lease_read_without_an_awake_clock_ages_on_wall_time(ctx):
    """The documented fall back: a reader with no awake clock (not Linux, or a
    stubbed wall clock) judges a stamped lease by the wall clock.
    Red proof (TLS-122 r1): `now_awake and` dropped from `awake_elapsed`
    failed here (TypeError: None is not subscriptable)."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-holder"), 8, "s", 1)
    c.suspend(lock.STALE_SECONDS + 1)
    rd = Ctx(paths=ctx.paths, run_id="run-rd", now=lambda: c.wall)
    assert rd.awake_now() is None
    assert lock.is_stale(lock.for_issue(rd, 8), rd.now(), rd.awake_now())


# ---- TLS-122 sol r1 ----------------------------------------------------------

@pytest.mark.parametrize("stamp", [1e20, 10**400, 500.0 + 61])
def test_a_dead_holders_lease_with_a_future_stamp_is_still_recovered(ctx, stamp):
    """A stamp ahead of the reader's clock (beyond AHEAD_TOLERANCE_S) is no
    reading: the dead holder's lease ages on the wall clock and is recovered,
    never stranded by a negative age; a 400-digit integer does not crash.
    Red proof (sol r1): the `<= now_awake[1] + AHEAD_TOLERANCE_S` bound
    dropped failed the 1e20 and 561 s cases (`lock-live`: stranded); the
    `except OverflowError` dropped failed the 400-digit case (OverflowError)."""
    c = Clocks()                                   # the reader's awake clock reads 500 s
    lock.acquire(c.ctx(ctx.paths, "run-dead"), 8, "s", 1)
    _put_stamp(ctx.paths, 8, ["boot-1", stamp])
    c.suspend(lock.STALE_SECONDS + 1)              # no beat for 30 min of wall time
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


@pytest.mark.parametrize("stamp,signed", [(-1e20, True), (-1.0, True), (1.0, False)])
def test_a_fresh_lease_with_an_early_stamp_is_not_recovered(ctx, stamp, signed):
    """A negative stamp, or a hand-edited early one, would make a just-beaten
    lease look stale and let recovery take a live run's worktree. It is judged
    on the wall clock instead: live.
    Since sol r2 the wall age no longer caps the awake age (a backward wall
    step would defeat a real stamp). A negative stamp is refused by `0 <= s`;
    an early positive one (1.0) can only come from an edit, and the sidecar's
    checksum refuses it. A correctly signed early stamp is a real reading: it
    is believed, by design.
    Red proof (sol r2): `0 <= s` dropped from `awake_elapsed` failed the two
    negative cases (stolen); the checksum test dropped from `lock._read_awake`
    failed the 1.0 case (stolen)."""
    c = Clocks()
    c.up = 10 * 3600.0                             # the host has been up 10 h
    lock.acquire(c.ctx(ctx.paths, "run-live"), 8, "s", 1)
    _put_stamp(ctx.paths, 8, ["boot-1", stamp], signed=signed)
    c.run(60)
    rec = c.ctx(ctx.paths, "run-rec")
    with pytest.raises(lock.LockHeld) as e:
        lock.steal_for_recovery(rec, lock.for_issue(rec, 8))
    assert e.value.code == "lock-live:run-live"


def test_a_stamp_slightly_ahead_of_the_reader_counts(ctx):
    """Readers that sample their clock before loading the lease can see a
    stamp a little ahead (a beat in between): within AHEAD_TOLERANCE_S it is a
    reading, with age 0, so the lease is live and judged on the awake clock."""
    then = ["boot-1", 530.0]
    assert state.awake_elapsed(then, ("boot-1", 500.0), 10.0) == (0.0, "awake")
    assert state.awake_elapsed(then, ("boot-1", 500.0 - state.AHEAD_TOLERANCE_S), 10.0) == (10.0, "wall")


# Master's readers before TLS-122 (c6f61f04, lock.py 78-93 and budget.py 45-58),
# copied: what an older executor sharing `.agent/` runs.

@dataclass
class OldLock:
    issue: int
    run_id: str
    session: str
    started: float
    heartbeat: float
    attempt: int
    recovering_by: str | None = None


def old_read_lease(path):
    data = json.loads(path.read_text(encoding="utf-8"))
    return OldLock(**data) if data else None


def old_checksum(counts, claimed_at, grants=None):
    payload = {"counts": counts, "claimed_at": claimed_at}
    if grants:
        payload["grants"] = grants
    raw = json.dumps(payload, sort_keys=True)
    return hashlib.sha256(raw.encode()).hexdigest()[:16]


def test_an_older_executor_reads_what_this_one_writes(ctx):
    """Coexistence and rollback: every lease file (claimed, beaten, stolen,
    handed over) loads in the pre-TLS-122 reader, and budget.json passes its
    checksum, because the awake stamps live in sidecars it never reads.
    Red proof (sol r1): `_lease_dict` returning `asdict(lk)` (the stamp in the
    lease) failed here with TypeError in `OldLock(**data)`; a new key in
    `budget._checksum`'s payload failed the old checksum."""
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-a")
    lock.acquire(holder, 8, "s", 1)
    budget.init(holder, claimed_at=holder.now())
    budget.charge(holder, "subagents")
    lock.beat(holder)
    c.run(lock.STALE_SECONDS + 1)
    rec = c.ctx(ctx.paths, "run-b")
    lock.steal_for_recovery(rec, lock.for_issue(rec, 8))
    seen = [old_read_lease(p) for p in ctx.paths.locks_dir.glob("issue-*.json")]   # old code's glob
    assert [(o.issue, o.run_id, o.recovering_by) for o in seen] == [(8, "run-a", "run-b")]
    lock.handover(rec, 8, started=1.0)
    assert old_read_lease(ctx.paths.lease(8)).run_id == "run-b"
    b = json.loads((holder.run_dir / "budget.json").read_text(encoding="utf-8"))
    assert b["checksum"] == old_checksum(b["counts"], b["claimed_at"], b.get("grants"))


def test_a_beat_by_an_older_executor_ages_on_the_wall_clock(ctx):
    """An older executor's beat rewrites the lease (new heartbeat), not the
    sidecar: the lease is 15 min old on the wall clock, so live, not 35 min
    on the awake clock since the sidecar's beat.
    Red proof (sol r2, after the wall-age cap went): the heartbeat match
    dropped from `lock._read_awake` failed here (stolen) and in
    `test_a_sidecar_newer_than_the_lease_does_not_count`."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-a"), 8, "s", 1)
    c.run(20 * 60)
    old = old_read_lease(ctx.paths.lease(8))
    old.heartbeat = c.wall
    ctx.paths.lease(8).write_text(json.dumps(asdict(old)), encoding="utf-8")
    c.run(15 * 60)
    rec = c.ctx(ctx.paths, "run-rec")
    with pytest.raises(lock.LockHeld):
        lock.steal_for_recovery(rec, lock.for_issue(rec, 8))


def test_a_sidecar_newer_than_the_lease_does_not_count(ctx):
    """The lease file is the authority. A sidecar written for a LATER heartbeat
    than the lease now holds (an older executor wrote back a copy it read
    before a newer beat) must not make the lease look fresh: it is 35 min old
    on the wall clock since its own heartbeat, so stale.
    Red proof (sol r1): the heartbeat match dropped from `lock._read_awake`
    failed here (the sidecar's 15 min counted: `lock-live`)."""
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-a")
    lock.acquire(holder, 8, "s", 1)
    copy = ctx.paths.lease(8).read_text(encoding="utf-8")   # read before the beat
    c.run(20 * 60)
    lock.beat(holder)
    ctx.paths.lease(8).write_text(copy, encoding="utf-8")   # written back after it
    c.run(15 * 60)
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


def test_beat_takes_the_registry_mutex(ctx, monkeypatch):
    """`beat` rereads and rewrites the lease under `registry`, so it cannot
    write an old copy back over `steal_for_recovery`'s rewrite.
    Red proof (sol r1): `with mutex(ctx, "registry"):` dropped from `beat`
    failed here."""
    taken = []
    real = lock.mutex

    @contextmanager
    def spy(c, name, **kw):
        taken.append(name)
        with real(c, name, **kw):
            yield

    lock.acquire(ctx, 8, "s", 1)
    monkeypatch.setattr(lock, "mutex", spy)
    lock.beat(ctx)
    assert taken == ["registry"]


def test_release_takes_the_registry_mutex(ctx, monkeypatch):
    """The interleaving: `release` unlinks its lease; a sibling claims the
    same issue (new lease and stamp at the same paths); `release` then
    unlinks "its" sidecar, which is now the sibling's. Under `registry` the
    claim cannot land in between (here it times out), and the sibling's stamp
    survives.
    Red proof (sol r2): `with mutex(ctx, "registry"):` dropped from `release`
    failed here (the claim landed in the window and its sidecar was deleted)."""
    short = lambda c, name, timeout=30.0: real(c, name, timeout=0.3)   # noqa: E731
    real = lock.mutex
    monkeypatch.setattr(lock, "mutex", short)
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-a"), 8, "s", 1)
    sib = c.ctx(ctx.paths, "run-b")
    real_unlink = Path.unlink
    claimed = []

    def unlink(self, missing_ok=False):
        real_unlink(self, missing_ok=missing_ok)
        if self == ctx.paths.lease(8) and not claimed:
            claimed.append(True)
            try:
                lock.acquire(sib, 8, "s", 1)          # the sibling's claim, in the window
            except MutexTimeout:
                pass

    monkeypatch.setattr(Path, "unlink", unlink)
    lock.release(c.ctx(ctx.paths, "run-a"))
    monkeypatch.setattr(Path, "unlink", real_unlink)
    assert claimed                                    # the window was reached
    held = lock.for_issue(sib, 8)
    assert held is None or held.beat_awake is not None   # no claim, or one that keeps its stamp


def test_handover_survives_a_failed_sidecar_write(ctx, monkeypatch):
    """The awake sidecar is optional: a failed write must not interrupt the
    lease and merge-lane transfer in `handover` (nor a claim, beat or steal).
    The handed-over lease names the recovering run, and so does the lane.
    Red proof (sol r2): the `except OSError` dropped from `lock._write_awake`
    failed here (OSError out of `handover`, the lane still the dead run's)."""
    c = Clocks()
    dead = c.ctx(ctx.paths, "run-dead")
    lock.acquire(dead, 8, "s", 1)
    lock.take_lane(dead, 8, pr=5)
    c.run(lock.STALE_SECONDS + 1)
    rec = c.ctx(ctx.paths, "run-rec")
    lock.steal_for_recovery(rec, lock.for_issue(rec, 8))
    real_write = lock.write_json

    def failing(cx, path, data):
        if "awake" in path.parts:
            raise OSError("disk full")
        real_write(cx, path, data)

    monkeypatch.setattr(lock, "write_json", failing)
    lock.handover(rec, 8, started=1.0)
    assert lock.for_issue(rec, 8).run_id == "run-rec"
    assert lock.lane_state(rec)[0]["run_id"] == "run-rec"


@pytest.mark.parametrize("junk", ['{"run_', "", "\xff\xfe"])
def test_an_unreadable_sidecar_reads_as_absent(ctx, junk):
    """A truncated or unparsable sidecar (lease or budget) is no stamp: status,
    recover, beat and the deadline all still work, on the wall clock.
    Red proof (sol r2): `read_optional` replaced by `read_json` in
    `lock._read_awake` failed every case here (JSONDecodeError or
    UnicodeDecodeError out of `all_locks`); in `budget._claimed_awake`, the
    budget half failed."""
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-a")
    lock.acquire(holder, 8, "s", 1)
    budget.init(holder, claimed_at=holder.now())
    (ctx.paths.locks_dir / "awake" / "issue-8.json").write_bytes(junk.encode("latin-1"))
    (holder.run_dir / "budget-awake.json").write_bytes(junk.encode("latin-1"))
    assert lock.for_issue(holder, 8).beat_awake is None
    assert budget.snapshot(holder)["elapsed_clock"] == "wall"
    budget.check_deadline(holder)
    lock.beat(holder)
    c.suspend(lock.STALE_SECONDS + 1)
    (ctx.paths.locks_dir / "awake" / "issue-8.json").write_bytes(junk.encode("latin-1"))
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


def test_a_deeply_nested_sidecar_reads_as_absent(ctx):
    """JSON nested deeper than the parser allows (RecursionError) in a lease or
    budget sidecar is no stamp: listing leases, a beat, the deadline and a
    recover still work, on the wall clock (sol r3).
    Red proof (sol r3): `RecursionError` dropped from `state.read_optional`
    failed here (RecursionError out of `all_locks`).
    The shape check before each checksum (`is_stamp`) is defensive: on Python
    3.14 the encoder handled every depth the parser accepted (measured to
    56,000; the parser stops near 58,000), so no parsed value reaches the
    checksum's recursion, and dropping that check here stays green."""
    deep = "[" * 100_000 + "]" * 100_000
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-a")
    lock.acquire(holder, 8, "s", 1)
    budget.init(holder, claimed_at=holder.now())
    side = ctx.paths.locks_dir / "awake" / "issue-8.json"
    side.write_text(deep, encoding="utf-8")
    (holder.run_dir / "budget-awake.json").write_text(deep, encoding="utf-8")
    assert lock.for_issue(holder, 8).beat_awake is None
    budget.check_deadline(holder)
    lock.beat(holder)
    c.suspend(lock.STALE_SECONDS + 1)
    side.write_text(deep, encoding="utf-8")
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


def _race_after_suspend(ctx, monkeypatch):
    """The race (sol r3): after a 2 h suspend, an unlocked reader loads the
    pre-suspend lease; the holder beats (new lease, then new sidecar); the
    reader then loads the NEW sidecar. The pair does not match, and the old
    lease's wall heartbeat (2 h) would read the live run as stale. Returns
    (clocks, armed): `armed` is empty once the beat has landed in the window."""
    c = Clocks()
    holder = c.ctx(ctx.paths, "run-a")
    lock.acquire(holder, 8, "s", 1)
    lock.take_lane(holder, 8, pr=5)
    c.run(60)
    c.suspend(2 * 3600)
    real = lock.read_optional
    armed = [True]

    def beat_first(path):
        if armed and "awake" in path.parts:
            armed.pop()
            lock.beat(holder)                    # lands between the reader's two reads
        return real(path)

    monkeypatch.setattr(lock, "read_optional", beat_first)
    return c, armed


def test_a_beat_in_the_read_window_still_reports_the_lease_live(ctx, monkeypatch):
    """REPORTING (`status`/`preflight` lease rows) sees the live run: unlocked,
    `_entries` reads the lease again on a mismatch and gets the beat's pair.
    (Since sol r4, `clear-halt` itself decides under `registry`, where no beat
    is half-written: see the pause tests in test_cli_flow.)
    Red proof (sol r3, re-run for r4 on the rows): the lease re-read dropped
    from `lock._entries` failed here (the row read stale)."""
    from agentflow import cli
    c, armed = _race_after_suspend(ctx, monkeypatch)
    op = Ctx(paths=ctx.paths, run_id=None, now=lambda: c.wall, awake=lambda: (c.boot, c.up))
    assert [r["stale"] for r in cli._lease_rows(op)] == [False]
    assert not armed                             # the beat did land in the window


def test_a_beat_in_the_read_window_keeps_the_merge_lane_busy(ctx, monkeypatch):
    """The unlocked `lane_state` (reporting: `merge-gate`, `status`, and
    cleanup's early refusal) reads the lane `busy`, not `orphaned`. (Since sol
    r4 cleanup's delete decides again under `registry`: see test_cli_flow.)
    Red proof (sol r3): the lease re-read dropped from `lock._entries` failed
    here (the lane read `orphaned`)."""
    c, armed = _race_after_suspend(ctx, monkeypatch)
    assert lock.lane_state(c.ctx(ctx.paths, "run-x"))[1] == "busy"
    assert not armed


def _between_the_reads(ctx, monkeypatch, act):
    """Run `act()` once, between an unlocked reader's lease read and its
    sidecar read (where the re-read happens), with a mismatched pair."""
    real = lock.read_optional
    armed = [True]

    def hook(path):
        if armed and "awake" in path.parts:
            armed.pop()
            act()
        return real(path)

    monkeypatch.setattr(lock, "read_optional", hook)
    return armed


def test_a_lease_that_vanishes_between_the_reads_is_left_out(ctx, monkeypatch):
    """Unlocked, a lease released between the reader's two reads has no
    coherent snapshot: it is left out, never kept as its first read (sol r4).
    Red proof (sol r4): `continue` replaced by keeping the first read failed
    here (the released lease was listed)."""
    lock.acquire(ctx, 8, "s", 1)                   # no awake clock: no sidecar, a mismatch
    armed = _between_the_reads(ctx, monkeypatch, lambda: lock.release(ctx))
    assert lock.all_locks(ctx) == []
    assert not armed


def test_a_lease_replaced_between_the_reads_is_read_anew(ctx, monkeypatch):
    """Unlocked, a lease replaced between the reads by ANOTHER run with the
    SAME wall heartbeat is the other run's now: the reader reports that run,
    not the first read (sol r4: the heartbeat alone missed it).
    Red proof (sol r4): the re-read compared on the heartbeat alone failed
    here (the released run was still listed)."""
    lock.acquire(ctx, 8, "s", 1)
    other = Ctx(paths=ctx.paths, run_id="run-other", now=ctx.now)   # same `now`: same heartbeat

    def replace():
        lock.release(ctx)
        lock.acquire(other, 8, "s", 1)

    armed = _between_the_reads(ctx, monkeypatch, replace)
    assert [lk.run_id for lk in lock.all_locks(ctx)] == ["run-other"]
    assert not armed


def test_a_backward_wall_step_does_not_save_a_dead_lease(ctx):
    """35 min awake with no beat, while the wall clock stepped back 10 min
    (25 min of wall age): the lease is stale on the awake clock, and recovered.
    Red proof (sol r2): the `age <= wall_elapsed` cap restored in
    `awake_elapsed` failed here (`lock-live`)."""
    c = Clocks()
    lock.acquire(c.ctx(ctx.paths, "run-dead"), 8, "s", 1)
    c.run(35 * 60)
    c.wall -= 10 * 60
    rec = c.ctx(ctx.paths, "run-rec")
    assert lock.steal_for_recovery(rec, lock.for_issue(rec, 8)).recovering_by == "run-rec"


def test_a_backward_wall_step_does_not_extend_the_run(ctx):
    """25 h awake, while the wall clock stepped back 2 h (23 h of wall time):
    the run is past its 24 h cap.
    Red proof (sol r2): the `age <= wall_elapsed` cap restored in
    `awake_elapsed` failed here (no deadline)."""
    c = Clocks()
    run = c.ctx(ctx.paths, ctx.run_id)
    budget.init(run, claimed_at=run.now())
    c.run(25 * 3600)
    c.wall -= 2 * 3600
    with pytest.raises(budget.DeadlineExceeded):
        budget.check_deadline(run)


@pytest.fixture
def fake_proc(tmp_path, monkeypatch):
    """A fake /proc and a fake clock API, so the clock tests run on any OS
    (sol r2): CLOCK_MONOTONIC reads 111.0, CLOCK_BOOTTIME 999.0. The host's
    clock API is deleted first, so every OS runs these tests as Windows
    (which has no `clock_gettime`) would.
    Red proof (sol r2): the `clock_gettime` stub dropped (the API deleted, as
    on Windows) failed the namespace, clock-choice and no-host-API tests:
    `host_awake` returned None. Restored, all pass on every OS."""
    boot = tmp_path / "boot_id"
    boot.write_text("b0\n", encoding="utf-8")
    ns = tmp_path / "timens_offsets"
    ns.write_text("monotonic           0         0\nboottime            0         0\n", encoding="utf-8")
    monkeypatch.setattr(state, "BOOT_ID", boot)
    monkeypatch.setattr(state, "TIMENS_OFFSETS", ns)
    for n in ("clock_gettime", "CLOCK_MONOTONIC", "CLOCK_BOOTTIME"):
        monkeypatch.delattr(time, n, raising=False)
    monkeypatch.setattr(time, "CLOCK_MONOTONIC", 1, raising=False)
    monkeypatch.setattr(time, "CLOCK_BOOTTIME", 7, raising=False)
    monkeypatch.setattr(time, "clock_gettime", lambda clk: {1: 111.0, 7: 999.0}[clk], raising=False)
    return ns


def test_the_clock_tests_need_no_host_clock_api(fake_proc):
    """The fixture alone supplies the clock API (sol r2's portability finding)."""
    assert state.host_awake() == ("b0/0.0", 111.0)


def test_the_clock_id_names_the_time_namespace_offset(ctx, fake_proc):
    """Two time namespaces on one boot can shift CLOCK_MONOTONIC by different
    offsets: a lease stamped in one and read in the other ages on the wall
    clock (here 31 min: stale), never on two incomparable readings.
    Red proof (sol r1): the offset dropped from the clock id failed here (the
    two readings were compared: an age of ~0 s, live)."""
    w = [1_000_000.0]
    real = Ctx(paths=ctx.paths, run_id="run-a", now=lambda: w[0], awake=state.host_awake)
    lock.acquire(real, 8, "s", 1)
    fake_proc.write_text("monotonic        3600         0\nboottime            0         0\n",
                         encoding="utf-8")
    w[0] += lock.STALE_SECONDS + 1
    rd = Ctx(paths=ctx.paths, run_id="run-rd", now=lambda: w[0], awake=state.host_awake)
    assert lock.is_stale(lock.for_issue(rd, 8), rd.now(), rd.awake_now())
    fake_proc.unlink()                             # no time namespaces: offset 0
    assert state.host_awake()[0] == "b0/0.0"


def test_host_awake_reads_clock_monotonic(fake_proc, monkeypatch):
    """The production clock is CLOCK_MONOTONIC, which stops in a suspend.
    CLOCK_BOOTTIME counts the suspend and would bring the theft back.
    Red proof (sol r1): CLOCK_MONOTONIC replaced by CLOCK_BOOTTIME in
    `host_awake` failed here (999.0)."""
    assert state.host_awake() == ("b0/0.0", 111.0)


def test_host_awake_falls_back_when_the_clock_cannot_be_read(fake_proc, monkeypatch):
    """A clock read that fails means no awake clock (wall time), not a crash.
    Red proof (sol r1): the clock read moved out of the `try` failed here."""
    def broken(clk):
        raise OSError("no clock")
    monkeypatch.setattr(time, "clock_gettime", broken, raising=False)
    assert state.host_awake() is None


def test_the_real_awake_clock_reads_on_linux():
    if not Path("/proc/sys/kernel/random/boot_id").exists():
        pytest.skip("no boot id: the awake clock is Linux only")
    boot, up = Ctx(paths=None).awake_now()
    assert boot and up > 0


def test_halt_blocks_unless_terminal(ctx):
    assert lock.halted(ctx) is None
    lock.set_halt(ctx, "regression on master")
    assert lock.halted(ctx) == "regression on master"
    with pytest.raises(lock.HaltError):
        lock.require_not_halted(ctx)
    lock.require_not_halted(ctx, terminal=True)


def test_clear_halt_writes_its_audit_line_before_removing_the_file(ctx, monkeypatch):
    """T8. The ORDERING, proved by breaking the second step rather than by
    reading the code. A clear with no record is the one outcome that must be
    impossible, so the append cannot sit after the unlink.

    PRECONDITION HAND-STATED: an operator context (the `ctx` fixture carries
    run_id="run-test", which `require_operator` refuses) and no lock file.
    """
    op = Ctx(paths=ctx.paths, run_id=None, now=ctx.now)
    lock.set_halt(op, "regression: merge abc123456789 failed post-merge gates")
    # (a) the happy path.
    cleared = lock.clear_halt(op, "regression: merge abc123456789 failed post-merge gates", "operator")
    assert cleared == "regression: merge abc123456789 failed post-merge gates"
    assert lock.halted(op) is None
    lines = [json.loads(l) for l in op.paths.audit.read_text(encoding="utf-8").splitlines()]
    assert lines[-1] == {"at": 1_000_000.0, "run_id": None, "action": "halt-clear",
                         "reason": cleared, "by": "operator"}
    # `set_halt`'s own best-effort line is there too, and two per incident is
    # expected (see its docstring), so this reads the LAST line, not the only.
    assert lines[0]["action"] == "halt-set"

    # (b) the ordering half. If the append happened AFTER the unlink, a clear
    # that dies half-way would leave no record at all -- so make the unlink
    # die and assert the record is already on disk.
    lock.set_halt(op, "a second incident")
    before = len(lines)

    def raising_unlink(self, *a, **kw):
        raise OSError("simulated failure between the audit line and the unlink")

    monkeypatch.setattr(Path, "unlink", raising_unlink)
    with pytest.raises(OSError):
        lock.clear_halt(op, "a second incident", "operator")
    after = [json.loads(l) for l in op.paths.audit.read_text(encoding="utf-8").splitlines()]
    assert [l for l in after[before:] if l["action"] == "halt-clear"], after[before:]
    assert op.paths.halt.exists()                    # ...and the switch really did survive


def test_set_halt_lands_when_its_audit_mutex_times_out(ctx, monkeypatch):
    """HALT must land even when its record cannot. The audit line now waits
    for mutex `audit`, so a `MutexTimeout` there is one more way the record
    can fail, and it must be swallowed like an `OSError`."""
    from agentflow import state

    def busy(c, name, **kw):
        raise state.MutexTimeout(f"mutex {name!r} busy")

    monkeypatch.setattr(state, "mutex", busy)
    lock.set_halt(ctx, "regression on master")
    assert lock.halted(ctx) == "regression on master"


def test_require_operator_allows_a_stale_lock_but_not_a_live_one(ctx):
    """The deliberate ASYMMETRY in `require_operator`, which would silently
    vanish if someone "tightened" it to refuse any lock at all. `cmd_recover`
    is the only thing that clears a stale lock and it is itself reachable
    under HALT, so refusing on staleness would deadlock the operator for the
    full 30 minutes."""
    lock.acquire(ctx, 8, "s", 1)                      # heartbeat at 1_000_000.0
    live = Ctx(paths=ctx.paths, run_id=None, now=lambda: 1_000_000.0)
    with pytest.raises(lock.OperatorOnly):
        lock.require_operator(live, "clear HALT")
    stale = Ctx(paths=ctx.paths, run_id=None, now=lambda: 1_000_000.0 + lock.STALE_SECONDS + 1)
    lock.require_operator(stale, "clear HALT")        # allowed, on purpose
    # ...and a run id is refused regardless of the lock's age.
    with pytest.raises(lock.OperatorOnly):
        lock.require_operator(Ctx(paths=ctx.paths, run_id="run-test",
                                  now=stale.now), "clear HALT")


def test_dry_run_never_touches_lock_or_halt(dry_ctx):
    before = tree_snapshot(dry_ctx.paths.root)
    for call in (
        lambda: lock.acquire(dry_ctx, 8, "s", 1),
        lambda: lock.set_halt(dry_ctx, "x"),
        # T14: `write_guard` is the FIRST statement of `clear_halt`, so what
        # a --dry-run caller gets here is DryRunViolation and NOT
        # HaltClearRefused("not-halted") -- the write-freedom guarantee must
        # not depend on which later check happens to fire first. The
        # `tree_snapshot` equality below now also covers `.agent/audit.jsonl`.
        lambda: lock.clear_halt(dry_ctx, "x", "me"),
    ):
        with pytest.raises(DryRunViolation):
            call()
    assert tree_snapshot(dry_ctx.paths.root) == before
