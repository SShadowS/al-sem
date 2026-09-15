import pytest

from agentflow import worktrees
from agentflow.gitops import Git, GitError
from agentflow.tests.conftest import commit_file


def test_destroy_refuses_a_path_it_does_not_own(repo_pair, tmp_path, monkeypatch):
    """`destroy` runs `shutil.rmtree`. The ONLY thing between it and an
    arbitrary directory is the name prefix, so that guard gets its own pin --
    and the pin asserts the files SURVIVED, not merely that it raised.

    THE SECOND HALF USED TO BE UNFALSIFIABLE, and that is the reason for the
    stub below. `create`'s first two lines are `_guard(path)` then
    `destroy(git, path)`, and `destroy`'s own `_guard` raises the IDENTICAL
    message -- so `pytest.raises(..., match=...)` was satisfied by whichever
    guard happened to still exist, and `precious.txt` survived either way.
    MEASURED before this fix: deleting `_guard(path)` from `worktrees.create`
    left `test_worktrees.py` at 3 passed AND the whole suite at 231 passed,
    exit 0. The assertion and its comment read as coverage of create's own
    guard and provided none. That is the doctrine's "break redundant with a
    second code path"."""
    _, clone = repo_pair
    d = tmp_path / "not-ours"
    d.mkdir()
    (d / "precious.txt").write_text("someone's actual work\n")
    with pytest.raises(RuntimeError, match="not an agentflow verification worktree"):
        worktrees.destroy(Git(clone), d)
    assert (d / "precious.txt").read_text() == "someone's actual work\n"
    # `create` guards first too, before it would destroy anything. With
    # `destroy` stubbed out, ONLY create's own `_guard` can raise here -- and
    # `calls == []` says more than the raise does: it states that create
    # refused BEFORE it would have handed this path to an `rm -rf`. The stub
    # is installed here rather than at the top so the first half above still
    # drives the REAL `destroy`.
    calls = []
    monkeypatch.setattr(worktrees, "destroy", lambda *a, **kw: calls.append(a) or True)
    with pytest.raises(RuntimeError, match="not an agentflow verification worktree"):
        worktrees.create(Git(clone), d, Git(clone).rev("master"))
    assert calls == [], calls
    assert (d / "precious.txt").exists()


def test_create_replaces_a_leftover_directory_and_lands_detached_on_the_commit(repo_pair):
    """PRECONDITION HAND-STATED: a leftover directory at the exact verify
    path, with junk in it. Measured: `git worktree add` over an existing
    directory fails rc=128, so a crashed previous run's leftover would
    otherwise make every later verification unrunnable."""
    _, clone = repo_pair
    g = Git(clone)
    sha = commit_file(clone, "thing.txt", "x\n", "a commit to verify")
    p = worktrees.verify_path(clone, "merge", sha)
    p.mkdir()
    (p / "junk.txt").write_text("left behind by a crashed run\n")

    worktrees.create(g, p, sha)
    assert not (p / "junk.txt").exists()
    wg = Git(p)
    assert wg.rev("HEAD") == sha
    assert wg.branch() == "HEAD"                     # detached: no branch of its own
    assert p.name.startswith(worktrees.VERIFY_PREFIX) and p.parent == clone.parent
    assert (p / "thing.txt").exists()                # a real checkout, not an empty dir

    assert worktrees.destroy(g, p) is True
    assert not p.exists()
    assert worktrees.VERIFY_PREFIX not in g.out("worktree", "list")


def test_create_cleans_up_after_its_own_postcondition_failure(repo_pair, monkeypatch):
    """`worktree_add_detached` has already checked a directory out by the time
    the `landed != want` postcondition is evaluated, so a raise from there used
    to leave the worktree behind -- on a path whose whole contract is that
    nothing survives it. `cmd_post_merge`'s `create` sat OUTSIDE the
    `try/finally: removed = worktrees.destroy(...)` that follows it, so nothing
    else removed it either and `removed` kept the `True` it was initialised
    with and never reassigned.

    PRECONDITION HAND-STATED BY ASSIGNMENT. Production cannot be made to land
    a 40-hex commit id on the wrong commit -- `git worktree add --detach <sha>`
    either lands there or fails -- so the mismatch is constructed: the `Git`
    the function builds FOR THE NEW WORKTREE reports a different HEAD. The
    `git` handed in is untouched, so `want` is still the real answer and
    `destroy`'s own behaviour is unchanged.

    The `not path.exists()` assertion is OUTSIDE the `pytest.raises` block on
    purpose: inside, the block aborts at the raise and the assertion would
    never run -- which is how three round-1 proofs were weakened."""
    _, clone = repo_pair
    g = Git(clone)
    sha = commit_file(clone, "thing.txt", "x\n", "a commit to verify")
    p = worktrees.verify_path(clone, "merge", sha)

    class LandsSomewhereElse(Git):
        def rev(self, ref):
            return "0" * 40 if ref == "HEAD" else super().rev(ref)

    monkeypatch.setattr(worktrees, "Git", LandsSomewhereElse)
    with pytest.raises(GitError, match="landed on"):
        worktrees.create(g, p, sha)
    assert not p.exists()
    # ...and the registration is gone too, or the NEXT `create` at this path
    # fails rc=128 on a stale entry.
    assert worktrees.VERIFY_PREFIX not in g.out("worktree", "list")
