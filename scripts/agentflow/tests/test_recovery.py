import json
import shutil
import subprocess
from pathlib import Path

import pytest

from agentflow import incidents, lock, recovery
from agentflow.gh import Gh, GhError
from agentflow.gitops import Git, GitError
from agentflow.state import Ctx, Paths
from agentflow.tests.conftest import FakeRunner, commit_file

REPO = "SShadowS/al-sem"


def gh_ok(extra=None):
    """A `gh` fake that lets the incident bookkeeping through.

    DELIBERATELY INVALIDATED PIN, and the invalidation is itself the finding.
    This used to key the label write exactly:

        "issue edit 8 --add-label agent-regressed": ""

    and that was never a PIN. It was a canned RESPONSE that made the call
    succeed -- nothing here, and nothing in the five tests that flow through
    `_bookkeeping`, ever asserted WHICH label was sent. Not one of them would
    have failed if the label had been wrong, which is exactly how
    `agent-regressed` came to be stamped on merge 19f654e1 (three real gates,
    0/0/0) with the entire suite green. It is the repo's named recurring
    defect in its purest form: the fixture tracked the code instead of
    pinning it.

    It is a WILDCARD now for a mechanical reason too -- `add_labels` sends
    both axes in ONE argv (`--add-label A --add-label B`) -- but the important
    change is that the label CONTENT is now asserted where an assertion can
    FAIL: see `test_green_gates_...`, `test_a_red_gate_whose_revert_is_blocked
    _is_still_labelled_regressed`, and the CLI-level pair.

    The labels-list read is new: `_bookkeeping` calls `ensure_labels` first,
    because `gh issue edit --add-label X` hard-fails on a label the repo does
    not have.
    """
    resp = {"issue reopen 8": "", "issue edit 8 --add-label *": "", "issue comment 8 *": "",
            "issue edit 8 --remove-label agent-working": "",
            f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
                json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]])}
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
    def rerun(wt):
        seen.append(lock.halted(ctx))
        return True
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), issue=8, merge_sha=bad, rerun_gates=rerun)
    assert seen == ["regression: merge of #8 (" + bad[:12] + ") failed post-merge gates"]
    assert out.halted and out.reverted and out.pushed
    g.fetch()
    # DELIBERATELY INVALIDATED. The old pin here was
    #     assert not (clone / "bad.txt").exists()
    # i.e. it read the SHARED working tree and treated its state as a proxy
    # for what had been PUSHED. That was only ever true because the revert was
    # built on the shared local `master` -- the contamination this change
    # removes -- and it is the same tree-state-as-verdict conflation the
    # 2026-09-14 incident came from. What is actually being claimed is a
    # property of the pushed COMMIT, so assert that instead, plus the new
    # claim that the shared checkout was left exactly as found.
    assert g.rev("origin/master") == out.revert_sha
    assert not g.ok("cat-file", "-e", f"{out.revert_sha}:bad.txt")   # absent from the reverted TREE
    assert g.ok("cat-file", "-e", f"{out.revert_sha}:README.md")     # ...and it is a real tree, not empty
    assert (clone / "bad.txt").exists() and g.rev("master") == bad   # shared checkout untouched


def test_post_merge_failure_pushes_only_the_commit_it_validated(repo_pair):
    """The revert push must send the id `rerun_gates` validated, not whatever
    `master` names once the gates are done. In production that callable runs
    the whole gate suite for tens of minutes; a commit landing on local
    `master` in that window is the real-world shape, and the callable
    parameter is where a test can simply STATE it.

    The stray commit is made on the SHARED clone deliberately, not in the tree
    the closure is handed: `master` moving under the flow's feet while the
    gates run is exactly the precondition, and the revert worktree is detached
    and has no `master` to move."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    stray = {}

    def rerun(wt):
        stray["sha"] = commit_file(clone, "stray.txt", "never gated\n", "concurrent ungated work")
        return True

    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=rerun)
    assert out.pushed and out.reason == "reverted"
    # The two mid-test asserts are the same discipline as asserting a scripted
    # break applied: without them a refactor that stopped creating the stray
    # commit would leave this test green while proving nothing.
    assert stray["sha"] != out.revert_sha            # the precondition really held
    assert g.rev("master") == stray["sha"]           # ...and local master really moved
    g.fetch()
    assert g.rev("origin/master") == out.revert_sha
    assert not g.is_ancestor(stray["sha"], "origin/master")


def test_post_merge_failure_push_names_both_sides_of_the_refspec(repo_pair):
    """Pins the refspec FORM at the argv. When nothing else moves, the form is
    the ONLY place the difference between `origin master` and
    `<sha>:refs/heads/master` lives -- which is exactly why the pre-existing
    suite could not see it."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")   # setup, deliberately NOT through the recorder
    ctx = make_ctx(clone)
    calls = []

    def recording(argv, **kw):
        calls.append(list(argv))
        return subprocess.run(argv, **kw)

    rg = Git(clone, run=recording)
    out = recovery.post_merge_failure(ctx, rg, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda wt: True)
    assert out.pushed
    pushes = [c for c in calls if "push" in c]   # exact element membership: `fetch`/`rev-parse` never match
    assert len(pushes) == 1, pushes
    src, sep, dst = pushes[0][-1].partition(":")
    assert sep == ":" and dst == "refs/heads/master", pushes[0]
    assert src == out.revert_sha, pushes[0]


