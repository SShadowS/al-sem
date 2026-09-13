"""Incident paths: validated revert after a red master, stale-run recovery,
worktree cleanup with ownership checks, and local retention of evidence.

Order in `post_merge_failure` is deliberate: HALT is written first so no
other tick can resume while a fallible remote action is in flight.
"""
from __future__ import annotations

import shutil
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from . import lock
from .gh import Gh
from .gitops import Git, GitError
from .state import Ctx

NOTIFY_KINDS = ("regressed", "halted", "sanitize-failed", "gh-unavailable", "reviewer-unavailable", "crashed")
WORKTREE_PREFIX = "al-sem-issue-"


def worktree_name(issue: int, attempt: int) -> str:
    return f"{WORKTREE_PREFIX}{issue}-a{attempt}"


def notify(ctx: Ctx, kind: str, message: str) -> None:
    line = f"NOTIFY: {kind}: {message}"
    print(line, file=sys.stderr)
    try:
        ctx.run_dir.mkdir(parents=True, exist_ok=True)
        with open(ctx.run_dir / "notify.log", "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except (RuntimeError, OSError):  # no run_id, or an unwritable run directory
        pass


@dataclass
class RevertOutcome:
    halted: bool
    reverted: bool
    pushed: bool
    reason: str
    revert_sha: str | None = None


def post_merge_failure(ctx: Ctx, git: Git, gh: Gh, issue: int, merge_sha: str,
                       rerun_gates: Callable[[], bool]) -> RevertOutcome:
    # HALT first, with a SHA-only reason: this must land before any git call
    # that can raise (an unfetched or unknown merge_sha raises GitError).
    reason = f"regression: merge {merge_sha[:12]} failed post-merge gates"
    lock.set_halt(ctx, reason)
    git.fetch()
    try:
        subject = git.out("log", "-1", "--format=%s", merge_sha)
        reason = f"regression: {subject} ({merge_sha[:12]}) failed post-merge gates"
        lock.set_halt(ctx, reason)
    except GitError:
        pass  # merge_sha not (yet) reachable locally; keep the SHA-only reason
    git.checkout("master")
    if not git.ff("origin/master") or git.rev("master") != git.rev("origin/master"):
        git.out("reset", "-q", "--hard", "origin/master")
        _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; local `master` had diverged from "
                                 f"`origin/master`. Reset to `origin/master`; nothing reverted or pushed. HALT set.")
        notify(ctx, "regressed", f"#{issue}: local master diverged from origin/master, refusing to act")
        return RevertOutcome(True, False, False, "master-not-ff")
    if not git.revert(merge_sha):
        _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; automatic revert CONFLICTED. `master` untouched. HALT set.")
        notify(ctx, "regressed", f"#{issue}: revert conflicted, master left red")
        return RevertOutcome(True, False, False, "revert-conflict")
    revert_sha = git.rev("HEAD")
    if not rerun_gates():
        git.out("reset", "-q", "--hard", "origin/master")
        _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; the revert ALSO fails gates, not pushed. `master` untouched. HALT set.")
        notify(ctx, "regressed", f"#{issue}: revert fails gates, master left red")
        return RevertOutcome(True, True, False, "revert-failed-gates", revert_sha)
    git.fetch()
    if git.rev("origin/master") != merge_sha:
        git.out("reset", "-q", "--hard", "origin/master")
        _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; `master` advanced meanwhile, revert NOT pushed. HALT set.")
        notify(ctx, "regressed", f"#{issue}: master advanced, revert not pushed")
        return RevertOutcome(True, True, False, "master-advanced", revert_sha)
    if not git.push("origin", "master"):
        git.out("reset", "-q", "--hard", "origin/master")
        _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; revert push REJECTED. HALT set.")
        notify(ctx, "regressed", f"#{issue}: revert push rejected")
        return RevertOutcome(True, True, False, "push-rejected", revert_sha)
    gh.reopen_issue(issue)
    _bookkeeping(gh, issue, f"Post-merge gates failed on {merge_sha}; reverted in {revert_sha}. HALT set; a human must look before the loop resumes.")
    notify(ctx, "regressed", f"#{issue}: reverted {merge_sha[:12]} as {revert_sha[:12]}")
    return RevertOutcome(True, True, True, "reverted", revert_sha)


