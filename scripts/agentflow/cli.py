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
import subprocess
import time
from dataclasses import asdict
from pathlib import Path

from . import budget, discoveries, eligibility, lock, mergeops, protect, recovery, sanitize, supervise
from .gh import Gh, GhError
from .gitops import Git, GitError
from .state import Ctx, DryRunViolation, Paths, new_run_id, read_json, write_json

LABELS = ["agent-working", "agent-done", "agent-blocked", "agent-answered", "agent-regressed", "agent-filed"]
# Every gate's exit code is a claim about the repository, so the interpreter
# that runs it has to be the right one. A bare "bash" is resolved by
# CreateProcess against the inherited PATH, and from PowerShell that finds
# C:\WINDOWS\system32\bash.exe (the WSL launcher) before Git Bash -- which
# turns an environment problem into a RED GATE on every issue. `resolve_bash`
# below picks the interpreter; these two build their argv at CALL time from it
# rather than freezing a "bash" at import time.
REQUIRED_GATE_KEYS = ("ci-steps-all", "check-goldens-coverage", "check-goldens")
CDO_GATE_KEY = "cdo-gate"
_REJECTED_BASH_DIRS = ("system32", "windowsapps")


def _git_exec_path() -> str | None:
    """`git --exec-path`, or None when git is absent or fails. Seam for tests."""
    try:
        r = subprocess.run(["git", "--exec-path"], capture_output=True, text=True)
    except OSError:
        return None
    return r.stdout.strip() or None if r.returncode == 0 else None


def resolve_bash() -> str:
    """The bash the gates run under: `$AGENTFLOW_BASH`, else the one shipped with
    the Git installation `git` itself runs from, else a `bash` on PATH that is
    neither the WSL launcher nor a WindowsApps alias. No usable candidate is an
    environment error, never a silent fallback to whatever PATH offers."""
    override = os.environ.get("AGENTFLOW_BASH")
    if override:
        return override
    exec_path = _git_exec_path()
    if exec_path:
        # Git for Windows: <git-root>/mingw64/libexec/git-core -- walk up to the
        # ancestor that owns `usr/bin` and take its bash.
        p = Path(exec_path.replace("\\", "/"))
        for anc in (p, *p.parents):
            if not (anc / "usr" / "bin").is_dir():
                continue
            for cand in (anc / "usr" / "bin" / "bash.exe", anc / "bin" / "bash.exe",
                         anc / "usr" / "bin" / "bash", anc / "bin" / "bash"):
                if cand.exists():
                    return str(cand)
            break
    found = shutil.which("bash")
    if found and not any(d in found.replace("\\", "/").lower() for d in _REJECTED_BASH_DIRS):
        return found
    raise RuntimeError("no usable bash")


def gates() -> list[tuple[str, list[str], int]]:
    b = resolve_bash()
    return [
        ("ci-steps-all", [b, "scripts/ci-steps", "all"], 45),
        ("check-goldens", [b, "scripts/check-goldens"], 45),
    ]


def cdo_gate() -> tuple[str, list[str], int]:
    return (CDO_GATE_KEY, [resolve_bash(), "scripts/cdo-gate"], 45)


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


def _with_bash(cmd: list[str]) -> list[str]:
    """Rewrite a child argv whose first element is the bare token `bash` to the
    interpreter `resolve_bash` picked. The conductor's gate lines read
    `-- bash scripts/ci-steps all`, and a bare `bash` is resolved by
    CreateProcess against the inherited PATH -- from PowerShell, the WSL
    launcher, which fails every gate red instead of failing as an environment
    error. Only the exact token is rewritten: a caller that already named its
    interpreter, by full path or otherwise, is passed through untouched. The
    rewrite is idempotent, so applying it at both the CLI boundary and here
    costs nothing."""
    return [resolve_bash(), *cmd[1:]] if cmd and cmd[0] == "bash" else list(cmd)


