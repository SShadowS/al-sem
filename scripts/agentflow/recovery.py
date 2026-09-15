"""Incident paths: the validated revert after a red master, the HALT-only stop
when every gate passed but the tree came back unverified, stale-run recovery,
worktree cleanup with ownership checks, and local retention of evidence.

Order in `post_merge_failure` and `post_merge_unverified` is deliberate: HALT
is written first so no other tick can resume while a fallible remote action is
in flight.

Only `post_merge_failure` may revert, and only on an actual non-zero gate
result -- see `post_merge_unverified` for why that line is load-bearing. The
revert itself is built and validated in a DISPOSABLE detached worktree; the
shared checkout's only role on that path is refs (fetch and query) plus the
single push.
"""
from __future__ import annotations

import shutil
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from . import incidents, lock, worktrees
from .gh import Gh
from .gitops import Git, GitError
from .state import Ctx

# ---- the two-axis incident vocabulary --------------------------------------
REGRESSED = "agent-regressed"
UNVERIFIED = "agent-gates-green-unverified"
REVERT_LANDED = "agent-revert-landed"
REVERT_BLOCKED = "agent-revert-blocked"
INCIDENT_LABELS = (REGRESSED, UNVERIFIED, REVERT_LANDED, REVERT_BLOCKED)


def incident_labels(*, gate_red: bool, revert_landed: bool | None) -> list[str]:
    """Two INDEPENDENT axes, one label each.

    EVIDENCE -- did a gate actually go RED? `agent-regressed` when yes,
    `agent-gates-green-unverified` when no gate returned a red verdict and the
    merge still could not be verified. "No gate said no" covers both
    `tree-dirty-after-gates` (every gate returned 0) and `gate-timed-out` (a
    gate was killed at its timeout, so it returned nothing at all); neither is
    evidence against the commit, which is the only thing this axis claims.

    ACTION -- did a validated revert land? `agent-revert-landed` when
    `origin/master` is clean again, `agent-revert-blocked` when MASTER IS
    STILL CARRYING THE COMMIT. `revert_landed=None` means no revert was
    attempted at all, and there is then no action label to stamp.

    Both axes exist because of 2026-09-14. `_bookkeeping` used to stamp
    `agent-regressed` unconditionally, so merge 19f654e1 -- three real gates,
    0/0/0, a byte-identical dirty file -- was labelled a regression on the
    strength of the second axis alone. The naive inverse (label only when a
    revert lands) is just as wrong in the other direction: a red gate IS a
    regression whether or not the revert could be pushed, so `agent-regressed`
    has to survive `master-not-ff`, `revert-conflict` and `push-rejected` --
    the cases where a human most needs to know master is still red.

    `agent-blocked` is deliberately NOT reused for any of this: it already
    means "the work never landed", and on a CLOSED issue whose code is sitting
    on master that reads as a contradiction.
    """
    labels = [REGRESSED if gate_red else UNVERIFIED]
    if revert_landed is not None:
        labels.append(REVERT_LANDED if revert_landed else REVERT_BLOCKED)
    return labels


WORKTREE_PREFIX = "al-sem-issue-"


def worktree_name(issue: int, attempt: int) -> str:
    return f"{WORKTREE_PREFIX}{issue}-a{attempt}"


def _branch_exists(git: Git, branch: str) -> bool:
    return git.ok("show-ref", "--verify", "--quiet", f"refs/heads/{branch}")


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
    # Rides out through `asdict` into `post-merge`'s `revert` payload, which is
    # how the conductor and the human reading that JSON learn WHICH axis moved
    # without having to re-derive it from `reason`.
    labels: list[str] = field(default_factory=list)


