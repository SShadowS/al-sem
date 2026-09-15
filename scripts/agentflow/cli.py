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

from . import (budget, discoveries, eligibility, incidents, lock, mergeops, protect, recovery,
               sanitize, supervise, worktrees)
from .gh import Gh, GhError
from .gitops import Git, GitError
from .state import Ctx, DryRunViolation, Paths, new_run_id, read_json, write_json

# `agent-regressed` is no longer spelled here: it comes from
# `recovery.INCIDENT_LABELS` along with the three two-axis names, so the
# vocabulary has exactly one definition. Order is kept stable -- `claim` calls
# `ensure_labels(LABELS)`, and a stable list keeps that read/create pass
# deterministic.
LABELS = ["agent-working", "agent-done", "agent-blocked", "agent-answered", "agent-filed",
          *recovery.INCIDENT_LABELS]
# Every gate's exit code is a claim about the repository, so the interpreter
# that runs it has to be the right one. A bare "bash" is resolved by
# CreateProcess against the inherited PATH, and from PowerShell that finds
# C:\WINDOWS\system32\bash.exe (the WSL launcher) before Git Bash -- which
# turns an environment problem into a RED GATE on every issue. `resolve_bash`
# below picks the interpreter; these two build their argv at CALL time from it
# rather than freezing a "bash" at import time.
REQUIRED_GATE_KEYS = ("ci-steps-all", "check-goldens-coverage", "check-goldens")
CDO_GATE_KEY = "cdo-gate"
# How many `git status` lines a payload lists for a checkout whose content
# matches HEAD but whose porcelain disagrees. The list is CUT; the count beside
# it never is -- see `recovery.HaltOutcome.dirty`.
MAX_ANOMALY_LISTED = 50
_REJECTED_BASH_DIRS = ("system32", "windowsapps")


def _git_exec_path() -> str | None:
    """`git --exec-path`, or None when git is absent or fails. Seam for tests."""
    try:
        r = subprocess.run(["git", "--exec-path"], capture_output=True, text=True)
    except OSError:
        return None
    return r.stdout.strip() or None if r.returncode == 0 else None


def resolve_bash() -> str:
    """The bash the gates run under: `$AGENTFLOW_BASH` if it names a file that
    exists, else the one shipped with the Git installation `git` itself runs
    from, else a `bash` on PATH that is neither the WSL launcher nor a
    WindowsApps alias. No usable candidate is an environment error, never a
    silent fallback to whatever PATH offers. A typo'd override falls THROUGH
    rather than being trusted, so it is reported by `preflight` instead of by
    a FileNotFoundError out of `Popen` much later."""
    override = os.environ.get("AGENTFLOW_BASH")
    if override and os.path.isfile(override):
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


def _verify_target_dir(ctx: Ctx) -> str:
    """The build cache a gate running in a disposable verification worktree
    writes to. The SOURCE tree is isolated; the build CACHE deliberately is
    not.

    MEASURED, and the reason this is not an oversight for a tidier to fix:
    `target/` in this checkout is 65G and `U:` has 55G free, and at post-merge
    time the issue worktree is still on disk (cleanup runs later). A per-
    worktree cache does not merely cost time, it does not FIT.

    Sharing it reintroduces nothing the isolation exists for: the dirt that
    can green-light a bad revert is SOURCE dirt, which a fresh checkout
    eliminates; cargo keys artifacts by content; and today both the
    verification run and the revert rerun already share exactly this directory
    because both run in the root. It also keeps the worktree source-only,
    which is what makes `rm -rf` fast and unlocked.

    Precedence: an explicit `AGENTFLOW_VERIFY_TARGET_DIR` wins, then an
    operator's inherited `CARGO_TARGET_DIR`, then the root's `target/`."""
    return (os.environ.get("AGENTFLOW_VERIFY_TARGET_DIR")
            or os.environ.get("CARGO_TARGET_DIR")
            or str(ctx.paths.root / "target"))


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


