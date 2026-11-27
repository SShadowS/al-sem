import json
from pathlib import Path

import pytest

from agentflow import worktrees
from agentflow.gitops import Git, GitError
from agentflow.state import Ctx, Paths
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
        worktrees.create(Ctx(Paths(clone)), Git(clone), d, Git(clone).rev("master"))
    assert calls == [], calls
    assert (d / "precious.txt").exists()


def test_create_replaces_a_leftover_verification_tree_and_lands_detached_on_the_commit(repo_pair):
    """PRECONDITION HAND-STATED: a crashed previous run's verification tree (a
    worktree git registered, with junk in it) at the exact verify path.
    Measured: `git worktree add` over an existing directory fails rc=128, so
    such a leftover would otherwise make every later verification unrunnable."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    sha = commit_file(clone, "thing.txt", "x\n", "a commit to verify")
    p = worktrees.verify_path(clone)
    g.worktree_add_detached(p, g.rev("master"))
    (p / "junk.txt").write_text("left behind by a crashed run\n")

    token = worktrees.create(ctx, g, p, sha)
    assert not (p / "junk.txt").exists()
    wg = Git(p)
    assert wg.rev("HEAD") == sha
    assert wg.branch() == "HEAD"                     # detached: no branch of its own
    assert p.name.startswith(worktrees.VERIFY_PREFIX) and p.parent == clone.parent
    assert (p / "thing.txt").exists()                # a real checkout, not an empty dir

    assert worktrees.remove(ctx, g, p, token) is True
    assert not p.exists()
    assert worktrees.VERIFY_PREFIX not in g.out("worktree", "list")


def test_create_refuses_an_unregistered_directory_at_the_path(repo_pair):
    """A directory at the verify path that git has not registered as a
    worktree of this repository is not agentflow's: `create` refuses
    (`NotOurs`) and the directory survives, untouched (sol r12). So does
    `remove` (a refused create gives no token), which reports the path as not
    freed.
    Red proof (sol r12, run at sol r14): the `registered` check dropped from
    `worktrees._check_and_detach_held` failed here (the directory was
    deleted)."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    p.mkdir()
    (p / "precious.txt").write_text("not agentflow's\n")
    with pytest.raises(worktrees.NotOurs):
        worktrees.create(ctx, g, p, g.rev("master"))
    assert (p / "precious.txt").read_text() == "not agentflow's\n"
    assert worktrees.remove(ctx, g, p, None) is False
    assert (p / "precious.txt").exists()


def _reservation(ctx, p):
    return ctx.paths.agent / "verify-reservations" / f"{p.name}.json"


def test_the_invocation_that_lost_the_race_leaves_the_winners_tree(repo_pair, monkeypatch):
    """Two invocations (the same run id twice) reach the verification tree.
    The winner's `create` reserves the path for this attempt, under
    `registry`, before `git worktree add`. The loser finds a reservation of a
    live invocation and refuses at once, with no token. The winner then EXITS
    between the loser's refusal and the loser's `finally`, leaving a dirty
    tree: the loser's teardown, `remove(..., None)`, still touches nothing
    (sol r14). Reclaiming the dead winner's tree is a new create's or an
    explicit cleanup's job, never a teardown's.
    The invocations are one test process told apart by `worktrees._ME`; the
    winner "exits" by its reservation's pid being made a dead one.
    Red proof (sol r14): `remove` with no token falling back to
    `_take_owned_held` (a teardown that reclaims) failed here (the loser
    deleted the dead winner's dirty tree)."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    sha = commit_file(clone, "thing.txt", "x\n", "a commit to verify")
    p = worktrees.verify_path(clone)
    winner = dict(worktrees._ME)
    worktrees.create(ctx, g, p, sha)                 # the winner's attempt
    monkeypatch.setattr(worktrees, "_ME", {**winner, "nonce": "0" * 16})
    with pytest.raises(worktrees.NotOurs, match="reserved by another invocation"):
        worktrees.create(ctx, g, p, sha)             # the loser: refused, no token
    res = json.loads(_reservation(ctx, p).read_text(encoding="utf-8"))
    _reservation(ctx, p).write_text(json.dumps({**res, "pid": 2 ** 22 + 12345}), encoding="utf-8")
    (p / "evidence.txt").write_text("the winner died here\n")   # the winner exits, tree dirty
    assert worktrees.remove(ctx, g, p, None) is False            # the loser's `finally`
    assert Git(p).rev("HEAD") == sha and (p / "evidence.txt").exists()


def test_remove_acts_only_on_its_own_tokens_reservation(repo_pair):
    """`remove` takes the tree only while the reservation still equals the
    token its `create` returned: a stale or foreign token touches nothing."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    token = worktrees.create(ctx, g, p, g.rev("master"))
    assert worktrees.remove(ctx, g, p, {**token, "attempt": "another"}) is False
    assert p.exists()
    assert worktrees.remove(ctx, g, p, token) is True
    assert not p.exists() and not _reservation(ctx, p).exists()