def test_revert_is_built_and_validated_in_a_disposable_worktree(repo_pair):
    """The revert must be built in a tree that was checked out seconds ago,
    not in the shared checkout the failing gates just ran in.

    PRECONDITIONS HAND-STATED, never produced by production code: a modified
    TRACKED file and an untracked artifact are written directly into the
    shared clone, exactly as a gate run would have left them."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    (clone / "README.md").write_text("contaminated by the first gate run\n")
    (clone / "leftover.log").write_text("x\n")
    ctx = make_ctx(clone)
    seen = {}

    def rerun(p):
        seen.update(path=p, is_clone=(p == clone), dirty=Git(p).tracked_dirty(),
                    has_bad=(p / "bad.txt").exists(), readme=(p / "README.md").read_text())
        return True

    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=rerun)
    assert seen["is_clone"] is False
    assert seen["path"].parent == clone.parent
    assert seen["path"].name.startswith("al-sem-verify-")
    assert seen["dirty"] is False          # the validation tree was CLEAN although the checkout was not
    assert seen["has_bad"] is False        # ...and really held the revert's content, not the merge's
    assert seen["readme"] == "hello\n"     # the shared root's modification was invisible to it
    assert out.pushed
    g.fetch()
    assert g.rev("origin/master") == out.revert_sha
    assert not g.ok("cat-file", "-e", f"{out.revert_sha}:bad.txt")
    assert g.ok("cat-file", "-e", f"{out.revert_sha}:README.md")
    assert not seen["path"].exists()
    assert "al-sem-verify-" not in g.out("worktree", "list")


def test_a_gate_artifact_in_the_shared_checkout_cannot_green_light_a_revert(repo_pair):
    """The incident's exact polarity. PRECONDITION HAND-STATED: a token file
    left behind in the SHARED checkout, as a first gate run would leave it.
    The rerun passes only if that token is in the tree it is handed -- so if
    the validation ran in the shared root, a revert that is genuinely RED gets
    reported green and pushed over `origin/master`."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    (clone / "stale-pass-token").write_text("left behind by the first gate run\n")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad,
                                      rerun_gates=lambda p: (p / "stale-pass-token").exists())
    assert not out.pushed and out.reverted is True and out.reason == "revert-failed-gates"
    g.fetch()
    assert g.rev("origin/master") == bad                    # the remote was never touched
    assert (clone / "stale-pass-token").exists()            # the precondition was live throughout


def test_post_merge_failure_never_moves_the_shared_checkout(repo_pair):
    """PRECONDITION HAND-STATED: the shared clone is left DETACHED with a
    modified tracked file, as a crashed earlier attempt would leave it. The
    old code's `git.checkout("master")` would have moved it; this function now
    performs no checkout, no merge, no reset and no revert there."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    g._run("checkout", "-q", "--detach", bad)
    (clone / "README.md").write_text("a human's uncommitted edit\n")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda p: True)
    assert out.pushed
    g.fetch()
    assert g.rev("origin/master") == out.revert_sha
    assert g.branch() == "HEAD"                             # still detached
    assert g.rev("HEAD") == bad
    assert g.rev("master") == bad                           # local master never advanced onto a revert
    assert (clone / "README.md").read_text() == "a human's uncommitted edit\n"
    assert (clone / "bad.txt").exists()                     # nothing here was reset


@pytest.mark.parametrize("case", ["rerun-red", "master-advanced", "push-rejected"])
def test_revert_worktree_is_torn_down_on_every_exit(repo_pair, tmp_path, monkeypatch, case):
    """Peak extra disk is one worktree only if teardown runs on EVERY exit, so
    it is asserted on every branch rather than only the happy one. Each case
    states its own precondition literally."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    seen = {}

    def rerun(p):
        seen["path"] = p
        if case == "master-advanced":
            other = tmp_path / "other"
            subprocess.run(["git", "clone", "-q", str(repo_pair[0]), str(other)], check=True)
            Git(other).out("config", "user.email", "o@x")
            Git(other).out("config", "user.name", "o")
            commit_file(other, "z.txt", "z\n", "someone else")
            assert Git(other).push("origin", "master")
        return case != "rerun-red"

    if case == "push-rejected":
        # Hand-stated rather than engineered out of a real remote: the claim
        # is "a rejected push still tears the tree down", not "here is how to
        # make git reject one".
        monkeypatch.setattr(Git, "push_master", lambda self, sha: False)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=rerun)
    expected = {"rerun-red": "revert-failed-gates", "master-advanced": "master-advanced",
                "push-rejected": "push-rejected"}[case]
    assert out.reason == expected and not out.pushed
    assert "path" in seen                                   # the rerun really ran
    assert not seen["path"].exists()
    assert "al-sem-verify-" not in g.out("worktree", "list")