def _run_gate(ctx: Ctx, name: str, cmd: list[str], minutes: int, cwd: Path, *,
              cargo_target_dir: str | None = None) -> supervise.Result:
    ctx.write_guard("run gate")
    # `_grammar` is derived from `ctx.paths.root`, NOT from `cwd`, and that is
    # load-bearing rather than incidental: it is what lets a gate run in a
    # worktree that has no submodule checkout of its own. See `worktrees`.
    env = supervise.sanitized_env(os.environ.copy(), _grammar(ctx), cargo_target_dir)
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
    # THE ONE IMPLEMENTATION OF THE PROBE -- `Git.tree_state`, the same one
    # `cmd_post_merge` asks. Raw `git status --porcelain` (`is_clean`) was
    # still here, which meant the 2026-09-14 checkout -- a tracked file whose
    # CONTENT matches HEAD, materialised with the other line ending, reported
    # ` M` by porcelain forever -- failed EVERY tick at preflight. The arc
    # moved the probe where it caused a revert and left it where it stops the
    # loop: the same lie, pointed the other way.
    #
    # `refresh=not ctx.dry_run` because `update-index -q --refresh` WRITES
    # `.git/index` (the stat cache). Under `--dry-run` this command must be
    # provably write-free, and the refresh is an optimisation -- it settles a
    # pure mtime difference before the status probe -- never a correctness
    # requirement: the content comparison decides `semantic` either way.
    #
    # `semantic` ONLY. An untracked path is not evidence about the tree's
    # contents -- `is_clean()` counted one as dirt, which is why a stray
    # scratch file used to stop the whole loop -- and a content-identical
    # path whose porcelain disagrees is an operationally odd checkout, not a
    # changed one. The latter is REPORTED below, in its own non-fatal field.
    state = git.tree_state(refresh=not ctx.dry_run)
    if state.semantic:
        fails.append("tree-dirty")
    try:
        git.fetch()
    except GitError:
        fails.append("fetch-failed")
    if git.rev("master") != git.rev("origin/master"):
        fails.append("master-differs-from-origin")
    if (h := lock.halted(ctx)) is not None:
        fails.append(f"halt:{h}")
    # Computed INDEPENDENTLY of the halt row above, and that independence IS
    # the point. HALT is one global file; clearing it erases the obligation it
    # stood for, so on its own it would let a careless `clear-halt` resume the
    # loop over a merge nothing ever verified. An incident is keyed by MERGE
    # SHA and only `resolve-incident` (operator-only) or a genuinely clean
    # post-merge pass closes one. Pure read: `read_json` of a missing file
    # returns the default, so the dry-run write-freedom guarantee holds.
    #
    # TERMINAL records only. A record still in `verifying` state -- a
    # post-merge that opened its obligation and was killed before it reached a
    # verdict -- is REPORTED below and fails nothing: this row would otherwise
    # gate the `recover` that discharges it, because the conductor stops the
    # tick on `ok:false` before it ever reaches `recover`. Visible, not fatal.
    for inc in incidents.unresolved(ctx):
        fails.append(f"incident:{inc['merge_sha'][:12]}")
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
    out = {"ok": not fails, "failures": fails, "stale_lock": asdict(lk) if stale else None,
           "incidents": incidents.visible(ctx),
           "reviewers_check": "conductor: pi_models must list both reviewer models"}
    if state.status_only:
        # Visible, never a verdict: this branch must not touch `fails`. The
        # checkout disagrees with `.gitattributes`; nothing about it says a
        # tracked file changed. `_total` is the real count, always -- the list
        # beside it is the cut one.
        out["tree_anomaly"] = state.status_lines[:MAX_ANOMALY_LISTED]
        out["tree_anomaly_total"] = len(state.status_lines)
    return _emit(out, 0 if not fails else 1)


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
        #
        # `started=lk.started` -- the ORIGINAL claim's date, not `now`. This
        # lock holds the SAME work the stale lock held; it is not a new
        # claim. `recover_stale`'s recency axis asks "did GitHub date this
        # merge before the work was claimed?", and a follow-through lock
        # stamped `now` is always later than the merge it just adopted, so
        # that axis would refuse its own work BY CONSTRUCTION: a post-merge
        # killed mid-gates inside a follow-through could never be recovered
        # again, and master would keep an unverified merge behind a
        # `verifying` row. `heartbeat` deliberately stays `now` (see
        # `lock.acquire`) -- carrying it back would make this lock born stale.
        lock.acquire(ctx, lk.issue, "recover", lk.attempt, started=lk.started)
        ctx.run_dir.mkdir(parents=True, exist_ok=True)
        budget.init(ctx, claimed_at=ctx.now())
        worktree = str(ctx.paths.root.parent / recovery.worktree_name(lk.issue, lk.attempt))
        write_json(ctx, ctx.run_dir / "claim.json",
                   {"issue": lk.issue, "attempt": lk.attempt, "branch": out["branch"], "worktree": worktree})
        # The recovering run performed no merge of its own, so it has no
        # `cmd_merge` to mint the record `post-merge` demands. The merged PR
        # `recover_stale` just read IS that provenance, and it is not caller
        # text: the branch prefix came from the stale LOCK's issue, and GitHub
        # itself reports the state as MERGED and names the commit. Writing the
        # equivalent record keeps this path supported without giving anyone
        # else a way around the check.
        mergeops.write_merge_record(ctx, mergeops.MergeRecord(
            issue=lk.issue, pr=out["pr"], merge_sha=out["merge_sha"],
            run_id=ctx.run_id, source="recover"))
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


def cmd_clear_halt(args, ctx, gh, git):
    """OPERATOR-ONLY. `lock.require_operator` is what enforces that, not the
    absence of a line in the conductor prompt."""
    try:
        cleared = lock.clear_halt(ctx, args.reason, args.by)
    except lock.OperatorOnly as e:
        return _emit({"cleared": None, "refused": "operator-only", "detail": str(e)}, 1)
    except lock.HaltClearRefused as e:
        return _emit({"cleared": None, "refused": e.reason, "detail": e.detail}, 1)
    # Surfacing the surviving obligations in the SUCCESS payload is the point
    # of the incident record: the operator learns here, not two commands
    # later, that the loop is still stopped and which merge is stopping it.
    return _emit({"cleared": cleared,
                  "unresolved_incidents": [i["merge_sha"] for i in incidents.unresolved(ctx)]})