def test_a_new_create_reclaims_a_dead_invocations_reservation(repo_pair):
    """A reservation whose process has ended (a crashed post-merge, a retained
    tree) does not block the NEXT create, which reclaims the tree."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    worktrees.create(ctx, g, p, g.rev("master"))
    dead = {"pid": 2 ** 22 + 12345, "start": "1", "nonce": "f" * 16, "attempt": "x", "path": str(p)}
    _reservation(ctx, p).write_text(json.dumps(dead), encoding="utf-8")
    token = worktrees.create(ctx, g, p, g.rev("master"))
    assert json.loads(_reservation(ctx, p).read_text(encoding="utf-8")) == token


def test_liveness_is_three_valued(monkeypatch):
    """ALIVE, DEAD or UNKNOWN (sol r14): this process with its own start time
    is ALIVE; the same PID with another start time is a reused PID, DEAD; a
    PID with no /proc entry is DEAD; a stat that cannot be read is UNKNOWN,
    never DEAD; a malformed record is UNKNOWN.
    Red proof (sol r14): the unreadable-stat case returning DEAD (the
    `except (OSError, ValueError): return UNKNOWN` arm folded into the
    FileNotFoundError arm) failed here."""
    if not Path("/proc").is_dir():
        pytest.skip("the /proc branch")
    me = worktrees._ME
    assert worktrees._liveness(me) == worktrees.ALIVE
    assert worktrees._liveness({**me, "start": "1"}) == worktrees.DEAD
    assert worktrees._liveness({**me, "pid": 2 ** 22 + 12345}) == worktrees.DEAD
    assert worktrees._liveness({"pid": "x"}) == worktrees.UNKNOWN

    def unreadable(pid):
        raise PermissionError("hidepid")

    monkeypatch.setattr(worktrees, "_stat_start", unreadable)
    assert worktrees._liveness(me) == worktrees.UNKNOWN


def test_an_unknown_reservation_is_never_reclaimed(repo_pair, monkeypatch):
    """A reservation whose owner's liveness is UNKNOWN is never reclaimed
    automatically: a new `create` refuses, naming the reservation file so
    explicit cleanup can report it (sol r14)."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    worktrees.create(ctx, g, p, g.rev("master"))
    monkeypatch.setattr(worktrees, "_ME", {**worktrees._ME, "nonce": "0" * 16})
    monkeypatch.setattr(worktrees, "_liveness", lambda owner: worktrees.UNKNOWN)
    with pytest.raises(worktrees.NotOurs, match="unknown"):
        worktrees.create(ctx, g, p, g.rev("master"))
    assert p.exists()


