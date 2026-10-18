"""Gate-run records and the post-merge identity check (spec 2026-10-18).

The records are the evidence `post-merge` may skip on, so every test states
its precondition by hand: a real git repo, a record written by `run`, an
environment pinned through the `_rustc` seam."""
import json
import subprocess
import sys
from types import SimpleNamespace

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


def _grammar_clone(clone):
    """A committed, clean `<root>/tree-sitter-al` (untracked by the root)."""
    grammar = clone / "tree-sitter-al"
    grammar.mkdir()
    for args in (["init", "-q", "-b", "main"], ["config", "user.email", "t@e"], ["config", "user.name", "T"]):
        subprocess.run(["git", "-C", str(grammar), *args], check=True, capture_output=True)
    return grammar, commit_file(grammar, "grammar.js", "x\n", "g")


def test_dirty_grammar_is_recorded(repo_pair):
    _, clone = repo_pair
    grammar, sha = _grammar_clone(clone)
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


# ---- the identity check -----------------------------------------------------

GATES = ["ci-steps-all", "check-goldens", cli.CDO_GATE_KEY]
GREEN = {"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0, cli.CDO_GATE_KEY: 0}


def _git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True).stdout.strip()


def _write_records(ctx, records):
    p = ctx.run_dir / identity.GATE_RUNS
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("".join(json.dumps(r) + "\n" for r in records))


@pytest.fixture
def gated(repo_pair, monkeypatch, tmp_path):
    """The happy state, BY HAND: B on master; H = a code commit; final_head = H
    plus one evidence file; the squash of final_head landed on B and pushed;
    this run's attestation and `merge` record; one green, clean record per gate
    on H's tree in today's environment."""
    _, clone = repo_pair
    ws = tmp_path / "cdo"
    ws.mkdir()
    monkeypatch.setenv("CDO_WS", str(ws))
    _grammar_clone(clone)
    B = Git(clone).rev("HEAD")
    _git(clone, "checkout", "-q", "-b", "issue/8")
    H = commit_file(clone, "src/a.rs", "fn a() {}\n", "code")
    final_head = commit_file(clone, ".agent/issue-8/ledger.md", "ledger\n", "evidence")
    _git(clone, "checkout", "-q", "master")
    _git(clone, "merge", "-q", "--squash", "issue/8")
    _git(clone, "commit", "-q", "-m", "squash")
    merge_sha = Git(clone).rev("HEAD")
    _git(clone, "push", "-q", "origin", "master")
    ctx = _ctx(clone)
    from agentflow import mergeops
    from agentflow.state import write_json
    mergeops.write_attestation(ctx, mergeops.Attestation(
        issue=8, B=B, H=H, final_head=final_head, register_hash="r", gates=dict(GREEN), body_hash="b"))
    write_json(ctx, ctx.run_dir / "merge.json", {"issue": 8, "pr": 12, "merge_sha": merge_sha,
                                                  "run_id": "run-test", "source": "merge"})
    tree = Git(clone).rev(f"{H}^{{tree}}")
    recs = [{"name": n, "exit_code": 0, "timed_out": False, "supervised": True, "head": H, "tree": tree,
             "clean_before": True, "clean_after": True,
             "env": identity.environment(ctx, clone, cdo=n == cli.CDO_GATE_KEY)} for n in GATES]
    _write_records(ctx, recs)
    return SimpleNamespace(clone=clone, ctx=ctx, B=B, H=H, final_head=final_head, merge_sha=merge_sha,
                           records=recs)


def _refusals(g):
    return identity.identity_refusals(g.ctx, Git(g.clone), 8, g.merge_sha, GATES, False)


def _att(g, **change):
    from agentflow import mergeops
    from dataclasses import replace
    mergeops.write_attestation(g.ctx, replace(mergeops.read_attestation(g.ctx), **change))


def _rec(g, gate, **change):
    for r in g.records:
        if r["name"] == gate:
            r.update(change)
    _write_records(g.ctx, g.records)


def _merge_json(g, **change):
    from agentflow.state import read_json, write_json
    p = g.ctx.run_dir / "merge.json"
    write_json(g.ctx, p, {**read_json(p), **change})


BREAKERS = {
    "not-merged-by-this-run": lambda g: _merge_json(g, source="recover"),
    "no-attestation": lambda g: (g.ctx.run_dir / "attestation.json").unlink(),
    "attestation-other-issue": lambda g: _att(g, issue=9),
    "attested-gates-not-green": lambda g: _att(g, gates={**GREEN, cli.CDO_GATE_KEY: 1}),
    "base-moved": lambda g: _att(g, B=g.H),
    "tree-differs": lambda g: _att(g, final_head=g.H),
    "non-evidence-after-H": lambda g: _att(g, H=g.B),
    "no-record:cdo-gate": lambda g: _write_records(g.ctx, g.records[:2]),
    "record-red:check-goldens": lambda g: _rec(g, "check-goldens", exit_code=1),
    "record-red:ci-steps-all": lambda g: _rec(g, "ci-steps-all", timed_out=True),
    "record-dirty:check-goldens": lambda g: _rec(g, "check-goldens", clean_before=False),
    "record-other-tree:ci-steps-all": lambda g: _rec(g, "ci-steps-all", tree="0" * 40),
    "rustc-changed:ci-steps-all": lambda g: _rec(g, "ci-steps-all",
                                                 env={**g.records[0]["env"], "rustc": "rustc 0.0.1"}),
    "grammar-changed:check-goldens": lambda g: _rec(g, "check-goldens",
                                                    env={**g.records[1]["env"],
                                                         "grammar": {"commit": "x", "dirty": False}}),
    "cdo-identity-changed:cdo-gate": lambda g: _rec(g, cli.CDO_GATE_KEY,
                                                    env={**g.records[2]["env"],
                                                         "cdo": {**g.records[2]["env"]["cdo"], "closure": "x"}}),
}


def test_all_conditions_hold_skips(gated):
    assert _refusals(gated) == []


@pytest.mark.parametrize("code", sorted(BREAKERS))
def test_each_condition_alone_refuses(gated, code):
    BREAKERS[code](gated)
    assert code in _refusals(gated)


def test_refuses_when_final_head_is_unknown(gated):
    _att(gated, final_head="f" * 40)
    out = _refusals(gated)                                 # never raises
    assert "tree-differs" in out or any(r.startswith("error:") for r in out)


def test_cdo_ws_unset_refuses_when_cdo_required(gated, monkeypatch):
    monkeypatch.delenv("CDO_WS")
    assert "cdo-identity-changed:cdo-gate" in _refusals(gated)


def test_a_dirty_grammar_now_refuses_even_when_the_record_matches(gated, monkeypatch):
    real = identity._grammar
    monkeypatch.setattr(identity, "_grammar", lambda ctx: {**real(ctx), "dirty": True})
    assert any(r.startswith("grammar-changed:") for r in _refusals(gated))


def test_docs_only_needs_no_cdo_record(gated):
    _write_records(gated.ctx, gated.records[:2])
    assert identity.identity_refusals(gated.ctx, Git(gated.clone), 8, gated.merge_sha, GATES[:2], True) == []


def test_run_outside_a_git_tree_still_runs_and_records_no_tree(capsys, root):
    code, out = _run(capsys, root, "noop", sys.executable, "-c", "pass", run_id="solo-run")
    assert code == 0 and out["exit_code"] == 0
    rec = json.loads(_records_file(root, "solo-run").read_text().splitlines()[-1])
    assert rec["tree"] is None and rec["clean_before"] is False
