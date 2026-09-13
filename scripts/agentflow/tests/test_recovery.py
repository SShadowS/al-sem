import shutil

import pytest

from agentflow import lock, recovery
from agentflow.gh import Gh, GhError
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths
from agentflow.tests.conftest import FakeRunner, commit_file

REPO = "SShadowS/al-sem"


def gh_ok(extra=None):
    resp = {"issue reopen 8": "", "issue edit 8 --add-label agent-regressed": "", "issue comment 8 *": "",
            "issue edit 8 --remove-label agent-working": ""}
    resp.update(extra or {})
    return FakeRunner(resp)


def make_ctx(clone):
    (clone / ".agent").mkdir(exist_ok=True)
    c = Ctx(paths=Paths(clone), run_id="run-test", now=lambda: 1_000_000.0)
    lock.acquire(c, 8, "s", 1)
    return c


def test_post_merge_failure_sets_halt_first_then_validated_revert_and_push(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    seen = []
    def rerun():
        seen.append(lock.halted(ctx))
        return True
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), issue=8, merge_sha=bad, rerun_gates=rerun)
    assert seen == ["regression: merge of #8 (" + bad[:12] + ") failed post-merge gates"]
    assert out.halted and out.reverted and out.pushed
    g.fetch()
    assert not (clone / "bad.txt").exists() and g.rev("origin/master") == out.revert_sha


def test_post_merge_failure_does_not_push_when_revert_fails_gates(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda: False)
    assert out.halted and out.reverted and not out.pushed and out.reason == "revert-failed-gates"
    g.fetch()
    assert g.rev("origin/master") == bad


def test_post_merge_failure_refuses_push_when_master_advanced(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    other = tmp_path / "other"
    import subprocess
    subprocess.run(["git", "clone", "-q", str(repo_pair[0]), str(other)], check=True)
    Git(other).out("config", "user.email", "o@x"); Git(other).out("config", "user.name", "o")
    commit_file(other, "z.txt", "z\n", "someone else")
    Git(other).push("origin", "master")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda: True)
    assert out.reverted and not out.pushed and out.reason == "master-advanced"


def test_post_merge_failure_refuses_when_local_master_diverged(repo_pair):
    """C1 (round 2 ruling): an unpushed local commit on top of the merge SHA must
    never ride along on the revert push, but the flow must also never DISCARD
    state it did not create. The function must refuse (`master-not-ff`), leave
    local `master` and the working tree exactly as found, and leave the remote
    untouched."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    extra = commit_file(clone, "extra.txt", "extra\n", "unpushed local work")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda: True)
    assert out.reason == "master-not-ff" and not out.pushed and not out.reverted
    g.fetch()
    assert g.rev("origin/master") == bad  # remote untouched
    assert g.rev("master") == extra  # local left exactly as found, nothing discarded
    assert (clone / "extra.txt").exists()


def test_post_merge_failure_sets_halt_even_when_merge_sha_is_unknown(repo_pair):
    """I2: HALT must be written before any git call that can raise on a bad SHA
    (e.g. one handed over from `recover_stale` before a fetch)."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    bogus = "0" * 40
    try:
        recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bogus, rerun_gates=lambda: True)
    except Exception:
        pass
    assert lock.halted(ctx) is not None


