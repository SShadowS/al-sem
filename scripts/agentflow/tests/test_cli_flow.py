import json
import shutil
import sys

from agentflow import cli, lock, mergeops, recovery, supervise
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths, tree_snapshot, write_json
from agentflow.tests.conftest import FakeRunner, commit_file

REPO = "SShadowS/al-sem"
ISSUES = json.dumps([[{"number": 8, "title": "c10: scope", "body": "## Acceptance\nx", "user": {"login": "SShadowS"},
                       "labels": [], "created_at": "2026-09-12T00:00:00Z"},
                      {"number": 9, "title": "stranger", "body": "## Acceptance\nx", "user": {"login": "nobody"},
                       "labels": [], "created_at": "2026-09-12T00:00:00Z"}]])
READS = {
    f"api repos/{REPO}/issues?state=open&per_page=100 --paginate --slurp": ISSUES,
    f"api repos/{REPO}/collaborators?permission=push&per_page=100 --paginate --slurp": json.dumps([[{"login": "SShadowS"}]]),
    "auth status": "",
    f"api repos/{REPO}/issues/8": json.dumps(json.loads(ISSUES)[0][0]),
    f"api repos/{REPO}/labels?per_page=100 --paginate --slurp": json.dumps([[{"name": n} for n in cli.LABELS]]),
}


def run(capsys, root, *args, gh_run, dry=False, run_id="run-test"):
    argv = ["--root", str(root), "--repo", REPO] + (["--dry-run"] if dry else []) + (["--run-id", run_id] if run_id else [])
    code = cli.main(argv + list(args), gh_run=gh_run)
    return code, json.loads(capsys.readouterr().out)


def test_dry_run_fetch_and_preflight_write_nothing(capsys, repo_pair, monkeypatch, tmp_path):
    _, clone = repo_pair
    # The grammar-presence check reads this file, and preflight's own
    # git-cleanliness check (git status --porcelain) counts ANY untracked
    # path as dirty -- so this fixture file must be committed (and pushed,
    # so local `master` still matches `origin/master`) rather than left as a
    # bare untracked file, or the test would spuriously fail on
    # "tree-dirty"/"master-differs-from-origin", neither of which the write-
    # freedom assertion below is about.
    commit_file(clone, "tree-sitter-al/src/node-types.json", "[]", "grammar stub")
    assert Git(clone).push("origin", "master")
    monkeypatch.setenv("CDO_WS", str(tmp_path))
    before = tree_snapshot(clone)
    gh = FakeRunner(READS, readonly=True)
    code, out = run(capsys, clone, "fetch", gh_run=gh, dry=True, run_id=None)
    assert code == 0 and [i["number"] for i in out["eligible"]] == [8]
    assert out["excluded"] == [{"number": 9, "reason": "author"}]
    code, out = run(capsys, clone, "preflight", gh_run=gh, dry=True, run_id=None)
    f = out["failures"]
    assert not f or (len(f) == 1 and f[0].startswith("disk-free:"))
    assert tree_snapshot(clone) == before
    assert lock.read(Ctx(Paths(clone))) is None


def test_claim_rolls_back_lock_when_gh_fails_midway(capsys, root):
    gh = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""}, fail_at=4)
    gh.responses["issue edit 8 --add-label agent-working"] = (1, "", "HTTP 500: boom")
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh)
    assert code == 1 and "claim rolled back" in out["error"]
    assert lock.read(Ctx(Paths(root))) is None
    assert (root / ".agent" / "runs" / "run-test" / "claim.json").exists()
    # I7: a rolled-back claim must not consume an attempt -- the next claim on
    # the same issue must still be attempt 1, not 2.
    gh2 = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""})
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope",
                     gh_run=gh2, run_id="run-2")
    assert code == 0 and out["attempt"] == 1