@dataclass
class HaltOutcome:
    """The outcome of a STOP that is not a revert. `reverted` and `pushed` are
    carried (always False) so a consumer reading this next to `RevertOutcome`
    finds the same two claims in the same two places: nothing was reverted,
    nothing was pushed."""
    halted: bool
    reverted: bool
    pushed: bool
    reason: str
    # TRUNCATED for the payload, and `dirty_total` always carries the real
    # count beside it. One truncation, in one place (`post_merge_unverified`),
    # and every number this system reports about the dirt is the number it
    # actually measured. The call site used to truncate too, at a DIFFERENT
    # bound, and the reason text then reported `len(dirty)` of the already-cut
    # list as if it were the whole -- a 200-file dirty tree announced to a
    # human as 50, with the "and N more" tail understating by 150. Reporting a
    # number the system cannot vouch for as if it were established is the
    # arc's own error in miniature.
    dirty: list[str]
    # The merge SHA the durable record is keyed by -- what
    # `resolve-incident --merge-sha` wants, without catting a gitignored file.
    incident: str | None = None
    labels: list[str] = field(default_factory=list)
    dirty_total: int = 0
    # The verification worktree the caller KEPT, or None when it tore one
    # down as usual. On `tree-dirty-after-gates` the dirt IS the evidence, and
    # a list of path names cut at 20 is not the dirt -- the 2026-09-14 root
    # cause was found by comparing a file's on-disk BYTES against the
    # committed blob, which no path list can support. This field is how the
    # payload, and through `asdict` the conductor and the human reading it,
    # learn WHERE those bytes still are. `gate-timed-out` carries None: a
    # killed gate's tree holds a partial build, not evidence about a file.
    retained_worktree: str | None = None


MAX_DIRTY_LISTED = 20

# The ways a post-merge can fall short of VERIFIED without any gate having
# said no. Both route to `post_merge_unverified` (HALT only, never a revert);
# each is its own machine-readable code so the halt payload, the incident
# record and the issue comment all say which one happened.
TREE_DIRTY = "tree-dirty-after-gates"
GATE_TIMED_OUT = "gate-timed-out"


