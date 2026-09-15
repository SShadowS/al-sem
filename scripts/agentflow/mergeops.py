"""Freeze boundary, attestation, and the merge gate.

`H` is the head the gates and the final panel passed on, AFTER the final
rebase. Only evidence commits (the two `.agent/issue-N/` files) may follow it.
The attestation binds base `B`, `H`, the final head, the findings-register
hash, the gate results, and the issue body hash. Immediately before merging,
all of base, head, and body are re-checked; the merge itself uses
`--match-head-commit` so GitHub refuses a head that moved in the last window.
"""
from __future__ import annotations

import hashlib
from dataclasses import asdict, dataclass
from pathlib import Path

from .gh import Gh
from .gitops import Git
from .protect import evidence_files
from .state import Ctx, read_json, write_json


@dataclass(frozen=True)
class Attestation:
    issue: int
    B: str
    H: str
    final_head: str
    register_hash: str
    gates: dict
    body_hash: str
    # Path of the findings register the hash above was taken from, RELATIVE to
    # the worktree the claim records -- this attestation is posted as a public
    # PR comment, so it must not carry a local absolute path. The merge joins
    # it back onto the claim's worktree to re-hash the same file and refuse one
    # that moved since. An attestation that never recorded a path (only a
    # hand-built one: `attest` always sets it) has nothing to re-check and is
    # left to the other four bindings.
    register_path: str = ""


def body_hash(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def register_hash(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def freeze_violations(git: Git, H: str, head: str, issue: int) -> list[str]:
    return [f for f in git.changed_files(H, head) if f not in evidence_files(issue)]


def write_attestation(ctx: Ctx, att: Attestation) -> Path:
    p = ctx.run_dir / "attestation.json"
    write_json(ctx, p, asdict(att))
    return p


def read_attestation(ctx: Ctx) -> Attestation:
    data = read_json(ctx.run_dir / "attestation.json")
    if not data:
        raise RuntimeError("no attestation for this run")
    return Attestation(**data)


@dataclass(frozen=True)
class MergeRecord:
    """What THIS run merged -- the only thing allowed to aim `post-merge`.

    `merge` writes one the moment GitHub reports the squash commit. The
    stale-run recovery writes the equivalent for a merge GitHub performed on
    the stale run's behalf, so the recovered follow-through stays supported
    without loosening the check for anyone else. `source` records which of the
    two minted it; it is forensic and is never compared.
    """
    issue: int
    pr: int
    merge_sha: str
    run_id: str
    source: str


def write_merge_record(ctx: Ctx, rec: MergeRecord) -> Path:
    p = ctx.run_dir / "merge.json"
    write_json(ctx, p, asdict(rec))
    return p


def read_merge_record(ctx: Ctx) -> MergeRecord | None:
    """This run's merge record, or None when there is no USABLE one: absent,
    not a JSON object, or carrying a different field set. Malformed reads as
    absent ON PURPOSE -- both mean "this run cannot prove it merged anything",
    and both must refuse. A record written before this field set existed (the
    bare `{pr, merge_sha}` this file used to hold) therefore refuses too,
    rather than being half-trusted on the two fields it happens to have.
    """
    data = read_json(ctx.run_dir / "merge.json")
    if not isinstance(data, dict):
        return None
    try:
        return MergeRecord(**data)
    except TypeError:
        return None


def ci_green(checks: list[dict], required_workflow: str | None = None) -> bool:
    """All checks completed with SUCCESS. Empty, skipped, cancelled, or pending is
    not green. When `required_workflow` is given, at least one check run from that
    workflow must also be PRESENT -- otherwise a bot check that succeeds before
    `ci.yml`'s own jobs register would read as green, which is the "a missing
    check is not green" half of the rule."""
    if not checks:
        return False
    for c in checks:
        if "conclusion" in c or "status" in c:  # check runs
            if c.get("status") != "COMPLETED" or c.get("conclusion") != "SUCCESS":
                return False
        elif c.get("state") != "SUCCESS":  # legacy status contexts
            return False
    if required_workflow is not None:
        return any(c.get("workflowName") == required_workflow for c in checks)
    return True


def merge_gate(git: Git, att: Attestation, pr_head_sha: str, issue_body_now: str) -> list[str]:
    reasons = []
    git.fetch()
    if git.rev("origin/master") != att.B:
        reasons.append("base-moved")
    if pr_head_sha != att.final_head:
        reasons.append("head-moved")
    if body_hash(issue_body_now) != att.body_hash:
        reasons.append("issue-edited")
    return reasons


def merge(ctx: Ctx, gh: Gh, pr: int, att: Attestation, gate_reasons: list[str]) -> str:
    """Merge only when `gate_reasons` (the merge gate's own verdict) is empty;
    returns the resolved squash-merge SHA the caller hands to `post_merge_failure`.
    """
    if gate_reasons:
        raise RuntimeError("merge refused: " + ", ".join(gate_reasons))
    ctx.write_guard("merge")
    gh.merge_pr(pr, att.final_head)
    # The merge HAS happened by this point. If GitHub has not populated
    # `mergeCommit` yet, say so by name rather than raising a TypeError out of
    # a subscript: the caller needs a diagnosable state, because `master`
    # already carries the commit and the post-merge backstop still has to run.
    merged = gh.pr_view(pr, "mergeCommit").get("mergeCommit") or {}
    oid = merged.get("oid")
    if not oid:
        raise RuntimeError("merge-sha-unavailable")
    return oid
