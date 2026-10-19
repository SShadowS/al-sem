"""Gate-run records, and the check that lets `post-merge` skip its gates.

WHY (spec 2026-10-18-gate-speedups): `post-merge` spent 15-30 minutes
rebuilding a merge whose code the gates had just passed. It may skip only
when the merge is PROVABLY that code, built in the same environment -- and
"provably" needs evidence the executor wrote, not the `--gates` JSON the
conductor types into `attest`. So `run` appends one record per gate run here.

Everything in this module fails CLOSED: a record that cannot be read, a git
call that fails, a value that is missing -- each means "no skip", never an
exception out of `post-merge` and never a pass.
"""
from __future__ import annotations

import hashlib
import json
import os
import subprocess
from pathlib import Path

from .gitops import Git, GitError
from .state import Ctx

GATE_RUNS = "gate-runs.jsonl"


def _rustc(cwd: Path) -> str | None:
    """`rustc --version` as seen from `cwd` (a toolchain file there wins). Seam
    for tests."""
    try:
        r = subprocess.run(["rustc", "--version"], cwd=cwd, capture_output=True, text=True)
    except OSError:
        return None
    return r.stdout.strip() or None if r.returncode == 0 else None


def _tree(cwd: Path, ignore: tuple[str, ...] = ()) -> tuple[str | None, str | None, bool]:
    """(HEAD, HEAD's tree, clean) -- (None, None, False) outside a git tree.
    `clean` ignores changed paths under the `ignore` prefixes."""
    try:
        g = Git(cwd)
        dirt = [p for p in g.tree_state(refresh=False).semantic_paths if not p.startswith(ignore)]
        return g.rev("HEAD"), g.rev("HEAD^{tree}"), not dirt
    except (GitError, OSError):
        return None, None, False


# No gate reads `.agent/` (the issue's ledger and register live there, and the
# conductor writes them while gates run). Edits there say nothing about what a
# gate built, so they are not dirt in a gate record. The record's `tree` is
# HEAD's tree, so a COMMITTED evidence change during a run still voids it.
GATE_IGNORED = (".agent/",)


def _grammar(ctx: Ctx) -> dict:
    head, _, clean = _tree(ctx.paths.root / "tree-sitter-al")
    return {"commit": head, "dirty": not clean}


def _alpackages(ws: Path) -> list[Path]:
    """`ws`'s own `.alpackages` and each ancestor's, up to the git top level --
    the discovery `dependencies::discover_app_files` does."""
    dirs = []
    for d in (ws, *ws.parents):
        dirs.append(d / ".alpackages")
        if (d / ".git").exists():
            return dirs
    return dirs[:1]


def cdo_identity(ws: str | None) -> dict:
    """What a CDO run reads: the workspace's commit, its dirt, and a hash of
    every dependency `.app` (gitignored, so git state alone cannot see a swap)."""
    if not ws:
        return {"path": None, "git_sha": None, "dirty": None, "closure": None}
    root = Path(ws)
    head, _, clean = _tree(root)
    h = hashlib.sha256()
    for pk in _alpackages(root):
        if not pk.is_dir():
            continue
        for app in sorted(pk.rglob("*.app")):
            data = app.read_bytes()
            rel = os.path.relpath(app, root).replace("\\", "/")
            h.update(f"{rel}\0{len(data)}\0{hashlib.sha256(data).hexdigest()}\n".encode())
    return {"path": ws, "git_sha": head, "dirty": not clean, "closure": h.hexdigest()}


def environment(ctx: Ctx, cwd: Path, *, cdo: bool) -> dict:
    return {"rustc": _rustc(cwd), "grammar": _grammar(ctx),
            "cdo": cdo_identity(os.environ.get("CDO_WS")) if cdo else None}


def probe(ctx: Ctx, cwd: Path, *, cdo: bool) -> dict:
    """The tree and environment at one instant. Taken before AND after a gate:
    only what did not move while the child ran names what it built."""
    head, tree, clean = _tree(cwd, ignore=GATE_IGNORED)
    return {"head": head, "tree": tree, "clean": clean, "env": environment(ctx, cwd, cdo=cdo)}


def record_gate_run(ctx: Ctx, name: str, result, cwd: Path, *, argv: list[str], supervised: bool,
                    before: dict, cdo: bool) -> dict:
    """Append one record. `before` is the caller's `probe` from BEFORE the
    child ran. A HEAD, tree or environment that moved during the run is
    recorded as None: the gate built something, but nothing says what."""
    after = probe(ctx, cwd, cdo=cdo)
    same_tree = (before["head"], before["tree"]) == (after["head"], after["tree"])
    rec = {"name": name, "argv": list(argv), "exit_code": result.exit_code, "timed_out": result.timed_out,
           "supervised": supervised, "cwd": str(cwd),
           "head": before["head"] if same_tree else None, "tree": before["tree"] if same_tree else None,
           "clean_before": before["clean"], "clean_after": after["clean"],
           "env": before["env"] if before["env"] == after["env"] else None, "at": ctx.now()}
    path = ctx.run_dir / GATE_RUNS
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(rec) + "\n")
    return rec