def _run_gate(ctx: Ctx, name: str, cmd: list[str], minutes: int, cwd: Path) -> supervise.Result:
    ctx.write_guard("run gate")
    env = supervise.sanitized_env(os.environ.copy(), _grammar(ctx))
    log = ctx.run_dir / "logs" / f"{name}.log"
    # Here as well as at the `run` boundary, so every caller is covered rather
    # than only the one that happens to pass conductor-supplied argv.
    return supervise.run(ctx, _with_bash(cmd), cwd=cwd, log_path=log, timeout_s=minutes * 60,
                         beat=_beat_if_owner(ctx), env=env)


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
    try:
        resolve_bash()
    except RuntimeError:
        fails.append("bash")
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
    out = recovery.recover_stale(ctx, git, gh, lk, worktrees_parent=ctx.paths.root.parent)
    if out.get("action") == "merged-needs-post-merge":
        # A merged stale run is not fully recovered until ITS follow-through
        # (post-merge, discoveries, finish, cleanup) runs -- under the
        # recovering run's OWN lock, so those calls are fenced exactly like
        # any other claimed work rather than running lock-free.
        lock.acquire(ctx, lk.issue, "recover", lk.attempt)
        ctx.run_dir.mkdir(parents=True, exist_ok=True)
        budget.init(ctx, claimed_at=ctx.now())
        worktree = str(ctx.paths.root.parent / recovery.worktree_name(lk.issue, lk.attempt))
        write_json(ctx, ctx.run_dir / "claim.json",
                   {"issue": lk.issue, "attempt": lk.attempt, "branch": out["branch"], "worktree": worktree})
        out["worktree"] = worktree
    return _emit(out)


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
    r = _run_gate(ctx, args.name, _with_bash(args.child), args.timeout, cwd)
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


def _gate_failures(blob: dict, docs_only: bool) -> list[str]:
    """The gate keys that are missing or not exit-code 0. `cdo-gate` is required
    for everything but a docs-only diff, which never runs it."""
    required = [*REQUIRED_GATE_KEYS] + ([] if docs_only else [CDO_GATE_KEY])
    bad = []
    for k in required:
        v = blob.get(k) if isinstance(blob, dict) else None
        if not isinstance(v, int) or isinstance(v, bool) or v != 0:
            bad.append(k)
    return bad


def _register_failures(entries) -> list[str]:
    """Ids of findings-register entries that block a merge: still `open`, not
    `accepted` by BOTH reviewers, or blocking-and-merely-`deferred`. Anything
    that is not a list of entries is itself a failure -- an unreadable register
    is never a converged one."""
    if not isinstance(entries, list):
        return ["<register is not a JSON list of entries>"]
    bad = []
    for i, e in enumerate(entries):
        if not isinstance(e, dict):
            bad.append(f"#{i}")
            continue
        eid = str(e.get("id", f"#{i}"))
        reviews = e.get("reviews")
        marks = [reviews.get("astra"), reviews.get("flash")] if isinstance(reviews, dict) else [None, None]
        blocking = e.get("blocking") is True or str(e.get("severity", "")).strip().lower() == "blocking"
        if e.get("disposition") == "open":
            bad.append(eid)
        elif marks != ["accepted", "accepted"]:
            bad.append(eid)
        elif blocking and e.get("disposition") == "deferred":
            bad.append(eid)
    return bad