def test_a_revert_worktree_that_cannot_be_created_refuses_to_push(repo_pair, monkeypatch):
    """An unvalidatable revert is never pushed. PRECONDITION HAND-STATED:
    `worktrees.create` raises, as it would on a full disk or a locked path."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    reruns = []

    def boom(git, path, commit):
        raise GitError("disk full")

    monkeypatch.setattr(recovery.worktrees, "create", boom)
    run = gh_ok()
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=run), 8, bad,
                                      rerun_gates=lambda p: reruns.append(p) or True)
    assert out.reason == "revert-worktree-failed"
    assert not out.reverted and not out.pushed
    assert reruns == []                                     # never even tried to validate
    assert lock.halted(ctx) is not None
    g.fetch()
    assert g.rev("origin/master") == bad
    # Was an exact-key membership test (`"issue edit 8 --add-label
    # agent-regressed" in run.calls`), which stopped matching once both axes
    # ride in one argv. A substring search over ALL recorded calls is the
    # stronger form anyway: it cannot be satisfied by a key that merely
    # happens to be in the fake's response dict.
    assert any("agent-regressed" in c for c in run.calls), run.calls
    assert any("agent-revert-blocked" in c for c in run.calls), run.calls
    assert any(c.startswith("issue comment 8") for c in run.calls)


def test_post_merge_unverified_halts_and_leaves_the_repo_alone(repo_pair):
    """Every gate passed; the tree came back dirty. That is evidence about the
    WORKING TREE, not about the merged commit. PRECONDITION HAND-STATED: the
    merge SHA and the dirty list are passed in literally -- the real
    2026-09-14 path -- so the test never depends on production code being able
    to produce a dirty tree."""
    _, clone = repo_pair
    g = Git(clone)
    merged = commit_file(clone, "ok.txt", "fine\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    # The label key moved from `agent-blocked` (G2's placeholder) to the
    # EVIDENCE-axis name, and `_bookkeeping` now reads the repo's label list
    # first. Both are fixture-shape changes; the assertions below are the
    # test's actual subject and are unchanged apart from the negative one,
    # which got stronger.
    run = gh_ok({"issue edit 8 --add-label agent-gates-green-unverified": ""})
    dirty = ["crates/al-syntax/src/raw/generated/node-types.sha256"]
    out = recovery.post_merge_unverified(ctx, Gh(ctx, REPO, run=run), 8, merged, dirty)
    assert out.halted and not out.reverted and not out.pushed
    assert out.reason == "tree-dirty-after-gates" and out.dirty == dirty
    halt = lock.halted(ctx)
    assert halt is not None and merged[:12] in halt and "NOT reverted" in halt
    assert "unverified" in (ctx.run_dir / "notify.log").read_text()
    assert "issue reopen 8" not in run.calls                # the merge stands
    # Substring over EVERY recorded call, not an exact key: the exact-key form
    # would silently stop meaning anything the moment the argv shape changed.
    assert not any("agent-regressed" in c for c in run.calls), run.calls
    g.fetch()
    assert g.rev("origin/master") == merged and g.rev("master") == merged


def test_post_merge_unverified_writes_halt_before_the_first_gh_call(repo_pair):
    """A gh outage may cost the labels and the comment; it must never cost the
    record that the loop has to stop. PRECONDITION HAND-STATED by priming the
    fake so the first mutating call returns HTTP 500."""
    _, clone = repo_pair
    g = Git(clone)
    merged = commit_file(clone, "ok.txt", "fine\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    # The labels-list READ is allowed through so the outage lands on the first
    # MUTATING call, which is what this test is about. Without it the fake
    # would 404 the read instead and the test would pass for the wrong reason.
    r = FakeRunner({f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
                        json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
                    "issue edit 8 --add-label *": (1, "", "HTTP 500: boom")})
    with pytest.raises(GhError):
        recovery.post_merge_unverified(ctx, Gh(ctx, REPO, run=r, sleep=lambda s: None), 8, merged, ["x.rs"])
    assert lock.halted(ctx) is not None
    assert (ctx.run_dir / "notify.log").exists()


def test_post_merge_unverified_cannot_reach_git_at_all():
    """A STRUCTURAL guarantee, not a behavioural one: the halt path takes no
    `Git` and calls no git-mutating method, so it has no way to revert, reset
    or push. A future author who wants one has to add the parameter and argue
    for it. Read with `ast` rather than by scanning the text, because the
    words `revert` and `reset` legitimately appear in this function's prose."""
    import ast
    import inspect
    import textwrap
    assert "git" not in inspect.signature(recovery.post_merge_unverified).parameters
    tree = ast.parse(textwrap.dedent(inspect.getsource(recovery.post_merge_unverified)))
    called = {n.func.attr for n in ast.walk(tree)
              if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)}
    called |= {n.id for n in ast.walk(tree) if isinstance(n, ast.Name)}
    assert called.isdisjoint({"revert", "push", "push_master", "checkout", "ff", "fetch", "Git"}), called
    # ...and the guard is not vacuous: the same read of the REVERT path finds
    # exactly those calls.
    other = ast.parse(textwrap.dedent(inspect.getsource(recovery.post_merge_failure)))
    other_called = {n.func.attr for n in ast.walk(other)
                    if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)}
    assert {"revert", "push_master", "fetch"} <= other_called, other_called


