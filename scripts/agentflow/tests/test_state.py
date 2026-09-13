import json

import pytest

from agentflow.state import DryRunViolation, new_run_id, read_json, tree_snapshot, write_json


def test_paths_layout(ctx):
    p = ctx.paths
    assert p.lock == p.root / ".agent" / "lock.json"
    assert p.halt == p.root / ".agent" / "HALT"
    assert p.run_dir("r1") == p.root / ".agent" / "runs" / "r1"


def test_write_json_is_atomic_and_readable(ctx):
    target = ctx.paths.agent / "x.json"
    write_json(ctx, target, {"a": 1})
    assert read_json(target) == {"a": 1}
    assert not list(ctx.paths.agent.glob("*.tmp"))


def test_write_json_refused_under_dry_run(dry_ctx):
    before = tree_snapshot(dry_ctx.paths.root)
    with pytest.raises(DryRunViolation):
        write_json(dry_ctx, dry_ctx.paths.agent / "x.json", {"a": 1})
    assert tree_snapshot(dry_ctx.paths.root) == before


def test_read_json_default_when_missing(ctx):
    assert read_json(ctx.paths.agent / "nope.json", default=[]) == []


def test_run_id_is_time_prefixed_and_unique():
    a = new_run_id(1_700_000_000.0)
    b = new_run_id(1_700_000_000.0)
    assert a.startswith("20231114-")
    assert a != b