def post_merge_failure(ctx: Ctx, git: Git, gh: Gh, issue: int, merge_sha: str,
                       rerun_gates: Callable[[Path], bool]) -> RevertOutcome:
    """A post-merge gate went RED. Build a revert, validate it, and push it --
    or refuse, loudly, and leave `master` red for a human.

    The revert is built and validated in a DISPOSABLE detached worktree, never
    on the shared local `master`. Two reasons. A revert validated in the tree
    the failing gates just ran in inherits their leavings, so a revert that is
    red from a clean checkout can be green-lit by them. And building it on the
    shared `master` is what forced the three `reset --hard origin/master`
    calls this function used to carry to unwind itself; with the revert on a
    detached HEAD inside a directory about to be deleted, there is nothing to
    unwind. This function now performs NO checkout, NO merge, NO reset and NO
    revert in the shared working tree; `fetch`, read-only queries and one
    `push` are all it does there.

    `rerun_gates` is handed the tree to validate. The callee names it because
    a closure that chose its own tree is exactly how the validation ends up in
    a contaminated one.

    DELIBERATE NON-CHANGE: after a pushed revert, local `master` in the shared
    checkout is left BEHIND `origin/master`, still at the reverted-away merge.
    A best-effort fast-forward was considered and rejected -- HALT is set and a
    human is coming, the checkout they walk up to is EVIDENCE, and silently
    rewriting a working tree at the moment the system has declared it does not
    trust its own gate results is the same mistake in a smaller form. The next
    `preflight` reports `master-differs-from-origin` beside the halt row; both
    are true, and one `git pull --ff-only` clears it.
    """
    # HALT first, with a SHA-only reason: this must land before any git call
    # that can raise (an unfetched or unknown merge_sha raises GitError).
    reason = f"regression: merge {merge_sha[:12]} failed post-merge gates"
    lock.set_halt(ctx, reason)
    # The durable obligation, opened BEFORE `git.fetch()` -- i.e. before the
    # first call in this function that can raise. A crash anywhere in the
    # fallible git work below then still leaves the record on disk, and the
    # next tick's `preflight` refuses on it even if someone clears HALT.
    incidents.open_incident(ctx, merge_sha=merge_sha, issue=issue, gate_red=True,
                            reason="post-merge gates went red")
    headline = f"Post-merge gates failed on {merge_sha}"

    def done(reverted: bool, pushed: bool, code: str, comment_tail: str,
             notify_note: str, revert_sha: str | None = None, *,
             reopen: bool = False) -> RevertOutcome:
        """The single tail every exit from this function takes.

        Inside this function a revert is always attempted-or-refused, never
        not-attempted, so `pushed` is the right value for the ACTION axis and
        is never None. `gate_red` is unconditionally True because after the
        dirty-tree split there is exactly ONE caller left and it reaches here
        only on a genuinely non-zero gate exit; a `gate_red` parameter would
        be a constant no production caller ever passes False.

        ORDERING, and it is the whole reason `reopen` is a parameter here
        rather than a call three lines above the `return`: every LOCAL,
        DURABLE artifact is written before the first fallible GitHub call.
        `record_outcome` first, then `notify`, and only then the two remote
        writes -- `gh issue reopen` and `_bookkeeping`.

        `gh.reopen_issue` used to run at the call site, ABOVE this function,
        on the ONE exit that has already pushed a revert to `origin/master`.
        A GitHub 500 or a rate limit there propagated out of
        `post_merge_failure` before any record was written, so `out` in
        `cmd_post_merge` was never assigned: the emitted payload read
        `{"ok": false, "revert": null, "halt": null}` plus an error, and
        `incidents.json` still said `reason: "post-merge gates went red",
        revert_landed: null, labels: []` for a merge whose revert was live on
        master. The system had rewritten `origin/master` and recorded nothing
        about it.

        The `GhError` is deliberately NOT swallowed. The ordering makes the
        record durable; silently eating a failed reopen would be a different
        lie -- the issue would stay closed with nobody told.
        """
        labels = incident_labels(gate_red=True, revert_landed=pushed)
        incidents.record_outcome(ctx, merge_sha, reason=code, revert_landed=pushed, labels=labels)
        notify(ctx, "regressed", f"#{issue}: {notify_note}")
        if reopen:
            gh.reopen_issue(issue)
        _bookkeeping(gh, issue, labels, f"{headline}; {comment_tail}")
        return RevertOutcome(True, reverted, pushed, code, revert_sha, labels)

    git.fetch()
    try:
        subject = git.out("log", "-1", "--format=%s", merge_sha)
        reason = f"regression: {subject} ({merge_sha[:12]}) failed post-merge gates"
        lock.set_halt(ctx, reason)
    except GitError:
        pass  # merge_sha not (yet) reachable locally; keep the SHA-only reason
    # READ-ONLY, and fully qualified on both sides. The old form answered "can
    # local master fast-forward?" by PERFORMING the fast-forward -- a write to
    # a tree that may be dirty, and a checkout of a ref this function no longer
    # needs. Verdict-equivalent in all four cases (equal: `--is-ancestor` is
    # reflexive, passes; behind: passes; ahead: refuses; diverged: refuses).
    # `refs/heads/` and `refs/remotes/` rather than the bare names because
    # `git rev-parse master` prefers `refs/tags/master` over
    # `refs/heads/master` when both exist (measured) -- a name is not a fact,
    # and this one decides whether a REVERT happens.
    if not git.is_ancestor("refs/heads/master", "refs/remotes/origin/master"):
        # Never discard state the flow did not create: local `master` is left
        # exactly as found. (Defence in depth now rather than a precondition
        # for correctness -- the revert is built from the fetched commit in a
        # worktree and can no longer carry unpushed local work.)
        return done(False, False, "master-not-ff",
                    "local `master` has diverged from `origin/master`. Left untouched; "
                    "nothing reverted or pushed. HALT set.",
                    "local master diverged from origin/master, refusing to act")
    wt = worktrees.verify_path(ctx.paths.root, "revert", merge_sha)
    try:
        try:
            worktrees.create(git, wt, merge_sha)
        except GitError as e:
            return done(False, False, "revert-worktree-failed",
                        f"a clean worktree to build the revert in could not be created ({e}). "
                        f"`master` untouched, nothing pushed. HALT set.",
                        "no clean worktree for the revert, master left red")
        wgit = Git(wt, run=git.run)  # propagate the run seam into the worktree
        if not wgit.revert(merge_sha):
            return done(False, False, "revert-conflict",
                        "automatic revert CONFLICTED. `master` untouched. HALT set.",
                        "revert conflicted, master left red")
        revert_sha = wgit.rev("HEAD")
        if not rerun_gates(wt):
            return done(True, False, "revert-failed-gates",
                        "the revert ALSO fails gates, not pushed. `master` untouched. HALT set.",
                        "revert fails gates, master left red", revert_sha)
        git.fetch()
        if git.rev("origin/master") != merge_sha:
            return done(True, False, "master-advanced",
                        "`master` advanced meanwhile, revert NOT pushed. HALT set.",
                        "master advanced, revert not pushed", revert_sha)
        # Pinned to the id captured above and validated by `rerun_gates`, never
        # to whatever `master` names by now: that gate re-run just spent tens of
        # minutes, and the two checks above guard `origin/master`, not the local
        # ref. A one-sided `git push origin master` here can send a commit
        # nothing in this function ever gated. The push happens while the
        # worktree still references the revert commit, so the object cannot be
        # unreferenced before it is sent.
        if not git.push_master(revert_sha):
            return done(True, False, "push-rejected",
                        "revert push REJECTED. HALT set.", "revert push rejected", revert_sha)
    finally:
        worktrees.destroy(git, wt)
    # `reopen=True`, never a `gh.reopen_issue(issue)` call here: this is the
    # one exit that has already rewritten `origin/master`, so its durable
    # record must land before any GitHub write. See `done`.
    return done(True, True, "reverted",
                f"reverted in {revert_sha}. HALT set; a human must look before the loop resumes.",
                f"reverted {merge_sha[:12]} as {revert_sha[:12]}", revert_sha, reopen=True)


