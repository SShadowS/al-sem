import json

from agentflow import cli, lock
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths, tree_snapshot
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
    assert out["failures"] == [] or out["failures"] == [f"disk-free:{out['failures'][0].split(':')[1]}"]
    assert tree_snapshot(clone) == before
    assert lock.read(Ctx(Paths(clone))) is None


def test_claim_rolls_back_lock_when_gh_fails_midway(capsys, root):
    gh = FakeRunner({**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""}, fail_at=4)
    gh.responses["issue edit 8 --add-label agent-working"] = (1, "", "HTTP 500: boom")
    code, out = run(capsys, root, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope", gh_run=gh)
    assert code == 1 and "claim rolled back" in out["error"]
    assert lock.read(Ctx(Paths(root))) is None
    assert (root / ".agent" / "runs" / "run-test" / "claim.json").exists()


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
