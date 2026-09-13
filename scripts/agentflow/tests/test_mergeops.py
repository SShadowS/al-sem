import json

import pytest

from agentflow import lock, mergeops
from agentflow.gh import Gh
from agentflow.gitops import Git
from agentflow.state import DryRunViolation
from agentflow.tests.conftest import FakeRunner, commit_file


def test_freeze_allows_only_own_evidence_files(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    H = commit_file(clone, "src/x.rs", "x\n", "code")
    e1 = commit_file(clone, ".agent/issue-8/ledger.md", "l\n", "ledger")
    assert mergeops.freeze_violations(g, H, e1, issue=8) == []
    e2 = commit_file(clone, "src/y.rs", "y\n", "sneaky")
    assert mergeops.freeze_violations(g, H, e2, issue=8) == ["src/y.rs"]
    e3 = commit_file(clone, ".agent/issue-9/ledger.md", "z\n", "wrong issue")
    assert ".agent/issue-9/ledger.md" in mergeops.freeze_violations(g, H, e3, issue=8)


def test_attestation_round_trip(ctx):
    att = mergeops.Attestation(issue=8, B="b" * 40, H="h" * 40, final_head="f" * 40, register_hash="r",
                               gates={"ci-steps all": 0}, body_hash="bh")
    p = mergeops.write_attestation(ctx, att)
    assert p == ctx.run_dir / "attestation.json"
    assert mergeops.read_attestation(ctx) == att


def test_ci_green_rules():
    ok = [{"status": "COMPLETED", "conclusion": "SUCCESS"}]
    assert mergeops.ci_green(ok)
    assert not mergeops.ci_green([])
    assert not mergeops.ci_green(ok + [{"status": "COMPLETED", "conclusion": "SKIPPED"}])
    assert not mergeops.ci_green([{"status": "IN_PROGRESS", "conclusion": None}])
    assert not mergeops.ci_green([{"state": "FAILURE"}])
    assert mergeops.ci_green([{"state": "SUCCESS"}])


def test_merge_gate_binds_base_head_and_body(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    B = g.rev("origin/master")
    H = commit_file(clone, "src/x.rs", "x\n", "code")
    att = mergeops.Attestation(8, B, H, H, "r", {}, mergeops.body_hash("body v1"))
    assert mergeops.merge_gate(g, att, pr_head_sha=H, issue_body_now="body v1") == []
    assert mergeops.merge_gate(g, att, pr_head_sha="0" * 40, issue_body_now="body v1") == ["head-moved"]
    assert mergeops.merge_gate(g, att, pr_head_sha=H, issue_body_now="body v2") == ["issue-edited"]
    g.checkout("master")
    commit_file(clone, "m.txt", "m\n", "master moved")
    g.push("origin", "master")
    g.fetch()
    assert "base-moved" in mergeops.merge_gate(g, att, pr_head_sha=H, issue_body_now="body v1")


def test_merge_calls_gh_with_match_head_returns_sha_and_refuses_dry_run(ctx, dry_ctx):
    lock.acquire(ctx, 8, "s", 1)
    att = mergeops.Attestation(8, "b", "h", "f" * 40, "r", {}, "bh")
    r = FakeRunner({f"pr merge 12 --squash --match-head-commit {'f' * 40}": "",
                    "pr view 12 --repo SShadowS/al-sem --json mergeCommit": json.dumps({"mergeCommit": {"oid": "m" * 40}})})
    sha = mergeops.merge(ctx, Gh(ctx, "SShadowS/al-sem", run=r), 12, att, [])
    assert sha == "m" * 40
    assert r.calls == [f"pr merge 12 --squash --match-head-commit {'f' * 40}",
                       "pr view 12 --repo SShadowS/al-sem --json mergeCommit"]
    with pytest.raises(DryRunViolation):
        mergeops.merge(dry_ctx, Gh(dry_ctx, "SShadowS/al-sem", run=FakeRunner(readonly=True)), 12, att, [])


def test_merge_refuses_without_calling_gh_when_gate_reasons_present(ctx):
    lock.acquire(ctx, 8, "s", 1)
    att = mergeops.Attestation(8, "b", "h", "f" * 40, "r", {}, "bh")
    r = FakeRunner(readonly=True)
    with pytest.raises(RuntimeError, match="merge refused"):
        mergeops.merge(ctx, Gh(ctx, "SShadowS/al-sem", run=r), 12, att, ["head-moved", "issue-edited"])
    assert r.calls == []
