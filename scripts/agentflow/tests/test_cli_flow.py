import json
import sys

from agentflow import cli, lock, mergeops
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths, tree_snapshot, write_json
from agentflow.tests.conftest import FakeRunner, commit_file

REPO = "SShadowS/al-sem"
ISSUES = json.dumps([[{"number": 8, "title": "c10: scope", "body": "## Acceptance\nx", "user": {"login": "SShadowS"},
                       "labels": [], "created_at": "2026-09-12T00:00:00Z"},
                      {"number": 9, "title": "stranger", "body": "## Acceptance\nx", "user": {"login": "nobody"},
                       "labels": [], "created_at": "2026-09-12T00:00:00Z"}]])
READS = {
    f"api repos/{REPO}/issues?state=open&per_page=100 --paginate --slurp": ISSUES,
    f"api repos/{REPO}/collaborators?permission=push&per_page=100 --paginate --slurp": json.dumps([[{"login": "SShadowS"}]]),
    "auth status": "",
    f"api repos/{REPO}/issues/8": json.dumps(json.loads(ISSUES)[0][0]),
    f"api repos/{REPO}/labels?per_page=100 --paginate --slurp": json.dumps([[{"name": n} for n in cli.LABELS]]),
}


def run(capsys, root, *args, gh_run, dry=False, run_id="run-test"):
    argv = ["--root", str(root), "--repo", REPO] + (["--dry-run"] if dry else []) + (["--run-id", run_id] if run_id else [])
    code = cli.main(argv + list(args), gh_run=gh_run)
    return code, json.loads(capsys.readouterr().out)


def test_dry_run_fetch_and_preflight_write_nothing(capsys, repo_pair, monkeypatch, tmp_path):
    _, clone = repo_pair
    # The grammar-presence check reads this file, and preflight's own
    # git-cleanliness check (git status --porcelain) counts ANY untracked
    # path as dirty -- so this fixture file must be committed (and pushed,
    # so local `master` still matches `origin/master`) rather than left as a
    # bare untracked file, or the test would spuriously fail on
    # "tree-dirty"/"master-differs-from-origin", neither of which the write-
    # freedom assertion below is about.
    commit_file(clone, "tree-sitter-al/src/node-types.json", "[]", "grammar stub")
    assert Git(clone).push("origin", "master")
    monkeypatch.setenv("CDO_WS", str(tmp_path))
    before = tree_snapshot(clone)
    gh = FakeRunner(READS, readonly=True)
    code, out = run(capsys, clone, "fetch", gh_run=gh, dry=True, run_id=None)
    assert code == 0 and [i["number"] for i in out["eligible"]] == [8]
    assert out["excluded"] == [{"number": 9, "reason": "author"}]
    code, out = run(capsys, clone, "preflight", gh_run=gh, dry=True, run_id=None)
    f = out["failures"]
    assert not f or (len(f) == 1 and f[0].startswith("disk-free:"))
    assert tree_snapshot(clone) == before
    assert lock.read(Ctx(Paths(clone))) is None


def test_claim_rolls_back_lock_when_gh_fails_midway(capsys, root):
    gh = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""}, fail_at=4)
    gh.responses["issue edit 8 --add-label agent-working"] = (1, "", "HTTP 500: boom")
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh)
    assert code == 1 and "claim rolled back" in out["error"]
    assert lock.read(Ctx(Paths(root))) is None
    assert (root / ".agent" / "runs" / "run-test" / "claim.json").exists()
    # I7: a rolled-back claim must not consume an attempt -- the next claim on
    # the same issue must still be attempt 1, not 2.
    gh2 = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""})
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope",
                     gh_run=gh2, run_id="run-2")
    assert code == 0 and out["attempt"] == 1


def test_claim_then_finish_blocked_releases_lock_and_retains(capsys, root, tmp_path, monkeypatch):
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    gh = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": "",
                     "issue edit 8 --add-label agent-blocked": "", "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh)
    assert code == 0 and out["branch"] == "issue/8-c10-scope-a1" and out["attempt"] == 1
    assert lock.read(Ctx(Paths(root))).issue == 8
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "blocked", "--reason", "spec-panel-cap", gh_run=gh)
    assert code == 0 and lock.read(Ctx(Paths(root))) is None
    assert (tmp_path / "home" / ".al-sem" / "agentflow" / "runs" / "run-test" / "claim.json").exists()
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh, run_id="run-2")
    assert out["attempt"] == 2 and out["branch"].endswith("-a2")


