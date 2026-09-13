"""Command-line surface of the executor. One JSON object per call on stdout.

Exit codes: 0 ok; 1 a checked condition failed (the JSON says which);
2 usage or environment error. Human-readable notes go to stderr.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import time
from dataclasses import asdict
from pathlib import Path

from . import budget, discoveries, eligibility, lock, mergeops, protect, recovery, sanitize, supervise
from .gh import Gh, GhError
from .gitops import Git, GitError
from .state import Ctx, DryRunViolation, Paths, new_run_id, read_json, write_json

LABELS = ["agent-working", "agent-done", "agent-blocked", "agent-answered", "agent-regressed", "agent-filed"]
GATES = [
    ("ci-steps-all", ["bash", "scripts/ci-steps", "all"], 45),
    ("check-goldens", ["bash", "scripts/check-goldens"], 45),
]
CDO_GATE = ("cdo-gate", ["bash", "scripts/cdo-gate"], 45)


class Fail(Exception):
    def __init__(self, payload: dict, code: int = 1):
        super().__init__(json.dumps(payload))
        self.payload, self.code = payload, code


def slug(title: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    return s[:31].rstrip("-")


def _emit(obj: dict, code: int = 0) -> int:
    print(json.dumps(obj, indent=2, default=str))
    return code


def _ctx(args) -> Ctx:
    # AGENTFLOW_NOW is the clock seam: without it every CLI-built Ctx uses the
    # real wall clock, so no test can reach a staleness/deadline branch without
    # actually waiting. A fixture that wants a frozen "now" for a CLI-driven
    # call sets this env var; direct callers of the library functions keep
    # passing their own `now=` callable to `Ctx` as before.
    now = (lambda: float(os.environ["AGENTFLOW_NOW"])) if "AGENTFLOW_NOW" in os.environ else time.time
    return Ctx(paths=Paths(Path(args.root).resolve()), dry_run=args.dry_run, run_id=args.run_id, now=now)


def _beat_if_owner(ctx: Ctx):
    lk = lock.read(ctx)
    if ctx.dry_run or lk is None or lk.run_id != ctx.run_id:
        return None
    return lock.beat


def _docs_only(git: Git, base: str, head: str) -> bool:
    files = git.changed_files(base, head)
    return bool(files) and all(f.startswith("docs/") or f.endswith(".md") for f in files)


def _grammar(ctx: Ctx) -> str:
    return str(ctx.paths.root / "tree-sitter-al")


def _run_gate(ctx: Ctx, name: str, cmd: list[str], minutes: int, cwd: Path) -> supervise.Result:
    ctx.write_guard("run gate")
    env = supervise.sanitized_env(os.environ.copy(), _grammar(ctx))
    log = ctx.run_dir / "logs" / f"{name}.log"
    return supervise.run(ctx, cmd, cwd=cwd, log_path=log, timeout_s=minutes * 60, beat=_beat_if_owner(ctx), env=env)


# ---- subcommands -----------------------------------------------------------

def cmd_preflight(args, ctx, gh, git):
    fails = []
    if git.branch() != "master":
        fails.append("not-on-master")
    if not git.is_clean():
        fails.append("tree-dirty")
    try:
        git.fetch()
    except GitError:
        fails.append("fetch-failed")
    if git.rev("master") != git.rev("origin/master"):
        fails.append("master-differs-from-origin")
    if (h := lock.halted(ctx)) is not None:
        fails.append(f"halt:{h}")
    lk = lock.read(ctx)
    stale = lk is not None and lock.is_stale(lk, ctx.now())
    if lk is not None and not stale:
        fails.append(f"lock-live:{lk.run_id}")
    if not gh.auth_ok():
        fails.append("gh-auth")
    cdo = os.environ.get("CDO_WS")
    if not cdo or not Path(cdo).exists():
        fails.append("cdo-ws")
    if not (ctx.paths.root / "tree-sitter-al" / "src" / "node-types.json").exists():
        fails.append("grammar")
    free_gb = shutil.disk_usage(ctx.paths.root).free // 2**30
    if free_gb < 20:
        fails.append(f"disk-free:{free_gb}GB")
    return _emit({"ok": not fails, "failures": fails, "stale_lock": asdict(lk) if stale else None,
                  "reviewers_check": "conductor: pi_models must list both reviewer models"}, 0 if not fails else 1)


def cmd_fetch(args, ctx, gh, git):
    issues = gh.list_open_issues()
    allowed = gh.collaborators() | {args.repo.split("/")[0]}
    ok, excluded = eligibility.eligible(issues, allowed)
    pick = eligibility.oldest(ok, 10)
    return _emit({
        "open_count": len(issues),
        "eligible": [{"number": i.number, "title": i.title, "body": eligibility.truncate_body(i.body),
                      "labels": sorted(i.labels), "author": i.author, "created_at": i.created_at} for i in pick],
        "eligible_total": len(ok),
        "excluded": [{"number": e.issue.number, "reason": e.reason} for e in excluded],
    })


def cmd_recover(args, ctx, gh, git):
    lk = lock.read(ctx)
    if lk is None or not lock.is_stale(lk, ctx.now()):
        raise Fail({"error": "no stale lock"})
    ctx.run_id = ctx.run_id or new_run_id(ctx.now())
    return _emit(recovery.recover_stale(ctx, git, gh, lk, worktrees_parent=ctx.paths.root.parent))


def cmd_claim(args, ctx, gh, git):
    lock.require_not_halted(ctx)
    ctx.run_id = ctx.run_id or new_run_id(ctx.now())
    attempts = read_json(ctx.paths.attempts, default={})
    attempt = attempts.get(str(args.issue), 0) + 1
    lk = lock.acquire(ctx, args.issue, args.session, attempt)
    try:
        ctx.run_dir.mkdir(parents=True, exist_ok=True)
        budget.init(ctx, claimed_at=lk.started)
        issue = gh.issue(args.issue)
        info = {
            "run_id": ctx.run_id, "issue": args.issue, "attempt": attempt, "session": args.session,
            "branch": f"issue/{args.issue}-{args.title_slug}-a{attempt}",
            "worktree": str(ctx.paths.root.parent / recovery.worktree_name(args.issue, attempt)),
            "body_hash": mergeops.body_hash(issue.body), "title": issue.title,
        }
        write_json(ctx, ctx.run_dir / "claim.json", info)
        gh.ensure_labels(LABELS)
        gh.add_labels(args.issue, ["agent-working"])
        gh.comment(args.issue, f"agentflow run `{ctx.run_id}` (attempt {attempt}) claimed this issue. Session: {args.session}")
        info["reconciled"] = discoveries.reconcile_pending(ctx, gh)
        # The attempt counter is only consumed once every fallible step above
        # has succeeded -- a rolled-back claim (nothing labelled, no branch
        # made) must not burn one of the three attempts.
        write_json(ctx, ctx.paths.attempts, {**attempts, str(args.issue): attempt})
    except Exception as e:
        try:
            gh.remove_label(args.issue, "agent-working")  # best-effort; may not have been set yet
        except Exception:
            pass
        try:
            lock.release(ctx)
        except Exception:
            pass  # never let a release failure mask the original error
        raise Fail({"error": f"claim rolled back: {e}"})
    return _emit(info)


def cmd_beat(args, ctx, gh, git):
    lock.beat(ctx)
    return _emit({"heartbeat": lock.read(ctx).heartbeat})


def cmd_halt_check(args, ctx, gh, git):
    h = lock.halted(ctx)
    return _emit({"halted": h}, 0 if (h is None or args.terminal) else 1)


def cmd_set_halt(args, ctx, gh, git):
    lock.set_halt(ctx, args.reason)
    return _emit({"halted": args.reason})


def cmd_unblock(args, ctx, gh, git):
    ctx.run_id = "unblock"
    lock.acquire(ctx, args.issue, "human", 0)
    try:
        gh.remove_label(args.issue, "agent-blocked")
    finally:
        lock.release(ctx)
    return _emit({"unblocked": args.issue})


def cmd_run(args, ctx, gh, git):
    lock.require_not_halted(ctx)
    lk = lock.read(ctx)
    if lk is not None and lk.run_id != ctx.run_id:
        # A run that has lost (or never held) the lock must not be allowed to
        # drive an unsupervised, unfenced child process under someone else's
        # claim -- refuse outright rather than silently running unsupervised.
        raise lock.FenceError(f"lock is {lk.run_id}, context is {ctx.run_id}")
    supervised = lk is not None  # lk.run_id == ctx.run_id, given the check above
    if supervised:
        budget.check_deadline(ctx)
    cwd = Path(args.cwd).resolve() if args.cwd else ctx.paths.root
    r = _run_gate(ctx, args.name, args.child, args.timeout, cwd)
    return _emit({"exit_code": r.exit_code, "log": str(r.log_path), "timed_out": r.timed_out,
                  "seconds": round(r.seconds, 1), "supervised": supervised},
                 0 if r.exit_code == 0 else 1)


def cmd_charge(args, ctx, gh, git):
    try:
        return _emit({"remaining": budget.charge(ctx, args.key, sub=args.sub)})
    except budget.BudgetExceeded as e:
        return _emit({"exhausted": e.key}, 1)
    except budget.DeadlineExceeded:
        return _emit({"exhausted": "wall-clock"}, 1)


def cmd_check_diff(args, ctx, gh, git):
    g = Git(args.cwd) if args.cwd else git
    reasons = protect.check_diff(g.changed_files(args.base, args.head), g.diff(args.base, args.head), args.issue)
    return _emit({"reasons": reasons}, 0 if not reasons else 1)


def cmd_sanitize(args, ctx, gh, git):
    cdo = os.environ.get("CDO_WS")
    found = {f: [asdict(v) for v in sanitize.scan_file(Path(f), cdo)] for f in args.files}
    found = {f: v for f, v in found.items() if v}
    return _emit({"violations": found}, 0 if not found else 1)


def cmd_freeze_check(args, ctx, gh, git):
    g = Git(args.cwd) if args.cwd else git
    v = mergeops.freeze_violations(g, args.H, "HEAD", args.issue)
    return _emit({"violations": v}, 0 if not v else 1)


def cmd_attest(args, ctx, gh, git):
    claim = read_json(ctx.run_dir / "claim.json")
    if claim is not None and claim.get("body_hash") != args.body_hash:
        return _emit({"error": "body-hash mismatch with claim"}, 1)
    att = mergeops.Attestation(issue=args.issue, B=args.B, H=args.H, final_head=args.final_head,
                               register_hash=mergeops.register_hash(Path(args.register)),
                               gates=json.loads(args.gates), body_hash=args.body_hash)
    return _emit({"path": str(mergeops.write_attestation(ctx, att))})


def cmd_body_hash(args, ctx, gh, git):
    return _emit({"body_hash": mergeops.body_hash(gh.issue(args.issue).body)})


def _gate_reasons(ctx, gh, git, pr: int):
    att = mergeops.read_attestation(ctx)
    pr_info = gh.pr_view(pr, "headRefOid,statusCheckRollup")
    reasons = mergeops.merge_gate(git, att, pr_info["headRefOid"], gh.issue(att.issue).body)
    green = mergeops.ci_green(pr_info.get("statusCheckRollup", []))
    if not green:
        reasons.append("ci-not-green")
    return att, reasons, green


def cmd_merge_gate(args, ctx, gh, git):
    _, reasons, green = _gate_reasons(ctx, gh, git, args.pr)
    return _emit({"reasons": reasons, "ci_green": green}, 0 if not reasons else 1)


def cmd_merge(args, ctx, gh, git):
    lock.require_not_halted(ctx)
    att, reasons, _ = _gate_reasons(ctx, gh, git, args.pr)
    if reasons:
        return _emit({"reasons": reasons}, 1)
    sha = mergeops.merge(ctx, gh, args.pr, att, reasons)
    write_json(ctx, ctx.run_dir / "merge.json", {"pr": args.pr, "merge_sha": sha})
    return _emit({"merge_sha": sha})


def cmd_post_merge(args, ctx, gh, git):
    ctx.write_guard("post-merge")
    lock.check_fence(ctx)
    git.checkout("master")
    git.fetch()
    if not git.ff("origin/master"):
        raise Fail({"error": "master does not fast-forward to origin/master"})
    results: dict = {}
    ok, revert, restore_failed = True, None, None
    try:
        # The merge-SHA checkout and the docs-only probe are themselves git
        # writes on the shared checkout; if either raises, the `finally`
        # below must still run to restore `master` (Important 2).
        git.checkout(args.merge_sha)
        gates = list(GATES)
        if not _docs_only(git, f"{args.merge_sha}~1", args.merge_sha):
            gates.append(CDO_GATE)

        def rerun_all():
            good = True
            for gname, gcmd, gminutes in gates:
                if _run_gate(ctx, gname + "-on-revert", gcmd, gminutes, ctx.paths.root).exit_code != 0:
                    good = False
            if _run_gate(ctx, "ci-steps-test-on-revert", ["bash", "scripts/ci-steps", "test"], 45, ctx.paths.root).exit_code != 0:
                good = False
            return good

        for name, cmd, minutes in gates:
            r = _run_gate(ctx, name, cmd, minutes, ctx.paths.root)
            results[name] = r.exit_code
            if r.exit_code != 0:
                out = recovery.post_merge_failure(ctx, git, gh, args.issue, args.merge_sha, rerun_all)
                ok, revert = False, asdict(out)
                break
        else:
            # All gates passed on their own terms; a gate that leaves a
            # tracked file modified (e.g. a build touching Cargo.lock) is
            # itself a regression the next tick's preflight would otherwise
            # report unexplained as `tree-dirty` (Important 3).
            if not git.is_clean():
                results["tree-dirty-after-gates"] = 1
                out = recovery.post_merge_failure(ctx, git, gh, args.issue, args.merge_sha, rerun_all)
                ok, revert = False, asdict(out)
    finally:
        # A failed restore must not raise and discard the verdict computed
        # above (Important 3) -- it becomes a field on the emitted JSON.
        try:
            git.checkout("master")
        except GitError as e:
            restore_failed = str(e)
    payload = {"ok": ok, "gates": results, "revert": revert}
    if restore_failed is not None:
        payload["restore_failed"] = restore_failed
        return _emit(payload, 1)
    return _emit(payload, 0 if ok else 1)


def cmd_file_discoveries(args, ctx, gh, git):
    lock.require_not_halted(ctx)  # issue filing is refused under HALT
    raw = json.loads(Path(args.file).read_text(encoding="utf-8"))
    ds = [discoveries.Discovery(**d) for d in raw]
    return _emit({"filed": discoveries.file_all(ctx, gh, ds, args.session)})


def cmd_cleanup(args, ctx, gh, git):
    lock.check_fence(ctx)
    recovery.remove_worktree(ctx, git, Path(args.worktree), args.branch, ctx.paths.root.parent, args.merge_sha)
    return _emit({"removed": args.worktree})


def cmd_finish(args, ctx, gh, git):
    label = {"merged": "agent-done", "blocked": "agent-blocked", "answered": "agent-answered"}[args.outcome]
    gh.add_labels(args.issue, [label])
    gh.remove_label(args.issue, "agent-working")
    if args.outcome == "answered":
        gh.comment(args.issue, f"agentflow run `{ctx.run_id}` answered this as a spike (no code change):\n\n{args.reason or '(see ledger)'}")
    if args.outcome == "blocked":
        gh.comment(args.issue, f"agentflow run `{ctx.run_id}` stopped: **{args.reason or 'blocked'}**. "
                               f"Branch and worktree are left in place. See the ledger in the run's PR or comments.")
        recovery.notify(ctx, "halted" if args.reason == "halted" else "blocked", f"#{args.issue}: {args.reason}")
    dest = recovery.retain(ctx)
    lock.release(ctx)
    return _emit({"outcome": args.outcome, "retained": str(dest)})


def cmd_loop_tick(args, ctx, gh, git):
    try:
        return _emit({"remaining": budget.loop_tick(ctx, args.max)})
    except budget.BudgetExceeded:
        return _emit({"exhausted": "loop"}, 1)


def cmd_loop_reset(args, ctx, gh, git):
    budget.loop_reset(ctx)
    return _emit({"reset": True})


def cmd_status(args, ctx, gh, git):
    lk = lock.read(ctx)
    b = None
    if ctx.run_id:
        try:
            b = budget.snapshot(ctx)
        except RuntimeError:
            b = None
    return _emit({"lock": asdict(lk) if lk else None, "halted": lock.halted(ctx), "budget": b})


# ---- argument parsing ------------------------------------------------------

def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="agentflow")
    p.add_argument("--root", default=".")
    p.add_argument("--repo", default="SShadowS/al-sem")
    p.add_argument("--dry-run", action="store_true")
    p.add_argument("--run-id", default=os.environ.get("AGENTFLOW_RUN_ID"))
    sp = p.add_subparsers(dest="cmd", required=True)

    def add(name, fn):
        s = sp.add_parser(name)
        s.set_defaults(fn=fn)
        return s

    add("preflight", cmd_preflight)
    add("fetch", cmd_fetch)
    add("recover", cmd_recover)
    s = add("claim", cmd_claim); s.add_argument("issue", type=int); s.add_argument("--session", required=True); s.add_argument("--title-slug", required=True)
    add("beat", cmd_beat)
    s = add("halt-check", cmd_halt_check); s.add_argument("--terminal", action="store_true")
    s = add("set-halt", cmd_set_halt); s.add_argument("reason")
    s = add("unblock", cmd_unblock); s.add_argument("issue", type=int)
    s = add("run", cmd_run); s.add_argument("--name", required=True); s.add_argument("--timeout", type=int, required=True); s.add_argument("--cwd"); s.add_argument("child", nargs=argparse.REMAINDER)
    s = add("charge", cmd_charge); s.add_argument("key"); s.add_argument("--sub")
    s = add("check-diff", cmd_check_diff); s.add_argument("--base", required=True); s.add_argument("--head", required=True); s.add_argument("--issue", type=int, required=True); s.add_argument("--cwd")
    s = add("sanitize", cmd_sanitize); s.add_argument("files", nargs="+")
    s = add("freeze-check", cmd_freeze_check); s.add_argument("--H", required=True); s.add_argument("--issue", type=int, required=True); s.add_argument("--cwd")
    s = add("attest", cmd_attest)
    for a in ("--B", "--H", "--final-head", "--register", "--gates", "--body-hash"):
        s.add_argument(a, required=True)
    s.add_argument("--issue", type=int, required=True)
    s = add("body-hash", cmd_body_hash); s.add_argument("issue", type=int)
    s = add("merge-gate", cmd_merge_gate); s.add_argument("--pr", type=int, required=True)
    s = add("merge", cmd_merge); s.add_argument("--pr", type=int, required=True)
    s = add("post-merge", cmd_post_merge); s.add_argument("--issue", type=int, required=True); s.add_argument("--merge-sha", required=True)
    s = add("file-discoveries", cmd_file_discoveries); s.add_argument("file"); s.add_argument("--session", required=True)
    s = add("cleanup", cmd_cleanup); s.add_argument("--worktree", required=True); s.add_argument("--branch", required=True); s.add_argument("--merge-sha", required=True)
    s = add("finish", cmd_finish); s.add_argument("--issue", type=int, required=True); s.add_argument("--outcome", choices=["merged", "blocked", "answered"], required=True); s.add_argument("--reason")
    s = add("loop-tick", cmd_loop_tick); s.add_argument("--max", type=int, required=True)
    add("loop-reset", cmd_loop_reset)
    add("status", cmd_status)
    return p


def main(argv: list[str], gh_run=None, git_run=None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd == "run" and args.child and args.child[0] == "--":
        args.child = args.child[1:]
    ctx = _ctx(args)
    gh_kw = {"run": gh_run} if gh_run else {}
    gh = Gh(ctx, args.repo, **gh_kw)
    git = Git(ctx.paths.root, run=git_run) if git_run else Git(ctx.paths.root)
    try:
        return args.fn(args, ctx, gh, git)
    except Fail as f:
        return _emit(f.payload, f.code)
    except DryRunViolation as e:
        return _emit({"error": f"dry-run refused write: {e}"}, 2)
    except (lock.LockHeld, lock.FenceError, lock.HaltError, GhError, GitError, budget.BudgetExceeded,
            budget.DeadlineExceeded, RuntimeError) as e:
        return _emit({"error": f"{type(e).__name__}: {e}"}, 1)
    except Exception as e:
        # No subcommand may exit without one JSON object on stdout -- an
        # unanticipated exception (KeyError, malformed JSON input, a missing
        # `gh`/`bash` on PATH, ...) is a usage/environment error, not a
        # checked condition, so it exits 2 rather than 1.
        return _emit({"error": f"{type(e).__name__}: {e}"}, 2)