def cmd_resolve_incident(args, ctx, gh, git):
    """OPERATOR-ONLY. The human asserting that a recorded incident is closed."""
    try:
        rec = incidents.resolve(ctx, args.merge_sha, by=args.by, note=args.note)
    except lock.OperatorOnly as e:
        return _emit({"resolved": None, "refused": "operator-only", "detail": str(e)}, 1)
    except incidents.IncidentRefused as e:
        return _emit({"resolved": None, "refused": e.reason, "detail": e.detail}, 1)
    return _emit({"resolved": rec})


def cmd_incidents(args, ctx, gh, git):
    return _emit({"incidents": incidents.all_records(ctx)})


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


DISPOSITIONS = ("open", "fixed", "refuted", "deferred")
SEVERITIES = ("critical", "important", "minor")
BLOCKING_SEVERITIES = ("critical", "important")


def _register_failures(entries) -> list[str]:
    """Ids of findings-register entries that block a merge: a `severity` or a
    `disposition` outside its closed vocabulary (missing included), an entry
    still `open`, one not `accepted` by BOTH reviewers, or a blocking finding
    merely `deferred`. Anything that is not a list of entries is itself a
    failure -- an unreadable register is never a converged one.

    "Blocking" is derived from the severity vocabulary the panel actually
    writes (`critical`/`important`, case-insensitively), or an explicit
    `blocking: true`. Matching the literal word "blocking" instead meant the
    rule could not fire against any register the commands produce, so a
    Critical finding both reviewers accepted as deferred merged.

    BOTH vocabularies fail CLOSED, and for the same reason. Testing severity
    for membership in the blocking pair alone would make every unrecognised
    spelling (`High`, `Blocker`, an empty string) a silent downgrade to
    non-blocking -- a guard failing open in the line next to one failing
    closed. An unrecognised word is a malformed entry, not a minor one."""
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
        disposition = e.get("disposition")
        severity = str(e.get("severity", "")).strip().lower()
        blocking = e.get("blocking") is True or severity in BLOCKING_SEVERITIES
        if severity not in SEVERITIES:
            bad.append(eid)  # missing or unknown: malformed, never "not blocking"
        elif disposition not in DISPOSITIONS:
            bad.append(eid)  # missing or unknown: a malformed register, not a pass
        elif disposition == "open":
            bad.append(eid)
        elif marks != ["accepted", "accepted"]:
            bad.append(eid)
        elif blocking and disposition == "deferred":
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
    # The provenance record `post-merge` now demands. `issue` comes from the
    # ATTESTATION, not from argv: the attestation is what bound this merge to
    # an issue, and `merge` has no `--issue` of its own to be wrong about.
    # `ctx.run_id` is provably non-None here -- `_gate_reasons` above called
    # `mergeops.read_attestation(ctx)`, which touches `ctx.run_dir`, and that
    # property raises RuntimeError("no run_id in context") when run_id is None.
    # (`cmd_merge` itself calls only `lock.require_not_halted`, never
    # `check_fence`, so the fence is NOT what guarantees this.)
    mergeops.write_merge_record(ctx, mergeops.MergeRecord(
        issue=att.issue, pr=args.pr, merge_sha=sha, run_id=ctx.run_id, source="merge"))
    return _emit({"merge_sha": sha})