def test_claim_then_finish_blocked_releases_lock_and_retains(capsys, root, tmp_path, monkeypatch):
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    gh = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": "",
                     "issue edit 8 --add-label agent-blocked": "", "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh)
    assert code == 0 and out["branch"] == "issue/8-c10-scope-a1" and out["attempt"] == 1
    assert lock.read(Ctx(Paths(root))).issue == 8
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "blocked", "--reason", "spec-panel-cap", gh_run=gh)
    assert code == 0 and lock.read(Ctx(Paths(root))) is None
    assert (tmp_path / "home" / ".al-sem" / "agentflow" / "runs" / "run-test" / "claim.json").exists()
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh, run_id="run-2")
    assert out["attempt"] == 2 and out["branch"].endswith("-a2")


def test_check_diff_and_freeze_via_cli(capsys, repo_pair):
    _, clone = repo_pair
    (clone / ".agent").mkdir()
    base = commit_file(clone, "src/a.rs", "a\n", "a")
    head = commit_file(clone, "scripts/evil.sh", "x\n", "evil")
    code, out = run(capsys, clone, "check-diff", "--base", base, "--head", head, "--issue", "8", gh_run=FakeRunner())
    assert code == 1 and out["reasons"] == ["protected-path:scripts/evil.sh"]
    H = head
    commit_file(clone, ".agent/issue-8/ledger.md", "l\n", "evidence")
    code, out = run(capsys, clone, "freeze-check", "--H", H, "--issue", "8", gh_run=FakeRunner())
    assert code == 0 and out["violations"] == []


def capture_child(monkeypatch):
    """Replace the supervisor with a probe that records the argv it was handed
    and never spawns anything."""
    seen = {}

    def fake_run(ctx, cmd, *, cwd, log_path, timeout_s, beat=None, beat_every=60.0, env=None):
        seen["cmd"] = list(cmd)
        return supervise.Result(exit_code=0, log_path=log_path, timed_out=False, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", fake_run)
    return seen


def test_run_maps_a_literal_bash_child_to_the_resolved_interpreter(capsys, root, monkeypatch):
    # Residual (3): the /issue gates reach the executor as
    # `run --name … -- bash scripts/ci-steps all`. That literal `bash` is
    # resolved by CreateProcess against the inherited PATH, which from
    # PowerShell is the WSL launcher -- the same red-gate-on-every-issue
    # failure resolve_bash() was introduced for.
    monkeypatch.setenv("AGENTFLOW_BASH", r"D:\tools\bash.exe")
    seen = capture_child(monkeypatch)
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     "bash", "scripts/ci-steps", "all", gh_run=FakeRunner(), run_id="solo-run")
    assert code == 0 and out["exit_code"] == 0
    assert seen["cmd"] == [r"D:\tools\bash.exe", "scripts/ci-steps", "all"]


def test_run_leaves_any_other_child_argv_alone(capsys, root, monkeypatch):
    # Only the exact token `bash` is rewritten: a child that already names its
    # interpreter -- including a bash by full path -- is passed through, or the
    # mapping would be second-guessing a caller who was explicit.
    monkeypatch.setenv("AGENTFLOW_BASH", r"D:\tools\bash.exe")
    seen = capture_child(monkeypatch)
    run(capsys, root, "run", "--name", "p1", "--timeout", "1", "--",
        sys.executable, "-c", "pass", gh_run=FakeRunner(), run_id="solo-run")
    assert seen["cmd"] == [sys.executable, "-c", "pass"]
    run(capsys, root, "run", "--name", "p2", "--timeout", "1", "--",
        "/usr/bin/bash", "-c", "true", gh_run=FakeRunner(), run_id="solo-run")
    assert seen["cmd"] == ["/usr/bin/bash", "-c", "true"]


def test_dry_run_run_refuses_before_spawning_child(capsys, root):
    # C1: `run`'s child must never actually spawn under --dry-run.
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), dry=True)
    assert code == 2 and "error" in out
    assert not (root / ".agent" / "runs" / "run-test" / "logs" / "probe.log").exists()


def test_dry_run_post_merge_refuses_before_moving_master(capsys, repo_pair):
    # C1: `post-merge` must never touch `master` under --dry-run.
    _, clone = repo_pair
    before = Git(clone).rev("master")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", before,
                     gh_run=FakeRunner(), dry=True)
    assert code == 2 and "error" in out
    assert Git(clone).rev("master") == before
    assert Git(clone).branch() == "master"


