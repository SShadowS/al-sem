import json
import os
from pathlib import Path

import pytest

from agentflow import state
from agentflow.state import DryRunViolation, new_run_id, read_json, tree_snapshot, write_json
from agentflow.tests.conftest import run_concurrently


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


def test_write_json_from_two_processes_never_tears_the_file(root):
    """Design test 31. Two REAL processes write one file 200 times each and
    read it back after every write. With one fixed temp name (`x.json.tmp`)
    both writers share a temp file, and one `os.replace` moves away the file
    the other is about to replace. Each write must land whole, and no temp
    file may be left behind."""
    body = """
    from agentflow.state import Ctx, Paths, read_json, write_json
    ctx = Ctx(Paths(ROOT), run_id=TAG)
    target = ROOT / ".agent" / "shared.json"
    barrier()
    for i in range(200):
        write_json(ctx, target, {"writer": TAG, "i": i, "pad": "x" * 4096})
        assert read_json(target)["pad"] == "x" * 4096
    """
    results = run_concurrently(root, body, ["a", "b"])
    assert [r[0] for r in results] == [0, 0], [r[2].strip().splitlines()[-1:] for r in results]
    final = read_json(root / ".agent" / "shared.json")
    assert final["i"] == 199 and final["writer"] in ("a", "b")
    assert not list((root / ".agent").glob("*.tmp"))


def test_write_json_retries_a_transient_permission_error_on_replace(ctx, monkeypatch):
    """Design test 32. On Windows `os.replace` raises `PermissionError` while
    another process has the target open. That is a reader, and it lets go in
    milliseconds, so the write retries instead of failing the command."""
    real, calls = os.replace, []

    def flaky(src, dst):
        calls.append(Path(dst))
        if len(calls) == 1:
            raise PermissionError(13, "the target is open in another process")
        return real(src, dst)

    monkeypatch.setattr(os, "replace", flaky)
    monkeypatch.setattr(state.time, "sleep", lambda s: None)
    target = ctx.paths.agent / "x.json"
    write_json(ctx, target, {"a": 1})
    assert calls == [target, target]
    assert read_json(target) == {"a": 1}
    assert not list(ctx.paths.agent.glob("*.tmp"))


def test_write_json_gives_up_after_its_retries_and_leaves_no_temp(ctx, monkeypatch):
    """The retry is bounded, and a write that never lands cleans up its temp
    file: a stray temp per failed write would pile up in `.agent/`."""
    calls = []

    def always(src, dst):
        calls.append(dst)
        raise PermissionError(13, "held for good")

    monkeypatch.setattr(os, "replace", always)
    monkeypatch.setattr(state.time, "sleep", lambda s: None)
    target = ctx.paths.agent / "x.json"
    with pytest.raises(PermissionError):
        write_json(ctx, target, {"a": 1})
    assert len(calls) == 1 + state.SHARING_RETRIES
    assert not target.exists()
    assert not list(ctx.paths.agent.glob("*.tmp"))


@pytest.mark.parametrize("first", ['{"a"', PermissionError(13, "open elsewhere")],
                         ids=["torn", "permission"])
def test_read_json_retries_a_bad_read(ctx, monkeypatch, first):
    """A torn read (the legacy in-place lease writer) or a Windows sharing
    error is retried. A file that still does not parse on its one retry
    raises, as before: the retry must not hide real corruption."""
    target = ctx.paths.agent / "x.json"
    target.write_text('{"a": 1}', encoding="utf-8")
    real, reads = Path.read_text, []

    def once_bad(self, *a, **kw):
        reads.append(self)
        if len(reads) == 1:
            if isinstance(first, Exception):
                raise first
            return first
        return real(self, *a, **kw)

    monkeypatch.setattr(Path, "read_text", once_bad)
    monkeypatch.setattr(state.time, "sleep", lambda s: None)
    assert read_json(target) == {"a": 1}
    assert len(reads) == 2

    target.write_text('{"a"', encoding="utf-8")       # still bad on the retry
    monkeypatch.setattr(Path, "read_text", real)
    with pytest.raises(json.JSONDecodeError):
        read_json(target)

    # A sharing error that never clears raises after SHARING_RETRIES retries.
    reads.clear()

    def held(self, *a, **kw):
        reads.append(self)
        raise PermissionError(13, "held for good")

    monkeypatch.setattr(Path, "read_text", held)
    with pytest.raises(PermissionError):
        read_json(target)
    assert len(reads) == 1 + state.SHARING_RETRIES


def test_run_id_is_time_prefixed_and_unique():
    a = new_run_id(1_700_000_000.0)
    b = new_run_id(1_700_000_000.0)
    assert a.startswith("20231114-")
    assert a != b