def post_merge_unverified(ctx: Ctx, gh: Gh, issue: int, merge_sha: str,
                          dirty: list[str], *, code: str = TREE_DIRTY,
                          detail: str = "", retained: str | None = None) -> HaltOutcome:
    """This run COULD NOT VERIFY the merge, and nothing it saw says the merge
    is bad. STOP -- do not revert.

    `code` says which way verification fell short, and it is a parameter
    rather than a hardcoded string because there is more than one way:

      `tree-dirty-after-gates`  every gate returned 0 and the working tree
                                came back with tracked modifications.
      `gate-timed-out`          the supervisor KILLED a gate at its timeout
                                (`detail` names it). `supervise.run` folds a
                                kill into `exit_code or 124`, so at the call
                                site a killed gate is indistinguishable from
                                one that said no unless `timed_out` is
                                carried -- and 124 routing to the REVERT path
                                would revert a commit on the strength of a
                                clock. A gate that never finished returned no
                                verdict at all.

    Overloading the dirty-file text for the second case was the alternative
    and is exactly the lie this function exists to stop telling: "passed every
    gate but left 0 files modified" is a false sentence about a run where no
    gate ever finished.

    A modified tracked file is evidence about the WORKING TREE, not about the
    merged commit. On 2026-09-14 the three real gates on merge 19f654e1
    returned 0, 0, 0 and the single dirty file was byte-IDENTICAL: it carried
    no line-ending attribute, so with `core.autocrlf` git materialised CRLF
    while the generator writes LF. The harness read that as proof the commit
    was bad and entered the revert path; it failed to push a revert of a
    fully-attested, CI-green commit only because `post_merge_failure` happened
    to hit its unrelated master-not-ff guard. The correct response to
    "unverified" is to stop, not to destroy.

    So: HALT, notify, label the issue for a human -- and leave `master`, the
    remote, and the dirt itself exactly as found. The dirt IS the evidence;
    the revert path's `reset --hard` would have wiped the very CRLF clue that
    root-caused the incident.

    `retained` is the CALLER's statement that it kept the tree the gates ran
    in, and it exists because for two rounds that sentence was false at the
    call site: `cmd_post_merge` wrote this HALT and then `rm -rf`'d the
    worktree holding the dirt one `finally` later, leaving only the path
    NAMES, cut at `MAX_DIRTY_LISTED`. Four texts -- the CHANGELOG, the spec
    twice, and the issue comment below -- told the operator to go and inspect
    a checkout that never had it. When it is set, this function names the
    retained path in the HALT reason, in the issue comment and in
    `HaltOutcome.retained_worktree`, so every surface points at the bytes.
    It is a STRING, not a `Path`, and this function still takes no `Git`: it
    reports where the evidence is and has no way to touch it.

    This function takes no `Git`. It has no way to revert, reset, or push, and
    that is the point: automatic revert belongs to `post_merge_failure` and
    requires an actual non-zero gate result.

    Ordering mirrors `post_merge_failure`: HALT and the notification -- the
    local, durable artifacts -- are written before the first fallible remote
    call, so a `gh` outage can cost the labels and the comment but never the
    record that the loop must stop.

    The issue is NOT reopened: unlike the revert path, the merged work stands.

    `dirty` must be the COMPLETE list of modified tracked paths. Every count
    this function reports -- the HALT reason, the notification, the issue
    comment, `HaltOutcome.dirty_total` -- is `len(dirty)`, so a caller that
    truncates first makes all four of them lie by the same amount. The one
    truncation lives here, at `MAX_DIRTY_LISTED`.
    """
    if code == GATE_TIMED_OUT:
        what = f"gate `{detail}` was KILLED at its timeout, so no gate verdict was reached"
        evidence = (
            f"The `{detail}` gate did not finish -- the supervisor killed it at its timeout, and "
            f"any gate after it never ran. A killed gate is **not** a gate that said no: it is a "
            f"clock running out, which says nothing at all about the merged commit. The gate's "
            f"log is under `.agent/runs/` in this checkout.")
    else:
        what = (f"every post-merge gate passed but the tree came back with {len(dirty)} tracked "
                f"file(s) modified")
        shown = dirty[:MAX_DIRTY_LISTED]
        more = len(dirty) - len(shown)
        listing = "\n".join(shown) + (f"\n... and {more} more" if more > 0 else "")
        evidence = (
            f"Post-merge gates ALL PASSED on {merge_sha}, but the working tree came back with "
            f"{len(dirty)} modified tracked file(s):\n\n```\n{listing}\n```\n\n"
            f"That is evidence about the working tree, not about the merged commit.")
    reason = f"unverified: merge {merge_sha[:12]} -- {what}; NOT reverted"
    if retained:
        reason += f"; evidence retained at {retained}"
    lock.set_halt(ctx, reason)
    notify(ctx, "unverified", f"#{issue}: {merge_sha[:12]} -- {what}; HALT set, nothing reverted")
    # EVIDENCE axis only: no gate returned a red verdict, so this is not a
    # regression. ACTION axis is None because no revert was attempted -- there
    # is nothing to say about whether one landed, and claiming either would be
    # a lie.
    labels = incident_labels(gate_red=False, revert_landed=None)
    # The durable record, written before the first GitHub call for the same
    # reason HALT is: a `gh` outage may cost the labels and the comment, never
    # the obligation that stops the loop.
    incidents.open_incident(ctx, merge_sha=merge_sha, issue=issue, gate_red=False, reason=code)
    incidents.record_outcome(ctx, merge_sha, reason=code, revert_landed=None, labels=labels)
    if retained:
        # POINT AT THE BYTES, not at a checkout that never held them. The
        # shared root is on `master` and clean -- no gate ran there -- so
        # "inspect the checkout" sent every operator to the wrong tree.
        where = (f"The tree the gates ran in is **retained** at `{retained}` -- the dirt itself, "
                 f"not just the path names above. Inspect it there: `git -C {retained} status` "
                 f"and `git -C {retained} diff`. A path `status` calls modified while `diff` "
                 f"shows nothing is a line-ending materialisation, not a change -- that exact "
                 f"comparison is how 2026-09-14 was root-caused. When you are done, remove it "
                 f"with `agentflow cleanup --worktree {retained}`.")
    else:
        where = ("A human must inspect the gate logs under `.agent/runs/` in this checkout.")
    _bookkeeping(gh, issue, labels,
                 f"{evidence}\n\nSo **nothing was reverted and nothing was pushed** -- `master` "
                 f"still carries {merge_sha[:12]} and the evidence is left exactly as found. "
                 f"{where} HALT is set and must be cleared before the loop resumes. The incident "
                 f"is recorded against `{merge_sha[:12]}` and keeps failing `preflight` until "
                 f"`resolve-incident` closes it -- clearing HALT alone is not enough, on purpose.")
    # `dirty` is handed over WHOLE by the caller and cut exactly once, here,
    # with the real count carried beside it. See `HaltOutcome.dirty`.
    return HaltOutcome(True, False, False, code, dirty[:MAX_DIRTY_LISTED], merge_sha, labels,
                       len(dirty), retained)