def test_check_diff_and_freeze_via_cli(capsys, repo_pair):
    _, clone = repo_pair
    (clone / ".agent").mkdir()
    base = commit_file(clone, "src/a.rs", "a\n", "a")
    head = commit_file(clone, "scripts/evil.sh", "x\n", "evil")
    code, out = run(capsys, clone, "check-diff", "--base", base, "--head", head, "--issue", "8", gh_run=FakeRunner())
    assert code == 1 and out["reasons"] == ["protected-path:scripts/evil.sh"]
    H = head
    commit_file(clone, ".agent/issue-8/ledger.md", "l\n", "evidence")
    code, out = run(capsys, clone, "freeze-check", "--H", H, "--issue", "8", gh_run=FakeRunner())
    assert code == 0 and out["violations"] == []


def test_dry_run_run_refuses_before_spawning_child(capsys, root):
    # C1: `run`'s child must never actually spawn under --dry-run.
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), dry=True)
    assert code == 2 and "error" in out
    assert not (root / ".agent" / "runs" / "run-test" / "logs" / "probe.log").exists()


def test_dry_run_post_merge_refuses_before_moving_master(capsys, repo_pair):
    # C1: `post-merge` must never touch `master` under --dry-run.
    _, clone = repo_pair
    before = Git(clone).rev("master")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", before,
                     gh_run=FakeRunner(), dry=True)
    assert code == 2 and "error" in out
    assert Git(clone).rev("master") == before
    assert Git(clone).branch() == "master"


def test_post_merge_refuses_when_run_id_does_not_own_lock(capsys, repo_pair):
    # I5: post-merge is fenced -- a run that does not hold (or own) the lock
    # must not be able to move `master`.
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="owner-run"), 8, "s", 1)
    before = Git(clone).rev("master")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", before,
                     gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    assert Git(clone).rev("master") == before
    assert Git(clone).branch() == "master"


def test_attest_refuses_body_hash_mismatch_with_claim(capsys, root, tmp_path):
    # I8: attest must cross-check its --body-hash against claim.json, not
    # trust a caller-supplied hash on its own.
    ctx = Ctx(Paths(root), run_id="run-test")
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": "abc123"})
    register = tmp_path / "findings.json"
    register.write_text("[]")
    code, out = run(capsys, root, "attest", "--issue", "8", "--B", "B", "--H", "H",
                     "--final-head", "F", "--register", str(register), "--gates", "{}",
                     "--body-hash", "different-hash", gh_run=FakeRunner())
    assert code == 1 and out["error"] == "body-hash mismatch with claim"


def test_attest_accepts_matching_body_hash(capsys, root, tmp_path):
    ctx = Ctx(Paths(root), run_id="run-test")
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": "abc123"})
    register = tmp_path / "findings.json"
    register.write_text("[]")
    code, out = run(capsys, root, "attest", "--issue", "8", "--B", "B", "--H", "H",
                     "--final-head", "F", "--register", str(register), "--gates", "{}",
                     "--body-hash", "abc123", gh_run=FakeRunner())
    assert code == 0 and "path" in out


def test_merge_gate_detects_moved_head(capsys, repo_pair):
    # I10(a): a PR head that moved since the attestation must be caught.
    _, clone = repo_pair
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head="cafebabe" * 5,
                                register_hash="r" * 40, gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({
        **READS,
        f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup": json.dumps(
            {"headRefOid": "deadbeef" * 5,
             "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}]}),
    })
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["reasons"] == ["head-moved"]


def test_post_merge_happy_path_restores_master_and_reports_ok(capsys, repo_pair, monkeypatch):
    # I10(b): the full post-merge success path -- both gate lists patched to
    # a real, instant no-op child so no actual build/test tooling is needed.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "GATES", [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "CDO_GATE", ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    # `.agent/` is the executor's own state directory; it is not yet
    # gitignored in this fixture repo (a known, separately-tracked gap --
    # Task 17 adds `.agent/*`), so the gates' own log-file writes under it
    # would otherwise make the new post-gate git.is_clean() check (Important
    # 3) spuriously fail on something unrelated to the property under test.
    commit_file(clone, ".gitignore", ".agent/\n", "ignore executor state")
    assert Git(clone).push("origin", "master")
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert code == 0 and out["ok"] is True and out["revert"] is None
    assert out["gates"] == {"noop": 0, "noop-cdo": 0}
    assert Git(clone).branch() == "master"
    assert Git(clone).is_clean()