def _bookkeeping(gh: Gh, issue: int, comment: str) -> None:
    gh.add_labels(issue, ["agent-regressed"])
    gh.remove_label(issue, "agent-working")
    gh.comment(issue, comment)


def recover_stale(ctx: Ctx, git: Git, gh: Gh, lk: lock.Lock, worktrees_parent: Path) -> dict:
    ctx.write_guard("recover stale run")
    pr = gh.pr_for_branch_prefix(f"issue/{lk.issue}-")
    if pr and pr.get("state") == "MERGED":
        ctx.paths.lock.unlink(missing_ok=True)
        return {"action": "merged-needs-post-merge", "merge_sha": pr["mergeCommit"]["oid"], "pr": pr["number"]}
    stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime(ctx.now()))
    moved = []
    try:
        for wt in worktrees_parent.glob(f"{WORKTREE_PREFIX}{lk.issue}-a*"):
            if ".crashed-" in wt.name:
                continue
            dest = wt.with_name(f"{wt.name}.crashed-{stamp}")
            wt.rename(dest)
            moved.append(str(dest))
        git.worktree_prune()
        found = f"open PR #{pr['number']} ({pr['state']})" if pr else "no PR"
        # Bookkeeping under the OLD run's fence: temporarily adopt its run id.
        old = Ctx(paths=ctx.paths, run_id=lk.run_id, now=ctx.now)
        gh_old = Gh(old, gh.repo, run=gh.run, sleep=gh.sleep)
        gh_old.comment(lk.issue, f"Run {lk.run_id} went silent (heartbeat older than 30 min). Found: {found}. "
                                 f"Worktree preserved as {moved or 'none'}. Labeled agent-blocked (crashed).")
        gh_old.add_labels(lk.issue, ["agent-blocked"])
        gh_old.remove_label(lk.issue, "agent-working")
        return {"action": "blocked-crashed", "moved": moved, "pr": pr["number"] if pr else None}
    finally:
        # A rename/prune/gh failure must still free the lock and notify, or a
        # stranded lock blocks every future tick with no forward progress.
        ctx.paths.lock.unlink(missing_ok=True)
        notify(ctx, "crashed", f"#{lk.issue}: stale run {lk.run_id} recovered")


def remove_worktree(ctx: Ctx, git: Git, path: Path, branch: str, expected_parent: Path, merge_sha: str) -> None:
    ctx.write_guard("remove worktree")
    path = path.resolve()
    if path.parent != expected_parent.resolve():
        raise RuntimeError(f"worktree {path} is outside {expected_parent}")
    if not Git(path).is_clean():
        raise RuntimeError(f"worktree {path} is not clean")
    # A squash-merged branch is never an ancestor of master; its TREE equals the
    # squash commit's tree (the merge gate held the base fixed), so compare trees.
    if not git.ok("diff", "--quiet", branch, merge_sha):
        raise RuntimeError(f"branch {branch} is not merged: tree differs from {merge_sha[:12]}")
    for attempt in range(3):
        try:
            shutil.rmtree(path)
            break
        except OSError:
            if attempt == 2:
                raise
            time.sleep(2)
    git.worktree_prune()
    git.branch_delete(branch)


def retain(ctx: Ctx, dest_root: Path | None = None) -> Path:
    ctx.write_guard("retain run dir")
    root = dest_root or (Path.home() / ".al-sem" / "agentflow" / "runs")
    dest = root / ctx.run_id
    if dest.exists():
        shutil.rmtree(dest)
    shutil.copytree(ctx.run_dir, dest)
    return dest