def _bookkeeping(gh: Gh, issue: int, labels: list[str], comment: str) -> None:
    """Stamp the incident labels, drop `agent-working`, and say what happened.

    `ensure_labels` runs FIRST and is not optional: `gh issue edit --add-label
    X` hard-FAILS when X does not exist on the repo, and the incident path is
    the worst possible place to fail. A run claimed before this vocabulary
    shipped did its claim-time `ensure_labels(LABELS)` against the OLD list,
    so the two-axis names may genuinely not exist yet. It costs one extra
    paginated read on a path that is already failing.
    """
    gh.ensure_labels(list(INCIDENT_LABELS))
    gh.add_labels(issue, labels)
    gh.remove_label(issue, "agent-working")
    gh.comment(issue, comment)


def _merged_before(pr: dict, when: float) -> bool:
    """True only when the PR's `mergedAt` PARSES and is strictly earlier than
    `when` -- i.e. only when GitHub's own answer is positive evidence that this
    merge predates the run being recovered.

    An absent, empty or unparseable `mergedAt` returns False. That is the
    arc's own rule applied to this guard: COULD NOT VERIFY is not PROVEN BAD,
    and refusing a self-heal because a timestamp did not parse would be the
    same error in a smaller form. The attempt match in `recover_stale` is what
    carries the guarantee; this is the second, independent axis.
    """
    raw = (pr or {}).get("mergedAt")
    if not raw:
        return False
    try:
        ts = datetime.fromisoformat(str(raw).replace("Z", "+00:00"))
    except (TypeError, ValueError):
        return False
    if ts.tzinfo is None:
        ts = ts.replace(tzinfo=timezone.utc)
    return ts.timestamp() < when