def test_green_gates_with_a_dirty_tree_is_never_labelled_a_regression(repo_pair):
    """T1. PINS THE USE: it drives the production `post_merge_unverified`, not
    `incident_labels`, because the defect on 2026-09-14 was which axis a CALL
    SITE claimed had moved, and a pure-function test cannot see that.

    PRECONDITION HAND-STATED: "every gate returned 0 and the tree came back
    dirty" is expressed by CALLING THE GREEN-GATES FUNCTION with a literal
    dirty list -- not by running gates and hoping they leave one behind. That
    is exactly the 2026-09-14 shape and nothing in the test depends on
    production code being able to reproduce it.
    """
    _, clone = repo_pair
    g = Git(clone)
    merged = commit_file(clone, "ok.txt", "fine\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    r = gh_ok({"issue edit 8 --add-label agent-gates-green-unverified": ""})
    out = recovery.post_merge_unverified(ctx, Gh(ctx, REPO, run=r), 8, merged,
                                         ["crates/al-syntax/src/raw/generated/node-types.sha256"])
    # Searching ALL of r.calls for the substring, rather than checking one
    # expected key, is what makes the negative assertion real: a key that is
    # merely absent from the fake's response dict proves nothing.
    assert not any("agent-regressed" in c for c in r.calls), r.calls
    assert any("agent-gates-green-unverified" in c and c.startswith("issue edit 8 --add-label")
               for c in r.calls), r.calls
    assert out.labels == ["agent-gates-green-unverified"]
    # THE DURABLE RECORD, which is a DIFFERENT claim from the return value and
    # was the one nobody checked. MEASURED IN THE ROUND-1 REVIEW: deleting this
    # function's `record_outcome` call left all 206 tests green -- `open_incident` two
    # lines above already writes the same `reason`, and `revert_landed` keeps
    # its `setdefault(None)`, so `labels` is the single field that goes. It
    # then stays `[]` on disk while the issue on GitHub carries
    # `agent-gates-green-unverified`, and the two records disagree about what
    # happened. The revert path's analogous call is pinned at T3; this is the
    # halt path's.
    assert incidents.get(ctx, merged)["labels"] == [recovery.UNVERIFIED]
    # No ACTION label at all: no revert was attempted, so there is nothing
    # truthful to say about whether one landed.
    assert not any("agent-revert-landed" in c or "agent-revert-blocked" in c for c in r.calls), r.calls
    # `ensure_labels` runs FIRST, and the ORDER is the claim: `gh issue edit
    # --add-label X` hard-fails on a label the repo lacks, and a run claimed
    # before this vocabulary shipped has never created these names.
    read = f"api repos/{REPO}/labels?per_page=100 --paginate --slurp"
    add = next(i for i, c in enumerate(r.calls) if c.startswith("issue edit 8 --add-label"))
    assert read in r.calls[:add], r.calls


def test_a_red_gate_whose_revert_is_blocked_is_still_labelled_regressed(repo_pair):
    """T2. THE ANTI-OVER-CORRECTION PIN, and the one a naive fix fails: a red
    gate is a regression whether or not the revert could be pushed.

    PRECONDITION HAND-STATED: an unpushed local commit on top of the merge
    SHA, so the ancestor guard refuses and NO revert is attempted. That is the
    same guard which accidentally saved 19f654e1 from being reverted.
    """
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    commit_file(clone, "extra.txt", "extra\n", "unpushed local work")
    ctx = make_ctx(clone)
    r = gh_ok()
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=r), 8, bad, rerun_gates=lambda wt: True)
    assert out.reason == "master-not-ff" and not out.reverted and not out.pushed
    assert out.labels == ["agent-regressed", "agent-revert-blocked"]
    assert any("agent-regressed" in c for c in r.calls), r.calls
    # The ACTION axis is the half a human acts on: master is STILL carrying it.
    assert any("agent-revert-blocked" in c for c in r.calls), r.calls
    assert not any("agent-revert-landed" in c for c in r.calls), r.calls
    assert not any("agent-gates-green-unverified" in c for c in r.calls), r.calls


