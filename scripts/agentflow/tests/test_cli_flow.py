import json
import shutil
import sys

from agentflow import cli, lock, mergeops, recovery
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


def test_attest_refuses_body_hash_mismatch_with_claim(capsys, root, tmp_path):
    # I8: attest must cross-check its --body-hash against claim.json, not
    # trust a caller-supplied hash on its own.
    ctx = Ctx(Paths(root), run_id="run-test")
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": "abc123"})
    register = tmp_path / "findings.json"
    register.write_text("[]")
    code, out = run(capsys, root, "attest", "--issue", "8", "--B", "B", "--H", "H",
                     "--final-head", "F", "--register", str(register), "--gates", "{}",
                     "--body-hash", "different-hash", gh_run=FakeRunner())
    assert code == 1 and out["error"] == "body-hash mismatch with claim"


def test_attest_accepts_matching_body_hash(capsys, root, tmp_path):
    ctx = Ctx(Paths(root), run_id="run-test")
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": "abc123"})
    register = tmp_path / "findings.json"
    register.write_text("[]")
    code, out = run(capsys, root, "attest", "--issue", "8", "--B", "B", "--H", "H",
                     "--final-head", "F", "--register", str(register), "--gates", "{}",
                     "--body-hash", "abc123", gh_run=FakeRunner())
    assert code == 0 and "path" in out


def test_merge_gate_detects_moved_head(capsys, repo_pair):
    # I10(a): a PR head that moved since the attestation must be caught.
    _, clone = repo_pair
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head="cafebabe" * 5,
                                register_hash="r" * 40, gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({
        **READS,
        f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
            {"headRefOid": "deadbeef" * 5,
             "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}]}),
    })
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["reasons"] == ["head-moved"]


def test_post_merge_happy_path_restores_master_and_reports_ok(capsys, repo_pair, monkeypatch):
    # I10(b): the full post-merge success path -- both gate lists patched to
    # a real, instant no-op child so no actual build/test tooling is needed.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "GATES", [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "CDO_GATE", ("noop-cdo", [sys.executable, "-c", "pass"], 1))
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
    monkeypatch.setattr(cli, "GATES", [("dirty", dirty_gate, 1)])
    monkeypatch.setattr(cli, "CDO_GATE", ("noop-cdo", [sys.executable, "-c", "pass"], 1))
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
    monkeypatch.setattr(cli, "GATES", [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "CDO_GATE", ("noop-cdo", [sys.executable, "-c", "pass"], 1))
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