def test_recover_stale_preserves_tree_and_finishes_if_merged(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-x-a1", "master")
    lk = lock.read(ctx)
    stale_ctx = Ctx(paths=ctx.paths, run_id="run-new", now=lambda: lk.heartbeat + 4000)
    r = FakeRunner({"pr list *": "[]", "issue comment 8 *": "", "issue edit 8 --add-label agent-blocked": "",
                    "issue edit 8 --remove-label agent-working": ""})
    rep = recovery.recover_stale(stale_ctx, g, Gh(stale_ctx, REPO, run=r), lk, worktrees_parent=tmp_path)
    assert rep["action"] == "blocked-crashed"
    assert not wt.exists() and list(tmp_path.glob(f"{recovery.worktree_name(8, 1)}.crashed-*"))
    assert lock.read(ctx) is None
    lk2 = lock.acquire(ctx, 8, "s", 2)
    r2 = FakeRunner({"pr list *": '[{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a2", "headRefOid": "h", "mergeCommit": {"oid": "abc"}, "mergedAt": "x"}]'})
    rep2 = recovery.recover_stale(Ctx(paths=ctx.paths, run_id="run-new2", now=lambda: lk2.heartbeat + 4000), g,
                                  Gh(ctx, REPO, run=r2), lk2, worktrees_parent=tmp_path)
    assert rep2 == {"action": "merged-needs-post-merge", "merge_sha": "abc", "pr": 3, "branch": "issue/8-x-a2"}


def test_recover_stale_removes_lock_even_when_gh_comment_fails(repo_pair, tmp_path):
    """I3: everything after the merged-PR check must run under try/finally so a
    gh outage during bookkeeping still frees the stale lock and still notifies."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-x-a1", "master")
    lk = lock.read(ctx)
    stale_ctx = Ctx(paths=ctx.paths, run_id="run-new", now=lambda: lk.heartbeat + 4000)
    r = FakeRunner({"pr list *": "[]"})
    r.responses["issue comment 8 *"] = (1, "", "HTTP 500: boom")
    with pytest.raises(GhError):
        recovery.recover_stale(stale_ctx, g, Gh(stale_ctx, REPO, run=r, sleep=lambda s: None),
                               lk, worktrees_parent=tmp_path)
    assert not ctx.paths.lock.exists()
    assert (stale_ctx.run_dir / "notify.log").exists()


def test_remove_worktree_checks_parent_clean_and_merged(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-x-a1", "master")
    (wt / "dirty.txt").write_text("d")
    with pytest.raises(RuntimeError, match="not clean"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=g.rev("master"))
    (wt / "dirty.txt").unlink()
    commit_file(wt, "f.txt", "f\n", "unmerged work")
    with pytest.raises(RuntimeError, match="not merged"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=g.rev("master"))
    with pytest.raises(RuntimeError, match="outside"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path / "elsewhere", merge_sha=g.rev("master"))
    sha = g.merge_squash("issue/8-x-a1", "squash")
    recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=sha)
    assert not wt.exists() and "issue/8-x-a1" not in g.out("branch", "--list")


def test_remove_spike_worktree_removes_a_commit_free_branch(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(9, 1)
    g.worktree_add(wt, "issue/9-spike-a1", "master")
    recovery.remove_spike_worktree(ctx, g, wt, "issue/9-spike-a1", expected_parent=tmp_path)
    assert not wt.exists() and "issue/9-spike-a1" not in g.out("branch", "--list")


def test_remove_spike_worktree_refuses_a_branch_with_a_commit(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(9, 2)
    g.worktree_add(wt, "issue/9-spike-a2", "master")
    commit_file(wt, "probe.txt", "spike code, not just reading\n", "spike accidentally committed code")
    with pytest.raises(RuntimeError, match="not spike-clean"):
        recovery.remove_spike_worktree(ctx, g, wt, "issue/9-spike-a2", expected_parent=tmp_path)
    assert wt.exists()


def test_remove_spike_worktree_ignores_untracked_but_refuses_tracked_dirt(repo_pair, tmp_path):
    # Review round 2, New Breakage 4: `/issue` step 1 always leaves an
    # untracked `.agent/issue-N/ledger.md` in a spike's worktree (a spike
    # never commits it), so `is_clean()` -- which counts untracked files as
    # dirt -- could never pass for the caller this function actually has.
    # `tracked_dirty()` must ignore the untracked ledger but still refuse a
    # genuinely modified TRACKED file.
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(9, 3)
    g.worktree_add(wt, "issue/9-spike-a3", "master")
    (wt / "ledger.md").write_text("untracked spike evidence\n")
    recovery.remove_spike_worktree(ctx, g, wt, "issue/9-spike-a3", expected_parent=tmp_path)
    assert not wt.exists() and "issue/9-spike-a3" not in g.out("branch", "--list")

    wt2 = tmp_path / recovery.worktree_name(9, 4)
    g.worktree_add(wt2, "issue/9-spike-a4", "master")
    (wt2 / "README.md").write_text("tracked and modified\n")  # README.md is tracked, see repo_pair
    with pytest.raises(RuntimeError, match="not clean"):
        recovery.remove_spike_worktree(ctx, g, wt2, "issue/9-spike-a4", expected_parent=tmp_path)
    assert wt2.exists()


def test_remove_spike_worktree_checks_parent(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(9, 5)
    g.worktree_add(wt, "issue/9-spike-a5", "master")
    with pytest.raises(RuntimeError, match="outside"):
        recovery.remove_spike_worktree(ctx, g, wt, "issue/9-spike-a5", expected_parent=tmp_path / "elsewhere")
    assert wt.exists()


def test_remove_worktree_succeeds_when_already_removed_by_hand(repo_pair, tmp_path):
    # Review round 2, New Breakage 5: a prior attempt can crash after deleting
    # the worktree directory but before `finish` ran. Re-running cleanup on
    # that now-nonexistent path must succeed (prune + delete the leftover
    # branch), not raise, or the tick that hits it can never make progress.
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(8, 5)
    g.worktree_add(wt, "issue/8-vanished-a1", "master")
    commit_file(wt, "f.txt", "f\n", "issue work")
    merge_sha = g.merge_squash("issue/8-vanished-a1", "squash issue/8-vanished-a1")
    shutil.rmtree(wt)
    recovery.remove_worktree(ctx, g, wt, "issue/8-vanished-a1", expected_parent=tmp_path, merge_sha=merge_sha)
    assert "issue/8-vanished-a1" not in g.out("branch", "--list")


def test_remove_spike_worktree_succeeds_when_already_removed_by_hand(repo_pair, tmp_path):
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(9, 6)
    g.worktree_add(wt, "issue/9-vanished-a1", "master")
    shutil.rmtree(wt)
    recovery.remove_spike_worktree(ctx, g, wt, "issue/9-vanished-a1", expected_parent=tmp_path)
    assert "issue/9-vanished-a1" not in g.out("branch", "--list")


def test_retain_copies_run_dir(ctx, tmp_path):
    (ctx.run_dir).mkdir(parents=True)
    (ctx.run_dir / "pi-sol-r1.md").write_text("review")
    dest = recovery.retain(ctx, dest_root=tmp_path / "keep")
    assert dest == tmp_path / "keep" / "run-test" and (dest / "pi-sol-r1.md").read_text() == "review"


def test_notify_writes_log(ctx, capsys):
    ctx.run_dir.mkdir(parents=True)
    recovery.notify(ctx, "regressed", "master red after #8")
    assert "NOTIFY: regressed: master red after #8" in capsys.readouterr().err
    assert "regressed" in (ctx.run_dir / "notify.log").read_text()
