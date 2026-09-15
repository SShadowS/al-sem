import json
from pathlib import Path

import pytest

from agentflow import lock
from agentflow.state import Ctx, DryRunViolation, Paths, tree_snapshot


def test_acquire_creates_lock_and_second_acquire_loses(ctx):
    got = lock.acquire(ctx, issue=8, session="s1", attempt=1)
    assert got.issue == 8 and got.run_id == "run-test" and got.attempt == 1
    other = Ctx(paths=ctx.paths, run_id="run-other", now=ctx.now)
    with pytest.raises(lock.LockHeld) as e:
        lock.acquire(other, issue=9, session="s2", attempt=1)
    assert e.value.existing.run_id == "run-test"


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
    on_disk = json.loads(ctx.paths.lock.read_text(encoding="utf-8"))
    assert on_disk["started"] == 12.0 and on_disk["heartbeat"] == 1_000_000.0
    # ...and the default is unchanged: an ordinary claim dates itself `now`.
    ctx.paths.lock.unlink()
    assert lock.acquire(ctx, 8, "s", 1).started == 1_000_000.0


def test_acquire_forwards_started_through_the_lost_race_retry(ctx, monkeypatch):
    """THE PATH NO FIXTURE DRIVES. `acquire` retries itself when the `x` open
    loses to an existing file that has since been released, and a parameter
    that is not forwarded there silently reverts to `now` -- on exactly the
    branch nothing else exercises.

    PRECONDITION HAND-STATED: the lock file is written by this test, and the
    race is staged by a `read` that removes it and reports absence -- which is
    what "the holder released between our open and our read" looks like from
    inside `acquire`."""
    ctx.paths.lock.write_text("{}", encoding="utf-8")   # someone else holds it
    reads = []

    def racy_read(c):
        reads.append(c.run_id)
        c.paths.lock.unlink()                           # ...and has just released it
        return None

    monkeypatch.setattr(lock, "read", racy_read)
    lk = lock.acquire(ctx, 8, "recover", 1, started=12.0)
    assert reads == ["run-test"]                        # the retry branch really ran
    assert lk.started == 12.0 and lk.heartbeat == 1_000_000.0


def test_beat_updates_heartbeat_and_release_removes(ctx):
    lock.acquire(ctx, 8, "s1", 1)
    later = Ctx(paths=ctx.paths, run_id="run-test", now=lambda: 1_000_500.0)
    lock.beat(later)
    assert lock.read(ctx).heartbeat == 1_000_500.0
    lock.release(later)
    assert lock.read(ctx) is None


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