def cmd_attest(args, ctx, gh, git):
    # Fail closed on an absent claim: skipping the cross-check turned the
    # issue-revision pin into a comparison of a caller-supplied hash with
    # itself, which is a guard that silently degrades to a no-op.
    claim = read_json(ctx.run_dir / "claim.json")
    if claim is None:
        return _emit({"error": "no claim.json for this run"}, 1)
    if claim.get("body_hash") != args.body_hash:
        return _emit({"error": "body-hash mismatch with claim"}, 1)
    # "All gates green" and "the register converged" are the two merge
    # preconditions the conductor used to simply assert. They are checked here,
    # before the PR exists, so a refusal costs nothing to recover from.
    bad_gates = _gate_failures(json.loads(args.gates), args.docs_only)
    if bad_gates:
        return _emit({"error": "gates-not-green", "gates": bad_gates}, 1)
    # The attestation is posted verbatim as a PR comment on a public repo, so
    # the register is named RELATIVE to the worktree the claim recorded -- an
    # absolute path would publish this machine's drive letter and layout, and
    # the sanitizer would not catch it (it is neither a CDO_WS nor an
    # .alpackages path). A register the worktree does not contain cannot be
    # named that way, and is not the issue's own register either.
    register = Path(args.register).resolve()
    worktree = claim.get("worktree")
    if not worktree:
        return _emit({"error": "claim has no worktree"}, 1)
    try:
        rel = register.relative_to(Path(worktree).resolve())
    except ValueError:
        return _emit({"error": "register-outside-worktree", "register": str(register)}, 1)
    bad_entries = _register_failures(read_json(register))
    if bad_entries:
        return _emit({"error": "register-not-converged", "entries": bad_entries}, 1)
    att = mergeops.Attestation(issue=args.issue, B=args.B, H=args.H, final_head=args.final_head,
                               register_hash=mergeops.register_hash(register),
                               gates=json.loads(args.gates), body_hash=args.body_hash,
                               register_path=rel.as_posix())
    return _emit({"path": str(mergeops.write_attestation(ctx, att))})


def cmd_body_hash(args, ctx, gh, git):
    return _emit({"body_hash": mergeops.body_hash(gh.issue(args.issue).body)})


def _attested_register(ctx: Ctx, att: mergeops.Attestation) -> Path | None:
    """The attested findings register as a real path: the attestation stores it
    relative to the worktree, and this run's `claim.json` is what names that
    worktree. None when the claim (or its worktree) is gone."""
    claim = read_json(ctx.run_dir / "claim.json") or {}
    worktree = claim.get("worktree")
    return Path(worktree) / att.register_path if worktree else None


_WORKFLOW_NAME = re.compile(r"^name:\s*(.+)$", re.MULTILINE)


