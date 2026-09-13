import pytest

from agentflow import budget
from agentflow.state import Ctx


def test_caps_match_spec():
    assert budget.CAPS == {
        "spec_rounds": 3, "plan_tasks": 12, "task_attempts": 3, "final_rounds": 3,
        "rebase_regate": 1, "ci_fix": 1, "discoveries": 5, "subagents": 60, "pi_calls": 14,
    }
    assert budget.WALL_CLOCK_SECONDS == 4 * 3600


def test_charge_counts_down_and_raises_at_cap(ctx):
    budget.init(ctx, claimed_at=ctx.now())
    assert budget.charge(ctx, "spec_rounds") == 2
    assert budget.charge(ctx, "spec_rounds") == 1
    assert budget.charge(ctx, "spec_rounds") == 0
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
