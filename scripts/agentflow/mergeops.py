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


def ci_green(checks: list[dict]) -> bool:
    """All checks completed with SUCCESS. Empty, skipped, cancelled, or pending is not green."""
    if not checks:
        return False
    for c in checks:
        if "conclusion" in c or "status" in c:  # check runs
            if c.get("status") != "COMPLETED" or c.get("conclusion") != "SUCCESS":
                return False
        elif c.get("state") != "SUCCESS":  # legacy status contexts
            return False
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
    return gh.pr_view(pr, "mergeCommit")["mergeCommit"]["oid"]
