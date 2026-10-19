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
    assert p.locks_dir == p.root / ".agent" / "locks"
    assert p.lease(8) == p.root / ".agent" / "locks" / "issue-8.json"
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


def test_a_nested_take_of_one_mutex_times_out_instead_of_hanging(ctx):
    """Design risk (g)9. An OS file lock is not re-entrant, so code that takes
    a mutex it already holds waits for itself. The rule is that only public
    entry points take a mutex; this pins that breaking it is a loud
    `MutexTimeout` after the timeout, not a hang."""
    import time
    with state.mutex(ctx, "registry"):
        t0 = time.monotonic()
        with pytest.raises(state.MutexTimeout):
            with state.mutex(ctx, "registry", timeout=0.3):
                pass
        assert time.monotonic() - t0 < 5
        with state.mutex(ctx, "audit", timeout=0.3):    # another name is a different lock
            pass
    with state.mutex(ctx, "registry", timeout=0.3):    # released on exit
        pass


def test_a_mutex_excludes_another_process(root, ctx):
    """The lock is an OS lock, so it holds against a second PROCESS, which is
    the case it exists for."""
    body = """
    from agentflow import state
    from agentflow.state import Ctx, Paths
    barrier()
    try:
        with state.mutex(Ctx(Paths(ROOT), run_id=TAG), "registry", timeout=0.5):
            print("got it")
    except state.MutexTimeout:
        print("timed out")
    """
    with state.mutex(ctx, "registry"):
        held = run_concurrently(root, body, ["a"])
    free = run_concurrently(root, body, ["b"])
    assert [(r[0], r[1].strip()) for r in held + free] == [(0, "timed out"), (0, "got it")], held + free


def test_a_mutex_under_dry_run_writes_nothing(dry_ctx):
    before = tree_snapshot(dry_ctx.paths.root)
    with state.mutex(dry_ctx, "registry"):
        pass
    assert tree_snapshot(dry_ctx.paths.root) == before


def test_update_json_writes_what_fn_returns_and_nothing_when_fn_raises(ctx, dry_ctx):
    target = ctx.paths.agent / "x.json"
    assert state.update_json(ctx, target, lambda d: {**d, "a": 1}, mutex_name="x", default={}) == {"a": 1}
    assert state.update_json(ctx, target, lambda d: {**d, "b": 2}, mutex_name="x", default={}) == {"a": 1, "b": 2}

    def refuse(d):
        raise ValueError("no")

    with pytest.raises(ValueError):
        state.update_json(ctx, target, refuse, mutex_name="x", default={})
    assert read_json(target) == {"a": 1, "b": 2}
    with pytest.raises(DryRunViolation):
        state.update_json(dry_ctx, dry_ctx.paths.agent / "y.json", lambda d: d, mutex_name="x")
    assert not (dry_ctx.paths.agent / "y.json").exists()


def test_append_audit_takes_the_audit_mutex(ctx, monkeypatch):
    """`open(..., "a")` from two processes is not an atomic append on
    Windows, so every audit line is written under mutex `audit`."""
    real, taken = state.mutex, []

    def recording(c, name, **kw):
        taken.append(name)
        return real(c, name, **kw)

    monkeypatch.setattr(state, "mutex", recording)
    state.append_audit(ctx, {"action": "x"})
    assert taken == ["audit"]
    assert json.loads(ctx.paths.audit.read_text(encoding="utf-8"))["action"] == "x"


def test_config_defaults_when_the_file_or_a_key_is_missing(ctx):
    """Design C1. The defaults ARE today's behaviour, so a checkout with no
    `.agent/config.json` acts exactly as before. Stated as literals."""
    assert ctx.paths.config == ctx.paths.root / ".agent" / "config.json"
    defaults = {"slots": 1, "heavy_gates": 1, "min_free_gb": 40, "reserve_gb_per_run": 30,
                "gate_wait_minutes": 180, "target_root": "", "cdo_ws": ""}
    assert state.config(ctx) == defaults
    ctx.paths.config.write_text(json.dumps({"slots": 2}), encoding="utf-8")
    assert state.config(ctx) == {**defaults, "slots": 2}


@pytest.mark.parametrize("bad", [{"slot": 2}, {"slots": "2"}, {"slots": True}, {"slots": -1}, [1],
                                 {"target_root": 5}],
                         ids=["unknown-key", "string", "bool", "negative", "not-an-object", "target-root-not-str"])
def test_config_refuses_a_value_it_would_otherwise_ignore(ctx, bad):
    """A misspelt key or a wrong type would silently fall back to the
    default. Two sessions must agree on these numbers, so a file the owner
    got wrong is an error, never a quiet default."""
    ctx.paths.config.write_text(json.dumps(bad), encoding="utf-8")
    with pytest.raises(state.ConfigError):
        state.config(ctx)


def test_run_id_is_time_prefixed_and_unique():
    a = new_run_id(1_700_000_000.0)
    b = new_run_id(1_700_000_000.0)
    assert a.startswith("20231114-")
    assert a != b