def cmd_post_merge(args, ctx, gh, git):
    ctx.write_guard("post-merge")
    lock.check_fence(ctx)
    # Provenance, before the first git call. CLAUDE.md licenses exactly one
    # automatic write to `master`: "the validated revert of a commit the flow
    # itself merged". `--issue`/`--merge-sha` are argv, and the fence above
    # only proves the LOCK names this run -- not that this run merged that
    # SHA. Together they let a malformed invocation aim the revert path at ANY
    # commit, a human's sitting at the tip included; one red gate is then
    # enough to revert it and push. On 2026-09-14 that path was entered on a
    # commit whose every real gate had passed, and only the unrelated
    # master-not-ff guard (recovery.py:66) stopped the push.
    #
    # This run's own `merge` recorded what it merged, and the stale-run
    # recovery records the merge GitHub performed on the stale run's behalf.
    # Nothing else may aim this command. Refusing HERE, rather than after the
    # gates, leaves `master` and the shared checkout untouched.
    #
    # Deliberately NOT a HALT: a refusal lets the lock go stale, and the next
    # tick's `recover` then adopts the merged PR, mints a trusted record from
    # GitHub's own answer, and re-runs this check properly. HALT would close
    # that self-heal and demand a human for what the flow can finish itself.
    rec = mergeops.read_merge_record(ctx)
    if rec is None:
        raise Fail({"error": "no merge record for this run",
                    "requested": {"issue": args.issue, "merge_sha": args.merge_sha,
                                  "run_id": ctx.run_id}})
    if rec.run_id != ctx.run_id or rec.issue != args.issue or rec.merge_sha != args.merge_sha:
        raise Fail({"error": "post-merge does not match this run's merge",
                    "recorded": {"issue": rec.issue, "merge_sha": rec.merge_sha,
                                 "run_id": rec.run_id},
                    "requested": {"issue": args.issue, "merge_sha": args.merge_sha,
                                  "run_id": ctx.run_id}})
    git.checkout("master")
    git.fetch()
    if not git.ff("origin/master"):
        raise Fail({"error": "master does not fast-forward to origin/master"})
    # REACHABILITY, after the fetch and before anything is written. Provenance
    # above proves only that SOME merge record in this run dir names this SHA;
    # it does not prove the commit is on `master` at all. `cmd_recover` mints
    # such a record from GitHub's answer about a merged PR, so a record can be
    # earned by a commit that has since been reverted off master, or -- before
    # `recover_stale` was scoped to the stale lock's own attempt -- by a
    # previous attempt's long-merged one.
    #
    # Gating a commit that is not on `master` is not a verification of
    # anything: a red gate there would HALT, open a preflight-blocking
    # incident and comment on the issue about a tree the loop does not own,
    # and `post_merge_failure` would try to revert a commit master does not
    # carry.
    #
    # BOTH SIDES FULLY QUALIFIED, and `refs/remotes/origin/master` rather than
    # local `master`: `git rev-parse master` prefers `refs/tags/master` over
    # `refs/heads/master` when both exist (measured -- see
    # `recovery.post_merge_failure`), and the remote ref is the one the revert
    # push is checked against. `is_ancestor` is reflexive, so the normal case
    # (this merge IS the tip) passes, and a master that advanced since still
    # passes. `Git.is_ancestor` runs through `ok()`, so an unknown or
    # unfetched SHA is False -- fail-closed.
    #
    # BEFORE `open_incident` below: a refused SHA must not leave a record that
    # then blocks preflight.
    if not git.is_ancestor(args.merge_sha, "refs/remotes/origin/master"):
        raise Fail({"error": "merge sha is not on origin/master", "merge_sha": args.merge_sha})
    # The obligation is opened BEFORE the gates, not only when one fails, and
    # that closes a real hole. Any exception inside the gate block below is
    # stashed into `original_exc` and re-raised at the tail -- no HALT, no
    # incident -- while the `finally` restores `master` and leaves the tree
    # clean, so the NEXT tick's preflight passes over a merge nothing ever
    # verified. With the record open from here, that crash leaves an
    # `incident:<sha12>` row instead.
    #
    # KNOWN LIMIT: the `restore_failed` path resolves the incident before the
    # restore is attempted -- `mark_verified` cannot see a failure that has
    # not happened yet. Preflight's own `not-on-master` / `tree-dirty` rows
    # are what catch a checkout left detached or dirty.
    #
    # Opened as `verifying`, NOT as a terminal obligation. A record in that
    # state is shown by `preflight`/`status` and fails neither: a post-merge
    # killed mid-gates -- the longest window in the tick -- must still be
    # recoverable by `recover` with no operator action. A terminal row here
    # would make preflight refuse in front of the very recovery that
    # discharges it, and the operator's only exit would be to record on the
    # durable audit trail that a merge nothing verified is closed, purely to
    # be allowed to go verify it.
    incidents.open_incident(ctx, merge_sha=args.merge_sha, issue=args.issue, gate_red=False,
                            reason="post-merge verification has not completed", verifying=True)
    results: dict = {}
    timeouts: list[str] = []
    verify_wt = worktrees.verify_path(ctx.paths.root, "merge", args.merge_sha)
    ok, revert, halt, restore_failed, original_exc = True, None, None, None, None
    tree_anomaly: list[str] = []
    tree_anomaly_total = 0
    removed = True
    # Set ONLY inside the `state.semantic` branch below. Never derived from
    # `halt is not None`: the gate-timed-out stop sets `halt` too and must
    # keep tearing its tree down.
    retain_verify_wt = False
    try:
        # The shared root stays on `master` for the whole command. The commit
        # under test is materialised in a DISPOSABLE worktree instead, so a
        # gate's leavings can never reach the run that decides a revert push,
        # and a modification sitting in the developer's own checkout is never
        # mistaken for something a gate did. The docs-only probe reads refs
        # only, so it never needed a checkout either. If anything below raises,
        # the `finally` must still run to restore `master` (Important 2).
        gate_list = gates()
        if not _docs_only(git, f"{args.merge_sha}~1", args.merge_sha):
            gate_list.append(cdo_gate())

        def rerun_all(work: Path) -> bool:
            """Validate the revert IN THE TREE THE CALLER BUILT IT IN. This
            closure never chooses the tree, so it cannot re-run in a
            contaminated one."""
            good = True
            for gname, gcmd, gminutes in gate_list:
                if _run_gate(ctx, gname + "-on-revert", gcmd, gminutes, work,
                             cargo_target_dir=_verify_target_dir(ctx)).exit_code != 0:
                    good = False
            rerun_cmd = [resolve_bash(), "scripts/ci-steps", "test"]
            if _run_gate(ctx, "ci-steps-test-on-revert", rerun_cmd, 45, work,
                         cargo_target_dir=_verify_target_dir(ctx)).exit_code != 0:
                good = False
            return good

        failed = killed = None
        try:
            # INSIDE the teardown `try`, not above it. `worktrees.create` can
            # raise after `git worktree add` has already checked a directory
            # out, and with the call sitting outside this block nothing
            # removed it while `removed` kept the initialiser it was never
            # reassigned from. `create` now tears down after its own
            # postcondition failure as well, so this is the second of two
            # independent guarantees rather than the only one -- but the
            # teardown block should cover the thing it tears down, and the
            # sibling call site in `recovery.post_merge_failure` has always
            # been arranged this way.
            try:
                worktrees.create(git, verify_wt, args.merge_sha)
            except GitError as e:
                raise Fail({"error": f"verify-worktree-failed: {e}"})
            for name, cmd, minutes in gate_list:
                r = _run_gate(ctx, name, cmd, minutes, verify_wt,
                              cargo_target_dir=_verify_target_dir(ctx))
                results[name] = r.exit_code
                if r.timed_out:
                    # A gate the SUPERVISOR killed is not a gate that said no.
                    # `supervise.run` folds a kill into `exit_code or 124`, so
                    # reading the exit code alone makes a clock running out
                    # indistinguishable from a verdict -- at the one decision
                    # point where that distinction IS the rule. Carried, and
                    # routed to the HALT-only path below: COULD NOT VERIFY is
                    # never PROVEN BAD. (Materially reachable, not theoretical:
                    # verification runs in a worktree cargo has never built at,
                    # so a cold `ci-steps all` crossing 45 minutes is a normal
                    # outcome, not an exotic one.)
                    timeouts.append(name)
                    killed = name
                    break
                if r.exit_code != 0:
                    failed = name
                    break
            else:
                # The probe asks the tree the gates ACTUALLY RAN IN. A probe of
                # a tree no gate ran in is not a measurement of the gates, and
                # reading the developer's checkout as if it were the gates'
                # output is what sent a fully-green merge into the revert path
                # on 2026-09-14. Untracked artifacts the gates' OWN run just
                # created (logs under .agent/runs, __pycache__, ...) are still
                # not dirt -- `tree_state` cannot see them at all.
                state = Git(verify_wt, run=git.run).tree_state()
                if state.status_only:
                    # Content identical to the commit, porcelain says modified:
                    # a checkout that disagrees with .gitattributes, not a
                    # change. Recorded, never a verdict -- this branch must not
                    # touch `ok`, `revert`, `halt` or `results`.
                    tree_anomaly = state.status_lines[:MAX_ANOMALY_LISTED]
                    tree_anomaly_total = len(state.status_lines)
                    recovery.notify(ctx, "tree-anomaly",
                                    f"#{args.issue}: {len(state.status_lines)} path(s) report dirty with "
                                    f"content identical to {args.merge_sha[:12]}; check .gitattributes. "
                                    f"Not a regression.")
                if state.semantic:
                    # A modified tracked file is evidence about the WORKING
                    # TREE, not about the merged commit, so it HALTS and is
                    # never auto-reverted. Automatic revert requires a gate to
                    # have actually said no -- which is why `results` holds
                    # gate exit codes and NOTHING else.
                    # UNTRUNCATED. `post_merge_unverified` reports `len(dirty)`
                    # in the HALT reason, the notification, the issue comment
                    # and `dirty_total`, and it does the one truncation itself
                    # at `MAX_DIRTY_LISTED`. Cutting the list here made all
                    # four of those numbers describe the cut, not the tree: a
                    # 200-file dirty tree was announced to a human as 50 and
                    # the "and N more" tail understated by 150.
                    #
                    # RETAINED, and the flag is set HERE -- inside the one
                    # branch where the dirt exists -- rather than derived
                    # later from `halt is not None`. The gate-timed-out stop
                    # also sets `halt` and must keep destroying its tree: a
                    # killed gate leaves a partial build, not evidence about a
                    # file. Set BEFORE the call, so a `gh` outage inside it
                    # cannot cost the evidence the HALT it already wrote
                    # points at.
                    retain_verify_wt = True
                    out = recovery.post_merge_unverified(ctx, gh, args.issue, args.merge_sha,
                                                         state.semantic_paths,
                                                         retained=str(verify_wt))
                    ok, halt = False, asdict(out)
                else:
                    # Every gate returned 0 AND the tree the gates ran in has
                    # no content-level modification. That, and only that, is
                    # what "this run verified the merge" means -- so it is the
                    # only automated close of the incident opened above.
                    incidents.mark_verified(ctx, args.merge_sha, ctx.run_id)
        finally:
            # Torn down BEFORE the revert path can build a second worktree, so
            # peak extra disk is one verification tree, never two.
            #
            # EXCEPT on `tree-dirty-after-gates`, the one stop where the tree
            # IS the evidence. This `finally` used to run unconditionally, so
            # the HALT was written and the dirt deleted microseconds later --
            # while the CHANGELOG, the spec (twice), this module's docstring
            # and the GitHub comment the operator actually reads all said the
            # dirt was left as found. The operator then ran `git status` in
            # the shared checkout, which no gate ran in and which is clean.
            # `removed` is reported honestly as False, and the retained path
            # rides out in `halt.retained_worktree`; `agentflow cleanup
            # --worktree <path>` removes it once a human has looked.
            #
            # KNOWN LIMIT, stated rather than discovered later: the retained
            # tree is NOT permanent. `worktrees.verify_path` is a pure
            # function of the merge SHA, and `worktrees.create` destroys
            # whatever sits at that path before checking out -- so a second
            # post-merge of the SAME merge SHA reclaims it and the evidence is
            # gone. That can only follow an operator clearing HALT, i.e.
            # someone who has been told the tree is there; it is pinned at the
            # unit level by `test_create_replaces_a_leftover_directory_...`.
            removed = False if retain_verify_wt else worktrees.destroy(git, verify_wt)
        if killed is not None:
            # HALT, never revert: the same routing a dirty tree gets, and for
            # the same reason. Automatic revert requires a gate to have
            # actually said no.
            out = recovery.post_merge_unverified(ctx, gh, args.issue, args.merge_sha, [],
                                                 code=recovery.GATE_TIMED_OUT, detail=killed)
            ok, halt = False, asdict(out)
        elif failed is not None:
            out = recovery.post_merge_failure(ctx, git, gh, args.issue, args.merge_sha, rerun_all)
            ok, revert = False, asdict(out)
    except AssertionError:
        raise  # a test double's invariant violation must never become a JSON payload
    except Exception as e:
        ok, original_exc = False, e
    finally:
        # A failed restore must not raise and discard the verdict computed
        # above -- it becomes a field on the emitted JSON either way, even
        # when the try block itself already raised. A no-op on the happy path
        # now that the root is never detached, but it keeps the postcondition
        # ("post-merge leaves the shared checkout on master") enforced.
        try:
            git.checkout("master")
        except Exception as e:
            restore_failed = str(e)
    # `timeouts` is a SEPARATE key rather than a flag folded into `gates`
    # because `gates` is a name -> exit-code map every existing consumer reads
    # that way, and 124 is an exit code like any other. The list names every
    # gate the supervisor killed, so the JSON a human reads can tell "said no"
    # from "never finished" without knowing supervise.py's `code or 124`.
    payload = {"ok": ok, "gates": results, "timeouts": timeouts, "revert": revert, "halt": halt,
               "verify_worktree_removed": removed}
    if tree_anomaly:
        payload["tree_anomaly"] = tree_anomaly
        payload["tree_anomaly_total"] = tree_anomaly_total
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
    # the conductor's own rule failed, so none of it is trustworthy. ONE read,
    # so the scanned bytes are provably the parsed bytes rather than only
    # probably. `file_all` scans each rendered body again -- that is the guard
    # that survives a future caller who does not come through this subcommand.
    text = Path(args.file).read_text(encoding="utf-8")
    _scan_or_fail(text)
    ds = [discoveries.Discovery(**d) for d in json.loads(text)]
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