def recover_stale(ctx: Ctx, git: Git, gh: Gh, lk: lock.Lock, worktrees_parent: Path) -> dict:
    ctx.write_guard("recover stale run")
    # SCOPED TO THIS STALE RUN'S OWN ATTEMPT, on two independent axes.
    #
    # The branch name is `issue/<n>-<slug>-a<attempt>`, and the stale LOCK
    # records the attempt (`lock.Lock.attempt`), so `-a{lk.attempt}` names the
    # branch THIS run would have pushed. Without it a prefix match adopts any
    # attempt the issue ever had, and `pr_for_branch_prefix` ranks MERGED
    # first -- so a previous attempt's long-merged PR beats this attempt's own
    # open one. The adopted SHA is then written as a trusted MergeRecord
    # (cli.cmd_recover), `cmd_post_merge`'s provenance check passes BY
    # CONSTRUCTION, and today's gates run against a months-old tree: a moved
    # CDO ratchet, a regenerated golden or a bumped CACHE_VERSION_GRAMMAR is
    # each enough to go red, which HALTs, opens a preflight-blocking incident
    # and comments on the issue about a commit this tick established nothing
    # about. If master has not moved since, `post_merge_failure`'s
    # master-advanced guard also passes and the revert is PUSHED.
    #
    # The recency refusal is defence in depth on the same claim: a merge that
    # GitHub says happened before THE WORK WAS CLAIMED cannot be the merge of
    # the work this lock was holding.
    #
    # `lk.started` is that claim date and NOT "when this lock file was
    # written". `cli.cmd_recover` mints its follow-through lock with
    # `started=lk.started`, carrying the original claim forward, precisely so
    # this comparison keeps measuring the work's own claim on a SECOND
    # recovery. Stamped `now` instead, a follow-through lock is always later
    # than the merge it just adopted, and this axis would then refuse every
    # recovery-minted lock by construction -- turning a post-merge killed
    # mid-gates into a permanently unrecoverable `blocked-crashed`. The
    # heartbeat is not carried back, so staleness still measures liveness.
    pr = gh.pr_for_branch_prefix(f"issue/{lk.issue}-", suffix=f"-a{lk.attempt}")
    if pr and pr.get("state") == "MERGED" and not _merged_before(pr, lk.started):
        ctx.paths.lock.unlink(missing_ok=True)
        return {"action": "merged-needs-post-merge", "merge_sha": pr["mergeCommit"]["oid"], "pr": pr["number"],
                "branch": pr["headRefName"]}
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


