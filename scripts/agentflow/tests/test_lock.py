import json
import os
from pathlib import Path

import pytest

from agentflow import lock
from agentflow.state import Ctx, DryRunViolation, Paths, tree_snapshot


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
    assert all(lock.is_stale(lk, later()) for lk in lock.all_locks(ctx))
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
    assert not lock.is_stale(lk, 1_000_000.0)
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
    assert not lock.is_stale(got, now=got.heartbeat + lock.STALE_SECONDS - 1)
    assert lock.is_stale(got, now=got.heartbeat + lock.STALE_SECONDS + 1)


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