def _validate_branch_name(git: Git, name: str) -> None:
    """`--branch`/`--head` must be a PLAIN branch name: not a refspec, not an
    option, not a fully-qualified ref. This is the guard that matters most in the
    package. A colon in a push argument means "push this local ref onto THAT
    remote ref", so a one-sided `git push origin feat:master` moves remote
    `master` -- and a name comparison never sees it, because `feat:master` is
    neither `master` nor resolves to it. `refs/` spellings are refused as well:
    the caller means a branch, and rejecting them keeps the two-sided refspec
    `push_branch` builds well-formed."""
    bad = (not name or name != name.strip() or any(c.isspace() for c in name)
           or ":" in name or name.startswith("-") or name.startswith("refs/") or name == "HEAD")
    if bad or not git.ok("check-ref-format", "--branch", name):
        raise Fail({"error": "not a plain branch name", "branch": name})


def cmd_pr_create(args, ctx, gh, git):
    _guard_external_write(ctx, "create PR")
    body = _body_or_fail(args.body_file)
    head = args.head.strip()
    if ":" in head:
        # The cross-repo spelling, `owner:branch`. Only the branch half names a
        # ref; GitHub owns the rest, so validate that half and no more.
        _, _, branch = head.partition(":")
        if branch.removeprefix("refs/heads/") == "master":
            raise Fail({"error": "refusing to open a PR from master"})
    else:
        if head.removeprefix("refs/heads/") == "master":
            raise Fail({"error": "refusing to open a PR from master"})
        _validate_branch_name(git, head)
    return _emit({"pr": gh.create_pr(args.title, body, head, args.base)})