def test_post_merge_failure_opens_and_closes_a_durable_incident(repo_pair):
    """T3. The PRODUCER pin for the incident record, so the hand-written
    fixtures in the preflight tests correspond to something production really
    emits rather than to a shape I invented for them."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    assert incidents.get(ctx, bad) is None          # nothing before the call
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad,
                                      rerun_gates=lambda wt: True)
    assert out.pushed and out.reason == "reverted"
    rec = incidents.get(ctx, bad)
    assert rec is not None
    assert rec["state"] == "open"                   # a revert does NOT close the obligation
    assert rec["gate_red"] is True and rec["merge_sha"] == bad and rec["issue"] == 8
    assert rec["revert_landed"] is True and rec["reason"] == "reverted"
    assert rec["labels"] == out.labels == ["agent-regressed", "agent-revert-landed"]
    assert rec["opened_at"] == 1_000_000.0          # the frozen ctx clock
    assert incidents.unresolved(ctx) == [rec]


def test_the_incident_record_survives_a_github_outage_during_bookkeeping(repo_pair):
    """T3, second half. The durable LOCAL obligation must not depend on
    GitHub being reachable -- which is why `record_outcome` runs BEFORE
    `_bookkeeping` inside `done`.

    PRECONDITION HAND-STATED twice over: the unpushed local commit that makes
    the ancestor guard refuse, and a `gh` fake primed to fail the label write
    outright (not a retryable 5xx -- one failure, no backoff)."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    commit_file(clone, "extra.txt", "extra\n", "unpushed local work")
    ctx = make_ctx(clone)
    r = gh_ok({"issue edit 8 --add-label *": (1, "", "github is down")})
    with pytest.raises(GhError):
        recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=r, sleep=lambda s: None), 8, bad,
                                    rerun_gates=lambda wt: True)
    rec = incidents.get(ctx, bad)
    assert rec is not None and rec["state"] == "open"
    # The TERMINAL fields, not merely the opening ones: without the ordering
    # this test exists to pin, `reason` would still read "post-merge gates
    # went red" and `revert_landed` would still be None.
    assert rec["reason"] == "master-not-ff"
    assert rec["revert_landed"] is False
    assert rec["labels"] == ["agent-regressed", "agent-revert-blocked"]


def test_post_merge_failure_does_not_push_when_revert_fails_gates(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda wt: False)
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
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda wt: True)
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
    out = recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bad, rerun_gates=lambda wt: True)
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
        recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=gh_ok()), 8, bogus, rerun_gates=lambda wt: True)
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


def _pr(number, state, branch, *, oid=None, merged_at=None):
    return {"number": number, "state": state, "headRefName": branch, "headRefOid": "h",
            "mergeCommit": {"oid": oid} if oid else None, "mergedAt": merged_at}


def _stale(ctx, lk, run_id="run-new"):
    return Ctx(paths=ctx.paths, run_id=run_id, now=lambda: lk.heartbeat + 4000)


def _blocked_gh(prs):
    return FakeRunner({"pr list *": json.dumps(prs), "issue comment 8 *": "",
                       "issue edit 8 --add-label agent-blocked": "",
                       "issue edit 8 --remove-label agent-working": ""})


def test_recover_stale_refuses_a_merged_pr_from_a_previous_attempt(repo_pair, tmp_path):
    """The record `recover` mints here is FULLY TRUSTED: `cmd_recover` writes
    it as a `MergeRecord`, and `cmd_post_merge`'s provenance check then passes
    by construction. So the PR it is minted from has to be THIS stale run's.

    PRECONDITION HAND-STATED: two PRs on the same issue -- a PREVIOUS
    attempt's MERGED one and this attempt's OPEN one -- with the stale lock on
    attempt 2. `gh.pr_for_branch_prefix` ranks MERGED first, so a prefix-only
    match returns the wrong one, and the adopted SHA is a commit this tick
    established nothing about: today's gates run against a months-old tree
    (a moved CDO ratchet, a regenerated golden, a bumped
    CACHE_VERSION_GRAMMAR each suffice to go red), which HALTs, opens a
    preflight-blocking incident and comments on the issue.

    The assertion is `blocked-crashed`, NOT `merged-needs-post-merge`: the
    honest outcome for a run that crashed before opening its own merge."""
    _, clone = repo_pair
    g = Git(clone)
    (clone / ".agent").mkdir(exist_ok=True)
    ctx = Ctx(paths=Paths(clone), run_id="run-test", now=lambda: 1_000_000.0)
    lk = lock.acquire(ctx, 8, "s", 2)
    assert lk.attempt == 2                                   # the precondition, stated
    # `mergedAt` is AFTER `lk.started` (1_000_000.0 == 1970-01-12T13:46:40Z) on
    # purpose. The recency guard is the OTHER axis and is pinned by its own
    # test; dating this PR early would let that guard refuse instead, and the
    # break that drops the attempt filter would then come back green through a
    # second code path -- measured, and corrected here rather than shrugged at.
    r = _blocked_gh([_pr(3, "MERGED", "issue/8-x-a1", oid="a" * 40, merged_at="1970-02-01T00:00:00Z"),
                     _pr(4, "OPEN", "issue/8-x-a2")])
    rep = recovery.recover_stale(_stale(ctx, lk), g, Gh(_stale(ctx, lk), REPO, run=r), lk,
                                 worktrees_parent=tmp_path)
    assert rep["action"] == "blocked-crashed", rep
    assert "merge_sha" not in rep                             # nothing to mint a record from
    # ...and the filter picked THIS attempt's PR rather than dropping every
    # candidate, which a suffix typo would also produce.
    assert rep["pr"] == 4, rep