def test_post_merge_refuses_when_run_id_does_not_own_lock(capsys, repo_pair):
    # I5: post-merge is fenced -- a run that does not hold (or own) the lock
    # must not be able to move `master`.
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="owner-run"), 8, "s", 1)
    before = Git(clone).rev("master")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", before,
                     gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    assert Git(clone).rev("master") == before
    assert Git(clone).branch() == "master"


GREEN_GATES = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0, "cdo-gate": 0})
CONVERGED = json.dumps([{"id": "F1", "severity": "important", "disposition": "fixed",
                         "reviews": {"astra": "accepted", "flash": "accepted"}}])


def attest(capsys, root, register, *, gates=GREEN_GATES, body_hash="abc123", extra=()):
    return run(capsys, root, "attest", "--issue", "8", "--B", "B", "--H", "H",
               "--final-head", "F", "--register", str(register), "--gates", gates,
               "--body-hash", body_hash, *extra, gh_run=FakeRunner())


def claimed_register(ctx, tmp_path, *, body_hash="abc123", contents=CONVERGED, name="wt"):
    """A claim naming a worktree, plus the findings register inside it. The
    attestation stores that register RELATIVE to the worktree -- an absolute
    local path would be published verbatim in the attestation's PR comment --
    so the claim and the register have to be set up together."""
    wt = tmp_path / name
    (wt / ".agent" / "issue-8").mkdir(parents=True, exist_ok=True)
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": body_hash, "worktree": str(wt)})
    register = wt / ".agent" / "issue-8" / "findings.json"
    register.write_text(contents)
    return register


def test_attest_refuses_body_hash_mismatch_with_claim(capsys, root, tmp_path):
    # I8: attest must cross-check its --body-hash against claim.json, not
    # trust a caller-supplied hash on its own.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register, body_hash="different-hash")
    assert code == 1 and out["error"] == "body-hash mismatch with claim"


def test_attest_accepts_matching_body_hash(capsys, root, tmp_path):
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register)
    assert code == 0 and "path" in out


def test_attest_stores_the_register_relative_to_the_worktree(capsys, root, tmp_path):
    # Residual (4): the attestation is posted verbatim as a PR comment on a
    # PUBLIC repository. An absolute path in it publishes the maintainer's
    # drive letter and directory layout, which the sanitizer does not catch
    # (it is neither a CDO_WS nor an .alpackages path).
    ctx = Ctx(Paths(root), run_id="run-test")
    register = claimed_register(ctx, tmp_path)
    code, out = attest(capsys, root, register)
    assert code == 0
    text = (ctx.run_dir / "attestation.json").read_text()
    assert json.loads(text)["register_path"] == ".agent/issue-8/findings.json"
    assert str(tmp_path) not in text and str(register.resolve()) not in text


def test_attest_refuses_a_register_outside_the_worktree(capsys, root, tmp_path):
    # A register the worktree does not contain cannot be named relative to it,
    # and is not the issue's own register either way.
    ctx = Ctx(Paths(root), run_id="run-test")
    claimed_register(ctx, tmp_path)
    stray = tmp_path / "findings.json"
    stray.write_text(CONVERGED)
    code, out = attest(capsys, root, stray)
    assert code == 1 and out["error"] == "register-outside-worktree"
    assert not (ctx.run_dir / "attestation.json").exists()


def test_attest_refuses_when_there_is_no_claim_for_this_run(capsys, root, tmp_path):
    # I6 (reversed ruling): with claim.json absent the body-hash cross-check
    # used to be SKIPPED, so the issue-revision pin degraded into comparing a
    # caller-supplied hash against itself. No claim, no attestation.
    register = tmp_path / "findings.json"
    register.write_text(CONVERGED)
    code, out = attest(capsys, root, register)
    assert code == 1 and out["error"] == "no claim.json for this run"
    assert not (root / ".agent" / "runs" / "run-test" / "attestation.json").exists()


