import json

from agentflow import cli, lock
from agentflow.tests.conftest import FakeRunner

REPO = "SShadowS/al-sem"


def run_cli(capsys, root, *args, gh_run=None, dry=False, run_id="run-test"):
    argv = ["--root", str(root), "--repo", REPO]
    if dry:
        argv.append("--dry-run")
    if run_id:
        argv += ["--run-id", run_id]
    code = cli.main(argv + list(args), gh_run=gh_run)
    out = capsys.readouterr().out
    return code, json.loads(out)


def test_slug():
    assert cli.slug("c10: Scope and elevation reachability across .app symbols") == "c10-scope-and-elevation-reachab"


def test_halt_check_and_set(capsys, root):
    code, out = run_cli(capsys, root, "halt-check")
    assert code == 0 and out["halted"] is None
    code, out = run_cli(capsys, root, "set-halt", "manual stop")
    assert code == 0 and out["halted"] == "manual stop"
    code, out = run_cli(capsys, root, "halt-check")
    assert code == 1 and out["halted"] == "manual stop"
    code, _ = run_cli(capsys, root, "halt-check", "--terminal")
    assert code == 0


def test_charge_and_status(capsys, root, ctx, monkeypatch):
    from agentflow import budget
    # AGENTFLOW_NOW is the clock seam cli._ctx honours: it lets this test hand
    # cli.main the SAME frozen clock the `ctx` fixture used to write
    # claimed_at, so budget.charge's wall-clock-deadline check compares a
    # frozen `now()` against a frozen `claimed_at`, not a frozen `claimed_at`
    # against the real wall clock (which would always exceed the 4h cap).
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    budget.init(ctx, claimed_at=ctx.now())
    code, out = run_cli(capsys, root, "charge", "ci_fix")
    assert code == 0 and out["remaining"] == 0
    code, out = run_cli(capsys, root, "charge", "ci_fix")
    assert code == 1 and out["exhausted"] == "ci_fix"
    code, out = run_cli(capsys, root, "status")
    assert out["budget"]["counts"] == {"ci_fix": 1}


def test_charge_unknown_key_yields_json_not_traceback(capsys, root):
    code, out = run_cli(capsys, root, "charge", "bogus_key")
    assert code == 2 and "KeyError" in out["error"]


def test_sanitize_command(capsys, root, tmp_path, monkeypatch):
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    f = tmp_path / "ledger.md"
    f.write_text("ok\nsee U:/Git/CDO/x.al\n")
    code, out = run_cli(capsys, root, "sanitize", str(f))
    assert code == 1 and out["violations"][str(f)][0]["kind"] == "cdo-path"


def test_unblock_removes_label_without_prior_lock(capsys, root):
    r = FakeRunner({"issue edit 8 --remove-label agent-blocked": ""})
    code, out = run_cli(capsys, root, "unblock", "8", gh_run=r, run_id=None)
    assert code == 0 and out["unblocked"] == 8 and lock.read(cli_ctx(root)) is None


def cli_ctx(root):
    from agentflow.state import Ctx, Paths
    return Ctx(Paths(root))


def test_loop_tick(capsys, root):
    run_cli(capsys, root, "loop-reset")
    code, out = run_cli(capsys, root, "loop-tick", "--max", "1")
    assert code == 0 and out["remaining"] == 0
    code, out = run_cli(capsys, root, "loop-tick", "--max", "1")
    assert code == 1