def _ci_workflow_name(ctx: Ctx) -> str | None:
    """The `name:` of `.github/workflows/ci.yml` -- the workflow whose checks must
    be present on the PR. Absent file or absent name means the gate cannot know
    what to require, which is a refusal rather than a pass."""
    try:
        text = (ctx.paths.root / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    except OSError:
        return None
    m = _WORKFLOW_NAME.search(text)
    return m.group(1).strip().strip("'\"") or None if m else None


def _gate_reasons(ctx, gh, git, pr: int):
    att = mergeops.read_attestation(ctx)
    pr_info = gh.pr_view(pr, "headRefOid,statusCheckRollup")
    reasons = mergeops.merge_gate(git, att, pr_info["headRefOid"], gh.issue(att.issue).body)
    if att.register_path:
        # The register was checked for convergence at attest time; re-hash the
        # same file here so an entry edited in the window between the two --
        # a `deferred` quietly flipped to `fixed` -- cannot ride into the merge.
        # The attestation names it relative to the worktree, so the claim is
        # what turns it back into a file; without the claim there is nothing to
        # verify against, which is a refusal rather than a pass.
        registered = _attested_register(ctx, att)
        if registered is None:
            reasons.append("register-unresolvable")
        else:
            try:
                if mergeops.register_hash(registered) != att.register_hash:
                    reasons.append("register-changed")
            except OSError:
                reasons.append("register-changed")
    workflow = _ci_workflow_name(ctx)
    if workflow is None:
        reasons.append("ci-workflow-unknown")
        return att, reasons, False
    green = mergeops.ci_green(pr_info.get("statusCheckRollup", []), workflow)
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
    ok, revert, restore_failed, original_exc = True, None, None, None
    try:
        # The merge-SHA checkout and the docs-only probe are themselves git
        # writes on the shared checkout; if either raises, the `finally`
        # below must still run to restore `master` (Important 2).
        git.checkout(args.merge_sha)
        gate_list = gates()
        if not _docs_only(git, f"{args.merge_sha}~1", args.merge_sha):
            gate_list.append(cdo_gate())

        def rerun_all():
            good = True
            for gname, gcmd, gminutes in gate_list:
                if _run_gate(ctx, gname + "-on-revert", gcmd, gminutes, ctx.paths.root).exit_code != 0:
                    good = False
            rerun_cmd = [resolve_bash(), "scripts/ci-steps", "test"]
            if _run_gate(ctx, "ci-steps-test-on-revert", rerun_cmd, 45, ctx.paths.root).exit_code != 0:
                good = False
            return good

        for name, cmd, minutes in gate_list:
            r = _run_gate(ctx, name, cmd, minutes, ctx.paths.root)
            results[name] = r.exit_code
            if r.exit_code != 0:
                out = recovery.post_merge_failure(ctx, git, gh, args.issue, args.merge_sha, rerun_all)
                ok, revert = False, asdict(out)
                break
        else:
            # All gates passed on their own terms; a TRACKED file left
            # modified (e.g. a build touching Cargo.lock) is itself a
            # regression -- untracked artifacts the gates' OWN run just
            # created (log files under .agent/runs, __pycache__, ...) must
            # NOT trip this, or every successful merge gets reverted.
            if git.tracked_dirty():
                results["tree-dirty-after-gates"] = 1
                out = recovery.post_merge_failure(ctx, git, gh, args.issue, args.merge_sha, rerun_all)
                ok, revert = False, asdict(out)
    except AssertionError:
        raise  # a test double's invariant violation must never become a JSON payload
    except Exception as e:
        ok, original_exc = False, e
    finally:
        # A failed restore must not raise and discard the verdict computed
        # above -- it becomes a field on the emitted JSON either way, even
        # when the try block itself already raised.
        try:
            git.checkout("master")
        except Exception as e:
            restore_failed = str(e)
    payload = {"ok": ok, "gates": results, "revert": revert}
    if restore_failed is not None:
        payload["restore_failed"] = restore_failed
        if original_exc is not None:
            raise Fail(payload, 1)
        return _emit(payload, 1)
    if original_exc is not None:
        raise original_exc
    return _emit(payload, 0 if ok else 1)


def _scan_or_fail(text: str) -> None:
    """Refuse any text that would leave this machine carrying a customer path,
    a dependency-package path, or a token. Raises rather than returning, so no
    caller can file/comment first and inspect the verdict afterwards."""
    violations = sanitize.scan(text, os.environ.get("CDO_WS"))
    if violations:
        raise Fail({"error": "sanitize-failed", "violations": [asdict(v) for v in violations]})


def cmd_file_discoveries(args, ctx, gh, git):
    lock.require_not_halted(ctx)  # issue filing is refused under HALT
    # The whole file is scanned before it is parsed: every field in it is
    # published verbatim to a public repository, and a violation anywhere means
    # the conductor's own rule failed, so none of it is trustworthy. `file_all`
    # scans each rendered body again -- that is the guard that survives a
    # future caller who does not come through this subcommand.
    _scan_or_fail(Path(args.file).read_text(encoding="utf-8", errors="replace"))
    raw = json.loads(Path(args.file).read_text(encoding="utf-8"))
    ds = [discoveries.Discovery(**d) for d in raw]
    return _emit({"filed": discoveries.file_all(ctx, gh, ds, args.session)})


def _guard_external_write(ctx: Ctx, what: str) -> None:
    """The three guards every outward-facing write shares: refuse under
    --dry-run, refuse under HALT, and refuse unless the lock names THIS run.
    The fence here is the strict one -- a PR, a PR comment and a branch push
    are all new work, never terminal bookkeeping, so an absent lock is a
    refusal, not the lenient `run`/`cleanup` case."""
    ctx.write_guard(what)
    lock.require_not_halted(ctx)
    lock.check_fence(ctx)


def _body_or_fail(path: str) -> str:
    body = Path(path).read_text(encoding="utf-8")
    _scan_or_fail(body)
    return body


def cmd_pr_create(args, ctx, gh, git):
    _guard_external_write(ctx, "create PR")
    body = _body_or_fail(args.body_file)
    if args.head.strip().removeprefix("refs/heads/") == "master":
        raise Fail({"error": "refusing to open a PR from master"})
    return _emit({"pr": gh.create_pr(args.title, body, args.head, args.base)})


def cmd_pr_comment(args, ctx, gh, git):
    _guard_external_write(ctx, "comment on PR")
    gh.pr_comment(args.pr, _body_or_fail(args.body_file))
    return _emit({"commented": args.pr})


def cmd_push_branch(args, ctx, gh, git):
    _guard_external_write(ctx, "push branch")
    g = Git(args.cwd) if args.cwd else git
    name = args.branch.strip()
    resolved = g.out("rev-parse", "--abbrev-ref", name) if g.ok("rev-parse", "--verify", name) else name
    if "master" in (name.removeprefix("refs/heads/"), resolved):
        # Nothing in code stopped a mistyped refspec from naming `master`
        # while this was a conductor-side `git push`. Now something does.
        raise Fail({"error": "refusing to push master"})
    if not g.push_branch(name, args.force_with_lease):
        raise Fail({"error": f"push of {name} was rejected"})
    return _emit({"pushed": name, "head": g.rev(name)})


def cmd_cleanup(args, ctx, gh, git):
    # Fenced like `run`, not like `post-merge`: the documented flow runs
    # cleanup BEFORE finish -- finish releases the lock, and running cleanup
    # first, while this run still holds it, is what stops a foreign run from
    # claiming the issue mid-cleanup -- so only a lock naming ANOTHER run is
    # refused. An absent lock is still accepted: the recovery follow-through's
    # own `finish` call comes AFTER cleanup and releases it only then, and a
    # standalone invocation may never have held one at all.
    lk = lock.read(ctx)
    if lk is not None and lk.run_id != ctx.run_id:
        raise lock.FenceError(f"lock is {lk.run_id}, context is {ctx.run_id}")
    if args.spike:
        if args.merge_sha:
            raise Fail({"error": "--spike and --merge-sha are mutually exclusive"}, 2)
        recovery.remove_spike_worktree(ctx, git, Path(args.worktree), args.branch, ctx.paths.root.parent)
    else:
        if not args.merge_sha:
            raise Fail({"error": "--merge-sha is required unless --spike is set"}, 2)
        recovery.remove_worktree(ctx, git, Path(args.worktree), args.branch, ctx.paths.root.parent, args.merge_sha)
    return _emit({"removed": args.worktree})


def _reason(args) -> str | None:
    """`--reason` or the contents of `--reason-file`. A spike's `## Answer` is
    multi-line markdown: passing it as an argv argument is fragile through two
    shells and capped near 32 KB on Windows, so a file is the honest transport."""
    if getattr(args, "reason_file", None):
        return Path(args.reason_file).read_text(encoding="utf-8")
    return args.reason


def cmd_finish(args, ctx, gh, git):
    reason = _reason(args)
    if args.outcome in ("answered", "blocked"):
        # Both outcomes post this text as a comment on a PUBLIC issue, and a
        # spike's answer is free-form output from a probe that may have read
        # CDO_WS. Refuse BEFORE any label or comment, so a rejected text never
        # leaves half-finished bookkeeping behind.
        _scan_or_fail(reason or "")
    if args.outcome != "regressed":
        # `regressed` is the post-merge revert path's terminal bookkeeping:
        # that path has ALREADY labelled the issue `agent-regressed`, commented
        # the failure, and set HALT. Relabelling here would be a second
        # transition saying nothing new; what is still missing is only the
        # local half -- retain the evidence and release the lock.
        label = {"merged": "agent-done", "blocked": "agent-blocked", "answered": "agent-answered"}[args.outcome]
        gh.add_labels(args.issue, [label])
        gh.remove_label(args.issue, "agent-working")
    if args.outcome == "answered":
        gh.comment(args.issue, f"agentflow run `{ctx.run_id}` answered this as a spike (no code change):\n\n{reason or '(see ledger)'}")
    if args.outcome == "blocked":
        gh.comment(args.issue, f"agentflow run `{ctx.run_id}` stopped: **{reason or 'blocked'}**. "
                               f"Branch and worktree are left in place. See the ledger in the run's PR or comments.")
        recovery.notify(ctx, "halted" if reason == "halted" else "blocked", f"#{args.issue}: {reason}")
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
    s.add_argument("--issue", type=int, required=True); s.add_argument("--docs-only", action="store_true")
    s = add("body-hash", cmd_body_hash); s.add_argument("issue", type=int)
    s = add("merge-gate", cmd_merge_gate); s.add_argument("--pr", type=int, required=True)
    s = add("merge", cmd_merge); s.add_argument("--pr", type=int, required=True)
    s = add("post-merge", cmd_post_merge); s.add_argument("--issue", type=int, required=True); s.add_argument("--merge-sha", required=True)
    s = add("pr-create", cmd_pr_create); s.add_argument("--title", required=True); s.add_argument("--body-file", required=True)
    s.add_argument("--head", required=True); s.add_argument("--base", default="master")
    s = add("pr-comment", cmd_pr_comment); s.add_argument("--pr", type=int, required=True); s.add_argument("--body-file", required=True)
    s = add("push-branch", cmd_push_branch); s.add_argument("--branch", required=True)
    s.add_argument("--force-with-lease", action="store_true"); s.add_argument("--cwd")
    s = add("file-discoveries", cmd_file_discoveries); s.add_argument("file"); s.add_argument("--session", required=True)
    s = add("cleanup", cmd_cleanup); s.add_argument("--worktree", required=True); s.add_argument("--branch", required=True); s.add_argument("--merge-sha"); s.add_argument("--spike", action="store_true")
    s = add("finish", cmd_finish); s.add_argument("--issue", type=int, required=True)
    s.add_argument("--outcome", choices=["merged", "blocked", "answered", "regressed"], required=True)
    g = s.add_mutually_exclusive_group(); g.add_argument("--reason"); g.add_argument("--reason-file")
    s = add("loop-tick", cmd_loop_tick); s.add_argument("--max", type=int, required=True)
    add("loop-reset", cmd_loop_reset)
    add("status", cmd_status)
    return p


def main(argv: list[str], gh_run=None, git_run=None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd == "run" and args.child and args.child[0] == "--":
        args.child = args.child[1:]
    try:
        # Construction lives inside the try too: a bad --root, a broken
        # AGENTFLOW_NOW, or any other setup failure must still emit one JSON
        # object rather than a bare traceback.
        ctx = _ctx(args)
        gh_kw = {"run": gh_run} if gh_run else {}
        gh = Gh(ctx, args.repo, **gh_kw)
        git = Git(ctx.paths.root, run=git_run) if git_run else Git(ctx.paths.root)
        return args.fn(args, ctx, gh, git)
    except Fail as f:
        return _emit(f.payload, f.code)
    except DryRunViolation as e:
        return _emit({"error": f"dry-run refused write: {e}"}, 2)
    except (lock.LockHeld, lock.FenceError, lock.HaltError, GhError, GitError, budget.BudgetExceeded,
            budget.DeadlineExceeded, RuntimeError) as e:
        return _emit({"error": f"{type(e).__name__}: {e}"}, 1)
    except AssertionError:
        # A test double's own invariant violation (e.g. FakeRunner's readonly
        # guard) must escape uncaught, not be laundered into a JSON payload.
        raise
    except Exception as e:
        # No subcommand may exit without one JSON object on stdout -- an
        # unanticipated exception (KeyError, malformed JSON input, a missing
        # `gh`/`bash` on PATH, ...) is a usage/environment error, not a
        # checked condition, so it exits 2 rather than 1.
        return _emit({"error": f"{type(e).__name__}: {e}"}, 2)