def test_remove_never_raises(repo_pair, monkeypatch):
    """`remove` is fully best-effort and runs in `finally` blocks (sol r13,
    r14): a `registry` that stays busy, or a `destroy` that fails (`git
    worktree prune` timing out on the `git` mutex), yields a result, never an
    exception that would mask the one being cleaned up after.
    Red proofs (sol r14): `remove`'s `except Exception` around the locked
    step narrowed back to `(NotOurs, OSError)` failed the busy-registry case;
    the `try` around `destroy` dropped failed the destroy case."""
    from contextlib import contextmanager
    from agentflow.state import MutexTimeout
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    token = worktrees.create(ctx, g, p, g.rev("master"))

    def failing_destroy(git, path):
        raise GitError("git worktree prune: mutex 'git' still held")

    monkeypatch.setattr(worktrees, "destroy", failing_destroy)
    assert worktrees.remove(ctx, g, p, token) is False    # reported, never raised
    assert not p.exists()                                 # it was moved aside first
    monkeypatch.undo()
    token = worktrees.create(ctx, g, p, g.rev("master"))

    @contextmanager
    def busy(c, name, **kw):
        raise MutexTimeout("mutex 'registry' still held")
        yield

    monkeypatch.setattr(worktrees, "mutex", busy)
    assert worktrees.remove(ctx, g, p, token) is False


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
    p = worktrees.verify_path(clone)

    class LandsSomewhereElse(Git):
        def rev(self, ref):
            return "0" * 40 if ref == "HEAD" else super().rev(ref)

    monkeypatch.setattr(worktrees, "Git", LandsSomewhereElse)
    with pytest.raises(GitError, match="landed on"):
        worktrees.create(Ctx(Paths(clone)), g, p, sha)
    assert not p.exists()
    # ...and the registration is gone too, or the NEXT `create` at this path
    # fails rc=128 on a stale entry.
    assert worktrees.VERIFY_PREFIX not in g.out("worktree", "list")


@pytest.mark.parametrize("start", [None, "", 12345, ["1"]])
def test_a_reservation_without_a_valid_start_time_is_unknown(start):
    """A reservation whose recorded start time is missing or invalid gives
    UNKNOWN, never DEAD: compared with a live process's real start time it
    would read as a reused PID, and its live owner would be reclaimed (sol r15).
    Red proof (sol r15): the recorded-start check removed from `_liveness`
    failed here for every case (DEAD: a live owner reclaimable)."""
    if not Path("/proc").is_dir():
        pytest.skip("the /proc branch")
    owner = {**worktrees._ME, "start": start}
    assert worktrees._liveness(owner) == worktrees.UNKNOWN


def test_remove_never_raises_from_its_final_check(repo_pair, monkeypatch):
    """`remove`'s last step, checking that the path is free, is best-effort
    too: an error there yields False, never an exception (sol r15).
    Red proof (sol r15): the final check outside the `try` failed here
    (PermissionError escaped)."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)
    token = worktrees.create(ctx, g, p, g.rev("master"))
    flaky = {"on": False}

    class FlakyPath(type(p)):
        def exists(self, *a, **kw):
            if flaky["on"]:
                raise PermissionError("cannot stat")
            return super().exists(*a, **kw)

    real_destroy = worktrees.destroy

    def destroy_then_flake(git, tomb):
        gone = real_destroy(git, tomb)
        flaky["on"] = True
        return gone

    monkeypatch.setattr(worktrees, "destroy", destroy_then_flake)
    assert worktrees.remove(ctx, g, FlakyPath(p), token) is False


def test_create_goes_on_when_its_prune_finds_the_git_mutex_busy(repo_pair, monkeypatch):
    """`create`'s prune before the add is best-effort: a `MutexTimeout` from a
    busy `git` mutex (not a GitError) is logged and the create goes on (sol
    r15). It runs inside the token's rollback, so any other failure there
    still tears the attempt down.
    Red proof (sol r15): the prune's `except` narrowed back to `GitError`
    failed here (MutexTimeout escaped, the attempt was rolled back)."""
    from agentflow.state import MutexTimeout
    _, clone = repo_pair
    g = Git(clone)
    ctx = Ctx(Paths(clone))
    p = worktrees.verify_path(clone)

    def busy_prune():
        raise MutexTimeout("mutex 'git' still held")

    monkeypatch.setattr(g, "worktree_prune", busy_prune)
    token = worktrees.create(ctx, g, p, g.rev("master"))
    assert Git(p).rev("HEAD") == g.rev("master")
    assert json.loads((ctx.paths.agent / "verify-reservations" / f"{p.name}.json")
                      .read_text(encoding="utf-8")) == token
