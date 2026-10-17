import pytest

from agentflow import budget
from agentflow.state import Ctx


def test_caps_match_spec():
    assert budget.CAPS == {
        "spec_rounds": 5, "plan_tasks": 12, "task_attempts": 3, "final_rounds": 5,
        "rebase_regate": 1, "ci_fix": 1, "discoveries": 5, "subagents": 60, "pi_calls": 20,
    }
    assert budget.WALL_CLOCK_SECONDS == 10 * 3600


def test_charge_counts_down_and_raises_at_cap(ctx):
    budget.init(ctx, claimed_at=ctx.now())
    for left in (4, 3, 2, 1, 0):
        assert budget.charge(ctx, "spec_rounds") == left
    with pytest.raises(budget.BudgetExceeded) as e:
        budget.charge(ctx, "spec_rounds")
    assert e.value.key == "spec_rounds"


def test_sub_keys_are_independent(ctx):
    budget.init(ctx, claimed_at=ctx.now())
    for _ in range(3):
        budget.charge(ctx, "task_attempts", sub="t1")
    with pytest.raises(budget.BudgetExceeded):
        budget.charge(ctx, "task_attempts", sub="t1")
    assert budget.charge(ctx, "task_attempts", sub="t2") == 2


def test_deadline_enforced_on_every_charge(ctx):
    budget.init(ctx, claimed_at=ctx.now())
    late = Ctx(paths=ctx.paths, run_id=ctx.run_id, now=lambda: ctx.now() + budget.WALL_CLOCK_SECONDS + 1)
    with pytest.raises(budget.DeadlineExceeded):
        budget.charge(late, "subagents")
    with pytest.raises(budget.DeadlineExceeded):
        budget.check_deadline(late)


def test_unknown_key_rejected(ctx):
    budget.init(ctx, claimed_at=ctx.now())
    with pytest.raises(KeyError):
        budget.charge(ctx, "nonsense")


def test_loop_counter_is_durable_across_contexts(ctx):
    budget.loop_reset(ctx)
    assert budget.loop_tick(ctx, max_issues=2) == 1
    again = Ctx(paths=ctx.paths, run_id="run-next", now=ctx.now)
    assert budget.loop_tick(again, max_issues=2) == 0
    with pytest.raises(budget.BudgetExceeded):
        budget.loop_tick(again, max_issues=2)


def test_two_loop_ticks_at_once_spend_one_slot_each(root):
    """Design test 14 (the `loop-tick` form). Two sessions at
    `issues_done = 2` under `--max 3`: without a mutex both read 2 and both
    write 3, and a 4th issue is claimed. Exactly one tick may succeed. Run
    through the real CLI in two real processes; the reads are slowed so both
    happen before either write."""
    import json
    from agentflow.tests.conftest import run_concurrently
    loop = root / ".agent" / "runs" / "loop.json"
    loop.parent.mkdir(parents=True)
    loop.write_text(json.dumps({"issues_done": 2}), encoding="utf-8")
    body = """
    from agentflow import cli
    slow_reads("loop.json")
    barrier()
    sys.exit(cli.main(["--root", str(ROOT), "--repo", "o/r", "--run-id", TAG, "loop-tick", "--max", "3"]))
    """
    results = run_concurrently(root, body, ["a", "b"])
    assert sorted(r[0] for r in results) == [0, 1], results
    assert sorted(json.loads(r[1]).get("exhausted", "ok") for r in results) == ["loop", "ok"]
    assert json.loads(loop.read_text(encoding="utf-8")) == {"issues_done": 3}