def test_attest_refuses_missing_and_red_gates(capsys, root, tmp_path):
    # I6: "all gates green" was a conductor assertion the executor recorded
    # verbatim and never read. A `--gates '{}'` must not be attestable.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register, gates="{}")
    assert code == 1 and out["error"] == "gates-not-green"
    assert set(out["gates"]) == {"ci-steps-all", "check-goldens-coverage", "check-goldens", "cdo-gate"}
    red = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 1, "cdo-gate": 0})
    code, out = attest(capsys, root, register, gates=red)
    assert code == 1 and out["error"] == "gates-not-green" and out["gates"] == ["check-goldens"]


def test_attest_docs_only_does_not_require_the_cdo_gate(capsys, root, tmp_path):
    # A docs-only diff legitimately never runs cdo-gate, so demanding the key
    # would make the check unusable exactly where it is least needed.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    three = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0})
    code, out = attest(capsys, root, register, gates=three)
    assert code == 1 and out["gates"] == ["cdo-gate"]
    code, out = attest(capsys, root, register, gates=three, extra=("--docs-only",))
    assert code == 0 and "path" in out


def test_attest_refuses_a_register_that_has_not_converged(capsys, root, tmp_path):
    # I6: "both reviewers converged" was likewise recorded as an opaque hash
    # and never opened. Each of the three non-convergence shapes is named.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    both = {"astra": "accepted", "flash": "accepted"}
    for entries, offender in (
        ([{"id": "F1", "severity": "minor", "disposition": "open", "reviews": both}], "F1"),
        ([{"id": "F2", "severity": "minor", "disposition": "fixed",
           "reviews": {"astra": "accepted", "flash": "re-raised"}}], "F2"),
        ([{"id": "F3", "severity": "blocking", "disposition": "deferred", "reviews": both}], "F3"),
        ([{"id": "F4", "severity": "minor", "blocking": True, "disposition": "deferred", "reviews": both}], "F4"),
    ):
        register.write_text(json.dumps(entries))
        code, out = attest(capsys, root, register)
        assert code == 1 and out["error"] == "register-not-converged", entries
        assert out["entries"] == [offender], entries


def test_attest_binds_the_register_path_and_merge_refuses_a_changed_register(capsys, repo_pair, tmp_path):
    # I6: hashing the register at attest time only helps if something re-checks
    # it. A register edited between the attestation and the merge -- a
    # `deferred` quietly flipped to `fixed`, say -- must stop the merge. The
    # merge resolves the attestation's relative path against the claim's
    # worktree to find the file again.
    _, clone = repo_pair
    write_ci_workflow(clone)
    ctx = Ctx(Paths(clone), run_id="run-test")
    body = json.loads(ISSUES)[0][0]["body"]
    register = claimed_register(ctx, tmp_path, body_hash=mergeops.body_hash(body))
    B = Git(clone).rev("origin/master")
    code, out = run(capsys, clone, "attest", "--issue", "8", "--B", B, "--H", B, "--final-head", B,
                     "--register", str(register), "--gates", GREEN_GATES,
                     "--body-hash", mergeops.body_hash(body), gh_run=FakeRunner())
    assert code == 0
    assert json.loads((ctx.run_dir / "attestation.json").read_text())["register_path"] == ".agent/issue-8/findings.json"
    register.write_text(json.dumps([{"id": "F1", "severity": "blocking", "disposition": "deferred",
                                     "reviews": {"astra": "accepted", "flash": "accepted"}}]))
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    gh = FakeRunner({**READS,
                     f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
                         {"headRefOid": B, "statusCheckRollup": [
                             {"workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"}]})})
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh)
    assert code == 1 and "register-changed" in out["reasons"]
    assert not any(c.startswith("pr merge") for c in gh.calls)


def write_ci_workflow(clone, name="CI"):
    (clone / ".github" / "workflows").mkdir(parents=True, exist_ok=True)
    (clone / ".github" / "workflows" / "ci.yml").write_text(f"name: {name}\n\non:\n  pull_request:\n")


