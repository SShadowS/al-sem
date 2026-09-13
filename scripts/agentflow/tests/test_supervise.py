import sys

from agentflow import supervise


def test_sanitized_env_drops_regen_and_sets_flags():
    env = supervise.sanitized_env({"REGEN_TEMP_GOLDENS": "1", "PATH": "p"}, tree_sitter_path="/g")
    assert "REGEN_TEMP_GOLDENS" not in env
    assert env["ALSEM_NO_PREFLIGHT_CACHE"] == "1" and env["TREE_SITTER_AL_PATH"] == "/g" and env["PATH"] == "p"


def test_run_captures_exit_code_and_log(ctx, tmp_path):
    log = tmp_path / "x.log"
    r = supervise.run(ctx, [sys.executable, "-c", "print('hello'); raise SystemExit(3)"], cwd=tmp_path,
                      log_path=log, timeout_s=30)
    assert r.exit_code == 3 and not r.timed_out
    assert "hello" in log.read_text()


def test_run_beats_while_child_runs(ctx, tmp_path):
    beats = []
    r = supervise.run(ctx, [sys.executable, "-c", "import time; time.sleep(0.6)"], cwd=tmp_path,
                      log_path=tmp_path / "y.log", timeout_s=30, beat=lambda c: beats.append(c.run_id), beat_every=0.2)
    assert r.exit_code == 0 and len(beats) >= 2


def test_run_times_out_and_kills(ctx, tmp_path):
    r = supervise.run(ctx, [sys.executable, "-c", "import time; time.sleep(30)"], cwd=tmp_path,
                      log_path=tmp_path / "z.log", timeout_s=0.5, beat_every=0.1)
    assert r.timed_out and r.exit_code != 0 and r.seconds < 10
