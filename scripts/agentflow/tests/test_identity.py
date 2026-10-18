"""Gate-run records and the post-merge identity check (spec 2026-10-18).

The records are the evidence `post-merge` may skip on, so every test states
its precondition by hand: a real git repo, a record written by `run`, an
environment pinned through the `_rustc` seam."""
import json
import subprocess
import sys

import pytest

from agentflow import cli, identity
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths
from agentflow.tests.conftest import FakeRunner, commit_file


@pytest.fixture(autouse=True)
def pinned_rustc(monkeypatch):
    monkeypatch.setattr(identity, "_rustc", lambda cwd: "rustc 9.9.9 (test)")


def _ctx(clone, run_id="run-test"):
    return Ctx(Paths(clone), dry_run=False, run_id=run_id, now=lambda: 1_000_000.0)


def _run(capsys, clone, name, *child, run_id="run-test"):
    argv = ["--root", str(clone), "--repo", "SShadowS/al-sem", "--run-id", run_id,
            "run", "--name", name, "--timeout", "1", "--cwd", str(clone), "--", *child]
    code = cli.main(argv, gh_run=FakeRunner())
    return code, json.loads(capsys.readouterr().out)


def _records_file(clone, run_id="run-test"):
    return _ctx(clone, run_id).run_dir / identity.GATE_RUNS


def test_record_names_tree_head_and_env(capsys, repo_pair):
    _, clone = repo_pair
    code, _ = _run(capsys, clone, "noop", sys.executable, "-c", "pass", run_id="solo-run")
    assert code == 0
    lines = _records_file(clone, "solo-run").read_text().splitlines()
    assert len(lines) == 1
    rec = json.loads(lines[0])
    g = Git(clone)
    assert rec["name"] == "noop" and rec["exit_code"] == 0 and rec["timed_out"] is False
    assert rec["head"] == g.rev("HEAD") and rec["tree"] == g.rev("HEAD^{tree}")
    assert rec["clean_before"] is True and rec["clean_after"] is True
    assert rec["env"]["rustc"] == "rustc 9.9.9 (test)"
    assert rec["env"]["cdo"] is None                       # only cdo-gate carries it


def test_cmd_run_records_a_dirty_tree(capsys, repo_pair):
    _, clone = repo_pair
    (clone / "README.md").write_text("edited\n")           # tracked file modified before the run
    _run(capsys, clone, "noop", sys.executable, "-c", "pass", run_id="solo-run")
    rec = json.loads(_records_file(clone, "solo-run").read_text().splitlines()[-1])
    assert rec["clean_before"] is False


def test_latest_record_wins(capsys, repo_pair):
    _, clone = repo_pair
    _run(capsys, clone, "g", sys.executable, "-c", "raise SystemExit(1)", run_id="solo-run")
    _run(capsys, clone, "g", sys.executable, "-c", "pass", run_id="solo-run")
    assert identity.latest_records(_ctx(clone, "solo-run"))["g"]["exit_code"] == 0
    _run(capsys, clone, "g", sys.executable, "-c", "raise SystemExit(3)", run_id="solo-run")
    assert identity.latest_records(_ctx(clone, "solo-run"))["g"]["exit_code"] == 3


def test_torn_line_is_ignored(capsys, repo_pair):
    _, clone = repo_pair
    _run(capsys, clone, "g", sys.executable, "-c", "pass", run_id="solo-run")
    with _records_file(clone, "solo-run").open("a") as f:
        f.write('{"name": "g", "exit_c')                   # a write killed mid-line
    recs = identity.latest_records(_ctx(clone, "solo-run"))
    assert recs["g"]["exit_code"] == 0


def test_no_records_file_is_no_records(repo_pair):
    _, clone = repo_pair
    assert identity.latest_records(_ctx(clone)) == {}


def test_dirty_grammar_is_recorded(repo_pair):
    _, clone = repo_pair
    grammar = clone / "tree-sitter-al"
    grammar.mkdir()
    for args in (["init", "-q", "-b", "main"], ["config", "user.email", "t@e"], ["config", "user.name", "T"]):
        subprocess.run(["git", "-C", str(grammar), *args], check=True, capture_output=True)
    sha = commit_file(grammar, "grammar.js", "x\n", "g")
    env = identity.environment(_ctx(clone), clone, cdo=False)
    assert env["grammar"] == {"commit": sha, "dirty": False}
    (grammar / "grammar.js").write_text("y\n")
    assert identity.environment(_ctx(clone), clone, cdo=False)["grammar"] == {"commit": sha, "dirty": True}


def test_cdo_identity_changes_when_an_app_changes(tmp_path):
    ws = tmp_path / "ws"
    (ws / ".alpackages").mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(ws)], check=True, capture_output=True)
    app = ws / ".alpackages" / "a.app"
    app.write_bytes(b"one")
    first = identity.cdo_identity(str(ws))
    assert first["path"] == str(ws)
    app.write_bytes(b"two")
    assert identity.cdo_identity(str(ws))["closure"] != first["closure"]
    assert identity.cdo_identity(None)["path"] is None


def test_cdo_gate_record_carries_the_cdo_identity(capsys, repo_pair, monkeypatch, tmp_path):
    _, clone = repo_pair
    ws = tmp_path / "ws"
    ws.mkdir()
    monkeypatch.setenv("CDO_WS", str(ws))
    _run(capsys, clone, cli.CDO_GATE_KEY, sys.executable, "-c", "pass", run_id="solo-run")
    rec = json.loads(_records_file(clone, "solo-run").read_text().splitlines()[-1])
    assert rec["env"]["cdo"]["path"] == str(ws)


def test_run_outside_a_git_tree_still_runs_and_records_no_tree(capsys, root):
    code, out = _run(capsys, root, "noop", sys.executable, "-c", "pass", run_id="solo-run")
    assert code == 0 and out["exit_code"] == 0
    rec = json.loads(_records_file(root, "solo-run").read_text().splitlines()[-1])
    assert rec["tree"] is None and rec["clean_before"] is False