def rollup(*, workflow="CI", conclusion="SUCCESS"):
    return [{"workflowName": workflow, "status": "COMPLETED", "conclusion": conclusion}]


def test_merge_gate_detects_moved_head(capsys, repo_pair):
    # I10(a): a PR head that moved since the attestation must be caught.
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head="cafebabe" * 5,
                                register_hash="r" * 40, gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({
        **READS,
        f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
            {"headRefOid": "deadbeef" * 5, "statusCheckRollup": rollup()}),
    })
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["reasons"] == ["head-moved"]


def test_merge_gate_is_not_green_without_the_ci_workflows_own_checks(capsys, repo_pair):
    # I7: an unrelated SUCCESS check (a bot, a second workflow) that lands
    # before ci.yml's jobs register must not read as green.
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup(workflow="Dependabot")})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["ci_green"] is False and out["reasons"] == ["ci-not-green"]


def test_merge_gate_reports_ci_workflow_unknown_when_the_workflow_file_is_missing(capsys, repo_pair):
    # I7: the required-workflow name is read from .github/workflows/ci.yml. If
    # that file (or its `name:`) is gone, the gate has no idea what to require
    # -- which is a refusal, not a green.
    _, clone = repo_pair
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup()})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["ci_green"] is False and out["reasons"] == ["ci-workflow-unknown"]


def test_merge_gate_passes_with_the_ci_workflows_checks_green(capsys, repo_pair):
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup()})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 0 and out["ci_green"] is True and out["reasons"] == []


def test_post_merge_happy_path_restores_master_and_reports_ok(capsys, repo_pair, monkeypatch):
    # I10(b): the full post-merge success path -- both gate lists patched to
    # a real, instant no-op child so no actual build/test tooling is needed.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    # Fix round 2, finding 1: an untracked file -- exactly what the
    # executor's own gates leave behind (log files under .agent/runs,
    # __pycache__, ...) -- must NOT trip the post-gate cleanliness probe.
    # No `.gitignore` accommodation is needed any more: tracked_dirty()
    # ignores untracked paths outright.
    (clone / "untracked-artifact.log").write_text("noise\n")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert code == 0 and out["ok"] is True and out["revert"] is None
    assert out["gates"] == {"noop": 0, "noop-cdo": 0}
    assert Git(clone).branch() == "master"
    assert not Git(clone).tracked_dirty()