def cmd_pr_comment(args, ctx, gh, git):
    _guard_external_write(ctx, "comment on PR")
    gh.pr_comment(args.pr, _body_or_fail(args.body_file))
    return _emit({"commented": args.pr})


def cmd_push_branch(args, ctx, gh, git):
    _guard_external_write(ctx, "push branch")
    g = Git(args.cwd) if args.cwd else git
    name = args.branch
    # Shape first, identity second: anything that is not a plain branch name is
    # refused before `master` is even considered, because a refspec argument can
    # reach `master` without ever containing a string that compares equal to it.
    _validate_branch_name(g, name)
    resolved = g.out("rev-parse", "--abbrev-ref", name) if g.ok("rev-parse", "--verify", f"refs/heads/{name}") else name
    # Casefolded: on a case-insensitive filesystem `refs/heads/Master` resolves
    # to the local `master` ref, so a case-sensitive comparison let `Master`
    # through to `git push` and left the refusal to the remote's own ref
    # collision. Against a case-sensitive remote that instead creates a stray
    # branch carrying master's commits.
    if "master" in (name.lower(), resolved.lower()):
        raise Fail({"error": "refusing to push master"})
    # Read the head BEFORE the push. Reading it after meant a failure on this
    # line reported a push that HAD happened as a refusal -- the same
    # false-verdict shape the `mergeCommit` guard exists to prevent.
    head = g.rev(f"refs/heads/{name}")
    if not g.push_branch(name, args.force_with_lease):
        raise Fail({"error": f"push of {name} was rejected"})
    return _emit({"pushed": name, "head": head})