def test_recover_stale_refuses_a_merged_pr_that_predates_the_stale_lock(repo_pair, tmp_path):
    """The SECOND, independent axis. The attempt matches here, so the filter
    above cannot be what refuses: a merge GitHub itself dates BEFORE this lock
    was acquired cannot be the merge of the work this lock was holding.

    Both directions are stated, because a refusal that also refuses the good
    case is not a guard, it is an outage."""
    _, clone = repo_pair
    g = Git(clone)
    (clone / ".agent").mkdir(exist_ok=True)
    ctx = Ctx(paths=Paths(clone), run_id="run-test", now=lambda: 1_000_000.0)
    lk = lock.acquire(ctx, 8, "s", 1)
    assert lk.started == 1_000_000.0                         # 1970-01-12T13:46:40Z
    old = _blocked_gh([_pr(3, "MERGED", "issue/8-x-a1", oid="a" * 40,
                           merged_at="1970-01-01T00:00:00Z")])
    rep = recovery.recover_stale(_stale(ctx, lk), g, Gh(_stale(ctx, lk), REPO, run=old), lk,
                                 worktrees_parent=tmp_path)
    assert rep["action"] == "blocked-crashed", rep
    assert "merge_sha" not in rep
    # THE CONTRAST: the same PR, merged AFTER the lock started, IS adopted.
    lk2 = lock.acquire(ctx, 8, "s", 1)
    new = FakeRunner({"pr list *": json.dumps(
        [_pr(3, "MERGED", "issue/8-x-a1", oid="a" * 40, merged_at="1970-02-01T00:00:00Z")])})
    rep2 = recovery.recover_stale(_stale(ctx, lk2, "run-new2"), g,
                                  Gh(_stale(ctx, lk2, "run-new2"), REPO, run=new), lk2,
                                  worktrees_parent=tmp_path)
    assert rep2["action"] == "merged-needs-post-merge" and rep2["merge_sha"] == "a" * 40


def test_a_gh_outage_on_reopen_still_leaves_the_durable_record_of_the_pushed_revert(repo_pair):
    """THE ORDERING, on the one exit that has already rewritten
    `origin/master`.

    `gh.reopen_issue` used to run at the call site, above `done()` -- i.e.
    before `record_outcome` and before `_bookkeeping`, breaking the rule
    `done`'s own docstring states three lines up. A GitHub 500 there left the
    revert LIVE on master with `incidents.json` still reading
    `reason: "post-merge gates went red", revert_landed: null, labels: []`,
    and `cmd_post_merge`'s payload reading `{"revert": null, "halt": null}`.

    PRECONDITION HAND-STATED through the existing `gh_ok` seam: `issue reopen
    8` returns non-zero, every other gh call succeeds. Every assertion is
    OUTSIDE the `pytest.raises` block -- inside, the block aborts at the raise
    and the rest would never run."""
    _, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "bad.txt", "bad\n", "merge of #8")
    g.push("origin", "master")
    ctx = make_ctx(clone)
    r = gh_ok({"issue reopen 8": (1, "", "github is down")})
    with pytest.raises(GhError):
        recovery.post_merge_failure(ctx, g, Gh(ctx, REPO, run=r, sleep=lambda s: None), 8, bad,
                                    rerun_gates=lambda wt: True)
    rec = incidents.get(ctx, bad)
    assert rec is not None
    assert rec["reason"] == "reverted"
    assert rec["revert_landed"] is True
    assert rec["labels"] == ["agent-regressed", "agent-revert-landed"]
    # THE SITUATION THE ORDERING EXISTS FOR, stated rather than implied: the
    # push really did happen, so the record above is describing live remote
    # state, not an intention.
    g.fetch()
    assert g.rev("origin/master") != bad
    assert g.rev("origin/master^") == bad          # a revert commit, on top of the merge
    # The local notification is a durable artifact too, and it is written
    # before the first remote call for the same reason.
    assert (ctx.run_dir / "notify.log").exists()


