"""Disposable verification worktrees: create one at a commit, run gates in it,
delete it.

WHY a gate must not run in the shared checkout. The post-merge path verifies a
merge commit and, if a gate goes red, builds a revert and RE-RUNS the gates to
decide whether that revert may be pushed to `origin/master`. When both runs
happen in the same working tree, everything the first run left behind -- a
regenerated golden, a touched `Cargo.lock`, a `gen-syntax` rewrite, a
line-ending materialisation -- is inherited by the run that decides the push.
A revert that is RED from a clean checkout can be green-lit by the leavings of
the run it is reverting. Each verification therefore gets a tree that was
checked out seconds ago and is deleted afterwards.

TEARDOWN is `rm -rf` plus `git worktree prune`, never `git worktree remove`:
that command REFUSES outright in a repository with a submodule ("working trees
containing submodules cannot be moved or removed"), and this repository has
`tree-sitter-al/`. See CLAUDE.md, Prerequisites.

GATES CAN COMPILE IN THERE even though a worktree gets no submodule checkout
of its own, because `cli._run_gate` derives `TREE_SITTER_AL_PATH` from
`ctx.paths.root` rather than from the gate's cwd, and `scripts/ci-steps`'s
`grammar_dir()` honours that env var over its own `git rev-parse
--show-toplevel` fallback. That is a real dependency between two files, so
`tests/test_cli_flow.py` pins it rather than leaving it to be rediscovered.
"""
from __future__ import annotations

import shutil
import time
from pathlib import Path

from .gitops import Git, GitError

# The name is the ownership proof. `destroy` below deletes a directory tree,
# and the ONLY thing standing between it and an arbitrary path is this prefix.
VERIFY_PREFIX = "al-sem-verify-"


def _guard(path: Path) -> None:
    """Refuse any path this module does not own. Called FIRST in both `create`
    and `destroy` -- it is the guard on an `rm -rf`, so it raises rather than
    returning a verdict a caller could ignore."""
    if not path.name.startswith(VERIFY_PREFIX):
        raise RuntimeError(f"{path} is not an agentflow verification worktree")


def verify_path(root: Path, purpose: str, sha: str) -> Path:
    """Where a verification worktree for `sha` lives: a SIBLING of the repo
    root, which is where the flow's issue worktrees already live
    (`cli.cmd_claim` uses `ctx.paths.root.parent`). The distinct prefix keeps
    it out of `recovery.recover_stale`'s glob, which matches
    `al-sem-issue-{issue}-a*`."""
    return root.parent / f"{VERIFY_PREFIX}{purpose}-{sha[:12]}"


def create(git: Git, path: Path, commit: str) -> Path:
    """A pristine detached worktree at `commit`. Raises GitError if it cannot
    be made, or if it does not land where it was asked to.

    Any leftover directory is destroyed first: `git worktree add` over an
    existing path fails rc=128, so a crashed previous run must never be able
    to make the next verification unrunnable.

    IT CLEANS UP AFTER ITS OWN FAILURE. `worktree_add_detached` has already
    created a checked-out directory by the time the postcondition below is
    evaluated, so a raise from here used to leave that directory -- and its
    `git worktree` registration -- behind, on a path whose whole contract is
    that nothing survives it. `cmd_post_merge` then reported
    `verify_worktree_removed: true` off an initialiser it never reassigned.
    Teardown belongs in the function that created the thing, so BOTH call
    sites get it rather than only the one that remembered."""
    _guard(path)
    destroy(git, path)
    git.worktree_add_detached(path, commit)
    try:
        landed = Git(path, run=git.run).rev("HEAD")
        want = git.rev(commit)
        if landed != want:
            raise GitError(f"verification worktree {path} landed on {landed}, wanted {want}")
    except BaseException:
        # `except BaseException` and not `except GitError`: the point is that
        # the directory does not outlive this call, whatever ended it -- a
        # GitError from either `rev`, the postcondition raise, or a
        # KeyboardInterrupt. `destroy` is best-effort and swallows its own
        # failures, so it cannot mask the error being re-raised.
        destroy(git, path)
        raise
    return path


def destroy(git: Git, path: Path) -> bool:
    """Delete the worktree directory and prune the stale registration. Returns
    whether the directory is gone.

    Everything after `_guard` is BEST EFFORT: a teardown failure must never
    change a gate verdict. Windows holds file locks on a tree a build just
    touched, so `shutil.rmtree` gets the same 3-attempt / 2-second retry
    `recovery.remove_worktree` uses -- and then gives up rather than raising,
    leaving the caller to report `verify_worktree_removed: false`."""
    _guard(path)
    for attempt in range(3):
        if not path.exists():
            break
        try:
            shutil.rmtree(path)
            break
        except OSError:
            if attempt == 2:
                break  # deliberately swallowed: see the docstring
            time.sleep(2)
    try:
        git.worktree_prune()
    except GitError:
        pass
    return not path.exists()