def _refuse_unclaimed_cleanup(ctx, args) -> None:
    """`--worktree` and `--branch` must name what THIS RUN CLAIMED.

    The same "a record this run wrote, not argv" discipline `cmd_post_merge`
    applies to `--issue`/`--merge-sha`, on the one other command whose action
    is `shutil.rmtree` plus `git branch -D`. Both values are already on disk:
    `cmd_claim` writes them to `.agent/runs/<run_id>/claim.json` and
    `cmd_recover` writes the same two keys on the recovery follow-through.
    `orchestrate.md` step 11 (and step 1.3 for the follow-through) passes
    exactly those values back, so a disagreement means the conductor is
    holding two worktrees in one tick and handed over the wrong one.

    DELIBERATELY TOLERANT of an absent record, in two ways. No `run_id` at all
    and no `claim.json` both mean "this invocation is not a claimed run's
    step" -- an operator cleaning up by hand after a crash, which must keep
    working. That makes this a NARROWING of the claimed path, never a new way
    for cleanup to be unavailable. The ownership guard in
    `recovery._refuse_unowned_path` is what covers the unclaimed case.
    """
    if not ctx.run_id:
        return
    claim = read_json(ctx.paths.run_dir(ctx.run_id) / "claim.json")
    if not isinstance(claim, dict):
        return
    want_wt, want_branch = claim.get("worktree"), claim.get("branch")
    if want_wt and Path(want_wt).resolve() != Path(args.worktree).resolve():
        raise Fail({"error": "cleanup does not match this run's claim",
                    "recorded": {"worktree": want_wt, "branch": want_branch},
                    "requested": {"worktree": args.worktree, "branch": args.branch}})
    if want_branch and args.branch and want_branch != args.branch:
        raise Fail({"error": "cleanup does not match this run's claim",
                    "recorded": {"worktree": want_wt, "branch": want_branch},
                    "requested": {"worktree": args.worktree, "branch": args.branch}})