def _refuse_unowned_path(path: Path, expected_parent: Path) -> None:
    """Refuse any directory this flow did not create.

    Both functions below end in `shutil.rmtree(path)`, and `path` arrives from
    argv. TWO conjuncts, because either alone is close to nothing:

    LOCATION -- a direct child of the repo root's parent. On this machine that
    parent is `U:/Git`, which holds 230 sibling directories, essentially all
    of them git repositories (`DO-cdo-baseline`, the pinned CDO baseline,
    among them). "Is a sibling of the checkout" is not ownership.

    NAME -- `al-sem-issue-<issue>-a<attempt>`, the name `worktree_name` builds
    and the only shape `cmd_claim` and `cmd_recover` ever hand out. This is
    the ownership proof, and it is the same one the sibling module states
    plainly about its own `rm -rf`: "the ONLY thing standing between it and an
    arbitrary path is this prefix" (`worktrees.VERIFY_PREFIX`).

    Neither the cleanliness probe nor the merge proof covers this. The probe
    asks about the tree's CONTENT -- a clean sibling checkout passes it -- and
    `git diff --quiet <branch> <merge_sha>` runs in the MAIN repo and says
    nothing about `path` at all (it is also satisfiable by its own argument:
    `git diff --quiet master master` exits 0, measured). So one wrong
    `--worktree`, with the other two arguments correct, was a deletion.

    Called FIRST in both removers -- above the already-gone early return,
    which is the path with the least standing between an argument and a
    delete. A verification worktree (`worktrees.VERIFY_PREFIX`) is
    deliberately NOT accepted here: it is detached and has no branch, so it
    has no business in a function that ends in `git branch -D`. `cmd_cleanup`
    routes it to `worktrees.destroy`, which owns that prefix and its guard.
    """
    if path.parent != expected_parent.resolve():
        raise RuntimeError(f"worktree {path} is outside {expected_parent}")
    if not path.name.startswith(WORKTREE_PREFIX):
        raise RuntimeError(
            f"worktree {path} is not an agentflow issue worktree "
            f"(name must start with {WORKTREE_PREFIX!r})")


def _refuse_protected_branch(git: Git, branch: str) -> None:
    """A branch this module may never run `git branch -D` on.

    Both functions below end in `git.branch_delete(branch)`, and their
    "is this branch finished with?" proofs do not discriminate `master`. In
    `remove_worktree` the proof is `git diff --quiet <branch> <merge_sha>`:
    with `--branch master --merge-sha <the commit just merged>` the two trees
    are IDENTICAL, so the proof passes and the delete runs. In
    `remove_spike_worktree` it is `rev-list --count master..<branch>`, which is
    0 for `master` itself. And the already-gone early return in both deletes
    the branch with no proof at all. `_refuse_unowned_path` above constrains
    the worktree PATH and says nothing about the branch; this is the branch's
    own guard.

    The checked-out branch is refused for the same reason plus a practical
    one: git refuses to delete it anyway, so the only outcomes were a
    confusing `GitError` or -- with the root detached -- a repo left with no
    local `master` for the next tick's `git.rev("master")` to read.

    Casefolded: on a case-insensitive filesystem `Master` resolves to the same
    ref, which is exactly how a name comparison lets one through.
    """
    name = (branch or "").strip()
    if name.lower() == "master" or name.removeprefix("refs/heads/").lower() == "master":
        raise RuntimeError(f"refusing to delete branch {branch!r}: master is never a worktree branch")
    try:
        head = git.branch()
    except GitError:  # a detached or unborn HEAD has no branch to protect
        head = "HEAD"
    if head != "HEAD" and head.strip().lower() == name.lower():
        raise RuntimeError(f"refusing to delete branch {branch!r}: it is the checked-out branch")


