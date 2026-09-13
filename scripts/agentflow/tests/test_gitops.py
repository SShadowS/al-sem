from agentflow.gitops import Git
from agentflow.tests.conftest import commit_file


def test_basic_queries(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    assert g.branch() == "master" and g.is_clean()
    a = g.rev("HEAD")
    b = commit_file(clone, "src/x.rs", "fn x(){}\n", "add x")
    assert g.changed_files(a, b) == ["src/x.rs"]
    assert "+fn x(){}" in g.diff(a, b)
    assert g.is_ancestor(a, b) and not g.is_ancestor(b, a)


def test_tracked_dirty_ignores_untracked_but_sees_modified_tracked_files(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    assert not g.tracked_dirty()
    (clone / "untracked.txt").write_text("x\n")
    assert not g.tracked_dirty()  # untracked: not tracked-dirty
    assert not g.is_clean()       # but the plain porcelain check DOES see it
    (clone / "untracked.txt").unlink()
    (clone / "README.md").write_text("changed\n")
    assert g.tracked_dirty()      # a modified TRACKED file: dirty


def test_worktree_add_and_prune(repo_pair, tmp_path):
    origin, clone = repo_pair
    g = Git(clone)
    wt = tmp_path / "wt-a1"
    g.worktree_add(wt, "issue/8-x-a1", "master")
    assert Git(wt).branch() == "issue/8-x-a1"
    import shutil
    shutil.rmtree(wt)
    g.worktree_prune()
    g.branch_delete("issue/8-x-a1")
    assert "issue/8-x-a1" not in g.out("branch", "--list")


def test_rebase_ok_and_conflict_aborts(repo_pair, tmp_path):
    origin, clone = repo_pair
    g = Git(clone)
    g.out("checkout", "-q", "-b", "feat")
    commit_file(clone, "a.txt", "feat\n", "feat a")
    g.out("checkout", "-q", "master")
    commit_file(clone, "b.txt", "master\n", "master b")
    g.out("push", "-q", "origin", "master")
    g.out("checkout", "-q", "feat")
    r = g.rebase("origin/master")
    assert r.ok and r.conflicts == []
    commit_file(clone, "b.txt", "feat edit\n", "feat b")
    g.out("checkout", "-q", "master")
    commit_file(clone, "b.txt", "master edit\n", "master b2")
    g.out("push", "-q", "origin", "master")
    g.out("checkout", "-q", "feat")
    r = g.rebase("origin/master")
    assert not r.ok and r.conflicts == ["b.txt"] and g.is_clean() and g.branch() == "feat"


def test_revert_push_and_ff(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "c.txt", "bad\n", "bad")
    assert g.push("origin", "master")
    assert g.revert(bad)
    assert not (clone / "c.txt").exists()
    assert g.push("origin", "master")
    g.out("reset", "-q", "--hard", "HEAD~1")
    assert g.ff("origin/master") and not (clone / "c.txt").exists()


def test_merge_squash_helper(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    g.out("checkout", "-q", "-b", "feat")
    commit_file(clone, "d.txt", "d\n", "d")
    g.out("checkout", "-q", "master")
    sha = g.merge_squash("feat", "squash feat")
    assert g.rev("HEAD") == sha and (clone / "d.txt").exists()