def test_post_merge_reverts_when_a_gate_leaves_a_tracked_file_modified(capsys, repo_pair, monkeypatch):
    # Fix round 2, finding 1's other half: a gate that leaves a TRACKED file
    # modified (unlike the untracked artifact above) must still be caught,
    # named `tree-dirty-after-gates`, and routed through the revert path.
    _, clone = repo_pair
    dirty_gate = [sys.executable, "-c", "open('README.md', 'a').write('x')"]
    monkeypatch.setattr(cli, "gates", lambda: [("dirty", dirty_gate, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    gh = FakeRunner({
        "issue edit 8 --add-label agent-regressed": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "",
        "issue reopen 8": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1
    assert out["gates"]["tree-dirty-after-gates"] == 1
    assert Git(clone).branch() == "master"


def test_post_merge_reports_restore_failed_when_the_final_checkout_fails(capsys, repo_pair, monkeypatch):
    # Fix round 2, finding 3: a failed restore-to-master must always surface
    # as a `restore_failed` JSON field, never a bare exception.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)

    calls = {"master": 0}
    original_checkout = Git.checkout

    def flaky_checkout(self, ref):
        if ref == "master":
            calls["master"] += 1
            if calls["master"] > 1:
                raise RuntimeError("simulated restore failure")
        return original_checkout(self, ref)

    monkeypatch.setattr(Git, "checkout", flaky_checkout)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert code == 1 and "restore_failed" in out and "simulated restore failure" in out["restore_failed"]


def test_cleanup_succeeds_with_no_lock_and_removes_worktree(capsys, repo_pair):
    # Fix round 2, finding 2: cleanup follows finish (which releases the
    # lock), so an ABSENT lock is the normal case and must not be refused.
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-cleanup-a1"
    g.worktree_add(wt, "issue/8-x-a1", "master")
    commit_file(wt, "issue.txt", "x\n", "issue work")
    merge_sha = g.merge_squash("issue/8-x-a1", "squash issue/8-x-a1")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-x-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert not wt.exists()


def test_cleanup_refuses_foreign_lock_and_keeps_worktree(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-cleanup-a2"
    g.worktree_add(wt, "issue/8-y-a1", "master")
    commit_file(wt, "issue2.txt", "y\n", "issue work 2")
    merge_sha = g.merge_squash("issue/8-y-a1", "squash issue/8-y-a1")
    lock.acquire(Ctx(Paths(clone), run_id="owner-run"), 8, "s", 1)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-y-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    assert wt.exists()


def test_recover_of_a_merged_stale_run_claims_the_lock_for_the_recovering_run(capsys, repo_pair, monkeypatch):
    # Review round 1, Important 6: recovering a merged stale run must not stop
    # at unlinking the old lock -- the recovering run needs its OWN lock and
    # budget/claim.json so post-merge/discoveries/cleanup/finish can run
    # fenced, exactly like any other claimed work.
    _, clone = repo_pair
    old_ctx = Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0)
    lk = lock.acquire(old_ctx, 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + 4000))
    gh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1",
                                               "headRefOid": "h", "mergeCommit": {"oid": "abc"}, "mergedAt": "x"}])})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0
    assert out["action"] == "merged-needs-post-merge" and out["merge_sha"] == "abc" and out["branch"] == "issue/8-x-a1"
    expected_worktree = str(clone.parent / recovery.worktree_name(8, 1))
    assert out["worktree"] == expected_worktree
    lk2 = lock.read(Ctx(Paths(clone)))
    assert lk2 is not None and lk2.run_id == "run-new" and lk2.issue == 8 and lk2.attempt == 1
    claim = json.loads((clone / ".agent" / "runs" / "run-new" / "claim.json").read_text())
    assert claim == {"issue": 8, "attempt": 1, "branch": "issue/8-x-a1", "worktree": expected_worktree}