def test_a_large_dirty_tree_is_announced_at_its_real_size(repo_pair):
    """Every number this function reports about the dirt is `len(dirty)`, so a
    caller that truncated first made all of them lie by the same amount -- the
    HALT reason, the notification, the issue comment, and the comment's own
    "and N more" tail, which was computed against the already-cut list.

    PRECONDITION HAND-STATED BY ASSIGNMENT: 200 literal paths. No gate is
    asked to produce them; a fixture that depends on the code under test
    cannot state a size that code has to report honestly."""
    _, clone = repo_pair
    ctx = make_ctx(clone)
    merged = "e" * 40
    dirty = [f"src/gen/file{i:03d}.rs" for i in range(200)]
    r = gh_ok()
    out = recovery.post_merge_unverified(ctx, Gh(ctx, REPO, run=r), 8, merged, dirty)
    assert out.dirty_total == 200
    assert len(out.dirty) == recovery.MAX_DIRTY_LISTED        # the list is cut, the count is not
    assert out.dirty == dirty[:recovery.MAX_DIRTY_LISTED]
    assert "200 tracked file(s) modified" in lock.halted(ctx)
    body_call = next(c for c in r.calls if c.startswith("issue comment 8 --body-file "))
    body = Path(body_call.split(" --body-file ", 1)[1]).read_text(encoding="utf-8")
    assert "200 modified tracked file(s)" in body
    assert f"... and {200 - recovery.MAX_DIRTY_LISTED} more" in body
    assert "and 30 more" not in body                          # the old, cut-list arithmetic