def remove_worktree(ctx: Ctx, git: Git, path: Path, branch: str, expected_parent: Path, merge_sha: str) -> None:
    ctx.write_guard("remove worktree")
    path = path.resolve()
    _refuse_unowned_path(path, expected_parent)
    # BEFORE the already-gone early return, which deletes the branch with no
    # merge proof whatsoever -- that path is the one with the least standing
    # between an argument and `git branch -D`.
    _refuse_protected_branch(git, branch)
    if not path.exists():
        # Already gone -- e.g. a previous attempt got this far and crashed
        # before `finish` ran. Treating this as a failure (an unmerged-branch
        # raise, or a bare OSError from `is_clean`'s `git status`) turns a
        # transient crash into a self-repeating tick that never makes progress
        # (review round 2, New Breakage 5): idempotent success instead.
        git.worktree_prune()
        if _branch_exists(git, branch):
            git.branch_delete(branch)
        return
    if Git(path).tracked_dirty():
        # ONE implementation of the probe, the same one `remove_spike_worktree`
        # uses -- see `Git.tree_state`. `is_clean()` was raw `git status
        # --porcelain`, the exact lie that sent merge 19f654e1 into the revert
        # path on 2026-09-14: a tracked file whose CONTENT matches HEAD but
        # which git materialised with the other line ending reads as modified
        # forever, and here that stranded the worktree permanently -- every
        # later `cleanup` raising "is not clean" on a tree with nothing in it.
        #
        # This DOES change the untracked-file semantics on this path, and the
        # change is intended, not incidental: `is_clean()` counted an untracked
        # file as dirt, `tracked_dirty()` does not. An untracked path in an
        # issue worktree is the caller's own tooling (gate logs, `__pycache__`,
        # a ledger the flow never commits), never evidence about the work. A
        # TRACKED file left modified is real dirt and still refuses -- that is
        # the half worth keeping, and the reason this is not simply "check
        # less".
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


def remove_spike_worktree(ctx: Ctx, git: Git, path: Path, branch: str, expected_parent: Path) -> None:
    """A spike never commits code (Step 2 of `/issue` says so), so there is no
    merge SHA to compare trees against the way `remove_worktree` does. The
    ownership/cleanliness checks are the same; the merge-proof check becomes
    "the branch has zero commits of its own on top of master" instead."""
    ctx.write_guard("remove spike worktree")
    path = path.resolve()
    _refuse_unowned_path(path, expected_parent)  # same `rm -rf`, same guard
    _refuse_protected_branch(git, branch)  # same `branch -D` hazard, same guard
    if not path.exists():
        # See the matching comment in `remove_worktree`: already gone is
        # success, not a failure to route through HALT.
        git.worktree_prune()
        if _branch_exists(git, branch):
            git.branch_delete(branch)
        return
    if Git(path).tracked_dirty():
        # THE SAME PROBE `remove_worktree` uses. The two paths used to differ
        # -- this one on `tracked_dirty()`, the other on raw `is_clean()` --
        # and this comment used to read as if that asymmetry were a decision
        # about spikes. It was not; it was an unfinished migration, and it
        # left the main issue-worktree path stranding on the 2026-09-14
        # line-ending class. Both are now `Git.tree_state`'s content-vs-HEAD
        # comparison.
        #
        # What made it show up here FIRST: `/issue` step 1 unconditionally
        # leaves an untracked `.agent/issue-N/ledger.md` in every spike's
        # worktree (a spike never commits), so `is_clean()` could never pass
        # for the caller this function actually has (review round 2, New
        # Breakage 4). A tracked file left modified is real dirt and still
        # refuses.
        raise RuntimeError(f"worktree {path} is not clean")
    ahead = int(git.out("rev-list", "--count", f"master..{branch}"))
    if ahead != 0:
        raise RuntimeError(f"branch {branch} has {ahead} commit(s) ahead of master; not spike-clean")
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