def test_cleanup_spike_removes_a_commit_free_worktree_and_branch(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-spike-a1"
    g.worktree_add(wt, "issue/9-spike-a1", "master")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a1", "--spike",
                     gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert not wt.exists() and "issue/9-spike-a1" not in g.out("branch", "--list")


def test_cleanup_spike_refuses_a_branch_with_a_commit(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-spike-a2"
    g.worktree_add(wt, "issue/9-spike-a2", "master")
    commit_file(wt, "probe.txt", "code, not just a read-only probe result\n", "spike accidentally committed code")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a2", "--spike",
                     gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and "not spike-clean" in out["error"]
    assert wt.exists()


def test_cleanup_rejects_spike_and_merge_sha_together(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-spike-a3"
    g.worktree_add(wt, "issue/9-spike-a3", "master")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a3", "--spike",
                     "--merge-sha", "deadbeef", gh_run=FakeRunner(), run_id="run-test")
    assert code == 2 and "mutually exclusive" in out["error"]
    assert wt.exists()


def test_cleanup_succeeds_when_worktree_already_removed_by_hand(capsys, repo_pair):
    # Review round 2, New Breakage 5: a crash between cleanup deleting the
    # directory and `finish` running must not turn a re-run into a permanent
    # stuck lock -- an already-absent worktree is success.
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-vanished-a1"
    g.worktree_add(wt, "issue/8-vanished-a1", "master")
    commit_file(wt, "issue.txt", "x\n", "issue work")
    merge_sha = g.merge_squash("issue/8-vanished-a1", "squash issue/8-vanished-a1")
    shutil.rmtree(wt)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-vanished-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert "issue/8-vanished-a1" not in g.out("branch", "--list")


def test_cleanup_spike_succeeds_when_worktree_already_removed_by_hand(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / "wt-vanished-spike-a1"
    g.worktree_add(wt, "issue/9-vanished-spike-a1", "master")
    shutil.rmtree(wt)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-vanished-spike-a1",
                     "--spike", gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert "issue/9-vanished-spike-a1" not in g.out("branch", "--list")


def test_file_discoveries_refuses_a_file_carrying_a_customer_path(capsys, root, tmp_path, monkeypatch):
    # C1: the discoveries file is conductor-written and every field in it is
    # published verbatim to a PUBLIC repository, so the file is scanned before
    # it is even parsed -- nothing is filed when it carries a CDO_WS path.
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    f = tmp_path / "discoveries.json"
    f.write_text(json.dumps([{
        "subsystem": "resolve", "locator": "U:/Git/CDO/App/Src/Thing.al",
        "symptom": "edge dropped", "kind": "bug", "origin_issue": 8,
        "reproducer": "aldump --program-call-graph-stats", "pre_existing": True,
        "capability": "x resolves", "acceptance": "y",
    }]), encoding="utf-8")
    gh = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/90\n"})
    code, out = run(capsys, root, "file-discoveries", str(f), "--session", "https://s", gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert out["violations"][0]["kind"] == "cdo-path"
    assert gh.calls == []


def test_finish_refuses_a_reason_that_leaks_a_dependency_path(capsys, root):
    # C1: a spike answer and a block reason are both posted as issue comments.
    # The refusal must land BEFORE any label or comment, so a rejected text
    # leaves no half-finished bookkeeping behind.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = FakeRunner({"issue edit 8 --add-label agent-blocked": "", "issue edit 8 --remove-label agent-working": "",
                     "issue comment 8 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "blocked",
                     "--reason", "fails only against .alpackages/Microsoft_Base Application.app",
                     gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert out["violations"][0]["kind"] == "alpackages-path"
    assert gh.calls == []
    assert lock.read(Ctx(Paths(root))) is not None  # nothing terminal happened


def test_finish_accepts_an_answer_from_a_reason_file(capsys, root, tmp_path, monkeypatch):
    # I9: a spike's `## Answer` is multi-line markdown. Passing it as an argv
    # argument is fragile through two shells and capped near 32 KB, so the
    # conductor hands over a file instead.
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    ctx = Ctx(Paths(root), run_id="run-test")
    lock.acquire(ctx, 8, "s", 1)
    ctx.run_dir.mkdir(parents=True)  # `claim` makes this; retention copies it
    answer = tmp_path / "answer.md"
    answer.write_text("## Answer\n\nYes -- `resolve_in_table_scope` already covers it.\n\n- one\n- two\n")
    gh = FakeRunner({"issue edit 8 --add-label agent-answered": "", "issue edit 8 --remove-label agent-working": "",
                     "issue comment 8 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "answered",
                     "--reason-file", str(answer), gh_run=gh)
    assert code == 0 and out["outcome"] == "answered"
    assert any(c.startswith("issue comment 8") for c in gh.calls)


def test_finish_regressed_releases_the_lock_without_relabelling(capsys, root, tmp_path, monkeypatch):
    # I8: post-merge's revert path has already labelled the issue
    # `agent-regressed` and set HALT. The tick still needs its terminal
    # bookkeeping -- release the lock, retain the evidence -- without a second
    # label transition and without being refused by the HALT it just set.
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    ctx = Ctx(Paths(root), run_id="run-test")
    lock.acquire(ctx, 8, "s", 1)
    ctx.run_dir.mkdir(parents=True)  # `claim` makes this; retention copies it
    lock.set_halt(ctx, "regression: merge deadbeef failed post-merge gates")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "regressed", gh_run=gh)
    assert code == 0 and out["outcome"] == "regressed"
    assert gh.calls == []
    assert lock.read(Ctx(Paths(root))) is None
    assert (tmp_path / "home" / ".al-sem" / "agentflow" / "runs" / "run-test").exists()


# ---- I4: PR creation, PR comments and branch pushes belong to the executor --
# Before this, all three were conductor-side raw `gh`/`git`: outside the
# dry-run guard, outside the fence, outside the HALT check, and outside the
# sanitizer -- while the spec and CHANGELOG both claimed every mutation went
# through the executor.

def test_pr_create_refuses_master_as_head(capsys, repo_pair):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "master", gh_run=gh)
    assert code == 1 and out["error"] == "refusing to open a PR from master"
    assert gh.calls == []


def test_pr_create_refuses_a_body_that_leaks_a_customer_path(capsys, repo_pair, monkeypatch):
    _, clone = repo_pair
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("## Gate results\n\ncdo-gate log: U:/Git/CDO/App/Src/Thing.al\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert gh.calls == []


def test_pr_create_and_pr_comment_refuse_without_a_lock(capsys, repo_pair):
    _, clone = repo_pair
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 1 and "FenceError" in out["error"]
    code, out = run(capsys, clone, "pr-comment", "--pr", "12", "--body-file", str(body), gh_run=gh)
    assert code == 1 and "FenceError" in out["error"]
    assert gh.calls == []


def test_pr_create_then_comment_under_the_lock(capsys, repo_pair):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("## Ledger\n\nall gates green\n")
    gh = FakeRunner({"pr create *": "https://github.com/SShadowS/al-sem/pull/77\n",
                     "pr comment *": ""})
    code, out = run(capsys, clone, "pr-create", "--title", "c10: scope (#8)", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 0 and out["pr"] == 77
    assert any(c.startswith("pr create --title c10: scope (#8)") and "--head issue/8-x-a1 --base master" in c
               for c in gh.calls)
    code, out = run(capsys, clone, "pr-comment", "--pr", "77", "--body-file", str(body), gh_run=gh)
    assert code == 0 and out["commented"] == 77
    assert any(c.startswith(f"pr comment 77 --repo {REPO} --body-file") for c in gh.calls)


def test_push_branch_refuses_master_and_anything_resolving_to_it(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    before = g.rev("origin/master")
    for ref in ("master", "refs/heads/master", "HEAD"):
        code, out = run(capsys, clone, "push-branch", "--branch", ref, gh_run=FakeRunner(readonly=True))
        assert code == 1 and out["error"] == "refusing to push master", ref
    assert g.rev("origin/master") == before


def test_push_branch_refuses_without_a_lock(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    head = commit_file(clone, "src/x.rs", "x\n", "work")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    assert code == 1 and "FenceError" in out["error"]
    assert not g.ok("rev-parse", "--verify", "origin/issue/8-x-a1")
    assert head  # the branch exists locally; only the push was refused


def test_push_branch_pushes_a_feature_branch_and_sets_upstream(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    head = commit_file(clone, "src/x.rs", "x\n", "work")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["pushed"] == "issue/8-x-a1" and out["head"] == head
    g.fetch()
    assert g.rev("origin/issue/8-x-a1") == head


def test_push_branch_force_with_lease_after_a_rebase(capsys, repo_pair):
    # The force-with-lease carve-out becomes enforceable rather than
    # aspirational only if the force form is issued by code that refuses
    # `master` -- which is the whole point of routing it through here.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    commit_file(clone, "src/x.rs", "x\n", "work")
    run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    g._run("commit", "-q", "--amend", "-m", "work, rebased")  # history rewritten
    rewritten = g.rev("HEAD")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", "--force-with-lease",
                     gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["head"] == rewritten
    g.fetch()
    assert g.rev("origin/issue/8-x-a1") == rewritten


def test_run_fences_against_a_foreign_lock_and_flags_unsupervised(capsys, root):
    # Pin I6: a lock owned by another run refuses `run` outright; no lock at
    # all runs unsupervised and says so in the JSON.
    lock.acquire(Ctx(Paths(root), run_id="owner-run"), 8, "s", 1)
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    lock.release(Ctx(Paths(root), run_id="owner-run"))
    code, out = run(capsys, root, "run", "--name", "probe2", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), run_id="solo-run")
    assert code == 0 and out["supervised"] is False