def _same_command(recorded, expected: list[str]) -> bool:
    """The recorded argv is the command post-merge would run for that gate.
    The interpreter is compared by file name only (one bash, several paths)."""
    def exe(p: str) -> str:
        return Path(p.replace("\\", "/")).name.lower().removesuffix(".exe")
    return (isinstance(recorded, list) and len(recorded) == len(expected) and bool(recorded)
            and exe(recorded[0]) == exe(expected[0]) and recorded[1:] == expected[1:])


def identity_refusals(ctx: Ctx, git: Git, issue: int, merge_sha: str, commands: dict[str, list[str]],
                      docs_only: bool) -> list[str]:
    """Why `post-merge` may NOT skip its gates; empty means it may. `commands`
    is post-merge's own gate list, name -> argv. Every check runs (so a
    fallback names every reason), each guarded: a check that raises is a
    refusal, `error:<check>:<type>`. The caller still wraps the call, since
    the environment probe below is outside any `check`."""
    from . import cli, incidents, mergeops  # cli imports this module
    out: list[str] = []

    def check(code: str, failed) -> None:
        try:
            if failed():
                out.append(code)
        except Exception as e:  # noqa: BLE001 -- fail closed, by design
            out.append(f"error:{code}:{type(e).__name__}")

    rec = mergeops.read_merge_record(ctx)
    # `recover` mints its record without this run's attestation: never a skip.
    if rec is None or rec.source != "merge" or rec.merge_sha != merge_sha:
        out.append("not-merged-by-this-run")

    def bad_history() -> bool:
        # A merge an earlier post-merge stopped on (a red gate, a revert that
        # could not be pushed, a HALT that left the record `open`) goes back
        # through the gates: that stop is evidence the records cannot answer.
        # `cmd_post_merge` opens the record as `verifying` just before this.
        inc = incidents.all_records(ctx).get(merge_sha)
        return (inc is None or inc.get("state") != "verifying" or bool(inc.get("gate_red"))
                or inc.get("revert_landed") is False or bool(inc.get("terminal")))
    check("incident-history", bad_history)
    try:
        att = mergeops.read_attestation(ctx)
    except Exception:  # noqa: BLE001
        return [*out, "no-attestation"]
    check("attestation-other-issue", lambda: att.issue != issue)
    check("attested-gates-not-green", lambda: bool(cli._gate_failures(att.gates, docs_only)))
    check("base-moved", lambda: git.rev(f"{merge_sha}^1") != att.B)
    check("tree-differs", lambda: git.rev(f"{merge_sha}^{{tree}}") != git.rev(f"{att.final_head}^{{tree}}"))
    check("non-evidence-after-H", lambda: bool(mergeops.freeze_violations(git, att.H, att.final_head, issue)))

    records = latest_records(ctx)
    try:
        h_tree = git.rev(f"{att.H}^{{tree}}")
    except Exception as e:  # noqa: BLE001
        return [*out, f"error:H-tree:{type(e).__name__}"]
    now = environment(ctx, ctx.paths.root, cdo=cli.CDO_GATE_KEY in commands)
    grammar_now = now["grammar"]
    for name, cmd in commands.items():
        r = records.get(name)
        if r is None:
            out.append(f"no-record:{name}")
            continue
        env = r.get("env") or {}
        check(f"record-other-command:{name}", lambda: not _same_command(r.get("argv"), cmd))
        check(f"record-red:{name}", lambda: r.get("exit_code") != 0 or r.get("timed_out") is not False
              or r.get("supervised") is not True)
        check(f"record-dirty:{name}", lambda: r.get("clean_before") is not True or r.get("clean_after") is not True)
        check(f"record-other-tree:{name}", lambda: r.get("tree") != h_tree)
        check(f"rustc-changed:{name}", lambda: now["rustc"] is None or env.get("rustc") != now["rustc"])
        check(f"grammar-changed:{name}", lambda: grammar_now["commit"] is None or grammar_now["dirty"]
              or env.get("grammar") != grammar_now)
        if name == cli.CDO_GATE_KEY:
            check(f"cdo-identity-changed:{name}", lambda: now["cdo"]["path"] is None
                  or env.get("cdo") != now["cdo"])
    return out


def latest_records(ctx: Ctx) -> dict[str, dict]:
    """The last parseable record per gate name. A torn line (a run killed
    mid-write) is skipped: it is no evidence, and the record before it still is."""
    try:
        lines = (ctx.run_dir / GATE_RUNS).read_text(encoding="utf-8").splitlines()
    except OSError:
        return {}
    out: dict[str, dict] = {}
    for line in lines:
        try:
            rec = json.loads(line)
        except ValueError:
            continue
        if isinstance(rec, dict) and isinstance(rec.get("name"), str):
            out[rec["name"]] = rec
    return out
