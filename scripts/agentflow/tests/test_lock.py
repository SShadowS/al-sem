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


def test_dry_run_never_touches_lock_or_halt(dry_ctx):
    before = tree_snapshot(dry_ctx.paths.root)
    for call in (
        lambda: lock.acquire(dry_ctx, 8, "s", 1),
        lambda: lock.set_halt(dry_ctx, "x"),
    ):
        with pytest.raises(DryRunViolation):
            call()
    assert tree_snapshot(dry_ctx.paths.root) == before