def test_remove_worktree_checks_parent_clean_and_merged(repo_pair, tmp_path):
    """DELIBERATELY RESTATED PRECONDITION, and the restatement is the finding.

    The "not clean" arm used to hand-state its dirt as an UNTRACKED file
    (`dirty.txt`), because the probe was raw `git status --porcelain`. That
    probe is the 2026-09-14 lie: a tracked file whose CONTENT matches HEAD but
    which git materialised with the other line ending reads as modified
    forever, so an issue worktree on such a checkout could never be cleaned up
    again. This path now asks `Git.tree_state` like its spike sibling, so dirt
    means a modified TRACKED file -- stated here as one -- and an untracked
    path is deliberately not dirt, which the final removal asserts at the call
    site rather than by asking the probe directly."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    wt = tmp_path / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-x-a1", "master")
    (wt / "README.md").write_text("modified in the worktree\n")
    # PRECONDITION with raw git, independent of the code under test.
    assert subprocess.run(["git", "-C", str(wt), "diff", "--name-only", "HEAD", "--"],
                          capture_output=True, text=True).stdout.split() == ["README.md"]
    with pytest.raises(RuntimeError, match="not clean"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=g.rev("master"))
    assert wt.exists()                                   # refused AND left alone
    (wt / "README.md").write_text("hello\n")             # restored by hand, byte-for-byte
    commit_file(wt, "f.txt", "f\n", "unmerged work")
    with pytest.raises(RuntimeError, match="not merged"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=g.rev("master"))
    with pytest.raises(RuntimeError, match="outside"):
        recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path / "elsewhere", merge_sha=g.rev("master"))
    sha = g.merge_squash("issue/8-x-a1", "squash")
    # THE UNTRACKED CONTRAST, at the call site: the caller's own tooling
    # (a gate log, `__pycache__`, a ledger the flow never commits) is not
    # evidence about the work, so it must not strand the cleanup.
    (wt / "gate.log").write_text("the gates' own output\n")
    recovery.remove_worktree(ctx, g, wt, "issue/8-x-a1", expected_parent=tmp_path, merge_sha=sha)
    assert not wt.exists() and "issue/8-x-a1" not in g.out("branch", "--list")


def test_remove_worktree_refuses_a_path_it_does_not_own(repo_pair, tmp_path):
    """THE `rm -rf` HAD NO OWNERSHIP PROOF. Its only path guard was "is a
    direct child of the repo root's parent" -- on this machine `U:/Git`, which
    holds 230 sibling directories, essentially all of them git repositories,
    the pinned `DO-cdo-baseline` among them. One wrong `--worktree`, with the
    other two arguments correct, was a deletion.

    EVERY OTHER GUARD IS SATISFIED HERE, and each is asserted rather than
    assumed, so the ONLY thing that can refuse is ownership: the location
    check passes (it is the expected parent's own child), the cleanliness
    probe passes (a freshly checked-out worktree), and the merge proof passes
    (the branch really is squashed into `merge_sha`). Without those three
    assertions this test could pass for any of four reasons.

    THE ALREADY-GONE ARM is not decoration: that early return deletes the
    branch with NO proof at all, so it is the path with the least standing
    between an argument and a delete, and the guard has to precede it."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    foreign = tmp_path / "not-ours"                 # a name this flow never hands out
    g.worktree_add(foreign, "issue/8-x-a1", "master")
    commit_file(foreign, "f.txt", "f\n", "somebody's work")
    merge_sha = g.merge_squash("issue/8-x-a1", "squash issue/8-x-a1")
    assert foreign.parent == tmp_path                            # location: passes
    assert not Git(foreign).tracked_dirty()                      # cleanliness: passes
    assert g.ok("diff", "--quiet", "issue/8-x-a1", merge_sha)     # merge proof: passes
    with pytest.raises(RuntimeError, match="not an agentflow issue worktree"):
        recovery.remove_worktree(ctx, g, foreign, "issue/8-x-a1", expected_parent=tmp_path,
                                 merge_sha=merge_sha)
    # THE CONSEQUENCE, outside the raises block: the directory and its work
    # are still there, and so is the branch.
    assert foreign.is_dir() and (foreign / "f.txt").exists()
    assert "issue/8-x-a1" in g.out("branch", "--list")

    # The spike remover deletes by the same `rm -rf` and needs the same proof.
    spike = tmp_path / "somebody-elses-checkout"
    g.worktree_add(spike, "issue/9-spike-a1", "master")
    with pytest.raises(RuntimeError, match="not an agentflow issue worktree"):
        recovery.remove_spike_worktree(ctx, g, spike, "issue/9-spike-a1", expected_parent=tmp_path)
    assert spike.is_dir() and "issue/9-spike-a1" in g.out("branch", "--list")

    # THE ALREADY-GONE ARM: no directory at all, so only the guard stands
    # between this call and `git branch -D`.
    gone = tmp_path / "never-ours"
    assert not gone.exists()
    with pytest.raises(RuntimeError, match="not an agentflow issue worktree"):
        recovery.remove_worktree(ctx, g, gone, "issue/9-spike-a1", expected_parent=tmp_path,
                                 merge_sha=merge_sha)
    assert g.ok("show-ref", "--verify", "--quiet", "refs/heads/issue/9-spike-a1")

    # NON-VACUITY: an OWNED name in the same parent, with the same three
    # guards satisfied, is still removed. Without this the guard could be
    # "refuse everything" and all four arms above would pass.
    ours = tmp_path / recovery.worktree_name(8, 2)
    g.worktree_add(ours, "issue/8-x-a2", "master")
    commit_file(ours, "g.txt", "g\n", "our work")
    ours_sha = g.merge_squash("issue/8-x-a2", "squash issue/8-x-a2")
    recovery.remove_worktree(ctx, g, ours, "issue/8-x-a2", expected_parent=tmp_path,
                             merge_sha=ours_sha)
    assert not ours.exists() and "issue/8-x-a2" not in g.out("branch", "--list")


def test_remove_worktree_refuses_master_even_when_the_worktree_is_already_gone(repo_pair, tmp_path):
    """The branch argument reaches `git branch -D`, and neither function's
    "is this branch finished with?" proof discriminates `master`.

    PRECONDITION HAND-STATED, and the already-gone shape is the one that
    matters: that early return deletes the branch with NO proof at all, so it
    is the arm with the least standing between an argument and the delete. The
    non-gone arm is stated too, where the merge proof passes TRIVIALLY --
    `git diff --quiet master <the commit master points at>` compares a tree
    with itself.

    This is the LIBRARY pin. `cmd_cleanup` refuses the same argument earlier
    and is pinned separately in test_cli_flow.py; if only the CLI guard
    existed, every other caller of these functions would still be free."""
    _, clone = repo_pair
    g = Git(clone)
    ctx = make_ctx(clone)
    head = g.rev("master")
    gone = tmp_path / recovery.worktree_name(8, 1)       # never created
    assert not gone.exists()
    for fn in (lambda: recovery.remove_worktree(ctx, g, gone, "master", expected_parent=tmp_path,
                                                merge_sha=head),
               lambda: recovery.remove_spike_worktree(ctx, g, gone, "master", expected_parent=tmp_path)):
        with pytest.raises(RuntimeError, match="refusing to delete branch"):
            fn()
    # THE CONSEQUENCE, outside the raises blocks: master still exists.
    assert g.ok("rev-parse", "--verify", "refs/heads/master")
    assert g.rev("master") == head
    # A real, non-master worktree branch is still removable -- without this the
    # guard could be "refuse everything" and both halves above would pass.
    wt = tmp_path / recovery.worktree_name(9, 1)
    g.worktree_add(wt, "issue/9-ok-a1", "master")
    recovery.remove_spike_worktree(ctx, g, wt, "issue/9-ok-a1", expected_parent=tmp_path)
    assert not wt.exists() and "issue/9-ok-a1" not in g.out("branch", "--list")


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