def cmd_cleanup(args, ctx, gh, git):
    # Fenced like `run`, not like `post-merge`: the documented flow runs
    # cleanup BEFORE finish -- finish releases the lock, and running cleanup
    # first, while this run still holds it, is what stops a foreign run from
    # claiming the issue mid-cleanup -- so only a lock naming ANOTHER run is
    # refused. An absent lock is still accepted: the recovery follow-through's
    # own `finish` call comes AFTER cleanup and releases it only then, and a
    # standalone invocation may never have held one at all.
    lk = lock.read(ctx)
    # A retained verification worktree is reached through a STALE foreign
    # lock by construction, so the fence below carves that case out.
    # `post-merge`'s `tree-dirty-after-gates` stop retains the tree and HALTs
    # WITHOUT releasing the lock; the operator who comes to remove it arrives
    # after the run is long dead, holding no run id of their own. Refusing a
    # stale foreign lock here therefore refuses the exact invocation the HALT
    # comment, the CHANGELOG and orchestrate.md all name as the exit -- a
    # directory the system retains and documents an exit for, that no
    # supported command can remove.
    #
    # A LIVE foreign lock still refuses everything: another run is acting on
    # this checkout, and its post-merge may own that very tree. Only
    # staleness opens the carve-out, and only for this one route, which
    # cannot touch a branch or a merge SHA. This mirrors `lock.require_operator`
    # (lock.py:144-149) and `cmd_preflight`, both of which fail on a LIVE lock
    # and deliberately let a stale one through.
    is_verify = Path(args.worktree).name.startswith(worktrees.VERIFY_PREFIX)
    if lk is not None and lk.run_id != ctx.run_id and not (is_verify and lock.is_stale(lk, ctx.now())):
        raise lock.FenceError(f"lock is {lk.run_id}, context is {ctx.run_id}")
    # THE EXIT FOR A RETAINED VERIFICATION WORKTREE. `post-merge`'s
    # `tree-dirty-after-gates` stop keeps the tree the gates ran in, because
    # on that stop the dirt IS the evidence -- and a directory with no
    # supported way to remove it is how an obligation nobody can discharge
    # gets created. It is DETACHED, so it has no branch and no merge SHA to
    # prove anything about; the two removers below end in `git branch -D` and
    # refuse this prefix outright (`recovery._refuse_unowned_path`). The
    # module that owns this prefix removes it, and its own `_guard` is the
    # ownership proof -- the same one standing behind every other `rm -rf` in
    # `worktrees`.
    if is_verify:
        ctx.write_guard("remove verification worktree")
        # `is not None`, not truthiness: `--branch ""` was GIVEN, and an
        # argument that was given and is wrong must be refused, not ignored.
        if args.branch is not None or args.merge_sha or args.spike:
            raise Fail({"error": "a verification worktree is detached: "
                                 "--branch, --merge-sha and --spike do not apply",
                        "worktree": args.worktree}, 2)
        # THE LOCATION CONJUNCT. `worktrees._guard` proves the NAME, and that
        # is the only thing standing between this recursive delete and an
        # arbitrary path -- but this is the one `destroy` call whose path comes
        # from ARGV. Every other caller passes a path `worktrees.verify_path`
        # built, so location is guaranteed there by construction and here by
        # nothing. `recovery._refuse_unowned_path` states the rule this
        # restores: TWO conjuncts, because either alone is close to nothing --
        # `U:/Git` holds ~230 sibling directories, nearly all of them git
        # repositories, so "is a sibling of the checkout" is not ownership, and
        # a matching name anywhere else on disk is not ownership either.
        target = Path(args.worktree).resolve()
        if target.parent != ctx.paths.root.parent.resolve():
            raise Fail({"error": "refusing a verification worktree outside this "
                                 "checkout's parent directory",
                        "worktree": str(target),
                        "expected_parent": str(ctx.paths.root.parent.resolve())})
        if not worktrees.destroy(git, target):
            raise Fail({"error": f"could not remove {args.worktree}"})
        return _emit({"removed": args.worktree})
    # `is None` -- ABSENT, not empty. `--branch ""` was given and is wrong,
    # and `_validate_branch_name` below is the guard that says so ("not a
    # plain branch name"); turning it into a usage error here would retire
    # that arm of the shape check without any test noticing.
    if args.branch is None:
        raise Fail({"error": "--branch is required unless --worktree is a verification worktree"}, 2)
    # `--branch` was the ONE branch argument in the CLI that reached git
    # unvalidated, and both paths below end in `git branch -D`. Shape first,
    # identity second -- the same split `cmd_push_branch` makes, and for the
    # same reason: a refspec-shaped argument can reach `master` without ever
    # containing a string that compares equal to it, so a name comparison
    # alone is not a guard.
    _validate_branch_name(git, args.branch)
    resolved = (git.out("rev-parse", "--abbrev-ref", args.branch)
                if git.ok("rev-parse", "--verify", f"refs/heads/{args.branch}") else args.branch)
    # Casefolded: on a case-insensitive filesystem `refs/heads/Master`
    # resolves to the local `master` ref, so a case-sensitive comparison lets
    # `Master` through (see `cmd_push_branch`).
    if "master" in (args.branch.lower(), resolved.lower()):
        raise Fail({"error": "refusing to clean up master", "branch": args.branch})
    # AFTER the branch shape/identity refusals (so those keep failing for
    # their own reason) and BEFORE either remover, which is the first point
    # where an argument becomes a deletion.
    _refuse_unclaimed_cleanup(ctx, args)
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
        # `regressed` is the terminal bookkeeping for EITHER post-merge stop,
        # and the CONTRACT this branch rests on is the same for both: the stop
        # has ALREADY made the issue's label transition, commented, set HALT
        # and recorded the durable incident, so a label here would be a second
        # transition saying nothing new. What is still missing is only the
        # local half -- retain the evidence and release the lock.
        #
        # Deliberately NOT a list of what post-merge stamps. There are two
        # exits (`recovery.post_merge_failure` and `post_merge_unverified`)
        # and two INDEPENDENT label axes, decided in exactly one place,
        # `recovery.incident_labels`. The previous version of this comment
        # named `agent-regressed` as the single label post-merge applies; that
        # was true of one exit and false of the other -- a stop where no gate
        # returned a red verdict never stamps it -- and restating the
        # vocabulary here is how the comment went stale the last time it
        # moved. The contract holds whichever labels that function returns.
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
    # Status is the operator's dashboard, and reporting HALT without the
    # obligation behind it is what let a cleared HALT read as "all clear".
    # `visible`, not `unresolved`: the dashboard shows a verification still in
    # flight too. It is not an obligation the operator has to close, but a row
    # nobody can see is how one gets forgotten.
    return _emit({"lock": asdict(lk) if lk else None, "halted": lock.halted(ctx), "budget": b,
                  "incidents": incidents.visible(ctx)})


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
    # OPERATOR-ONLY (see `lock.require_operator`). They appear in `--help`
    # deliberately: the defence is the gate in lock.py, never obscurity.
    s = add("clear-halt", cmd_clear_halt); s.add_argument("--reason", required=True); s.add_argument("--by", required=True)
    s = add("resolve-incident", cmd_resolve_incident); s.add_argument("--merge-sha", required=True)
    s.add_argument("--by", required=True); s.add_argument("--note", default="")
    add("incidents", cmd_incidents)
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
        # `--branch` is NOT `required=True`: a retained verification worktree is
    # detached and has none, and that path is the only supported way to
    # remove one. `cmd_cleanup` requires it for every other worktree.
    s = add("cleanup", cmd_cleanup); s.add_argument("--worktree", required=True); s.add_argument("--branch"); s.add_argument("--merge-sha"); s.add_argument("--spike", action="store_true")
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
