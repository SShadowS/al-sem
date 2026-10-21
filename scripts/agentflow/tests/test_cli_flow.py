import json
import shutil
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

from agentflow import cli, incidents, lock, mergeops, recovery, supervise, worktrees
from agentflow.gitops import Git, GitError
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


def _crlf_consistent(clone):
    """Put the whole clone on `core.autocrlf=true` AND renormalize whatever
    `repo_pair` already committed under the machine's own setting.

    Without the renormalize this fixture is machine-dependent: on a box whose
    git has `core.autocrlf=false`, `repo_pair`'s `README.md` blob holds CRLF,
    and flipping the repo to `true` makes it read as content-modified -- so
    the tree would be genuinely dirty and the assertions below would be about
    the wrong file. Measured on this machine: git's SYSTEM config already sets
    `core.autocrlf=true`, so the renormalize is a no-op here and the guard is
    for somebody else's checkout."""
    _git_raw(clone, "config", "core.autocrlf", "true")
    _git_raw(clone, "add", "--renormalize", ".")
    if subprocess.run(["git", "-C", str(clone), "diff", "--cached", "--quiet"]).returncode != 0:
        _git_raw(clone, "commit", "-q", "-m", "renormalize under core.autocrlf=true")


def test_preflight_does_not_call_a_byte_identical_crlf_checkout_dirty(
        capsys, repo_pair, monkeypatch, tmp_path):
    """THE INCIDENT'S OWN CHECKOUT, one tick later. The arc moved the probe
    where it caused a revert and left raw `git status --porcelain` here, where
    it STOPS THE LOOP: on the 2026-09-14 checkout -- a tracked file whose
    content matches HEAD, materialised with the other line ending, reported
    ` M` by porcelain forever -- `preflight` emitted `tree-dirty` and every
    tick refused, permanently.

    TWO HALVES, so neither is satisfiable by deleting the row: (a) the
    byte-identical file is not `tree-dirty` and is reported in its own
    non-fatal field; (b) a genuine content edit in the SAME tree still is."""
    _, clone = repo_pair
    _crlf_consistent(clone)
    preflight_ready(clone, monkeypatch, tmp_path)
    (clone / "gen.sha256").write_bytes(b"deadbeef\r\n")
    _git_raw(clone, "add", "gen.sha256")
    _git_raw(clone, "commit", "-q", "-m", "sidecar committed from CRLF bytes")
    assert Git(clone).push("origin", "master")
    (clone / "gen.sha256").write_bytes(b"deadbeef\n")          # what gen-syntax does
    # PRECONDITION with raw git, independent of the code under test.
    assert _git_raw(clone, "status", "--porcelain", "-uno").stdout.strip() != ""
    assert _git_raw(clone, "diff", "--name-only", "HEAD", "--").stdout.strip() == ""
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))

    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "tree-dirty" not in out["failures"], out["failures"]
    assert code == 0, out["failures"]
    assert any("gen.sha256" in l for l in out["tree_anomaly"]), out
    assert out["tree_anomaly_total"] == len(out["tree_anomaly"])

    # (b) THE CONTRAST. Without it, half (a) is satisfied by deleting the row.
    (clone / "README.md").write_bytes(b"genuinely different\n")
    assert _git_raw(clone, "diff", "--name-only", "HEAD", "--").stdout.split() == ["README.md"]
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "tree-dirty" in out["failures"], out["failures"]
    assert code == 1


def test_preflight_refuses_below_the_free_disk_floor(capsys, repo_pair, monkeypatch, tmp_path):
    """One GB under the default `min_free_gb` fails preflight with
    `disk-free:`; exactly the floor does not. Stated as numbers, not via the
    default, so lowering the floor back to 20 fails this test. No
    `.agent/config.json` exists here, so this is the default."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    assert not (clone / ".agent" / "config.json").exists()
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=39 * 2**30))
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "disk-free:39GB" in out["failures"], out["failures"]
    assert code == 1
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=40 * 2**30))
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert not any(f.startswith("disk-free:") for f in out["failures"]), out["failures"]


def test_preflight_reads_its_free_disk_floor_from_the_config_file(capsys, repo_pair, monkeypatch, tmp_path):
    """`min_free_gb` in `.agent/config.json` moves the floor, in both
    directions, and a broken config file is an error, not the default."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    config = clone / ".agent" / "config.json"
    config.write_text(json.dumps({"min_free_gb": 100}), encoding="utf-8")
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=99 * 2**30))
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "disk-free:99GB" in out["failures"], out["failures"]
    config.write_text(json.dumps({"min_free_gb": 10}), encoding="utf-8")
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=10 * 2**30))
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert not any(f.startswith("disk-free:") for f in out["failures"]), out["failures"]
    config.write_text(json.dumps({"min_free_gb": "lots"}), encoding="utf-8")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 2 and "min_free_gb" in out["error"], out


def test_preflight_lists_every_lease_and_fails_slots_full_only_on_live_ones(
        capsys, repo_pair, monkeypatch, tmp_path):
    """`locks` lists every lease with its staleness. `slots-full` counts LIVE
    leases only: a stale one is listed for `recover`, and failing on it would
    stop the tick before it reaches the recovery that frees the slot. The
    `lock-live:` row is gone; with `slots: 1` one live lease is `slots-full`."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    (clone / ".agent" / "config.json").write_text(json.dumps({"slots": 2}), encoding="utf-8")
    lock.acquire(Ctx(Paths(clone), run_id="run-live", now=lambda: 1_000_000.0), 8, "s", 1)
    lock.acquire(Ctx(Paths(clone), run_id="run-dead", now=lambda: 1.0), 9, "s", 1)
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 0, out["failures"]
    assert [(lk["issue"], lk["run_id"], lk["stale"]) for lk in out["locks"]] == [
        (8, "run-live", False), (9, "run-dead", True)]
    assert out["stale_lock"]["run_id"] == "run-dead"
    assert [lk["run_id"] for lk in out["stale_locks"]] == ["run-dead"]      # the list `recover` reads
    assert out["warnings"] == []
    (clone / ".agent" / "config.json").write_text(json.dumps({"slots": 3}), encoding="utf-8")
    lock.acquire(Ctx(Paths(clone), run_id="run-live2", now=lambda: 1_000_000.0), 10, "s", 1)
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "slots-full" not in out["failures"], out["failures"]      # 2 live < 3
    (clone / ".agent" / "config.json").write_text(json.dumps({"slots": 2}), encoding="utf-8")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 1 and out["failures"] == ["slots-full"], out["failures"]


def test_preflight_warns_when_cargo_target_dir_is_set(capsys, repo_pair, monkeypatch, tmp_path):
    """Design risk (g)3. An inherited CARGO_TARGET_DIR makes every issue
    worktree share one build dir, so two runs' gates contend (#60). A
    warning, not a failure: one run at a time is still fine with it."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))
    monkeypatch.setenv("CARGO_TARGET_DIR", "U:/shared-target")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 0, out["failures"]
    assert out["warnings"] == ["cargo-target-dir-set:U:/shared-target"]


def test_preflight_under_dry_run_never_refreshes_the_index(capsys, repo_pair, monkeypatch, tmp_path):
    """`tree_state(refresh=True)` runs `git update-index -q --refresh`, which
    WRITES `.git/index` (the stat cache). `preflight` is one of the two
    commands that must be provably write-free under `--dry-run`, so it passes
    `refresh=not ctx.dry_run`.

    Asserted on the ARGV git was asked to run, not on the file: `.agent`'s own
    snapshot helper skips `.git/` outright, so an index write is invisible to
    the write-freedom assertion the dry-run test already makes. Both
    directions are stated -- a guard that never refreshes at all would satisfy
    the first half alone."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))
    seen = []

    def recording(argv, capture_output=True, text=True, **kw):
        seen.append(list(argv))
        return subprocess.run(argv, capture_output=capture_output, text=text, **kw)

    base = ["--root", str(clone), "--repo", REPO]
    cli.main(base + ["--dry-run", "preflight"], gh_run=FakeRunner(READS), git_run=recording)
    capsys.readouterr()
    assert not any("update-index" in a for a in seen), [a for a in seen if "update-index" in a]

    seen.clear()
    cli.main(base + ["--run-id", "run-test", "preflight"], gh_run=FakeRunner(READS), git_run=recording)
    capsys.readouterr()
    assert any("update-index" in a for a in seen), seen


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


# ---- per-issue leases (parallel-issues design, commit 5) --------------------

CLAIM_GH = {**READS, f"api repos/{REPO}/issues/9": json.dumps(json.loads(ISSUES)[0][1]),
            "issue edit 8 --add-label agent-working": "", "issue comment 8 *": "",
            "issue edit 9 --add-label agent-working": "", "issue comment 9 *": ""}


def _claim(capsys, root, issue, gh, run_id):
    return run(capsys, root, "claim", str(issue), "--session", "https://s", "--title-slug", "x",
               gh_run=gh, run_id=run_id)


def _slots(root, n):
    (root / ".agent").mkdir(exist_ok=True)
    (root / ".agent" / "config.json").write_text(json.dumps({"slots": n}), encoding="utf-8")


def _attempts(root):
    p = root / ".agent" / "attempts.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else {}


def test_a_second_claim_of_a_held_issue_is_issue_held_and_touches_nothing(capsys, root):
    """Design test 8. Even with a free slot, the second claim of one issue
    fails `issue-held:<run>` before any gh call and leaves `attempts.json`
    as it was."""
    _slots(root, 2)
    assert _claim(capsys, root, 8, FakeRunner(CLAIM_GH), "run-a")[0] == 0
    before = _attempts(root)
    gh = FakeRunner(CLAIM_GH)
    code, out = _claim(capsys, root, 8, gh, "run-b")
    assert code == 1 and out["error"] == "issue-held:run-a", out
    assert gh.calls == []
    assert _attempts(root) == before == {"8": 1}
    assert [lk.run_id for lk in lock.all_locks(Ctx(Paths(root)))] == ["run-a"]
    assert not (root / ".agent" / "runs" / "run-b").exists()


def test_slots_one_still_refuses_a_second_issue_as_slots_full(capsys, root):
    """The default `slots: 1` keeps today's rule: one issue at a time."""
    assert _claim(capsys, root, 8, FakeRunner(CLAIM_GH), "run-a")[0] == 0
    gh = FakeRunner(CLAIM_GH)
    code, out = _claim(capsys, root, 9, gh, "run-b")
    assert code == 1 and out["error"] == "slots-full", out
    assert gh.calls == [] and _attempts(root) == {"8": 1}


CLAIM_CHILD = """
import json, shutil, types
from agentflow import cli
from agentflow.tests.conftest import FakeRunner, PLENTY_OF_DISK
# The parent's `plenty_of_disk` patch does not reach a child process.
shutil.disk_usage = lambda p: types.SimpleNamespace(free=PLENTY_OF_DISK)
issue, responses = ARGS[0], json.loads(ARGS[1])
if len(ARGS) > 2:
    slow_reads(ARGS[2])
gh = FakeRunner(responses)
barrier()
code = cli.main(["--root", str(ROOT), "--repo", "SShadowS/al-sem", "--run-id", f"run-{TAG}",
                 "claim", issue, "--session", "s", "--title-slug", "x"], gh_run=gh)
print("EXIT", code)
"""


def test_two_processes_claiming_one_issue_at_once_get_one_lease(root):
    """Design test 9, two real processes released together. Exactly one wins;
    the other is `issue-held`. One lease, one attempt bump.

    `slots: 2`, so the loser cannot be refused for `slots-full` instead, and
    the config read is slowed so the check-then-create window is wide."""
    from agentflow.tests.conftest import run_concurrently
    _slots(root, 2)
    res = run_concurrently(root, CLAIM_CHILD, ["a", "b"], extra=["8", json.dumps(CLAIM_GH), "config.json"])
    codes = sorted(int(o.split("EXIT")[-1]) for _, o, _ in res)
    assert codes == [0, 1], res
    loser = next(o for _, o, _ in res if o.strip().endswith("EXIT 1"))
    assert '"issue-held:run-' in loser, loser
    leases = lock.all_locks(Ctx(Paths(root)))
    assert [lk.issue for lk in leases] == [8], leases
    assert _attempts(root) == {"8": 1}


def test_two_processes_claiming_two_issues_at_once_keep_both_attempt_bumps(root):
    """Design test 10. Each claim bumps its own key in `attempts.json`. The
    early read is slowed, so both read `{}`; a claim that writes back its own
    snapshot then loses the other's bump."""
    from agentflow.tests.conftest import run_concurrently
    _slots(root, 2)
    body = CLAIM_CHILD.replace('issue, responses = ARGS[0]', 'issue, responses = {"a": "8", "b": "9"}[TAG]')
    res = run_concurrently(root, body, ["a", "b"], extra=["-", json.dumps(CLAIM_GH), "attempts.json"])
    assert all(o.strip().endswith("EXIT 0") for _, o, _ in res), res
    assert _attempts(root) == {"8": 1, "9": 1}


def test_two_processes_claiming_two_issues_with_one_slot_get_one_lease(root):
    """The slot count is a check-then-act across DIFFERENT lease files, so
    create-exclusive does not cover it; only mutex `registry` does. With
    `slots: 1` and the config read slowed, two claims of two issues at once
    must still end with one lease and one `slots-full`."""
    from agentflow.tests.conftest import run_concurrently
    _slots(root, 1)
    body = CLAIM_CHILD.replace('issue, responses = ARGS[0]', 'issue, responses = {"a": "8", "b": "9"}[TAG]')
    res = run_concurrently(root, body, ["a", "b"], extra=["-", json.dumps(CLAIM_GH), "config.json"])
    outs = [o for _, o, _ in res]
    assert sorted(o.strip()[-6:] for o in outs) == ["EXIT 0", "EXIT 1"], res
    assert '"slots-full"' in next(o for o in outs if o.strip().endswith("EXIT 1")), res
    assert len(lock.all_locks(Ctx(Paths(root)))) == 1


# ---- claim --loop-max (parallel-issues design, commit 9) --------------------

def _loop(root):
    p = root / ".agent" / "runs" / "loop.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def _set_loop(root, n):
    p = root / ".agent" / "runs" / "loop.json"
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps({"issues_done": n}), encoding="utf-8")


def _claim_max(capsys, root, issue, gh, run_id, loop_max):
    return run(capsys, root, "claim", str(issue), "--session", "https://s", "--title-slug", "x",
               "--loop-max", str(loop_max), gh_run=gh, run_id=run_id)


def test_claim_with_loop_max_spends_the_loop_budget_only_on_a_claim_that_lands(capsys, root):
    """`claim --loop-max M` charges `loop.json` inside the claim's own
    registry critical section. A landed claim spends one; `issue-held` spends
    nothing (the conductor tries its next pick); an exhausted budget is
    `loop-exhausted` with no lease and no gh call. Without `--loop-max`,
    `loop.json` is not touched (the old `loop-tick` + `claim` shape)."""
    _slots(root, 3)
    _set_loop(root, 2)
    code, out = _claim_max(capsys, root, 8, FakeRunner(CLAIM_GH), "run-a", 3)
    assert code == 0, out
    assert _loop(root) == {"issues_done": 3}
    gh = FakeRunner(CLAIM_GH)
    code, out = _claim_max(capsys, root, 8, gh, "run-b", 5)
    assert code == 1 and out["error"] == "issue-held:run-a", out
    assert _loop(root) == {"issues_done": 3} and gh.calls == []
    code, out = _claim_max(capsys, root, 9, gh, "run-b", 3)
    assert code == 1 and out["error"] == "loop-exhausted", out
    assert gh.calls == [] and lock.for_issue(Ctx(Paths(root)), 9) is None
    assert _loop(root) == {"issues_done": 3}
    code, out = _claim(capsys, root, 9, FakeRunner(CLAIM_GH), "run-c")
    assert code == 0, out
    assert _loop(root) == {"issues_done": 3}


def test_a_rolled_back_claim_gives_its_loop_budget_back(capsys, root):
    """The budget counts issues actually claimed. A claim that rolls back
    (a gh failure after the lease) returns the unit it spent."""
    _set_loop(root, 1)
    gh = FakeRunner({**CLAIM_GH, "issue edit 8 --add-label agent-working": (1, "", "HTTP 500: boom")})
    code, out = _claim_max(capsys, root, 8, gh, "run-a", 3)
    assert code == 1 and "claim rolled back" in out["error"], out
    assert lock.for_issue(Ctx(Paths(root)), 8) is None
    assert _loop(root) == {"issues_done": 1}


def test_two_processes_claiming_with_one_loop_unit_left_get_one_claim(root):
    """Design test 14, the `claim --loop-max` form. Two sessions claim two
    different issues at `issues_done = 2` under `--loop-max 3` with two free
    slots: exactly one claim lands; the other is `loop-exhausted`.

    The window that matters is AFTER the lease, where the claim does its gh
    calls: a budget checked in the critical section but charged there would
    let the second session check 2 < 3 too. The claim's read of
    `handoffs.json`, which sits in that window, is slowed to hold it open."""
    from agentflow.tests.conftest import run_concurrently
    _slots(root, 2)
    _set_loop(root, 2)
    (root / ".agent" / "handoffs.json").write_text("{}", encoding="utf-8")
    body = (CLAIM_CHILD.replace('issue, responses = ARGS[0]', 'issue, responses = {"a": "8", "b": "9"}[TAG]')
            .replace('"--title-slug", "x"]', '"--title-slug", "x", "--loop-max", "3"]'))
    assert body.count('"--loop-max", "3"') == 1
    res = run_concurrently(root, body, ["a", "b"], extra=["-", json.dumps(CLAIM_GH), "handoffs.json"])
    outs = [o for _, o, _ in res]
    assert sorted(o.strip()[-6:] for o in outs) == ["EXIT 0", "EXIT 1"], res
    assert '"loop-exhausted"' in next(o for o in outs if o.strip().endswith("EXIT 1")), res
    assert len(lock.all_locks(Ctx(Paths(root)))) == 1
    assert _loop(root) == {"issues_done": 3}


# ---- the root git mutex and make-worktree (commit 10) ------------------------

def _merge_from_a_sibling(origin, tmp_path, name):
    """Move `origin/master` from another clone, as a sibling session's merge
    does. The root's local `master` is left behind."""
    other = tmp_path / name
    subprocess.run(["git", "clone", "-q", str(origin), str(other)], check=True, capture_output=True)
    _git_raw(other, "config", "user.email", "o@x")
    _git_raw(other, "config", "user.name", "o")
    sha = commit_file(other, f"{name}.txt", "x\n", f"sibling merge {name}")
    assert Git(other).push("origin", "master")
    return sha


def test_make_worktree_branches_the_claim_from_origin_master(capsys, repo_pair, tmp_path):
    """Design test 37. `make-worktree` replaces the conductor's own
    `git worktree add` in the root: it takes the worktree and branch from
    THIS run's `claim.json`, fetches, and bases the branch on
    `origin/master`, not local `master`, which lags while a sibling's merge
    waits for its post-merge. It is fenced: a run with no lease gets nothing."""
    origin, clone = repo_pair
    code, claim = _claim(capsys, clone, 8, FakeRunner(CLAIM_GH), "run-test")
    assert code == 0, claim
    local_before = Git(clone).rev("master")
    sibling = _merge_from_a_sibling(origin, tmp_path, "sib")
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True), run_id="run-other")
    assert code == 1 and "FenceError" in out["error"], out
    assert not Path(claim["worktree"]).exists()
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 0, out
    assert out == {"worktree": claim["worktree"], "branch": claim["branch"], "base": sibling, "target": None}
    wt = Git(claim["worktree"])
    assert wt.branch() == claim["branch"] and wt.rev("HEAD") == sibling
    assert Git(clone).rev("master") == local_before          # the root's master is not moved


def test_make_worktree_waits_for_the_root_git_mutex(capsys, repo_pair, monkeypatch):
    """The CLI's root `Git` takes mutex `git` for its writes, so a sibling
    session's `git worktree add`/`fetch` and this one never collide on
    `.git/index.lock` or a ref lock. Held here by the test: the command waits
    and then fails `MutexTimeout` with nothing created; released, it works."""
    from agentflow.state import mutex
    _, clone = repo_pair
    code, claim = _claim(capsys, clone, 8, FakeRunner(CLAIM_GH), "run-test")
    assert code == 0, claim
    monkeypatch.setattr(cli, "GIT_MUTEX_TIMEOUT", 0.3)
    with mutex(Ctx(Paths(clone)), "git"):
        code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 1 and "MutexTimeout" in out["error"], out
    assert not Path(claim["worktree"]).exists()
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 0 and Path(claim["worktree"]).exists(), out


def test_preflight_passes_a_master_behind_origin_and_fails_a_diverged_one(
        capsys, repo_pair, monkeypatch, tmp_path):
    """With two sessions, a sibling's merge moves `origin/master`, and only
    that sibling's post-merge fast-forwards the root. A root `master` BEHIND
    `origin/master` is reported (`master_behind_origin`) and fails nothing;
    one that has DIVERGED (a commit `origin` lacks) still fails
    `master-differs-from-origin`."""
    origin, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 0 and out["master_behind_origin"] is False, out
    _merge_from_a_sibling(origin, tmp_path, "sib")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 0 and out["failures"] == [], out["failures"]
    assert out["master_behind_origin"] is True
    commit_file(clone, "local.txt", "x\n", "a local commit origin does not have")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 1 and out["failures"] == ["master-differs-from-origin"], out["failures"]


def test_finish_naming_another_issue_than_the_lease_refuses_before_any_gh_call(
        capsys, root, tmp_path, monkeypatch):
    """Design test 12. `finish --issue 9` under a lease for 8 used to label
    issue 9 and release 8's lease."""
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    assert _claim(capsys, root, 8, FakeRunner(CLAIM_GH), "run-test")[0] == 0
    gh = FakeRunner({"issue edit 9 --add-label agent-blocked": "", "issue edit 9 --remove-label agent-working": "",
                     "issue comment 9 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "9", "--outcome", "blocked", "--reason", "x", gh_run=gh)
    assert code == 1 and "lock is for issue 8, call names 9" in out["error"], out
    assert gh.calls == []
    assert lock.own(Ctx(Paths(root), run_id="run-test")).issue == 8


def test_a_legacy_lock_json_is_beaten_fenced_counted_and_released_by_new_code(
        capsys, root, tmp_path, monkeypatch):
    """Design test 13, through the CLI. A run that claimed under the old code
    holds `.agent/lock.json` (no `recovering_by` key) when the new code lands.
    Its next calls are new code: `beat` rewrites it in place, it holds its
    slot and its issue, and its `finish` removes it and leaves no lease."""
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    legacy = root / ".agent" / "lock.json"
    legacy.write_text(json.dumps({"issue": 8, "run_id": "run-legacy", "session": "s", "started": 999_000.0,
                                  "heartbeat": 999_500.0, "attempt": 1}), encoding="utf-8")
    code, out = run(capsys, root, "beat", gh_run=FakeRunner({}), run_id="run-legacy")
    assert code == 0 and out["heartbeat"] == 1_000_000.0, out
    assert json.loads(legacy.read_text(encoding="utf-8"))["heartbeat"] == 1_000_000.0
    assert not (root / ".agent" / "locks").exists()
    gh = FakeRunner(CLAIM_GH)
    code, out = _claim(capsys, root, 8, gh, "run-new")
    assert code == 1 and out["error"] == "issue-held:run-legacy", out
    code, out = _claim(capsys, root, 9, gh, "run-new")
    assert code == 1 and out["error"] == "slots-full", out
    assert gh.calls == []
    fin = FakeRunner({"issue edit 8 --add-label agent-blocked": "", "issue edit 8 --remove-label agent-working": "",
                      "issue comment 8 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "blocked", "--reason", "x",
                    gh_run=fin, run_id="run-legacy")
    assert code == 0, out
    assert not legacy.exists() and lock.all_locks(Ctx(Paths(root))) == []
    assert list((root / ".agent").glob("locks/*")) == []


def test_claim_and_preflight_need_the_floor_plus_a_reserve_per_live_run(
        capsys, repo_pair, monkeypatch, tmp_path):
    """Design test 30. Required free disk = `min_free_gb` + `reserve_gb_per_run`
    x live leases, measured on the worktrees' volume (the root's PARENT). With
    one live run and the defaults that is 40 + 30 = 70 GB: a claim at 60 GB
    is `disk-low` and creates nothing; at 80 GB it claims. `preflight`
    reports the same two numbers and fails on the same rule. A stale lease
    does not count (it builds nothing more), and `unblock` takes no slot so
    is not checked."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    _slots(clone, 3)
    lock.acquire(Ctx(Paths(clone), run_id="run-live", now=lambda: 1_000_000.0), 8, "s", 1)
    lock.acquire(Ctx(Paths(clone), run_id="run-dead", now=lambda: 1.0), 10, "s", 1)
    asked = []

    def disk(gb):
        def usage(p):
            asked.append(Path(p))
            return SimpleNamespace(free=gb * 2**30)
        monkeypatch.setattr(cli.shutil, "disk_usage", usage)

    disk(60)
    gh = FakeRunner(CLAIM_GH)
    code, out = _claim(capsys, clone, 9, gh, "run-new")
    assert code == 1 and out["error"] == "disk-low:60GB<70GB", out
    assert gh.calls == [] and lock.for_issue(Ctx(Paths(clone)), 9) is None
    assert asked and all(p == clone.parent for p in asked), asked
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert (out["disk_free_gb"], out["disk_required_gb"]) == (60, 70), out
    assert "disk-free:60GB" in out["failures"], out["failures"]
    code, out = run(capsys, clone, "unblock", "11", gh_run=FakeRunner(
        {"issue edit 11 --remove-label agent-blocked": ""}), run_id=None)
    assert code == 0, out
    disk(80)
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert (out["disk_free_gb"], out["disk_required_gb"]) == (80, 70), out
    assert not any(f.startswith("disk-free:") for f in out["failures"]), out["failures"]
    code, out = _claim(capsys, clone, 9, FakeRunner(CLAIM_GH), "run-new")
    assert code == 0, out


def test_fetch_excludes_an_issue_with_a_lease_even_without_its_label(capsys, root, monkeypatch):
    """Design test 11. The `agent-working` label is added AFTER the lease and
    can lag, so `fetch` reads the leases: an issue with one (live or stale)
    is `locked:<run>`, never offered to a second session."""
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    _slots(root, 2)
    mine = {"user": {"login": "SShadowS"}, "body": "## Acceptance\nx", "labels": [],
            "created_at": "2026-09-12T00:00:00Z"}
    issues = [[{**mine, "number": n, "title": f"t{n}"} for n in (8, 10, 11)]]
    lock.acquire(Ctx(Paths(root), run_id="run-live", now=lambda: 1_000_000.0), 8, "s", 1)
    lock.acquire(Ctx(Paths(root), run_id="run-dead", now=lambda: 1.0), 10, "s", 1)
    code, out = run(capsys, root, "fetch", gh_run=FakeRunner({**READS, READS_ISSUES_KEY: json.dumps(issues)}),
                    run_id=None)
    assert code == 0
    assert [i["number"] for i in out["eligible"]] == [11], out
    assert out["excluded"] == [{"number": 8, "reason": "locked:run-live"},
                               {"number": 10, "reason": "locked:run-dead"}], out


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


def capture_child(monkeypatch):
    """Replace the supervisor with a probe that records the argv it was handed
    and never spawns anything."""
    seen = {}

    def fake_run(ctx, cmd, *, cwd, log_path, timeout_s, beat=None, beat_every=60.0, env=None):
        seen["cmd"] = list(cmd)
        return supervise.Result(exit_code=0, log_path=log_path, timed_out=False, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", fake_run)
    return seen


def test_run_maps_a_literal_bash_child_to_the_resolved_interpreter(capsys, root, tmp_path, monkeypatch):
    # Residual (3): the /issue gates reach the executor as
    # `run --name … -- bash scripts/ci-steps all`. That literal `bash` is
    # resolved by CreateProcess against the inherited PATH, which from
    # PowerShell is the WSL launcher -- the same red-gate-on-every-issue
    # failure resolve_bash() was introduced for.
    bash = tmp_path / "my-bash.exe"
    bash.write_text("")
    monkeypatch.setenv("AGENTFLOW_BASH", str(bash))
    seen = capture_child(monkeypatch)
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     "bash", "scripts/ci-steps", "all", gh_run=FakeRunner(), run_id="solo-run")
    assert code == 0 and out["exit_code"] == 0
    assert seen["cmd"] == [str(bash), "scripts/ci-steps", "all"]


def test_run_leaves_any_other_child_argv_alone(capsys, root, monkeypatch):
    # Only the exact token `bash` is rewritten: a child that already names its
    # interpreter -- including a bash by full path -- is passed through, or the
    # mapping would be second-guessing a caller who was explicit. No
    # AGENTFLOW_BASH is needed: resolve_bash must never be consulted at all.
    seen = capture_child(monkeypatch)
    run(capsys, root, "run", "--name", "p1", "--timeout", "1", "--",
        sys.executable, "-c", "pass", gh_run=FakeRunner(), run_id="solo-run")
    assert seen["cmd"] == [sys.executable, "-c", "pass"]
    run(capsys, root, "run", "--name", "p2", "--timeout", "1", "--",
        "/usr/bin/bash", "-c", "true", gh_run=FakeRunner(), run_id="solo-run")
    assert seen["cmd"] == ["/usr/bin/bash", "-c", "true"]


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


GREEN_GATES = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0, "cdo-gate": 0})
CONVERGED = json.dumps([{"id": "F1", "severity": "important", "disposition": "fixed",
                         "reviews": {"astra": "accepted", "flash": "accepted"}}])


def attest(capsys, root, register, *, gates=GREEN_GATES, body_hash="abc123", extra=(),
           B="b" * 40, H="c" * 40, final_head="f" * 40):
    return run(capsys, root, "attest", "--issue", "8", "--B", B, "--H", H,
               "--final-head", final_head, "--register", str(register), "--gates", gates,
               "--body-hash", body_hash, *extra, gh_run=FakeRunner())


@pytest.mark.parametrize("flag", ["B", "H", "final_head"])
def test_attest_refuses_a_sha_that_is_not_full(capsys, root, tmp_path, flag):
    # #53: merge-gate compares GitHub's 40-char head SHA and `rev-parse
    # origin/master` to the stored strings with `!=`, so an abbreviated value
    # would later read as head-moved / base-moved. Refuse it at attest time.
    ctx = Ctx(Paths(root), run_id="run-test")
    register = claimed_register(ctx, tmp_path)
    code, out = attest(capsys, root, register, **{flag: "f0cc001e"})
    assert code == 1 and out["error"] == "sha-not-full"
    assert out["values"] == {flag: "f0cc001e"}
    assert not (ctx.run_dir / "attestation.json").exists()


def claimed_register(ctx, tmp_path, *, body_hash="abc123", contents=CONVERGED, name="wt"):
    """A claim naming a worktree, plus the findings register inside it. The
    attestation stores that register RELATIVE to the worktree -- an absolute
    local path would be published verbatim in the attestation's PR comment --
    so the claim and the register have to be set up together."""
    wt = tmp_path / name
    (wt / ".agent" / "issue-8").mkdir(parents=True, exist_ok=True)
    write_json(ctx, ctx.run_dir / "claim.json", {"body_hash": body_hash, "worktree": str(wt)})
    register = wt / ".agent" / "issue-8" / "findings.json"
    register.write_text(contents)
    return register


def test_attest_refuses_body_hash_mismatch_with_claim(capsys, root, tmp_path):
    # I8: attest must cross-check its --body-hash against claim.json, not
    # trust a caller-supplied hash on its own.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register, body_hash="different-hash")
    assert code == 1 and out["error"] == "body-hash mismatch with claim"


def test_attest_accepts_matching_body_hash(capsys, root, tmp_path):
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register)
    assert code == 0 and "path" in out


def test_attest_stores_the_register_relative_to_the_worktree(capsys, root, tmp_path):
    # Residual (4): the attestation is posted verbatim as a PR comment on a
    # PUBLIC repository. An absolute path in it publishes the maintainer's
    # drive letter and directory layout, which the sanitizer does not catch
    # (it is neither a CDO_WS nor an .alpackages path).
    ctx = Ctx(Paths(root), run_id="run-test")
    register = claimed_register(ctx, tmp_path)
    code, out = attest(capsys, root, register)
    assert code == 0
    text = (ctx.run_dir / "attestation.json").read_text()
    assert json.loads(text)["register_path"] == ".agent/issue-8/findings.json"
    assert str(tmp_path) not in text and str(register.resolve()) not in text


def test_attest_refuses_a_register_outside_the_worktree(capsys, root, tmp_path):
    # A register the worktree does not contain cannot be named relative to it,
    # and is not the issue's own register either way.
    ctx = Ctx(Paths(root), run_id="run-test")
    claimed_register(ctx, tmp_path)
    stray = tmp_path / "findings.json"
    stray.write_text(CONVERGED)
    code, out = attest(capsys, root, stray)
    assert code == 1 and out["error"] == "register-outside-worktree"
    assert not (ctx.run_dir / "attestation.json").exists()


def test_attest_refuses_when_there_is_no_claim_for_this_run(capsys, root, tmp_path):
    # I6 (reversed ruling): with claim.json absent the body-hash cross-check
    # used to be SKIPPED, so the issue-revision pin degraded into comparing a
    # caller-supplied hash against itself. No claim, no attestation.
    register = tmp_path / "findings.json"
    register.write_text(CONVERGED)
    code, out = attest(capsys, root, register)
    assert code == 1 and out["error"] == "no claim.json for this run"
    assert not (root / ".agent" / "runs" / "run-test" / "attestation.json").exists()


def test_attest_refuses_missing_and_red_gates(capsys, root, tmp_path):
    # I6: "all gates green" was a conductor assertion the executor recorded
    # verbatim and never read. A `--gates '{}'` must not be attestable.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    code, out = attest(capsys, root, register, gates="{}")
    assert code == 1 and out["error"] == "gates-not-green"
    assert set(out["gates"]) == {"ci-steps-all", "check-goldens-coverage", "check-goldens", "cdo-gate"}
    red = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 1, "cdo-gate": 0})
    code, out = attest(capsys, root, register, gates=red)
    assert code == 1 and out["error"] == "gates-not-green" and out["gates"] == ["check-goldens"]


def test_attest_docs_only_does_not_require_the_cdo_gate(capsys, root, tmp_path):
    # A docs-only diff legitimately never runs cdo-gate, so demanding the key
    # would make the check unusable exactly where it is least needed.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    three = json.dumps({"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0})
    code, out = attest(capsys, root, register, gates=three)
    assert code == 1 and out["gates"] == ["cdo-gate"]
    code, out = attest(capsys, root, register, gates=three, extra=("--docs-only",))
    assert code == 0 and "path" in out


def test_attest_refuses_a_register_that_has_not_converged(capsys, root, tmp_path):
    # I6: "both reviewers converged" was likewise recorded as an opaque hash
    # and never opened. Each of the three non-convergence shapes is named.
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    both = {"astra": "accepted", "flash": "accepted"}
    for entries, offender in (
        ([{"id": "F1", "severity": "minor", "disposition": "open", "reviews": both}], "F1"),
        ([{"id": "F2", "severity": "minor", "disposition": "fixed",
           "reviews": {"astra": "accepted", "flash": "re-raised"}}], "F2"),
        # N2: the blocking rule has to fire against the severities the panel
        # actually writes. `critical` and `important` ARE blocking; a register
        # only ever marked that way would otherwise deferred-and-merge a
        # finding both reviewers accepted as deferred.
        ([{"id": "F3", "severity": "Critical", "disposition": "deferred", "reviews": both}], "F3"),
        ([{"id": "F4", "severity": "important", "disposition": "deferred", "reviews": both}], "F4"),
        ([{"id": "F5", "severity": "minor", "blocking": True, "disposition": "deferred", "reviews": both}], "F5"),
        # N3: `disposition` is required by the schema, so a missing or unknown
        # one is a malformed register, not a silently-passing entry.
        ([{"id": "F6", "severity": "minor", "reviews": both}], "F6"),
        ([{"id": "F7", "severity": "minor", "disposition": "wontfix", "reviews": both}], "F7"),
        # B1: and `severity` the same way. Matching two exact words meant any
        # other spelling silently downgraded the entry to non-blocking, so a
        # `High` finding accepted as deferred attested green -- a guard that
        # fails open in the line next to one that fails closed.
        ([{"id": "F8", "severity": "High", "disposition": "deferred", "reviews": both}], "F8"),
        ([{"id": "F9", "severity": "blocking", "disposition": "deferred", "reviews": both}], "F9"),
        ([{"id": "F10", "disposition": "deferred", "reviews": both}], "F10"),
    ):
        register.write_text(json.dumps(entries))
        code, out = attest(capsys, root, register)
        assert code == 1 and out["error"] == "register-not-converged", entries
        assert out["entries"] == [offender], entries


def test_attest_accepts_a_declared_substitute_and_records_it(capsys, root, tmp_path):
    # A rostered reviewer is unreachable; a named stand-in reviewed instead. The
    # stand-in's marks fill that slot, and the attestation -- which is posted
    # verbatim as the PR comment -- says who actually signed.
    ctx = Ctx(Paths(root), run_id="run-test")
    signed = json.dumps([{"id": "F1", "severity": "important", "disposition": "fixed",
                          "reviews": {"fable": "accepted", "flash": "accepted"}}])
    register = claimed_register(ctx, tmp_path, contents=signed)

    # Without the declaration the astra slot is empty, so this must not pass:
    # a stand-in's marks never count by accident.
    code, out = attest(capsys, root, register)
    assert code == 1 and out["error"] == "register-not-converged"

    code, out = attest(capsys, root, register, extra=("--substitute", "astra=fable"))
    assert code == 0, out
    att = json.loads(Path(out["path"]).read_text())
    assert att["reviewers"] == {"astra": "fable", "flash": "flash"}


def test_attest_ignores_the_replaced_reviewers_marks(capsys, root, tmp_path):
    # Once a slot is substituted it is the stand-in's verdict that counts; the
    # original reviewer's acceptance cannot paper over a stand-in's re-raise.
    signed = json.dumps([{"id": "F1", "severity": "minor", "disposition": "fixed",
                          "reviews": {"astra": "accepted", "fable": "re-raised",
                                      "flash": "accepted"}}])
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path, contents=signed)
    code, out = attest(capsys, root, register, extra=("--substitute", "astra=fable"))
    assert code == 1 and out["error"] == "register-not-converged" and out["entries"] == ["F1"]


def test_attest_refuses_malformed_or_collapsing_substitutes(capsys, root, tmp_path):
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path)
    for extra in (
        ("--substitute", "fable"),                                   # no SLOT=
        ("--substitute", "astra="),                                  # no reviewer
        ("--substitute", "bob=fable"),                               # no such slot
        ("--substitute", "astra=fable", "--substitute", "astra=x"),  # slot twice
        # One reviewer holding both slots would sign the register with a single
        # perspective, which is what the two-reviewer rule exists to prevent.
        ("--substitute", "astra=flash"),
        ("--substitute", "astra=fable", "--substitute", "flash=fable"),
    ):
        code, out = attest(capsys, root, register, extra=extra)
        assert code == 1 and out["error"] == "bad-substitute", extra


def test_an_attestation_written_before_substitution_existed_still_loads(root):
    # `read_attestation` rebuilds from JSON; an older file has no `reviewers`
    # key and was, by construction, signed by the named roster.
    ctx = Ctx(Paths(root), run_id="run-test")
    ctx.run_dir.mkdir(parents=True, exist_ok=True)
    legacy = {"issue": 8, "B": "B", "H": "H", "final_head": "F", "register_hash": "r",
              "gates": {}, "body_hash": "b", "register_path": "x"}
    write_json(ctx, ctx.run_dir / "attestation.json", legacy)
    assert mergeops.read_attestation(ctx).reviewers == {"astra": "astra", "flash": "flash"}


def test_attest_binds_the_register_path_and_merge_refuses_a_changed_register(capsys, repo_pair, tmp_path):
    # I6: hashing the register at attest time only helps if something re-checks
    # it. A register edited between the attestation and the merge -- a
    # `deferred` quietly flipped to `fixed`, say -- must stop the merge. The
    # merge resolves the attestation's relative path against the claim's
    # worktree to find the file again.
    _, clone = repo_pair
    write_ci_workflow(clone)
    ctx = Ctx(Paths(clone), run_id="run-test")
    body = json.loads(ISSUES)[0][0]["body"]
    register = claimed_register(ctx, tmp_path, body_hash=mergeops.body_hash(body))
    B = Git(clone).rev("origin/master")
    code, out = run(capsys, clone, "attest", "--issue", "8", "--B", B, "--H", B, "--final-head", B,
                     "--register", str(register), "--gates", GREEN_GATES,
                     "--body-hash", mergeops.body_hash(body), gh_run=FakeRunner())
    assert code == 0
    assert json.loads((ctx.run_dir / "attestation.json").read_text())["register_path"] == ".agent/issue-8/findings.json"
    register.write_text(json.dumps([{"id": "F1", "severity": "blocking", "disposition": "deferred",
                                     "reviews": {"astra": "accepted", "flash": "accepted"}}]))
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    gh = FakeRunner({**READS,
                     f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
                         {"headRefOid": B, "statusCheckRollup": [
                             {"workflowName": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"}]})})
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh)
    assert code == 1 and "register-changed" in out["reasons"]
    assert not any(c.startswith("pr merge") for c in gh.calls)


BOTH = {"astra": "accepted", "flash": "accepted"}


def closes(eid, kind, key, *, disposition="fixed", severity="important"):
    """A register entry whose only remaining work is a post-panel record (P2)."""
    return {"id": eid, "severity": severity, "disposition": disposition, "reviews": BOTH,
            "closes_by": {"kind": kind, "key": key}}


def test_attest_accepts_closes_by_entries_whose_record_is_attested(capsys, root, tmp_path):
    # #190 final r2: "gate results empty" / "B/H empty" could only close after the
    # panel, so a reviewer who followed the rule re-raised them and forced a round.
    entries = [closes("S1-3", "gate", "cdo-gate"), closes("S2-2", "attest", "final_head"),
               closes("F1-7", "ci-wait", "ci-wait")]
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path, contents=json.dumps(entries))
    code, out = attest(capsys, root, register)
    assert code == 0, out
    att = json.loads(Path(out["path"]).read_text())
    assert att["closed_by"] == {e["id"]: e["closes_by"] for e in entries}


@pytest.mark.parametrize("entry", [
    closes("X", "gate", "bench"),                  # no such attested gate
    closes("X", "gate", "d4"),                     # attested GREEN, but not a required gate
    closes("X", "gate", "d5"),                     # attested red, and not a required gate
    closes("X", "attest", "base"),                 # not an attested SHA field
    closes("X", "ci-wait", "ci"),                  # ci-wait names only its own record
    closes("X", "later", "ci-steps-all"),          # unknown kind
    closes("X", "gate", ""),                       # empty key
    closes("X", "gate", "cdo-gate", disposition="refuted"),          # closes_by means `fixed`
    closes("X", "gate", "cdo-gate", disposition="deferred", severity="minor"),
    {"id": "X", "severity": "minor", "disposition": "fixed", "reviews": BOTH, "closes_by": "cdo-gate"},
    {"id": "X", "severity": "minor", "disposition": "fixed", "reviews": BOTH, "closes_by": {"kind": "gate"}},
], ids=lambda e: json.dumps(e.get("closes_by")) + e["disposition"])
def test_attest_refuses_a_closes_by_that_names_no_green_record(capsys, root, tmp_path, entry):
    # `d4` is green in the blob: a key `attest` does not itself require is still refused.
    gates = json.dumps({**json.loads(GREEN_GATES), "d4": 0, "d5": 1})
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path, contents=json.dumps([entry]))
    code, out = attest(capsys, root, register, gates=gates)
    assert code == 1 and out["error"] == "register-not-converged" and out["entries"] == ["X"], out


def ci_wait_gate(capsys, repo_pair, tmp_path):
    """An attested register with one `closes_by: ci-wait` entry; returns
    (clone, ctx, B, reasons()) where reasons() runs merge-gate on PR 12."""
    _, clone = repo_pair
    write_ci_workflow(clone)
    ctx = Ctx(Paths(clone), run_id="run-test")
    body = json.loads(ISSUES)[0][0]["body"]
    register = claimed_register(ctx, tmp_path, body_hash=mergeops.body_hash(body),
                                contents=json.dumps([closes("F1-7", "ci-wait", "ci-wait")]))
    B = Git(clone).rev("origin/master")
    code, out = run(capsys, clone, "attest", "--issue", "8", "--B", B, "--H", B, "--final-head", B,
                     "--register", str(register), "--gates", GREEN_GATES,
                     "--body-hash", mergeops.body_hash(body), gh_run=FakeRunner())
    assert code == 0, out
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft":
                     json.dumps({"headRefOid": B, "statusCheckRollup": rollup()})})

    def reasons():
        _, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
        return out["reasons"]

    return clone, ctx, B, reasons


def ci_wait_record(final_head, **over):
    """A green, supervised `ci-wait` record of the real command, as `run` writes it."""
    return {"name": "ci-wait", "argv": ["gh", "pr", "checks", "12", "--watch"], "exit_code": 0,
            "timed_out": False, "supervised": True, "head": final_head, **over}


@pytest.mark.parametrize("cdo", [None, 1, True])
def test_a_docs_only_attest_does_not_let_closes_by_cite_the_cdo_gate_it_skipped(capsys, root, tmp_path, cdo):
    # `--docs-only` stops `_gate_failures` asking for `cdo-gate`, so only the closes_by
    # check itself can see that the gate an entry cites is absent or red.
    gates = {"ci-steps-all": 0, "check-goldens-coverage": 0, "check-goldens": 0}
    if cdo is not None:
        gates["cdo-gate"] = cdo
    register = claimed_register(Ctx(Paths(root), run_id="run-test"), tmp_path,
                                contents=json.dumps([closes("X", "gate", "cdo-gate")]))
    code, out = attest(capsys, root, register, gates=json.dumps(gates), extra=("--docs-only",))
    assert code == 1 and out["error"] == "register-not-converged" and out["entries"] == ["X"], out


def test_merge_gate_holds_a_ci_wait_entry_until_a_green_ci_wait_record_on_the_final_head(capsys, repo_pair, tmp_path):
    from agentflow import identity
    clone, ctx, B, reasons = ci_wait_gate(capsys, repo_pair, tmp_path)
    assert reasons() == ["closes-by-unmet:F1-7"]                       # no record yet
    for bad in (dict(exit_code=1),                                     # red
                dict(head="e" * 40),                                   # another head
                dict(timed_out=True), dict(timed_out=None),            # killed, or unknown
                dict(supervised=False),                                # not under the lock
                dict(argv=["true"]),                                   # not the CI command
                dict(argv=["gh", "pr", "checks", "13", "--watch"]),    # another PR
                dict(argv=None)):
        identity.append_record(ctx, ci_wait_record(B, **bad))
        assert reasons() == ["closes-by-unmet:F1-7"], bad
    identity.append_record(ctx, ci_wait_record(B))
    assert reasons() == []


def test_a_ci_wait_run_of_some_other_command_does_not_close_the_entry(capsys, repo_pair, tmp_path):
    # Through the real `run`: supervised, exit 0, on the final head, and still no CI verdict.
    import time
    from agentflow import budget
    clone, ctx, B, reasons = ci_wait_gate(capsys, repo_pair, tmp_path)
    lock.acquire(ctx, 8, "s", 1)
    budget.init(ctx, claimed_at=time.time())
    code, out = run(capsys, clone, "run", "--name", "ci-wait", "--timeout", "1", "--",
                    sys.executable, "-c", "pass", gh_run=FakeRunner())
    assert code == 0 and out["supervised"] is True, out
    assert reasons() == ["closes-by-unmet:F1-7"]


def prose_rule_repo(repo_pair):
    """`master` (B) carries a CHANGELOG-only commit; branch `feat` then carries one
    commit per fix shape; `other` carries a CHANGELOG-only commit off the branch.
    Returns (clone, B, final_head, {shape: sha})."""
    _, clone = repo_pair
    g = Git(clone)
    on_master = commit_file(clone, "CHANGELOG.md", "master\n", "master prose")
    assert g.push("origin", "master")
    B = g.rev("HEAD")
    g._run("checkout", "-q", "-b", "other")
    off_branch = commit_file(clone, "CHANGELOG.md", "other\n", "prose on another branch")
    g._run("checkout", "-q", "-b", "feat", B)
    shas = {"on_master": on_master, "off_branch": off_branch}
    shas["code"] = commit_file(clone, "src/x.rs", "x\n", "code fix")
    shas["changelog"] = commit_file(clone, "CHANGELOG.md", "feat\n", "wording")
    (clone / "docs").mkdir(exist_ok=True)
    (clone / "docs" / "a.md").write_text("a\n")
    _git_raw(clone, "add", "docs/a.md")  # staged, so the next commit carries both files
    shas["docs_and_ledger"] = commit_file(clone, ".agent/issue-8/ledger.md", "l\n", "docs + ledger")
    shas["tests_md"] = commit_file(clone, "tests/prsummary.md", "t\n", "a golden the tests read")
    # A code file moved into docs/: rename detection would list only docs/x.md.
    _git_raw(clone, "mv", "src/x.rs", "docs/x.md")
    _git_raw(clone, "commit", "-q", "-m", "move code into docs")
    shas["renamed"] = g.rev("HEAD")
    # A commit that touches nothing: "every changed path is prose" holds vacuously.
    _git_raw(clone, "commit", "-q", "--allow-empty", "-m", "fixes nothing")
    shas["empty"] = g.rev("HEAD")
    return clone, B, g.rev("HEAD"), shas


def rule_entry(fix_commit, *, severity="minor", reviews=None, source="astra", **extra):
    """Raised by `source` (astra by default), so astra's slot is `accepted` and flash's
    the rule mark."""
    return {"id": "F2-1", "source": source, "severity": severity, "disposition": "fixed",
            "fix_commit": fix_commit,
            "reviews": reviews or {"astra": "accepted", "flash": "accepted-by-rule"}, **extra}


def attest_rule(capsys, clone, tmp_path, B, head, entry, extra=()):
    ctx = Ctx(Paths(clone), run_id="run-test")
    register = claimed_register(ctx, tmp_path, contents=json.dumps([entry]))
    return attest(capsys, clone, register, B=B, H=head, final_head=head, extra=extra)


def test_attest_accepts_a_raiser_only_mark_on_a_minor_prose_only_fix(capsys, repo_pair, tmp_path):
    # P3: #111 r3, #190 r3 and #160 r3 each existed only to confirm a wording fix.
    clone, B, head, shas = prose_rule_repo(repo_pair)
    for shape in ("changelog", "docs_and_ledger"):
        code, out = attest_rule(capsys, clone, tmp_path, B, head, rule_entry(shas[shape]))
        assert code == 0, (shape, out)
    # The other direction: flash raised it, so flash accepts and astra gets the rule mark.
    by_flash = rule_entry(shas["changelog"], source="flash",
                          reviews={"astra": "accepted-by-rule", "flash": "accepted"})
    assert attest_rule(capsys, clone, tmp_path, B, head, by_flash)[0] == 0


def test_a_raiser_only_mark_follows_a_substituted_slot(capsys, repo_pair, tmp_path):
    # Under `--substitute astra=sol` the marks sit under `sol`, and so does the source.
    clone, B, head, shas = prose_rule_repo(repo_pair)
    entry = rule_entry(shas["changelog"], source="sol",
                       reviews={"sol": "accepted", "flash": "accepted-by-rule"})
    code, out = attest_rule(capsys, clone, tmp_path, B, head, entry, extra=("--substitute", "astra=sol"))
    assert code == 0, out
    # Without the substitution `sol` fills no slot: no carve-out, and astra's slot is empty.
    code, out = attest_rule(capsys, clone, tmp_path, B, head, entry)
    assert code == 1 and out["entries"] == ["F2-1"], out


@pytest.mark.parametrize("shape, entry_kw", [
    ("code", {}),                                                      # a code fix
    ("tests_md", {}),                                                  # tests/**/*.md is read by tests
    ("renamed", {}),                                                   # src/x.rs -> docs/x.md
    ("empty", {}),                                                     # an empty commit fixes nothing
    ("on_master", {}),                                                 # not this branch's commit
    ("off_branch", {}),                                                # not on base..final_head
    ("changelog", {"severity": "important"}),                          # blocking
    ("changelog", {"blocking": True}),                                 # blocking by flag
    ("changelog", {"reviews": {"astra": "accepted-by-rule", "flash": "accepted-by-rule"}}),
    ("changelog", {"reviews": {"astra": "re-raised", "flash": "accepted-by-rule"}}),  # an objection
    # The rule mark in the RAISER's slot: astra raised it, so astra must accept it.
    ("changelog", {"reviews": {"astra": "accepted-by-rule", "flash": "accepted"}}),
    # Raised by no rostered reviewer: both must accept, as before.
    ("changelog", {"source": "code-review"}),
    ("changelog", {"source": "sonnet-task-review"}),
    ("changelog", {"source": "proof-T1"}),
    ("changelog", {"source": None}),
    (None, {}),                                                        # no fix_commit
    ("short", {}),                                                     # not a full SHA
])
def test_attest_refuses_a_raiser_only_mark_anywhere_else(capsys, repo_pair, tmp_path, shape, entry_kw):
    clone, B, head, shas = prose_rule_repo(repo_pair)
    fix = {None: None, "short": shas["changelog"][:8]}.get(shape, shas.get(shape))
    code, out = attest_rule(capsys, clone, tmp_path, B, head, rule_entry(fix, **entry_kw))
    assert code == 1 and out["error"] == "register-not-converged" and out["entries"] == ["F2-1"], out


def write_ci_workflow(clone, name="CI"):
    (clone / ".github" / "workflows").mkdir(parents=True, exist_ok=True)
    (clone / ".github" / "workflows" / "ci.yml").write_text(f"name: {name}\n\non:\n  pull_request:\n")


def rollup(*, workflow="CI", conclusion="SUCCESS"):
    return [{"workflowName": workflow, "status": "COMPLETED", "conclusion": conclusion}]


def test_merge_gate_detects_moved_head(capsys, repo_pair):
    # I10(a): a PR head that moved since the attestation must be caught.
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head="cafebabe" * 5,
                                register_hash="r" * 40, gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({
        **READS,
        f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
            {"headRefOid": "deadbeef" * 5, "statusCheckRollup": rollup()}),
    })
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["reasons"] == ["head-moved"]


def test_merge_gate_names_a_draft_pr(capsys, repo_pair):
    # The draft PR is opened before the final panel so CI runs during it
    # (`/issue` step 9); GitHub refuses to merge a draft, so the gate says so
    # by name instead of letting `gh pr merge` fail.
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup(), "isDraft": True})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["reasons"] == ["pr-draft"] and out["ci_green"] is True


def test_merge_gate_is_not_green_without_the_ci_workflows_own_checks(capsys, repo_pair):
    # I7: an unrelated SUCCESS check (a bot, a second workflow) that lands
    # before ci.yml's jobs register must not read as green.
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup(workflow="Dependabot")})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["ci_green"] is False and out["reasons"] == ["ci-not-green"]


def test_merge_gate_reports_ci_workflow_unknown_when_the_workflow_file_is_missing(capsys, repo_pair):
    # I7: the required-workflow name is read from .github/workflows/ci.yml. If
    # that file (or its `name:`) is gone, the gate has no idea what to require
    # -- which is a refusal, not a green.
    _, clone = repo_pair
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup()})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 1 and out["ci_green"] is False and out["reasons"] == ["ci-workflow-unknown"]


def test_merge_gate_passes_with_the_ci_workflows_checks_green(capsys, repo_pair):
    _, clone = repo_pair
    write_ci_workflow(clone)
    B = Git(clone).rev("master")
    body_hash = mergeops.body_hash(json.loads(ISSUES)[0][0]["body"])
    att = mergeops.Attestation(issue=8, B=B, H=B, final_head=B, register_hash="r" * 40,
                               gates={}, body_hash=body_hash)
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-test"), att)
    gh = FakeRunner({**READS, f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
        {"headRefOid": B, "statusCheckRollup": rollup()})})
    code, out = run(capsys, clone, "merge-gate", "--pr", "12", gh_run=gh)
    assert code == 0 and out["ci_green"] is True and out["reasons"] == []


def test_merge_records_what_it_merged_for_post_merge_to_verify(capsys, repo_pair, tmp_path):
    """Nothing read `merge.json` before this change, so nothing pinned what
    `merge` put in it. It is now the only thing that can aim `post-merge`, so
    its exact contents are a contract: the ISSUE comes from the attestation
    (merge has no `--issue`), the SHA is the squash commit GitHub reported (not
    the PR head), and the run id is this run's."""
    _, clone = repo_pair
    write_ci_workflow(clone)
    ctx = Ctx(Paths(clone), run_id="run-test")
    body = json.loads(ISSUES)[0][0]["body"]
    register = claimed_register(ctx, tmp_path, body_hash=mergeops.body_hash(body))
    B = Git(clone).rev("origin/master")
    code, _ = run(capsys, clone, "attest", "--issue", "8", "--B", B, "--H", B, "--final-head", B,
                  "--register", str(register), "--gates", GREEN_GATES,
                  "--body-hash", mergeops.body_hash(body), gh_run=FakeRunner())
    assert code == 0
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    merged_oid = "f" * 40
    # gh.py fake keys are exact argv: `merge_pr` (gh.py:182) does NOT pass
    # `--repo`, while `pr_view` (gh.py:132) does. Getting either wrong yields a
    # 404 fake and a confusing GhError rather than a silent pass.
    gh = FakeRunner({**READS,
                     f"pr view 12 --repo {REPO} --json headRefOid,statusCheckRollup,isDraft": json.dumps(
                         {"headRefOid": B, "statusCheckRollup": rollup()}),
                     f"pr merge 12 --squash --match-head-commit {B}": "",
                     f"pr view 12 --repo {REPO} --json mergeCommit": json.dumps(
                         {"mergeCommit": {"oid": merged_oid}})})
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh)
    assert code == 0 and out["merge_sha"] == merged_oid
    assert json.loads((ctx.run_dir / "merge.json").read_text()) == {
        "issue": 8, "pr": 12, "merge_sha": merged_oid, "run_id": "run-test", "source": "merge"}


# ---- the merge lane (parallel-issues design, commit 12) ---------------------

def _attested_run(clone, run_id, issue, base, **lock_kw):
    """A run holding `issue`'s lease with an attestation on `base`, stated by
    assignment (no register path, so only base/head/body/CI are re-checked)."""
    c = Ctx(Paths(clone), run_id=run_id, **lock_kw)
    lock.acquire(c, issue, "s", 1)
    body = json.loads(ISSUES)[0][issue - 8]["body"]
    mergeops.write_attestation(c, mergeops.Attestation(
        issue=issue, B=base, H=base, final_head=base, register_hash="r" * 40, gates={},
        body_hash=mergeops.body_hash(body)))
    return c


def _lane_gh(prs, *, conclusion="SUCCESS"):
    """gh fakes for `merge-gate`/`merge` of each `pr -> (head, merged oid)`."""
    responses = dict(CLAIM_GH)
    for pr, (head, oid) in prs.items():
        responses[f"pr view {pr} --repo {REPO} --json headRefOid,statusCheckRollup,isDraft"] = json.dumps(
            {"headRefOid": head, "statusCheckRollup": rollup(conclusion=conclusion)})
        responses[f"pr merge {pr} --squash --match-head-commit {head}"] = ""
        responses[f"pr view {pr} --repo {REPO} --json mergeCommit"] = json.dumps({"mergeCommit": {"oid": oid}})
    return FakeRunner(responses)


def _lane(clone):
    p = clone / ".agent" / "merge-lane.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def _noop_gates(monkeypatch):
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))


def test_a_second_merge_waits_for_the_first_post_merge_then_sees_the_base_move(
        capsys, repo_pair, monkeypatch):
    """Design tests 21 and 22. A and B both pass `merge-gate` on base B0. A
    merges and so takes the merge lane: B's `merge-gate` now reports
    `merge-lane-busy` and B's `merge` refuses it without ever calling `pr
    merge` (today B would merge onto a base it never tested). A's clean
    `post-merge` frees the lane; B's `merge` then sees `base-moved`, its real
    reason, still without merging."""
    _, clone = repo_pair
    _noop_gates(monkeypatch)
    write_ci_workflow(clone)
    _slots(clone, 2)
    base = Git(clone).rev("origin/master")
    _attested_run(clone, "run-a", 8, base)
    _attested_run(clone, "run-b", 9, base)
    squash = commit_file(clone, "src/a.rs", "fn a() {}\n", "squash merge of #8")   # what GitHub will push
    gh = _lane_gh({12: (base, squash), 13: (base, "b" * 40)})
    for rid, pr in (("run-a", 12), ("run-b", 13)):
        code, out = run(capsys, clone, "merge-gate", "--pr", str(pr), gh_run=gh, run_id=rid)
        assert code == 0 and out["reasons"] == [], (rid, out)
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh, run_id="run-a")
    assert code == 0 and out["merge_sha"] == squash, out
    assert _lane(clone)["run_id"] == "run-a" and _lane(clone)["merge_sha"] == squash
    assert Git(clone).push("origin", "master")                                     # GitHub merged it
    code, out = run(capsys, clone, "merge-gate", "--pr", "13", gh_run=gh, run_id="run-b")
    assert code == 1 and "merge-lane-busy" in out["reasons"], out
    code, out = run(capsys, clone, "merge", "--pr", "13", gh_run=gh, run_id="run-b")
    assert code == 1 and out["error"] == "merge-lane-busy", out
    assert not any(c.startswith("pr merge 13") for c in gh.calls)
    code, out = run(capsys, clone, "status", gh_run=FakeRunner(), run_id="run-b")
    assert out["merge_lane"]["run_id"] == "run-a" and out["merge_lane"]["state"] == "busy", out
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", squash,
                    gh_run=FakeRunner(), run_id="run-a")
    assert code == 0 and out["ok"] is True, out
    assert _lane(clone) is None
    code, out = run(capsys, clone, "merge", "--pr", "13", gh_run=gh, run_id="run-b")
    assert code == 1 and out["reasons"] == ["base-moved"], out
    assert not any(c.startswith("pr merge 13") for c in gh.calls)
    assert _lane(clone) is None                                       # a refused merge holds no lane


def test_a_merge_refused_by_its_gate_releases_the_lane_and_merge_needs_a_lease(capsys, repo_pair):
    """Design tests 23 and 25. A merge the gate refuses (`ci-not-green`)
    takes the lane and gives it back, so a sibling is not blocked by a merge
    that never happened. `merge` is now fenced: without a lease it is a
    `FenceError` before any gh call, with no lane. (Before, `gh`'s own fence
    on mutating calls refused only at `pr merge`, after the gate's reads.)"""
    _, clone = repo_pair
    write_ci_workflow(clone)
    base = Git(clone).rev("origin/master")
    _attested_run(clone, "run-a", 8, base)
    gh = _lane_gh({12: (base, "a" * 40)}, conclusion="FAILURE")
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh, run_id="run-a")
    assert code == 1 and out["reasons"] == ["ci-not-green"], out
    assert _lane(clone) is None
    mergeops.write_attestation(Ctx(Paths(clone), run_id="run-lost"),
                               mergeops.read_attestation(Ctx(Paths(clone), run_id="run-a")))
    gh = _lane_gh({12: (base, "a" * 40)})
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh, run_id="run-lost")
    assert code == 1 and "FenceError" in out["error"], out
    assert _lane(clone) is None and gh.calls == [], gh.calls
    # GitHub merged, then the merge commit could not be read: the merge DID
    # happen, so the lane stays for the post-merge or the recovery.
    gh = _lane_gh({12: (base, None)})
    code, out = run(capsys, clone, "merge", "--pr", "12", gh_run=gh, run_id="run-a")
    assert code == 1 and "merge-sha-unavailable" in out["error"], out
    assert any(c.startswith("pr merge 12") for c in gh.calls)
    assert _lane(clone)["run_id"] == "run-a"


def test_a_post_merge_stop_keeps_the_lane_until_finish(capsys, repo_pair, monkeypatch, tmp_path):
    """Design test 24. A post-merge that stops (here a gate killed at its
    timeout: HALT only) keeps the lane: no second merge may land on a master
    nothing verified. `finish --outcome regressed` releases it."""
    _, clone = repo_pair
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    monkeypatch.setattr(cli, "gates", lambda: [("slow", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    write_ci_workflow(clone)
    base = Git(clone).rev("origin/master")
    _attested_run(clone, "run-a", 8, base)
    squash = commit_file(clone, "src/a.rs", "fn a() {}\n", "squash merge of #8")
    gh = _lane_gh({12: (base, squash)})
    assert run(capsys, clone, "merge", "--pr", "12", gh_run=gh, run_id="run-a")[0] == 0
    assert Git(clone).push("origin", "master")

    def killed(ctx_, cmd, *, cwd, log_path, timeout_s, beat=None, beat_every=60.0, env=None):
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        return supervise.Result(exit_code=124, log_path=log_path, timed_out=True, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", killed)
    stop_gh = FakeRunner({f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
                              json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
                          "issue edit 8 --add-label *": "", "issue edit 8 --remove-label agent-working": "",
                          "issue comment 8 *": ""})
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", squash,
                    gh_run=stop_gh, run_id="run-a")
    assert code == 1 and out["halt"]["reason"] == "gate-timed-out", out
    assert _lane(clone)["run_id"] == "run-a"
    code, out = run(capsys, clone, "finish", "--issue", "8", "--outcome", "regressed",
                    gh_run=FakeRunner(), run_id="run-a")
    assert code == 0, out
    assert _lane(clone) is None


def test_post_merge_refuses_while_another_live_run_holds_the_lane(capsys, repo_pair):
    """`post-merge` must hold the lane. Another LIVE run holding it means
    two verifications at once on one master: refused before any git call,
    with no incident opened. A FREE lane is taken (a merge made before the
    lane existed still verifies)."""
    _, clone = repo_pair
    _slots(clone, 2)
    merge_sha = commit_file(clone, "src/b.rs", "fn b() {}\n", "squash merge of #9")
    assert Git(clone).push("origin", "master")
    _attested_run(clone, "run-a", 8, merge_sha)
    _attested_run(clone, "run-b", 9, merge_sha)
    record_merge(clone, merge_sha, issue=9, run_id="run-b")
    write_json(Ctx(Paths(clone)), clone / ".agent" / "merge-lane.json",
               {"run_id": "run-a", "issue": 8, "pr": 12, "merge_sha": None, "acquired": 1.0})
    code, out = run(capsys, clone, "post-merge", "--issue", "9", "--merge-sha", merge_sha,
                    gh_run=FakeRunner(readonly=True), run_id="run-b")
    assert code == 1 and out["error"] == "merge-lane-busy", out
    assert not (clone / ".agent" / "incidents.json").exists()


def test_an_orphaned_lane_refuses_merges_and_recover_hands_it_to_the_follow_through(
        capsys, repo_pair, monkeypatch):
    """Design test 26. The lane's owner died after merging: its lease is
    stale, so the lane is ORPHANED. A sibling's `merge` refuses
    `merge-lane-orphaned` (its `merge-gate` says so too). `recover` of that
    issue hands the lane to the follow-through with the lease, and the
    follow-through's `post-merge` is accepted and frees it."""
    _, clone = repo_pair
    _noop_gates(monkeypatch)
    write_ci_workflow(clone)
    _slots(clone, 2)
    merged = commit_file(clone, "src/thing.rs", "fn main() {}\n", "squash merge of #8")
    assert Git(clone).push("origin", "master")
    old = lock.acquire(Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0), 8, "s", 1)
    write_json(Ctx(Paths(clone)), clone / ".agent" / "merge-lane.json",
               {"run_id": "old-run", "issue": 8, "pr": 3, "merge_sha": merged, "acquired": 1_000_000.0})
    now = old.heartbeat + 4000
    monkeypatch.setenv("AGENTFLOW_NOW", str(now))
    _attested_run(clone, "run-b", 9, merged, now=lambda: now)
    gh = _lane_gh({13: (merged, "b" * 40)})
    code, out = run(capsys, clone, "merge-gate", "--pr", "13", gh_run=gh, run_id="run-b")
    assert code == 1 and "merge-lane-orphaned" in out["reasons"], out
    code, out = run(capsys, clone, "merge", "--pr", "13", gh_run=gh, run_id="run-b")
    assert code == 1 and out["error"] == "merge-lane-orphaned", out
    assert not any(c.startswith("pr merge") for c in gh.calls)
    rgh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1",
                                                "headRefOid": "h", "mergeCommit": {"oid": merged},
                                                "mergedAt": "x"}])})
    code, out = run(capsys, clone, "recover", "--issue", "8", gh_run=rgh, run_id="run-new")
    assert code == 0 and out["action"] == "merged-needs-post-merge", out
    assert _lane(clone)["run_id"] == "run-new" and _lane(clone)["issue"] == 8
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merged,
                    gh_run=FakeRunner(), run_id="run-new")
    assert code == 0 and out["ok"] is True, out
    assert _lane(clone) is None


def test_a_crashed_run_that_held_the_lane_frees_it_on_recovery(capsys, repo_pair, monkeypatch):
    """`recover`'s `blocked-crashed` path: the dead run merged nothing (its PR
    is open), so it has nothing to verify and the lane it named is freed."""
    _, clone = repo_pair
    old = lock.acquire(Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0), 8, "s", 1)
    write_json(Ctx(Paths(clone)), clone / ".agent" / "merge-lane.json",
               {"run_id": "old-run", "issue": 8, "pr": 3, "merge_sha": None, "acquired": 1_000_000.0})
    monkeypatch.setenv("AGENTFLOW_NOW", str(old.heartbeat + 4000))
    gh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "OPEN", "headRefName": "issue/8-x-a1",
                                               "headRefOid": "h", "mergeCommit": None, "mergedAt": None}]),
                     "issue comment 8 *": "", "issue edit 8 --add-label agent-blocked": "",
                     "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0 and out["action"] == "blocked-crashed", out
    assert _lane(clone) is None


def _git_raw(cwd, *args):
    """Raw git, so a fixture can state a precondition without going through
    the code under test."""
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, check=True)


# ---- provenance helpers for the `post-merge` tests -------------------------
# Every CLI-level `post-merge` test now needs a merge record: the command
# refuses, before its first git call, a `--merge-sha` this run never merged.

REGRESSED_GH = {"issue edit 8 --add-label agent-regressed": "",
                "issue edit 8 --remove-label agent-working": "",
                "issue comment 8 *": "", "issue reopen 8": ""}


def write_merge_json(clone, obj, *, run_id="run-test"):
    """Put a literal object at `<run dir>/merge.json`. The tests below state
    `post-merge`'s precondition BY ASSIGNMENT rather than by driving `merge`
    to produce it, so they survive any change to how the record is minted --
    and so a test can state a shape production code can no longer produce (a
    foreign run id, the pre-provenance field set)."""
    c = Ctx(Paths(clone), run_id=run_id)
    write_json(c, c.run_dir / "merge.json", obj)


def record_merge(clone, merge_sha, *, issue=8, pr=12, run_id="run-test", source="merge"):
    write_merge_json(clone, {"issue": issue, "pr": pr, "merge_sha": merge_sha,
                             "run_id": run_id, "source": source}, run_id=run_id)


def unrecorded_tip(clone, monkeypatch):
    """Every precondition the revert-and-push path needs EXCEPT provenance,
    hand-stated: master's tip is a commit the flow never merged (a human's),
    the gates are red on it, and the revert would turn them green.
    `red_until_reverted` fails while `human.txt` exists, so
    `post_merge_failure`'s rerun-on-revert -- the last thing standing between
    the revert and `git push` -- WOULD pass. With the guard removed, this
    invocation reaches the push."""
    # The probe is RELATIVE to the gate's cwd on purpose. Today that is
    # `ctx.paths.root`; if the gates ever move to a disposable worktree, the
    # verification tree (checked out at `tip`) still carries `human.txt` and
    # the revert tree does not, so the red/green contrast survives the move.
    # An absolute path into the shared clone would NOT survive it.
    red_until_reverted = [sys.executable, "-c",
                          "import os, sys; sys.exit(1 if os.path.exists('human.txt') else 0)"]
    monkeypatch.setattr(cli, "gates", lambda: [("red", red_until_reverted, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    # `post_merge_failure`'s rerun also runs `scripts/ci-steps test`, which is
    # NOT one of the two patched gate lists. Commit a stub that exits 0 so the
    # rerun's verdict is decided by `red_until_reverted` alone -- otherwise the
    # revert would always fail its rerun and the push could never be reached,
    # which would make the discrimination proof below prove nothing.
    commit_file(clone, "scripts/ci-steps", "#!/bin/sh\nexit 0\n", "ci-steps stub for the gate rerun")
    tip = commit_file(clone, "human.txt", "a human's commit\n", "human work the flow never merged")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    return tip


def test_post_merge_refuses_a_commit_this_run_did_not_merge(capsys, repo_pair, monkeypatch):
    """The 2026-09-14 incident's worst case. `--merge-sha` names the CURRENT
    tip of master -- a human's commit -- while this run's own merge record
    names a different SHA. The refusal must come before the first git call:
    no gate runs, no gh call is made, nothing is checked out."""
    _, clone = repo_pair
    tip = unrecorded_tip(clone, monkeypatch)
    record_merge(clone, "d" * 40)   # precondition: this run merged SOMETHING ELSE
    gh = FakeRunner(REGRESSED_GH)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", tip, gh_run=gh)
    assert code == 1
    assert out.get("error") == "post-merge does not match this run's merge"
    assert out["recorded"]["merge_sha"] == "d" * 40
    assert out["requested"]["merge_sha"] == tip
    assert gh.calls == []                                                   # no label, comment or reopen
    assert not (clone / ".agent" / "runs" / "run-test" / "logs").exists()   # no gate ran at all


def test_post_merge_never_reverts_or_pushes_a_commit_this_run_did_not_merge(capsys, repo_pair, monkeypatch):
    """Split from the payload test on purpose: this one names the CONSEQUENCE.
    Without the guard, this exact invocation reverts the human's commit and
    pushes that revert to origin/master -- on 2026-09-14 only the unrelated
    master-not-ff guard stopped it, and it would not have stopped it had local
    master been in sync, as it is here."""
    _, clone = repo_pair
    g = Git(clone)
    tip = unrecorded_tip(clone, monkeypatch)
    record_merge(clone, "d" * 40)
    run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", tip, gh_run=FakeRunner(REGRESSED_GH))
    g.fetch()
    assert g.rev("origin/master") == tip            # nothing pushed
    assert g.rev("master") == tip                   # nothing reverted locally
    assert (clone / "human.txt").exists()           # the human's work is still there
    assert g.branch() == "master"                   # not left detached
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is None


def test_post_merge_refuses_when_this_run_recorded_no_merge(capsys, repo_pair, monkeypatch):
    """`post-merge` invoked by a run that never merged anything. The absence of
    the record IS the hand-stated precondition -- nothing is written."""
    _, clone = repo_pair
    g = Git(clone)
    tip = unrecorded_tip(clone, monkeypatch)
    gh = FakeRunner(REGRESSED_GH)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", tip, gh_run=gh)
    assert code == 1 and out.get("error") == "no merge record for this run"
    assert out["requested"] == {"issue": 8, "merge_sha": tip, "run_id": "run-test"}
    g.fetch()
    assert g.rev("origin/master") == tip and g.rev("master") == tip
    assert gh.calls == []


def test_post_merge_refuses_a_merge_record_minted_by_another_run(capsys, repo_pair, monkeypatch):
    """Right issue, right SHA, wrong run. A run directory is an ordinary
    directory and `recovery.retain` copies whole ones around, so the record has
    to say which run minted it and that has to be checked -- position in the
    filesystem is not proof. Stated by assignment: production code cannot
    produce this file, which is exactly why the test writes it literally."""
    _, clone = repo_pair
    g = Git(clone)
    tip = unrecorded_tip(clone, monkeypatch)
    write_merge_json(clone, {"issue": 8, "pr": 12, "merge_sha": tip,
                             "run_id": "some-other-run", "source": "merge"})
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", tip,
                    gh_run=FakeRunner(REGRESSED_GH))
    assert code == 1 and out.get("error") == "post-merge does not match this run's merge"
    assert out["recorded"]["run_id"] == "some-other-run"
    g.fetch()
    assert g.rev("origin/master") == tip


def test_post_merge_refuses_a_merge_record_that_names_a_different_issue(capsys, repo_pair, monkeypatch):
    """Right run, right SHA, WRONG ISSUE -- the one third of the provenance
    guard that had no test. MEASURED IN THE ROUND-1 REVIEW: deleting
    `rec.issue != args.issue` from the condition left all 206 tests green, because every post-merge
    invocation in the suite passed `--issue 8` and `record_merge` defaults to
    `issue=8`, so no test varied the only field the conjunct reads.

    PRECONDITION HAND-STATED so this conjunct is the ONLY thing that can
    refuse: the record names this run and this SHA, and only the `--issue`
    on the argv disagrees. That is the transcription slip the guard's own
    comment names, and `lock.check_fence` cannot catch it -- the fence
    compares run ids and never looks at the issue.

    What the refusal prevents, and why `gh.calls == []` is the assertion that
    matters: the pre-gate `open_incident` records `issue=args.issue`, and a
    red gate then reopens issue 9, stamps `agent-regressed` +
    `agent-revert-blocked` on it and comments the failure there. Because
    `agent-revert-blocked` is in `eligibility.EXCLUDE_LABELS` and
    `cmd_unblock` removes only `agent-blocked`, that unrelated issue is
    skipped by the loop until a human strips the label by hand."""
    _, clone = repo_pair
    g = Git(clone)
    tip = unrecorded_tip(clone, monkeypatch)
    record_merge(clone, tip, issue=8)        # this run merged tip FOR ISSUE 8
    gh = FakeRunner(REGRESSED_GH)
    code, out = run(capsys, clone, "post-merge", "--issue", "9", "--merge-sha", tip, gh_run=gh)
    assert code == 1
    assert out.get("error") == "post-merge does not match this run's merge"
    # Both halves of the payload, because `recorded.issue` was reported as if
    # it had been compared while nothing compared it.
    assert out["recorded"]["issue"] == 8 and out["requested"]["issue"] == 9
    assert out["recorded"]["merge_sha"] == tip and out["requested"]["merge_sha"] == tip
    assert gh.calls == []                                                   # no label, comment or reopen
    assert not (clone / ".agent" / "runs" / "run-test" / "logs").exists()   # no gate ran at all
    # BEFORE the pre-gate `open_incident`, which would otherwise record
    # issue 9 against this SHA and block every later preflight.
    assert not (clone / ".agent" / "incidents.json").exists()
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is None
    g.fetch()
    assert g.rev("origin/master") == tip      # nothing reverted, nothing pushed


def test_post_merge_refuses_a_record_written_before_provenance_existed(capsys, repo_pair, monkeypatch):
    """The bare `{pr, merge_sha}` shape `merge` used to write carries no issue
    and no run, so it cannot prove what it would have to prove. It reads as
    absent rather than as half-trusted on the two fields it happens to have.
    A run that merged under the old code therefore refuses here instead of
    verifying -- deliberate, and the reason the stale-lock recovery path
    re-mints a full record rather than relying on whatever is already there."""
    _, clone = repo_pair
    g = Git(clone)
    tip = unrecorded_tip(clone, monkeypatch)
    write_merge_json(clone, {"pr": 12, "merge_sha": tip})
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", tip,
                    gh_run=FakeRunner(REGRESSED_GH))
    assert code == 1 and out.get("error") == "no merge record for this run"
    g.fetch()
    assert g.rev("origin/master") == tip


def test_post_merge_refuses_a_merge_sha_that_is_not_on_origin_master(capsys, repo_pair, monkeypatch):
    """REACHABILITY, which provenance does not prove. A merge record proves
    only that SOME record in this run dir names this SHA -- and `cmd_recover`
    mints such a record from GitHub's answer about a merged PR, so one can be
    earned by a commit that is not on `master` at all (reverted off it since,
    or -- before `recover_stale` was scoped to the stale lock's own attempt --
    a previous attempt's).

    PRECONDITION HAND-STATED so this check is the ONLY thing that can refuse:
    the commit is on a side branch that was never pushed or merged, and
    `record_merge` names it, so the provenance check PASSES. Gating a commit
    master does not carry verifies nothing, and a red gate there would HALT,
    open a preflight-blocking incident and comment on the issue about a tree
    the loop does not own."""
    _, clone = repo_pair
    g = Git(clone)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    g.out("checkout", "-q", "-b", "side")
    side_sha = commit_file(clone, "side.txt", "never merged\n", "work on a side branch")
    g.out("checkout", "-q", "master")
    # PRECONDITION with raw git: the SHA exists locally and is NOT on the remote.
    assert _git_raw(clone, "cat-file", "-e", side_sha + "^{commit}").returncode == 0
    assert subprocess.run(["git", "-C", str(clone), "merge-base", "--is-ancestor",
                           side_sha, "refs/remotes/origin/master"],
                          capture_output=True, text=True).returncode != 0
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, side_sha)                 # provenance passes by construction
    gh = FakeRunner(REGRESSED_GH)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", side_sha, gh_run=gh)
    assert code == 1
    assert out.get("error") == "merge sha is not on origin/master"
    assert out["merge_sha"] == side_sha
    assert gh.calls == []                                                   # no label, comment or reopen
    assert not (clone / ".agent" / "runs" / "run-test" / "logs").exists()   # no gate ran at all
    # BEFORE the pre-gate `open_incident`: a refused SHA must not leave a
    # record that then blocks every later preflight.
    assert not (clone / ".agent" / "incidents.json").exists()
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is None
    assert list(clone.parent.glob("al-sem-verify-*")) == []


def test_post_merge_happy_path_restores_master_and_reports_ok(capsys, repo_pair, monkeypatch):
    # I10(b): the full post-merge success path -- both gate lists patched to
    # a real, instant no-op child so no actual build/test tooling is needed.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    # The provenance precondition, hand-stated: THIS run merged THIS commit.
    # The old pin drove `post-merge` on an argv-supplied SHA that no `merge` of
    # run-test ever produced -- i.e. the suite's only post-merge happy path was
    # itself an instance of the 2026-09-14 bug, which is why 151 green tests
    # never noticed that nothing read `merge.json`. Everything the test
    # asserted below is unchanged; only the precondition is now stated.
    record_merge(clone, merge_sha)
    # Fix round 2, finding 1: an untracked file -- exactly what the
    # executor's own gates leave behind (log files under .agent/runs,
    # __pycache__, ...) -- must NOT trip the post-gate cleanliness probe.
    # No `.gitignore` accommodation is needed any more: tracked_dirty()
    # ignores untracked paths outright.
    (clone / "untracked-artifact.log").write_text("noise\n")
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert code == 0 and out["ok"] is True and out["revert"] is None
    assert out["halt"] is None                       # present and null, never a dropped key
    assert out["verify_worktree_removed"] is True
    assert out["gates"] == {"noop": 0, "noop-cdo": 0}
    assert Git(clone).branch() == "master"
    assert not Git(clone).tracked_dirty()
    assert list(clone.parent.glob("al-sem-verify-*")) == []
    # The incident opened before the gates is CLOSED by this pass, and closed
    # in a way that names its provenance. Without this assertion the
    # auto-resolve is unpinned and a future change could leave every
    # successful merge blocking `preflight` forever.
    recs = json.loads((clone / ".agent" / "incidents.json").read_text())
    assert recs[merge_sha]["state"] == "resolved"
    assert recs[merge_sha]["resolved_by"] == "post-merge:run-test"
    code, pre = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert [f for f in pre["failures"] if f.startswith("incident:")] == [], pre["failures"]


def test_post_merge_halts_without_reverting_when_a_gate_leaves_a_tracked_file_modified(
        capsys, repo_pair, monkeypatch):
    """Every gate returned 0 and the tree came back with a modified tracked
    file. That HALTS. It must not revert.

    DELIBERATELY INVALIDATED PIN. This test used to be
    `test_post_merge_reverts_when_a_gate_leaves_a_tracked_file_modified`, and
    it asserted `out["gates"]["tree-dirty-after-gates"] == 1` -- i.e. that a
    run whose every gate returned 0 should be stamped with a SYNTHETIC failed
    gate and routed into the destructive path. It encoded the exact defect.
    On 2026-09-14 merge 19f654e1's three real gates returned 0, 0, 0 and the
    single dirty file (`crates/al-syntax/src/raw/generated/node-types.sha256`)
    was byte-IDENTICAL -- no line-ending attribute, `core.autocrlf` against an
    LF generator, so `git diff` showed nothing while `git status` showed it
    modified. The harness entered the revert path on a fully-attested,
    CI-green commit and failed to push a revert of it only because
    `post_merge_failure` hit its unrelated master-not-ff guard. This test was
    green throughout. A test can pin a behaviour perfectly and still be
    pinning the wrong behaviour.

    Its SUBJECT is unchanged and still pinned: a gate that leaves a tracked
    file modified is still caught, still stops the run, and still names the
    file. Only the verdict moved from revert to halt. The dirt is now also
    made in the tree the gate actually ran in -- `open('README.md','a')`
    resolves against the gate's cwd, which is the verification worktree -- and
    it is a REAL content append, so it is semantic dirt rather than the
    line-ending anomaly the sibling test covers.
    """
    _, clone = repo_pair
    dirty_gate = [sys.executable, "-c", "open('README.md', 'a').write('x')"]
    monkeypatch.setattr(cli, "gates", lambda: [("dirty", dirty_gate, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)  # see the note in the happy-path test above

    def no_revert(*a, **kw):
        raise AssertionError("post_merge_failure reached with every gate green")

    # `cmd_post_merge` re-raises AssertionError by design and `main` does not
    # catch it, so a regression to the old routing EXPLODES rather than quietly
    # returning a payload whose fields this test would then have to disprove.
    monkeypatch.setattr(recovery, "post_merge_failure", no_revert)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["ok"] is False
    assert out["gates"] == {"dirty": 0, "noop-cdo": 0}          # every gate green: reverting would be a lie
    assert out["revert"] is None
    assert out["halt"]["reason"] == "tree-dirty-after-gates"
    assert out["halt"]["reverted"] is False and out["halt"]["pushed"] is False
    assert any(p.endswith("README.md") for p in out["halt"]["dirty"]), out["halt"]["dirty"]
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is not None
    # THE LABEL WIRING, at the call site. `recovery.incident_labels` is pinned
    # on its own in test_recovery.py, but a library test alone would not have
    # caught 2026-09-14: the defect was WHICH axis this call site claimed had
    # moved. Searching every recorded call for the string is what makes the
    # negative real -- an exact key can go stale into a vacuous pass.
    assert any("agent-gates-green-unverified" in c for c in gh.calls), gh.calls
    assert not any("agent-regressed" in c for c in gh.calls), gh.calls
    assert out["halt"]["labels"] == ["agent-gates-green-unverified"]
    # ...and the durable obligation, which outlives HALT (see cmd_preflight).
    recs = json.loads((clone / ".agent" / "incidents.json").read_text())
    assert list(recs) == [merge_sha]
    assert recs[merge_sha]["state"] == "open" and recs[merge_sha]["gate_red"] is False
    assert recs[merge_sha]["issue"] == 8 and recs[merge_sha]["revert_landed"] is None
    # THE LABELS ON THE RECORD, not only on the return value -- two different
    # claims, and only the second was checked. MEASURED IN THE ROUND-1 REVIEW:
    # deleting `post_merge_unverified`'s `record_outcome` call left all 206
    # tests green,
    # because the `open_incident` two lines above already writes the same
    # `reason` and `revert_landed` keeps its `setdefault(None)`. The one field
    # actually lost is this one, and an operator reading
    # `.agent/incidents.json` / `agentflow incidents` / `agentflow status`
    # would then see an open incident with NO labels while the issue on GitHub
    # carries `agent-gates-green-unverified` -- two records disagreeing about
    # what happened. The analogous call on the REVERT path is pinned in
    # test_recovery.py; this is the halt path's.
    assert recs[merge_sha]["labels"] == ["agent-gates-green-unverified"], recs[merge_sha]
    assert out["halt"]["incident"] == merge_sha
    g = Git(clone)
    g.fetch()
    assert g.rev("master") == merge_sha and g.rev("origin/master") == merge_sha
    assert g.branch() == "master"
    assert "issue reopen 8" not in gh.calls          # the merge stands; reopening would be a false claim
    # THE DIRT SURVIVES, AS BYTES. This assertion used to read
    # `verify_worktree_removed is True and glob("al-sem-verify-*") == []` --
    # it PINNED the deletion, while the CHANGELOG, the spec twice, this
    # module's docstring and the GitHub comment the operator reads all said
    # the dirt was left exactly as found. A test can pin a behaviour
    # perfectly and still pin the wrong one; that is the second time in this
    # one test (see the docstring). The path list alone is not evidence: the
    # 2026-09-14 root cause was found by comparing a file's ON-DISK BYTES
    # against the committed blob, which no list of names can support.
    retained = out["halt"]["retained_worktree"]
    assert retained, out["halt"]
    kept = Path(retained)
    assert kept.is_dir(), retained                                  # the DIRECTORY, not the name
    assert kept.parent == clone.parent and kept.name.startswith(worktrees.VERIFY_PREFIX)
    # THE BYTE COMPARISON the comment tells the operator to make, made here.
    # Not spelled as a literal: this checkout is `core.autocrlf=true`, so the
    # committed `hello\n` is materialised `hello\r\n` and a literal assertion
    # would pin the machine rather than the property.
    on_disk = (kept / "README.md").read_bytes()
    blob = subprocess.run(["git", "-C", str(kept), "show", "HEAD:README.md"],
                          capture_output=True).stdout
    assert on_disk.endswith(b"x"), on_disk       # the gate's own append, still on disk
    assert on_disk != blob                        # ...and it really differs from the commit
    assert out["verify_worktree_removed"] is False                  # reported honestly
    assert [p.name for p in clone.parent.glob("al-sem-verify-*")] == [kept.name]
    # ...and every surface a human reaches points at it: the HALT file and
    # the public comment, not only the JSON.
    assert retained in lock.halted(Ctx(Paths(clone), run_id="run-test"))
    body_call = next(c for c in gh.calls if c.startswith("issue comment 8 --body-file "))
    body = Path(body_call.split(" --body-file ", 1)[1]).read_text(encoding="utf-8")
    assert retained in body, body
    # AND THE EXIT EXISTS. A retained tree with no supported way to remove it
    # is the same defect shape as an incident row nothing can close, so the
    # path the comment names is driven here rather than described.
    code, out2 = run(capsys, clone, "cleanup", "--worktree", retained, gh_run=FakeRunner(),
                     run_id="run-test")
    assert code == 0 and out2["removed"] == retained, out2
    assert not kept.exists()


def test_a_large_dirty_tree_is_announced_at_its_real_size_at_the_call_site(
        capsys, repo_pair, monkeypatch):
    """THE CALL SITE for the count. test_recovery.py states 200 paths by
    assignment and pins what `post_merge_unverified` does with them; this one
    drives the real command, so the truncation that used to sit in `cmd_post_
    merge` -- `state.semantic_paths[:50]`, a different bound from the one the
    reporting function applies -- is covered. Without it, the helper is pinned
    and the use is free.

    PRECONDITION HAND-STATED: 60 committed files, and a gate that appends a
    byte to every one of them in the tree it runs in. 60 is above
    `MAX_DIRTY_LISTED` (20) and below the old call-site cut (50), so the two
    bounds give different answers and the assertion can tell them apart."""
    _, clone = repo_pair
    for i in range(60):
        (clone / "gen").mkdir(exist_ok=True)
        (clone / "gen" / f"f{i:02d}.txt").write_text(f"{i}\n")
    _git_raw(clone, "add", "gen")
    _git_raw(clone, "commit", "-q", "-m", "60 generated files")
    merge_sha = Git(clone).rev("HEAD")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    # RELATIVE to the gate's cwd, so the dirt lands in the verification
    # worktree -- the tree the gates actually ran in -- not in the clone.
    rewrite = [sys.executable, "-c",
               "import glob\nfor p in sorted(glob.glob('gen/*.txt')): open(p,'a').write('x')"]
    monkeypatch.setattr(cli, "gates", lambda: [("rewrite", rewrite, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))

    def no_revert(*a, **kw):
        raise AssertionError("post_merge_failure reached with every gate green")

    monkeypatch.setattr(recovery, "post_merge_failure", no_revert)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["halt"]["reason"] == "tree-dirty-after-gates"
    assert out["gates"] == {"rewrite": 0, "noop-cdo": 0}      # every gate green
    assert out["halt"]["dirty_total"] == 60, out["halt"]["dirty_total"]
    assert len(out["halt"]["dirty"]) == recovery.MAX_DIRTY_LISTED
    # The sentence a human actually reads. The HALT file is where the count
    # ended up wrong, so it is where the count is asserted.
    halt_text = lock.halted(Ctx(Paths(clone), run_id="run-test"))
    assert "60 tracked file(s) modified" in halt_text, halt_text


def test_post_merge_still_routes_a_red_gate_through_the_revert_path(capsys, repo_pair, monkeypatch):
    """The other half of the halt test above, and the reason it is safe to
    write. Before this existed, NO cli-level test drove red-gate -> revert, so
    an over-correction that disabled the revert path entirely would have stayed
    green. What is pinned here is the CALL SITE's choice to reach for
    `post_merge_failure` -- the helper's own behaviour is already pinned five
    ways in test_recovery.py."""
    _, clone = repo_pair
    red = [sys.executable, "-c", "raise SystemExit(3)"]
    monkeypatch.setattr(cli, "gates", lambda: [("red", red, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    seen = []

    def recording(ctx, git, gh, issue, merge_sha_, rerun):
        seen.append((issue, merge_sha_))
        return recovery.RevertOutcome(True, True, True, "reverted", "d" * 40)

    monkeypatch.setattr(recovery, "post_merge_failure", recording)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner())
    assert seen == [(8, merge_sha)]
    assert out["gates"] == {"red": 3}                 # the loop breaks before the cdo gate
    assert out["revert"]["reason"] == "reverted"
    assert out["halt"] is None
    assert code == 1 and out["ok"] is False


def test_the_revert_is_validated_in_the_revert_worktree_not_the_shared_checkout(
        capsys, repo_pair, monkeypatch):
    """THE ARC'S HEADLINE PROPERTY, at its only production call site.

    `recovery.post_merge_failure` is pinned five ways in test_recovery.py to
    HAND its revert worktree to `rerun_gates`. The one production
    implementation of that callable -- `cmd_post_merge`'s `rerun_all(work)` --
    can throw the parameter away: MEASURED IN THE ROUND-1 REVIEW, replacing
    `work` with `ctx.paths.root` at both `_run_gate` calls left all 206 tests
    green. The
    callee was pinned; the USE was free. That is this repo's named recurring
    defect applied to the change's own headline claim.

    Why no existing test could see it: the only CLI test that reaches the
    rerun (`test_post_merge_with_a_genuinely_red_gate_still_labels_regressed`)
    uses `raise SystemExit(1)`, a gate that is red in every tree, so it
    returns the same verdict whichever tree it runs in.

    PRECONDITION HAND-STATED, and the gate is cwd-SENSITIVE so the two trees
    give OPPOSITE answers:
      * `marker.txt` is added BY THE MERGE COMMIT, so the verification
        worktree (checked out at the merge) has it and the revert worktree
        (the merge reverted away) does not;
      * the gate probes it RELATIVE to its own cwd -- an absolute path into
        the clone would answer the same everywhere and pin nothing;
      * `scripts/ci-steps` is committed BEFORE the merge, so it survives the
        revert -- and it is marker-sensitive TOO, deliberately. `rerun_all`
        passes `work` to two different `_run_gate` calls, the gate loop and
        its own `ci-steps test`. A cwd-INSENSITIVE stub would leave the second
        one free: MEASURED, with a plain `exit 0` stub, breaking only the
        `ci-steps test` call site left this test green. With the stub reading
        the marker, each of the two call sites fails this test on its own.

    So: verification tree RED -> revert path; revert tree GREEN -> the revert
    is pushed. The shared root carries `marker.txt` for the whole command
    (`cmd_post_merge` deliberately leaves it on `master` at the merge), so
    pointing either rerun call there makes it RED and the reason
    `revert-failed-gates` with nothing pushed -- which is the production
    consequence in both directions: a real regression's revert would never
    land, and a revert that was red for a tree reason could be green-lit by
    the leavings of the run it is reverting."""
    _, clone = repo_pair
    g = Git(clone)
    # Committed BEFORE the merge on purpose: it must exist in the revert tree
    # too, or `rerun_all`'s `ci-steps test` fails there and the push could
    # never be reached -- which would make this proof prove nothing. It probes
    # the marker RELATIVE to its own cwd for the reason in the docstring.
    commit_file(clone, "scripts/ci-steps",
                "#!/bin/sh\nif [ -e marker.txt ]; then exit 1; fi\nexit 0\n",
                "ci-steps stub for the gate rerun")
    merge_sha = commit_file(clone, "marker.txt", "added by the merge under test\n", "merge of #8")
    assert g.push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    red_while_marker_exists = [sys.executable, "-c",
                               "import os, sys; sys.exit(1 if os.path.exists('marker.txt') else 0)"]
    monkeypatch.setattr(cli, "gates", lambda: [("marker", red_while_marker_exists, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    # PRECONDITION with raw git, independent of the code under test: the
    # shared checkout really does carry the marker while the command runs.
    assert (clone / "marker.txt").exists()
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "", "issue reopen 8": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert out["gates"] == {"marker": 1}              # the verification tree said no
    assert out["revert"] is not None and out["halt"] is None
    assert out["revert"]["reason"] == "reverted", out["revert"]
    assert out["revert"]["pushed"] is True, out["revert"]
    # THE CONSEQUENCE, read back from the remote: the revert the rerun
    # green-lit is what `origin/master` now carries.
    g.fetch()
    assert g.rev("origin/master") == out["revert"]["revert_sha"]
    assert out["revert"]["labels"] == ["agent-regressed", "agent-revert-landed"]
    assert code == 1 and out["ok"] is False
    assert list(clone.parent.glob("al-sem-verify-*")) == []


def test_a_dirty_shared_checkout_does_not_trigger_a_revert(capsys, repo_pair, monkeypatch):
    """The 2026-09-14 incident at CLI level. PRECONDITION HAND-STATED: a
    TRACKED file, modified, in the SHARED root, put there by nobody in this
    run -- with every gate green. The gates now run in a disposable worktree
    and the probe asks THAT tree, so the developer's checkout is not read as
    the gates' output."""
    _, clone = repo_pair
    g = Git(clone)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert g.push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    (clone / "README.md").write_text("modified in the developer checkout, by nobody in this run\n")
    assert g.tracked_dirty()                          # the precondition really holds
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert out["revert"] is None
    assert out["halt"] is None
    assert "tree-dirty-after-gates" not in out["gates"]
    assert not any("agent-regressed" in c or "issue reopen" in c for c in gh.calls), gh.calls
    g.fetch()
    assert g.rev("origin/master") == merge_sha
    # Nothing consumed or repaired the evidence.
    assert (clone / "README.md").read_text().startswith("modified in the developer checkout")
    assert code == 0


def test_post_merge_does_not_revert_when_the_only_dirt_is_line_endings(capsys, repo_pair, monkeypatch):
    """THE INCIDENT'S OWN SHAPE, at the call site. A gate rewrites a sidecar
    with LF while `core.autocrlf` materialised it as CRLF: `git status` says
    modified, the content is byte-identical. That must be RECORDED and must
    not revert.

    `FakeRunner(readonly=True)` is deliberate: the revert path's first act is
    `issue edit 8 --add-label agent-regressed`, which the fake raises
    AssertionError on, and `cmd_post_merge` re-raises AssertionError rather
    than folding it into JSON -- so a regression fails LOUDLY instead of
    surfacing as a confusing gh 404."""
    _, clone = repo_pair
    _git_raw(clone, "config", "core.autocrlf", "true")
    (clone / "gen.sha256").write_bytes(b"deadbeef\r\n")
    _git_raw(clone, "add", "gen.sha256")
    _git_raw(clone, "commit", "-q", "-m", "sidecar committed from CRLF bytes")
    merge_sha = Git(clone).rev("HEAD")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    # Literally what `scripts/ci-steps gen-syntax` does to that file.
    regen = [sys.executable, "-c", "open('gen.sha256','wb').write(b'deadbeef\\n')"]
    monkeypatch.setattr(cli, "gates", lambda: [("gen-syntax", regen, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["ok"] is True
    assert out["revert"] is None and out["halt"] is None
    assert "tree-dirty-after-gates" not in out["gates"]
    assert out["tree_anomaly"] and any("gen.sha256" in l for l in out["tree_anomaly"]), out
    assert Git(clone).branch() == "master"


def test_post_merge_runs_gates_in_a_worktree_with_the_main_checkout_grammar(capsys, repo_pair, monkeypatch):
    """CLAUDE.md's worktree/submodule constraint, pinned. A worktree gets no
    submodule checkout, so gates can only compile there because `_run_gate`
    derives TREE_SITTER_AL_PATH from `ctx.paths.root` rather than from the
    gate's cwd. PRECONDITION HAND-STATED: the grammar exists ONLY in the main
    checkout -- nothing initialises a submodule in the worktree, which is
    exactly the situation the rule warns about.

    IT ALSO PINS WHICH COMMIT THE GATES RAN ON, and that needs its own
    precondition. MEASURED IN THE ROUND-1 REVIEW:
    `worktrees.create(git, verify_wt, args.merge_sha)` -> `..., "master")`
    left all 206 tests green, because the prologue makes
    local `master` equal `origin/master` and in every other fixture that IS
    the merge. `master` carries no branch protection (CLAUDE.md), so a hand
    push between this run's `merge` and its `post-merge` makes the two
    different -- and the gates would then build the newer tip while
    `incidents.mark_verified(ctx, args.merge_sha, ...)` closes the obligation
    keyed to the merge. So the fixture below deliberately pushes ONE MORE
    commit after the merge and leaves local `master` behind it: without that,
    `"master"` and `args.merge_sha` name the same commit and the HEAD
    assertion cannot fail.

    THE TWO `delenv` CALLS ARE NOT TIDINESS. `_verify_target_dir` resolves
    `AGENTFLOW_VERIFY_TARGET_DIR` or `CARGO_TARGET_DIR` or `<root>/target-verify`,
    and its docstring names "an operator's inherited CARGO_TARGET_DIR" as a
    SUPPORTED case -- so the third-fallback assertion below inherited the real
    process environment and failed on exactly the machine configuration the
    production code was written for. Reproduced:
    `CARGO_TARGET_DIR=U:/shared-cargo-target python -m pytest ... -k grammar`
    -> 1 failed. A Rust developer on this repo who exports it got a red
    agentflow suite that reads as a harness regression. The precedence the
    chain claims is pinned separately, in test_cli_units.py."""
    _, clone = repo_pair
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("AGENTFLOW_VERIFY_TARGET_DIR", raising=False)
    commit_file(clone, "tree-sitter-al/src/node-types.json", "[]", "grammar stub")
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    # ORIGIN DELIBERATELY AHEAD OF THE MERGE, hand-stated with raw git: the
    # extra commit is made on a side branch and pushed onto `master`, so local
    # `master` is left at the merge and the prologue's `git.ff("origin/master")`
    # fast-forwards it to the newer tip -- exactly the state a hand push
    # between `merge` and `post-merge` produces.
    _git_raw(clone, "checkout", "-q", "-b", "newer")
    later = commit_file(clone, "someone-elses.txt", "pushed by hand\n", "a later commit on master")
    _git_raw(clone, "checkout", "-q", "master")
    _git_raw(clone, "push", "-q", "origin", "newer:master")
    g = Git(clone)
    g.fetch()
    assert g.rev("origin/master") == later != merge_sha
    assert g.rev("master") == merge_sha
    # A DESCENDANT tip, so the merge is still an ancestor of origin/master and
    # `cmd_post_merge`'s reachability guard does not refuse this fixture.
    assert g.is_ancestor(merge_sha, "refs/remotes/origin/master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    calls = []

    def fake_run(ctx, cmd, *, cwd, log_path, timeout_s, beat=None, beat_every=60.0, env=None):
        # The HEAD of the tree the gate is ACTUALLY being run in, read at the
        # moment it runs -- not derived from the path name, which is minted
        # from `args.merge_sha` and would agree with itself.
        calls.append({"cwd": str(cwd), "head": Git(Path(cwd)).rev("HEAD"), "env": dict(env or {})})
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        return supervise.Result(exit_code=0, log_path=log_path, timed_out=False, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", fake_run)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner())
    assert code == 0 and len(calls) == 2
    cwds = {c["cwd"] for c in calls}
    assert len(cwds) == 1, cwds                        # every gate in ONE tree
    cwd = Path(cwds.pop())
    assert cwd != clone and cwd.parent == clone.parent and cwd.name.startswith("al-sem-verify-")
    # ONE FIXED PATH, never one per merge: test binaries bake CARGO_MANIFEST_DIR, and a
    # per-SHA path left reused binaries pointing at the previous, deleted tree.
    assert cwd.name == worktrees.VERIFY_TREE and merge_sha[:12] not in str(cwd), cwd
    # EVERY gate on the commit this run says it verified, never on whatever
    # `master` names by now -- which the prologue has just moved to `later`.
    assert {c["head"] for c in calls} == {merge_sha}, [c["head"] for c in calls]
    assert Git(clone).rev("master") == later           # the shared root really did move
    for c in calls:
        assert c["env"]["TREE_SITTER_AL_PATH"] == str(clone / "tree-sitter-al")
        # Its own cache, never the main checkout's `target/`: binaries built in the
        # verify tree must not be reused by the checkout's own gates.
        assert c["env"]["CARGO_TARGET_DIR"] == str(clone / "target-verify") != str(clone / "target")
    assert out["verify_worktree_removed"] is True and not cwd.exists()
    assert Git(clone).branch() == "master"


def test_post_merge_leaves_no_worktree_when_create_lands_on_the_wrong_commit(
        capsys, repo_pair, monkeypatch):
    """THE CALL SITE for the create-failure teardown, and the CLI half of
    `verify-worktree-failed` (round 1 left that branch untested by choice).

    Two independent guarantees stand behind this assertion -- `create` tears
    down after its own postcondition failure, and `cmd_post_merge` now makes
    the `create` call INSIDE the `try/finally: removed = worktrees.destroy(..)`
    that follows it. MEASURED, and stated as a limit rather than implied:
    breaking either one alone leaves this test green; breaking both makes it
    fail. So this pins the pair, not each half. `test_worktrees.py`'s
    postcondition test pins the module half on its own.

    PRECONDITION HAND-STATED BY ASSIGNMENT, as in that test: the `Git` the
    module builds for the new worktree reports a HEAD that is not the commit
    asked for."""
    _, clone = repo_pair
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))

    class LandsSomewhereElse(Git):
        def rev(self, ref):
            return "0" * 40 if ref == "HEAD" else super().rev(ref)

    monkeypatch.setattr(cli.worktrees, "Git", LandsSomewhereElse)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner(readonly=True))
    assert code == 1 and out["error"].startswith("verify-worktree-failed: "), out
    # THE CONSEQUENCE. The glob deliberately does not ask the production path
    # helper for the name it expects.
    assert list(clone.parent.glob("al-sem-verify-*")) == [], list(clone.parent.glob("al-sem-verify-*"))
    assert Git(clone).branch() == "master"


def test_a_verification_worktree_that_cannot_be_created_refuses_and_leaves_nothing_behind(
        capsys, repo_pair, monkeypatch):
    """`cmd_post_merge`'s `except GitError -> Fail("verify-worktree-failed")`,
    pinned at the SOURCE of the failure rather than through one particular way
    of provoking it. Round 1 declined to pin this branch because "the only
    assertion available is the error string"; it is not. The CONSEQUENCES are
    assertable and they are what a human cares about: nothing on disk, nothing
    said on GitHub, and no TERMINAL obligation left blocking the loop.

    PRECONDITION HAND-STATED BY ASSIGNMENT: `worktrees.create` raises GitError.
    The sibling test provokes the same branch through the `landed != want`
    postcondition, which pins that ONE route; this one states the branch's own
    precondition, so it survives any change to how `create` can fail."""
    _, clone = repo_pair
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))

    def cannot_create(git, path, commit):
        raise GitError(f"git worktree add --detach -q {path} {commit}: fatal: disk is full")

    monkeypatch.setattr(cli.worktrees, "create", cannot_create)
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["error"].startswith("verify-worktree-failed: "), out
    assert "disk is full" in out["error"], out           # the cause is carried, not swallowed
    # THE CONSEQUENCES, which is what makes this more than a string assertion.
    assert gh.calls == [], gh.calls                      # no label, no comment, no reopen
    assert list(clone.parent.glob("al-sem-verify-*")) == []
    assert Git(clone).branch() == "master"
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is None
    # The pre-gate record IS on disk -- it is opened before the worktree --
    # but as `verifying`, which is not an obligation `preflight` refuses on.
    # A terminal row here would stop every later tick over a run that never
    # got as far as a gate.
    recs = json.loads((clone / ".agent" / "incidents.json").read_text())
    assert recs[merge_sha]["state"] == "verifying", recs
    assert incidents.unresolved(Ctx(Paths(clone), run_id="run-test")) == []


def test_a_teardown_failure_is_reported_and_does_not_change_the_gate_verdict(
        capsys, repo_pair, monkeypatch):
    """`verify_worktree_removed: false` -- the branch `worktrees.destroy`'s
    docstring advertises ("gives up rather than raising, leaving the caller to
    report `verify_worktree_removed: false`") and that no test had ever
    reached. It was asserted `is True` in three places and `is False` in none,
    so the whole false half was unpinned.

    BOTH assertions, never one: `worktrees.destroy` states that a teardown
    failure must never change a gate verdict, and only the PAIR pins that.
    `is False` alone would be satisfied by a change that failed the run;
    `ok is True` alone by one that always reports the teardown as successful.

    PRECONDITION HAND-STATED BY ASSIGNMENT and faithful to the real failure:
    Windows holds a lock on a tree a build just touched, `shutil.rmtree` gives
    up after its retries, the directory is STILL THERE and `destroy` returns
    False. So the stub deletes nothing -- and the test asserts the directory
    really did survive, which is the state `removed: false` describes. The
    stub is seen by `worktrees.create`'s own pre-clean call too (same module
    global); harmless here, since there is no leftover to clean. The real
    teardown is run at the end so this test leaves no worktree behind."""
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    real_destroy = worktrees.destroy
    monkeypatch.setattr(cli.worktrees, "destroy", lambda git, path: False)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner())
    assert out["verify_worktree_removed"] is False, out
    assert out["ok"] is True and code == 0, out          # the verdict is UNCHANGED
    assert out["gates"] == {"noop": 0, "noop-cdo": 0}
    assert out["revert"] is None and out["halt"] is None
    # The report matches the tree: the directory really is still there.
    live = list(clone.parent.glob("al-sem-verify-*"))
    assert len(live) == 1, live
    real_destroy(Git(clone), live[0])                    # leave nothing behind
    assert list(clone.parent.glob("al-sem-verify-*")) == []


def test_the_verification_worktree_is_gone_before_the_revert_path_runs(capsys, repo_pair, monkeypatch):
    """The peak-disk contract: teardown happens between the verdict and the
    dispatch, so at most ONE verification worktree exists at a time. The glob
    deliberately does not ask the production path helper for the name it
    expects."""
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("red", [sys.executable, "-c", "raise SystemExit(1)"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    seen = []

    def recording(ctx, git, gh, issue, merge_sha_, rerun):
        seen.append({"merge_sha": merge_sha_,
                     "live": sorted(p.name for p in clone.parent.glob("al-sem-verify-*"))})
        return recovery.RevertOutcome(True, False, False, "stub")

    monkeypatch.setattr(recovery, "post_merge_failure", recording)
    run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert len(seen) == 1 and seen[0]["merge_sha"] == merge_sha
    assert seen[0]["live"] == [], seen[0]["live"]


def test_post_merge_with_a_genuinely_red_gate_still_labels_regressed(capsys, repo_pair, monkeypatch):
    """T5. The OTHER direction at CLI level, and the reason the halt test's
    "no `agent-regressed` anywhere" assertion is safe to write: without this,
    that negative could be satisfied by a change that simply never stamps
    `agent-regressed` at all.

    PRECONDITION HAND-STATED: one gate child that exits 1. `post_merge_failure`
    is NOT stubbed here -- the whole point is that the real one runs and picks
    the labels. Its rerun re-runs that same red gate, so the revert is never
    pushed and the ACTION axis is `blocked`: master is still carrying it.
    """
    _, clone = repo_pair
    red = [sys.executable, "-c", "raise SystemExit(1)"]
    monkeypatch.setattr(cli, "gates", lambda: [("red", red, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "", "issue reopen 8": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["ok"] is False
    assert out["gates"] == {"red": 1}
    assert out["revert"]["reason"] == "revert-failed-gates"
    assert out["revert"]["labels"] == ["agent-regressed", "agent-revert-blocked"]
    # ONE argv carries both axes, so this is the exact call a human's issue
    # page will show.
    assert "issue edit 8 --add-label agent-regressed --add-label agent-revert-blocked" in gh.calls, gh.calls
    assert not any("agent-gates-green-unverified" in c for c in gh.calls), gh.calls
    assert out["halt"] is None
    recs = json.loads((clone / ".agent" / "incidents.json").read_text())
    assert recs[merge_sha]["gate_red"] is True and recs[merge_sha]["state"] == "open"
    assert recs[merge_sha]["revert_landed"] is False


def test_the_revert_rerun_gates_share_the_build_cache_and_never_regenerate_goldens(
        capsys, repo_pair, monkeypatch, tmp_path):
    """THE REVERT RERUN'S ENVIRONMENT, at both of the two call sites that
    build it. Nothing in the package looked at it.

    MEASURED BY THE INDEPENDENT MUTATION AUDIT, three ways: deleting
    `cargo_target_dir=_verify_target_dir(ctx)` from `rerun_all`'s gate-list
    call, from its `ci-steps-test-on-revert` call, and from BOTH at once, each
    left the whole suite green. The cause is a plain test gap and not a second
    writer -- `supervise.sanitized_env` sets `CARGO_TARGET_DIR` only under
    `if cargo_target_dir:` -- and the only gate-environment assertions in the
    package sit in `test_post_merge_runs_gates_in_a_worktree_with_the_main_
    checkout_grammar`, a green-gate happy path that never reaches `rerun_all`.
    `on-revert` appeared in the tests exactly once, in a docstring.

    WHY IT IS NOT COSMETIC, since `_run_gate`'s parameter defaults to None and
    the keyword therefore reads as redundant to a tidier. Without it the
    revert validation runs with cargo defaulting to `<revert worktree>/target`
    -- a cold build in a directory cargo has never seen, inside the tree
    `rm -rf` must clear (see `_verify_target_dir`'s docstring), with the issue
    worktree still on disk. It exhausts the disk or crosses the 45-minute cap,
    `rerun_all` returns False, and `post_merge_failure` exits
    `revert-failed-gates`: a CORRECT revert is never pushed, master stays red,
    and the run reports it as the revert being bad. That is this arc's own
    through-line -- COULD NOT VERIFY rendered as PROVEN BAD -- in the one path
    that decides whether master gets fixed.

    PRECONDITIONS HAND-STATED, each one the reason an assertion below can
    fail at all:
      * `AGENTFLOW_VERIFY_TARGET_DIR` names a path that is NOT
        `<root>/target`, so "the child got the right cache" cannot be
        satisfied by an accidental agreement with the fallback rung;
      * `CARGO_TARGET_DIR` is DELETED from this process. `_run_gate` builds
        the child env from `os.environ.copy()` and `CARGO_TARGET_DIR` is not
        in `DROP_ENV`, so an operator's exported one reaches the child even
        with the keyword dropped -- and `_verify_target_dir` returns that
        same value second in its chain, so the two would AGREE and the break
        would come back green. Deleting it leaves the keyword as the only way
        the variable can arrive;
      * the golden-regeneration variable named in `supervise.DROP_ENV` is
        EXPORTED here, so the absence asserted below is a drop that HAPPENED
        rather than a variable that was never there. `DROP_ENV` is pinned at
        the helper (test_supervise.py); this is the same promise at a USE, on
        the path that decides a push, which nothing checked.

    THE REVERT IS NEVER PUSHED HERE, deliberately: this fixture reaches the
    code that can rewrite `origin/master` and must not become the destructive
    action the arc exists to prevent. Only the LAST rerun gate says no, so
    every gate-list rerun still runs before the verdict (a first-failure
    short-circuit would otherwise hide one call site) and `rerun_all` still
    returns False -- `revert-failed-gates`, remote untouched."""
    _, clone = repo_pair
    g = Git(clone)
    shared = tmp_path / "shared-cargo-target"
    monkeypatch.setenv("AGENTFLOW_VERIFY_TARGET_DIR", str(shared))
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.setenv("REGEN_TEMP_GOLDENS", "1")
    expected_cache = cli._verify_target_dir(Ctx(Paths(clone)))
    assert expected_cache == str(shared) != str(clone / "target")
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert g.push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    monkeypatch.setattr(cli, "gates", lambda: [("red", [sys.executable, "-c", "raise SystemExit(1)"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    calls = []

    def fake_run(ctx, cmd, *, cwd, log_path, timeout_s, beat=None, beat_every=60.0, env=None):
        # The gate's name is the log's DIRECTORY since #43 (one file per run).
        calls.append({"name": log_path.parent.name, "cwd": str(cwd), "env": dict(env or {})})
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        # The verification gate says no -- the ONLY way into `rerun_all` -- and
        # on the revert only the final `ci-steps test` does. See the docstring.
        red = log_path.parent.name in ("red", "ci-steps-test-on-revert")
        return supervise.Result(exit_code=1 if red else 0, log_path=log_path,
                                timed_out=False, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", fake_run)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "", "issue reopen 8": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["revert"]["reason"] == "revert-failed-gates", out
    # BOTH call sites ran, by name: the gate-list loop appends `-on-revert` to
    # every entry of `gate_list`, and the `ci-steps test` rerun is a separate
    # call the loop does not cover.
    rerun = [c for c in calls if c["name"].endswith("-on-revert")]
    assert [c["name"] for c in rerun] == ["red-on-revert", "noop-cdo-on-revert",
                                          "ci-steps-test-on-revert"], calls
    revert_wt = worktrees.verify_path(clone.resolve())
    for c in rerun:
        # THE PIN, and `== expected_cache` rather than `in c["env"]`: a
        # `cargo_target_dir=""` regression sets no key at all, and a call site
        # that hardcoded `<root>/target` instead of asking `_verify_target_dir`
        # would satisfy a presence check while dropping rungs 1 and 2. `.get`
        # rather than `[...]` so a missing key fails with the recorded call in
        # the message instead of raising a bare KeyError.
        assert c["env"].get("CARGO_TARGET_DIR") == expected_cache, c
        # A validation must never be allowed to regenerate the goldens it is
        # being judged by.
        assert "REGEN_TEMP_GOLDENS" not in c["env"], c
        # CORROBORATION, not this test's pin: the discriminating witness for
        # "validated in the tree it was built in" is
        # `test_the_revert_is_validated_in_the_revert_worktree_not_the_shared_
        # checkout`, whose cwd-sensitive gate gives the two trees opposite
        # answers. Asserted here because the fixture already holds it.
        assert Path(c["cwd"]) == revert_wt != clone.resolve()
    # Nothing pushed: the revert failed its rerun, so the remote still carries
    # the merge under test.
    g.fetch()
    assert g.rev("origin/master") == merge_sha


def test_a_gate_killed_at_its_timeout_halts_and_never_reverts(capsys, repo_pair, monkeypatch):
    """A gate the SUPERVISOR killed is not a gate that said no.

    `supervise.run` folds a kill into `exit_code or 124` and returns
    `timed_out=True` beside it. The gate loop used to read only the exit code,
    so a 45-minute kill was indistinguishable from a verdict at the one
    decision point where that distinction is the entire rule -- and 124 routed
    to `post_merge_failure`: HALT, `agent-regressed`, a public comment saying
    the gates failed, and a pushed revert of a fully-attested CI-green commit.
    COULD NOT VERIFY is not PROVEN BAD.

    PRECONDITION HAND-STATED BY ASSIGNMENT: `supervise.run` is replaced by one
    that returns the exact `Result` a kill produces. Production cannot be
    asked to spend 45 minutes inside a test, so the state is constructed
    literally rather than provoked -- and it therefore survives any change to
    how the supervisor kills a tree.

    PINS THE USE: it drives the real `post-merge` subcommand, and stubs
    `recovery.post_merge_failure` to RAISE. `cmd_post_merge` re-raises
    AssertionError by design, so a regression to the old routing explodes
    rather than returning a payload this test would then have to disprove.
    """
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("slow", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)

    def killed_at_its_timeout(ctx_, cmd, *, cwd, log_path, timeout_s, beat=None,
                              beat_every=60.0, env=None):
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_text("")
        return supervise.Result(exit_code=124, log_path=log_path, timed_out=True, seconds=0.0)

    monkeypatch.setattr(cli.supervise, "run", killed_at_its_timeout)

    def no_revert(*a, **kw):
        raise AssertionError("revert path reached on a killed gate")

    monkeypatch.setattr(recovery, "post_merge_failure", no_revert)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "",
        "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "",
    })
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["ok"] is False
    assert out["revert"] is None                      # the destructive path was never entered
    assert out["halt"]["reason"] == "gate-timed-out"  # ...and it is not spelled as dirt
    assert out["halt"]["reverted"] is False and out["halt"]["pushed"] is False
    assert out["halt"]["dirty"] == []                 # no file was modified; claiming one would be a lie
    # THE PAYLOAD HALF, measured separately below: `gates` is a name -> exit
    # code map and 124 is an exit code like any other, so the kill is named
    # where a human reading the JSON cannot miss it.
    assert out["gates"] == {"slow": 124}              # the loop breaks before the cdo gate
    assert out["timeouts"] == ["slow"]
    # EVIDENCE axis: no gate returned a red verdict, so nothing may be
    # labelled a regression. Substring over EVERY recorded call -- an exact
    # key can go stale into a vacuous pass.
    assert any("agent-gates-green-unverified" in c for c in gh.calls), gh.calls
    assert not any("agent-regressed" in c for c in gh.calls), gh.calls
    assert "issue reopen 8" not in gh.calls           # the merge stands
    recs = json.loads((clone / ".agent" / "incidents.json").read_text())
    assert recs[merge_sha]["gate_red"] is False       # a clock running out is not evidence
    assert recs[merge_sha]["state"] == "open"         # ...but it IS a terminal obligation
    assert recs[merge_sha]["reason"] == "gate-timed-out"
    g = Git(clone)
    g.fetch()
    assert g.rev("origin/master") == merge_sha and g.rev("master") == merge_sha
    assert g.branch() == "master"
    assert lock.halted(Ctx(Paths(clone), run_id="run-test")) is not None
    # THE CONTRAST THAT PROVES THE RETENTION IS CONDITIONAL. `tree-dirty-
    # after-gates` keeps its worktree because the dirt IS the evidence; this
    # stop must still destroy, because a killed gate's tree holds a partial
    # build and nothing about a file. Both stops set `halt`, so a teardown
    # skipped on `halt is not None` would leak here -- and only this
    # assertion would say so.
    assert out["halt"]["retained_worktree"] is None, out["halt"]
    assert out["verify_worktree_removed"] is True
    assert list(clone.parent.glob("al-sem-verify-*")) == []


# ---- the durable incident obligation, at CLI level -------------------------

def preflight_ready(clone, monkeypatch, tmp_path):
    """Everything `preflight`'s non-incident rows need, so the assertions
    below can be about `incident:` rows and nothing else. `.agent/` is
    gitignored here exactly as it is in the real repo, so hand-writing a
    record into it does not show up as `tree-dirty`."""
    commit_file(clone, ".gitignore", ".agent/\n", "ignore local agent state")
    commit_file(clone, "tree-sitter-al/src/node-types.json", "[]", "grammar stub")
    assert Git(clone).push("origin", "master")
    monkeypatch.setenv("CDO_WS", str(tmp_path))
    (clone / ".agent").mkdir(exist_ok=True)


def write_incident(clone, merge_sha, **over):
    """The incident record, written LITERALLY. `post_merge_failure` is not
    asked to mint it -- test_recovery.py's producer tests are what tie this
    shape to production, and a setup that depends on the code under test
    cannot state a precondition that code can no longer create."""
    rec = {"merge_sha": merge_sha, "issue": 8, "state": "open", "gate_red": False,
           "opened_at": 1.0, "reason": "tree-dirty-after-gates", "revert_landed": None,
           "labels": ["agent-gates-green-unverified"], "run_id": "run-old",
           "resolved_at": None, "resolved_by": None, "note": ""}
    rec.update(over)
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "incidents.json").write_text(
        json.dumps({merge_sha: rec}, indent=2, sort_keys=True), encoding="utf-8")


def incident_rows(out):
    return [f for f in out["failures"] if f.startswith("incident:")]


SHA = "c" * 40


def test_preflight_refuses_an_unresolved_incident_with_no_halt_present(capsys, repo_pair, monkeypatch, tmp_path):
    """T9. The core of the incident record: the refusal is computed
    INDEPENDENTLY of HALT. The `lock.halted(...) is None` assertion below is
    the whole claim -- there is no kill switch here at all, and preflight
    still refuses."""
    _, clone = repo_pair
    preflight_ready(clone, monkeypatch, tmp_path)
    write_incident(clone, SHA)
    assert lock.halted(Ctx(Paths(clone))) is None
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 1
    assert incident_rows(out) == [f"incident:{SHA[:12]}"], out["failures"]
    assert [i["merge_sha"] for i in out["incidents"]] == [SHA]
    # `status` is the operator's dashboard, and reporting HALT (None here)
    # without the obligation behind it is what let a cleared HALT read as
    # "all clear".
    code, st = run(capsys, clone, "status", gh_run=FakeRunner(), run_id=None)
    assert st["halted"] is None and [i["merge_sha"] for i in st["incidents"]] == [SHA]
    # ...and the refusal is not permanent: a closed record stops blocking.
    write_incident(clone, SHA, state="resolved", resolved_by="operator")
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert incident_rows(out) == [], out["failures"]
    assert out["incidents"] == []


def test_clearing_halt_leaves_the_incident_that_blocks_preflight(capsys, repo_pair, monkeypatch, tmp_path):
    """T10. THE KEYSTONE. Clearing the one global HALT file must not silently
    resume the loop over a merge nothing ever verified. Both preconditions are
    written by hand: the incident record, and `.agent/HALT` with an exact
    text."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    write_incident(clone, SHA)
    halt_text = "regression: merge abc123 failed post-merge gates"
    lock.set_halt(Ctx(Paths(clone), run_id=None, now=lambda: 1_000_000.0), halt_text)
    code, out = run(capsys, clone, "clear-halt", "--reason", halt_text, "--by", "operator",
                    gh_run=FakeRunner(), run_id=None)
    assert code == 0 and out["cleared"] == halt_text
    # The surviving obligation is named in the SUCCESS payload, so the
    # operator learns here rather than two commands later.
    assert out["unresolved_incidents"] == [SHA]
    assert lock.halted(Ctx(Paths(clone))) is None
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 1 and incident_rows(out) == [f"incident:{SHA[:12]}"], out["failures"]


def test_resolve_incident_is_operator_only_and_then_preflight_passes(capsys, repo_pair, monkeypatch, tmp_path):
    """T11. The only hand close, and it is gated."""
    _, clone = repo_pair
    preflight_ready(clone, monkeypatch, tmp_path)
    write_incident(clone, SHA)
    # (a) automation asking.
    code, out = run(capsys, clone, "resolve-incident", "--merge-sha", SHA, "--by", "the loop",
                    gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and out["refused"] == "operator-only" and out["resolved"] is None
    assert json.loads((clone / ".agent" / "incidents.json").read_text())[SHA]["state"] == "open"
    # (b) an operator asking.
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    code, out = run(capsys, clone, "resolve-incident", "--merge-sha", SHA, "--by", "operator",
                    "--note", "CRLF, not a regression; verified by hand",
                    gh_run=FakeRunner(), run_id=None)
    assert code == 0
    assert out["resolved"]["state"] == "resolved" and out["resolved"]["resolved_by"] == "operator"
    assert out["resolved"]["note"] == "CRLF, not a regression; verified by hand"
    audit = [json.loads(l) for l in (clone / ".agent" / "audit.jsonl").read_text().splitlines()]
    assert audit[-1]["action"] == "incident-resolve" and audit[-1]["merge_sha"] == SHA
    # (c) preflight now has nothing to refuse on.
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert incident_rows(out) == [], out["failures"]
    # (d) again: a no-op must never read as success.
    code, out = run(capsys, clone, "resolve-incident", "--merge-sha", SHA, "--by", "operator",
                    gh_run=FakeRunner(), run_id=None)
    assert code == 1 and out["refused"] == "already-resolved"


def test_a_post_merge_killed_mid_gates_is_still_recoverable_by_recover(
        capsys, repo_pair, monkeypatch, tmp_path):
    """THE DEADLOCK THE `verifying` STATE EXISTS TO PREVENT.

    `cmd_post_merge` opens its obligation BEFORE the gates, which is right --
    a crash inside the gate block otherwise leaves a merge nothing verified
    with no record at all. But as a TERMINAL record it also made every such
    crash a hard `preflight` failure, and the conductor stops the tick on
    `ok:false` BEFORE it reaches `recover`. The longest window in the whole
    tick therefore deadlocked the loop, and the operator's only exit was
    `resolve-incident`: recording on the durable audit trail that a merge
    nothing verified is closed, purely to be allowed to go verify it. The
    obligation would have to be falsified as a precondition for discharging
    it.

    PRECONDITION HAND-STATED: the in-flight record and the stale lock are both
    written LITERALLY. Production can no longer be made to crash mid-gates
    inside a test, and a setup that depends on the code under test cannot
    state a precondition that code can no longer create.

    THE CONTRAST HALF is `test_preflight_refuses_an_unresolved_incident_with_
    no_halt_present` above, which states a `state: "open"` record and must
    stay RED at preflight. Without it this test is satisfied by deleting the
    incident check outright.
    """
    _, clone = repo_pair
    preflight_ready(clone, monkeypatch, tmp_path)
    write_incident(clone, SHA, state="verifying",
                   reason="post-merge verification has not completed")
    # A lock whose heartbeat is the epoch: stale by any clock, and written by
    # hand rather than by acquiring one and waiting 30 minutes.
    (clone / ".agent" / "lock.json").write_text(json.dumps(
        {"issue": 8, "run_id": "run-killed", "session": "https://s", "started": 1.0,
         "heartbeat": 0.0, "attempt": 1}), encoding="utf-8")
    # `disk-free` is a fact about whoever runs the suite, not about incidents.
    # Pinned so this test's exit code cannot be decided by their free space.
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert code == 0, out["failures"]
    assert incident_rows(out) == [], out["failures"]
    # ...and the tick can now reach the recovery that discharges it.
    assert out["stale_lock"] is not None and out["stale_lock"]["run_id"] == "run-killed"
    # VISIBLE, not fatal. An in-flight obligation nobody can see is how one
    # gets forgotten -- the point is that it does not FAIL the tick, not that
    # it disappears.
    assert [i["merge_sha"] for i in out["incidents"]] == [SHA]
    assert out["incidents"][0]["state"] == "verifying"
    code, st = run(capsys, clone, "status", gh_run=FakeRunner(), run_id=None)
    assert [i["merge_sha"] for i in st["incidents"]] == [SHA]


def test_a_post_merge_that_dies_mid_gates_leaves_a_recoverable_record(
        capsys, repo_pair, monkeypatch, tmp_path):
    """THE CALL SITE for the state above. The test before this one states the
    in-flight record by hand, so it survives any change to how the record is
    minted -- and cannot see which state `cmd_post_merge` actually writes. This
    one drives the real command and reads what it left on disk.

    PRECONDITION HAND-STATED: `supervise.run` raises. A test cannot have the
    machine killed under it, and the observable consequence is the same one
    that matters here -- the gate block is left through an exception, the
    verdict is never reached, and the pre-gate record is whatever it was
    written as.
    """
    _, clone = repo_pair
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)

    def machine_went_away(*a, **kw):
        raise RuntimeError("the machine went away mid-gates")

    monkeypatch.setattr(cli.supervise, "run", machine_went_away)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha,
                    gh_run=FakeRunner(readonly=True))
    assert code == 1 and out["error"] == "RuntimeError: the machine went away mid-gates"
    rec = json.loads((clone / ".agent" / "incidents.json").read_text())[merge_sha]
    assert rec["state"] == "verifying", rec       # NOT a terminal obligation: nothing was decided
    assert rec["gate_red"] is False
    # The consequence: the next tick can still get to `recover`. Asserted on
    # `incident_rows` rather than on `ok`, because the crashed run's own lock
    # is still on disk and still live -- a second, unrelated, honest row:
    # with the default `slots: 1`, that live lease fills the only slot.
    monkeypatch.setattr(cli.shutil, "disk_usage", lambda p: SimpleNamespace(free=500 * 2**30))
    code, pre = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert incident_rows(pre) == [], pre["failures"]
    assert pre["failures"] == ["slots-full"], pre["failures"]
    assert [i["merge_sha"] for i in pre["incidents"]] == [merge_sha]   # visible, not fatal
    assert list(clone.parent.glob("al-sem-verify-*")) == []            # teardown still ran


def rerun_post_merge_over_a_terminal_incident(capsys, clone, monkeypatch):
    """The operator re-run the round-1 review found: nothing in
    `cmd_post_merge` refuses a second invocation under the still-held lock, so
    its pre-gate `open_incident` runs again over a record that already carries
    a verdict.

    PRECONDITION HAND-STATED: the terminal record is written LITERALLY --
    `post_merge_failure` is never asked to mint it -- and the gates are green,
    which is the case where the re-run has something to claim.
    """
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    write_incident(clone, merge_sha, gate_red=True, revert_landed=False, terminal=True,
                   reason="push-rejected", run_id="run-old",
                   labels=["agent-regressed", "agent-revert-blocked"])
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    rec = json.loads((clone / ".agent" / "incidents.json").read_text())[merge_sha]
    return merge_sha, code, out, rec, gh


def test_a_post_merge_rerun_never_downgrades_the_incident_it_finds(capsys, repo_pair, monkeypatch):
    """THE CALL SITE for `open_incident`'s monotonicity. `cmd_post_merge`
    opens the record with `gate_red=False, reason="post-merge verification has
    not completed"` before the gates -- which used to overwrite a terminal
    `gate_red: True` / `"push-rejected"` record unconditionally, erasing the
    evidence that a gate had ever gone red even when the retry then crashed.

    The library half is `test_reopening_an_incident_never_downgrades_a_red_
    gate` in test_incidents.py. This one exists because that call site is what
    actually runs, and a helper can be pinned while its use is free.
    """
    _, clone = repo_pair
    merge_sha, code, out, rec, gh = rerun_post_merge_over_a_terminal_incident(
        capsys, clone, monkeypatch)
    assert rec["gate_red"] is True                    # the evidence axis never moves backwards
    assert rec["reason"] == "push-rejected"           # ...nor does a terminal reason
    assert rec["revert_landed"] is False
    assert rec["state"] == "open"                     # and `verifying` never demotes a verdict
    assert rec["labels"] == ["agent-regressed", "agent-revert-blocked"]
    assert rec["opened_at"] == 1.0                    # still ONE incident


def test_post_merge_cannot_auto_close_an_incident_that_says_master_may_be_red(
        capsys, repo_pair, monkeypatch):
    """THE CALL SITE for `mark_verified`'s refusal. A green pass proves
    something about THIS run; it proves nothing about the red gate and the
    unpushed revert the record already carries. `agent-revert-blocked` means
    MASTER MAY STILL BE RED, and closing that is an operator's decision.

    The routing is pinned too, not just the raise: `IncidentRefused` is a
    RuntimeError subclass, so `cmd_post_merge`'s `except Exception` stashes it
    and the tail re-raises it into `main`'s RuntimeError handler -- one JSON
    object, exit 1.
    """
    _, clone = repo_pair
    merge_sha, code, out, rec, gh = rerun_post_merge_over_a_terminal_incident(
        capsys, clone, monkeypatch)
    assert code == 1
    assert out["error"].startswith("IncidentRefused: evidence-against-close"), out
    assert merge_sha[:12] in out["error"]
    assert rec["state"] == "open" and rec["resolved_by"] is None
    assert gh.calls == []                             # nothing was said to a human either way
    # ...and preflight still refuses, which is the consequence that matters.
    code, pre = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert incident_rows(pre) == [f"incident:{merge_sha[:12]}"], pre["failures"]


def test_claim_ensures_the_two_axis_incident_labels_exist(capsys, root):
    """T12. `gh issue edit --add-label X` hard-fails on a label the repo does
    not have, and the incident path is the worst possible place to discover
    that. PINS THE USE (`cmd_claim`'s `ensure_labels(LABELS)`), not
    `Gh.ensure_labels`, which is already pinned in test_gh.py.

    PRECONDITION HAND-STATED, and deliberately NOT the module-level `READS`:
    that fixture MIRRORS `cli.LABELS`, so extending LABELS alone can never
    fail a test that uses it -- another fixture that tracks the code instead
    of pinning it. Here the repo has NO labels at all, so the creation path
    actually runs."""
    gh = FakeRunner({**READS,
                     f"api repos/{REPO}/labels?per_page=100 --paginate --slurp": json.dumps([[]]),
                     f"api repos/{REPO}/labels -X POST *": "",
                     "issue edit 8 --add-label agent-working": "", "issue comment 8 *": ""})
    code, out = run(capsys, root, "claim", "8", "--session", "https://s",
                    "--title-slug", "c10-scope", gh_run=gh)
    assert code == 0 and out["attempt"] == 1
    created = [c for c in gh.calls if c.startswith(f"api repos/{REPO}/labels -X POST")]
    for name in recovery.INCIDENT_LABELS:
        assert any(f"name={name}" in c for c in created), (name, created)


def test_post_merge_reports_restore_failed_when_the_final_checkout_fails(capsys, repo_pair, monkeypatch):
    # Fix round 2, finding 3: a failed restore-to-master must always surface
    # as a `restore_failed` JSON field, never a bare exception.
    _, clone = repo_pair
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    # Without this the command refuses before the first `git.checkout`, the
    # flaky-checkout monkeypatch is never reached and `restore_failed` never
    # appears -- see the note in the happy-path test above.
    record_merge(clone, merge_sha)

    calls = {"master": 0}
    original_checkout = Git.checkout

    def flaky_checkout(self, ref):
        if ref == "master":
            calls["master"] += 1
            if calls["master"] > 1:
                raise RuntimeError("simulated restore failure")
        return original_checkout(self, ref)

    monkeypatch.setattr(Git, "checkout", flaky_checkout)
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    assert code == 1 and "restore_failed" in out and "simulated restore failure" in out["restore_failed"]


def test_cleanup_succeeds_with_no_lock_and_removes_worktree(capsys, repo_pair):
    # Fix round 2, finding 2: cleanup follows finish (which releases the
    # lock), so an ABSENT lock is the normal case and must not be refused.
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-x-a1", "master")
    commit_file(wt, "issue.txt", "x\n", "issue work")
    merge_sha = g.merge_squash("issue/8-x-a1", "squash issue/8-x-a1")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-x-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert not wt.exists()


def _crlf_worktree(clone, g, name, branch):
    """An issue worktree whose ONE tracked difference is a line ending: the
    file is committed from CRLF bytes and then rewritten as LF, exactly what
    `scripts/ci-steps gen-syntax` does to `node-types.sha256`. Returns the
    merge SHA that squashes the branch, so `remove_worktree`'s merge proof
    passes and cleanliness is the only thing left to decide."""
    _crlf_consistent(clone)
    wt = clone.parent / name
    g.worktree_add(wt, branch, "master")
    (wt / "gen.sha256").write_bytes(b"deadbeef\r\n")
    _git_raw(wt, "add", "gen.sha256")
    _git_raw(wt, "commit", "-q", "-m", "sidecar committed from CRLF bytes")
    merge_sha = g.merge_squash(branch, f"squash {branch}")
    (wt / "gen.sha256").write_bytes(b"deadbeef\n")
    return wt, merge_sha


def test_cleanup_removes_a_worktree_whose_only_dirt_is_line_endings(capsys, repo_pair):
    """The main issue-worktree cleanup used raw `git status --porcelain` while
    its spike sibling already used the content probe -- and the round-1 diff
    edited the SPIKE function's comment to advertise the new semantics, so the
    asymmetry read as a decision. It was an unfinished migration: a
    line-ending-materialised file in an issue worktree stranded cleanup
    forever, every later `cleanup` raising "is not clean" on a tree with
    nothing in it.

    Driven through the `cleanup` SUBCOMMAND, so it pins `cmd_cleanup` ->
    `remove_worktree` -- the use -- rather than the probe."""
    _, clone = repo_pair
    g = Git(clone)
    wt, merge_sha = _crlf_worktree(clone, g, recovery.worktree_name(8, 1), "issue/8-crlf-a1")
    # PRECONDITION with raw git IN THE WORKTREE: porcelain says modified,
    # content is byte-identical.
    assert _git_raw(wt, "status", "--porcelain", "-uno").stdout.strip() != ""
    assert _git_raw(wt, "diff", "--name-only", "HEAD", "--").stdout.strip() == ""
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-crlf-a1",
                    "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert not wt.exists() and "issue/8-crlf-a1" not in g.out("branch", "--list")


def test_cleanup_refuses_a_worktree_with_a_genuine_tracked_modification(capsys, repo_pair):
    """THE CONTRAST to the test above, in the same file and through the same
    subcommand. Without it, that one is satisfied by a probe that calls
    everything clean -- which is the failure mode in the other direction, and
    the worse one: cleanup would `rm -rf` a worktree holding real uncommitted
    work."""
    _, clone = repo_pair
    g = Git(clone)
    wt, merge_sha = _crlf_worktree(clone, g, recovery.worktree_name(8, 2), "issue/8-crlf-a2")
    (wt / "README.md").write_bytes(b"real uncommitted work\n")
    assert sorted(_git_raw(wt, "diff", "--name-only", "HEAD", "--").stdout.split()) == ["README.md"]
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-crlf-a2",
                    "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and "is not clean" in out["error"], out
    assert wt.exists() and (wt / "README.md").read_bytes() == b"real uncommitted work\n"


def test_cleanup_refuses_master_as_a_branch_name(capsys, repo_pair):
    """`--branch` was the one branch argument in the CLI that reached git
    unvalidated, and both cleanup paths end in `git branch -D`. The merge
    proof does not discriminate: `git diff --quiet master <the commit just
    merged>` compares a tree with itself and PASSES, and the already-gone
    early return deletes the branch with no proof at all.

    FOUR ARMS, none redundant: with and without `--spike` (different callee),
    with the worktree ALREADY GONE (the unproofed early return), and a case
    variant (on a case-insensitive filesystem `Master` resolves to the same
    ref, which is how a case-sensitive comparison lets one through)."""
    _, clone = repo_pair
    g = Git(clone)
    head = g.rev("master")
    wt = clone.parent / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-guard-a1", "master")
    gone = clone.parent / recovery.worktree_name(8, 9)
    assert not gone.exists()
    arms = [
        ["--worktree", str(wt), "--branch", "master", "--merge-sha", head],
        ["--worktree", str(wt), "--branch", "master", "--spike"],
        ["--worktree", str(gone), "--branch", "master", "--merge-sha", head],
        ["--worktree", str(wt), "--branch", "Master", "--merge-sha", head],
    ]
    for arm in arms:
        code, out = run(capsys, clone, "cleanup", *arm, gh_run=FakeRunner(), run_id="run-test")
        assert code == 1, (arm, out)
        assert out.get("error") == "refusing to clean up master", (arm, out)
        # THE CONSEQUENCE, asserted every time rather than once at the end.
        assert g.ok("rev-parse", "--verify", "refs/heads/master"), arm
        assert g.rev("master") == head, arm
    assert wt.exists()


def test_cleanup_refuses_a_refspec_shaped_branch(capsys, repo_pair):
    """Shape has to be checked FIRST and separately. `feat:master` is neither
    `master` nor resolves to it, so an identity comparison never sees it --
    and a colon in a git ref argument means "this local ref onto THAT remote
    ref". This is why `cmd_push_branch` splits the two checks, and why
    `cmd_cleanup` now does too."""
    _, clone = repo_pair
    g = Git(clone)
    head = g.rev("master")
    wt = clone.parent / recovery.worktree_name(8, 1)
    g.worktree_add(wt, "issue/8-refspec-a1", "master")
    for bad in ("feat:master", "refs/heads/master", "-D", "HEAD", "two words", ""):
        # `--branch=-D` rather than `--branch -D`: argparse refuses the spaced
        # form as a usage error before the executor ever sees the value, so
        # only the `=` form reaches the validator under test. (Same idiom as
        # the `push-branch` shape test.)
        arg = [f"--branch={bad}"] if bad.startswith("-") else ["--branch", bad]
        code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), *arg,
                        "--merge-sha", head, gh_run=FakeRunner(), run_id="run-test")
        assert code == 1, (bad, out)
        assert out.get("error") == "not a plain branch name", (bad, out)
        assert g.ok("rev-parse", "--verify", "refs/heads/master"), bad
    assert wt.exists() and "issue/8-refspec-a1" in g.out("branch", "--list")


def test_cleanup_refuses_a_worktree_this_run_never_claimed(capsys, repo_pair):
    """`--worktree` and `--branch` are argv, and `cleanup`'s action is
    `shutil.rmtree` plus `git branch -D`. `cmd_post_merge` already refuses an
    `--issue`/`--merge-sha` that disagrees with the record THIS RUN wrote;
    this is the same discipline on the one other command that deletes.

    IT MATTERS BECAUSE THE CONDUCTOR HOLDS TWO. `orchestrate.md` step 1 has a
    tick juggling a recovered run's worktree and its own next pick, and step
    1.3 and step 11 both invoke `cleanup` with a path read from somewhere
    else.

    BOTH FIXTURES CARRY OWNED NAMES, on purpose: the prefix guard cannot be
    what refuses, so what is measured here is the claim binding alone.

    PRECONDITION WRITTEN LITERALLY: `claim.json` is composed here rather than
    obtained by running `claim`, so this test keeps stating its own premise
    whatever shape that command grows."""
    _, clone = repo_pair
    g = Git(clone)
    claimed = clone.parent / recovery.worktree_name(8, 1)
    other = clone.parent / recovery.worktree_name(9, 1)
    g.worktree_add(claimed, "issue/8-x-a1", "master")
    g.worktree_add(other, "issue/9-y-a1", "master")
    commit_file(claimed, "ours.txt", "x\n", "our work")
    commit_file(other, "theirs.txt", "y\n", "the other run's work")
    ours_sha = g.merge_squash("issue/8-x-a1", "squash issue/8-x-a1")
    theirs_sha = g.merge_squash("issue/9-y-a1", "squash issue/9-y-a1")
    claim = clone / ".agent" / "runs" / "run-test" / "claim.json"
    claim.parent.mkdir(parents=True, exist_ok=True)
    claim.write_text(json.dumps({"issue": 8, "attempt": 1, "branch": "issue/8-x-a1",
                                 "worktree": str(claimed)}), encoding="utf-8")
    # (a) the WRONG worktree, everything else correct and internally consistent.
    code, out = run(capsys, clone, "cleanup", "--worktree", str(other), "--branch", "issue/9-y-a1",
                    "--merge-sha", theirs_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and out["error"] == "cleanup does not match this run's claim", out
    assert other.is_dir() and (other / "theirs.txt").exists()
    assert "issue/9-y-a1" in g.out("branch", "--list")
    # (b) the right worktree, the WRONG branch -- the argument that reaches
    # `git branch -D`.
    code, out = run(capsys, clone, "cleanup", "--worktree", str(claimed), "--branch", "issue/9-y-a1",
                    "--merge-sha", theirs_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and out["error"] == "cleanup does not match this run's claim", out
    assert claimed.is_dir() and "issue/9-y-a1" in g.out("branch", "--list")
    # (c) NON-VACUITY: what this run DID claim is still removed. Without this
    # the binding could be "refuse everything" and (a) and (b) would pass.
    code, out = run(capsys, clone, "cleanup", "--worktree", str(claimed), "--branch", "issue/8-x-a1",
                    "--merge-sha", ours_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(claimed), out
    assert not claimed.exists() and "issue/8-x-a1" not in g.out("branch", "--list")


def test_cleanup_of_a_verification_worktree_takes_the_detached_route(capsys, repo_pair):
    """The retained `tree-dirty-after-gates` evidence tree is DETACHED: it has
    no branch and no merge SHA, and the two removers in `recovery` end in
    `git branch -D` and refuse this prefix outright. So `cleanup` routes it to
    the module that owns the prefix -- and refuses the three arguments that
    cannot mean anything for it, rather than silently ignoring them.

    WHY THIS TEST EXISTS AT ALL: a directory the system retains and cannot
    remove is the same defect shape as an incident row nothing can close. The
    success path is driven end-to-end by the tree-dirty halt test; this one
    pins the refusals and the library-level refusal that makes the route
    necessary.

    PRECONDITION HAND-STATED: the directory is a real detached worktree at
    `master`, created here with the production helper that owns the name."""
    _, clone = repo_pair
    g = Git(clone)
    kept = worktrees.create(g, worktrees.verify_path(clone),
                            g.rev("master"))
    assert kept.name.startswith(worktrees.VERIFY_PREFIX) and kept.is_dir()
    # The library refusal that makes the route necessary, stated first.
    with pytest.raises(RuntimeError, match="not an agentflow issue worktree"):
        recovery.remove_worktree(Ctx(Paths(clone), run_id="run-test"), g, kept, "issue/8-x-a1",
                                 expected_parent=clone.parent, merge_sha=g.rev("master"))
    assert kept.is_dir()
    # Arguments that cannot apply are REFUSED, not ignored -- an argument
    # silently dropped is how a caller learns the wrong thing about what ran.
    for extra in (["--branch", "issue/8-x-a1"], ["--branch", ""], ["--merge-sha", g.rev("master")],
                  ["--spike"]):
        code, out = run(capsys, clone, "cleanup", "--worktree", str(kept), *extra,
                        gh_run=FakeRunner(), run_id="run-test")
        assert code == 2, (extra, out)
        assert "detached" in out["error"], (extra, out)
        assert kept.is_dir(), extra
    # ...and an ISSUE worktree still requires `--branch`, so making it
    # optional did not make it optional everywhere.
    code, out = run(capsys, clone, "cleanup", "--worktree",
                    str(clone.parent / recovery.worktree_name(8, 1)), "--merge-sha",
                    g.rev("master"), gh_run=FakeRunner(), run_id="run-test")
    assert code == 2 and "--branch is required" in out["error"], out
    # THE ROUTE ITSELF.
    code, out = run(capsys, clone, "cleanup", "--worktree", str(kept),
                    gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(kept), out
    assert not kept.exists()


def test_cleanup_of_a_retained_verification_worktree_survives_the_dead_run_s_stale_lock(
        capsys, repo_pair, monkeypatch):
    """The documented exit must be reachable by the operator who actually
    arrives -- one holding no run id, long after the run that retained the
    tree died.

    WHY THIS EXISTS: `post-merge`'s `tree-dirty-after-gates` stop RETAINS the
    verification worktree and HALTs WITHOUT releasing the lock. So by
    construction the operator who comes to remove that tree meets a FOREIGN,
    STALE lock. `cmd_cleanup`'s fence used to refuse any foreign lock at all,
    above the detached route -- which refused the one invocation the HALT
    reason, CHANGELOG.md and orchestrate.md all name as the exit. The
    retained directory then had no supported way to be removed, which is the
    same defect shape as an incident row nothing can close.

    A LIVE foreign lock must STILL refuse: another run is acting on this
    checkout and its own post-merge may own that very tree. Only staleness
    opens the carve-out, mirroring `lock.require_operator` and
    `cmd_preflight`, which both fail on a live lock and let a stale one by.

    PRECONDITIONS HAND-STATED, not produced by driving post-merge: the tree
    is a real detached worktree made by the production helper that owns the
    name, and the lock is written directly with a heartbeat this test chooses
    -- so neither depends on post-merge still being able to create them.

    Since the merge lane (parallel-issues commit 12) the "foreign lock" that
    matters is the LANE holder's: only its post-merge makes verify trees.
    The retaining run still holds the lane on that stop, so it is
    hand-stated here too. A sibling's lease WITHOUT the lane no longer
    refuses (the last arm)."""
    _, clone = repo_pair
    g = Git(clone)
    kept = worktrees.create(g, worktrees.verify_path(clone), g.rev("master"))
    assert kept.name.startswith(worktrees.VERIFY_PREFIX) and kept.is_dir()

    dead = Ctx(Paths(clone), run_id="dead-run", now=lambda: 1_000_000.0)
    lk = lock.acquire(dead, 8, "s", 1)
    write_json(dead, clone / ".agent" / "merge-lane.json",
               {"run_id": "dead-run", "issue": 8, "pr": 12, "merge_sha": g.rev("master"), "acquired": 1.0})

    # ARM 1 -- the foreign lock is LIVE: refused, and the tree is kept.
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + 1))
    code, out = run(capsys, clone, "cleanup", "--worktree", str(kept),
                    gh_run=FakeRunner(), run_id=None)
    assert code == 1 and "FenceError" in out["error"], out
    assert kept.is_dir()

    # ARM 2 -- the same lock, now STALE: the operator's exit works.
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + lock.STALE_SECONDS + 1))
    code, out = run(capsys, clone, "cleanup", "--worktree", str(kept),
                    gh_run=FakeRunner(), run_id=None)
    assert code == 0 and out["removed"] == str(kept), out
    assert not kept.exists()

    # ARM 3 -- staleness does NOT open the fence for an ISSUE worktree. The
    # carve-out is scoped to the one detached route, not to cleanup at large.
    issue_wt = clone.parent / recovery.worktree_name(8, 3)
    g.worktree_add(issue_wt, "issue/8-z-a3", "master")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(issue_wt),
                    "--branch", "issue/8-z-a3", "--merge-sha", g.rev("master"),
                    gh_run=FakeRunner(), run_id=None)
    assert code == 1 and "FenceError" in out["error"], out
    assert issue_wt.exists()

    # ARM 4 -- a LIVE sibling that does NOT hold the lane is no reason to
    # keep a verify tree it never made (before the lane: refused).
    _slots(clone, 2)
    later = lk.heartbeat + lock.STALE_SECONDS + 1
    lock.acquire(Ctx(Paths(clone), run_id="sibling-run", now=lambda: later), 9, "s", 1)
    again = worktrees.create(g, worktrees.verify_path(clone), g.rev("master"))
    code, out = run(capsys, clone, "cleanup", "--worktree", str(again),
                    gh_run=FakeRunner(), run_id=None)
    assert code == 0 and not again.exists(), out


def test_cleanup_refuses_a_verify_named_directory_outside_the_checkouts_parent(
        capsys, repo_pair, tmp_path):
    """The name prefix alone is not ownership.

    This is the ONE `worktrees.destroy` call whose path arrives from ARGV --
    every other caller passes a path `worktrees.verify_path` built, so location
    is guaranteed there by construction. `worktrees._guard` proves only the
    NAME, so before the location conjunct any directory anywhere on disk that
    happened to carry the prefix was one `cleanup --worktree` away from a
    recursive delete. `recovery._refuse_unowned_path` already states the rule
    for its own removers: two conjuncts, because either alone is close to
    nothing.

    PRECONDITION HAND-STATED: a directory carrying the real production prefix,
    placed deliberately OUTSIDE the checkout's parent, holding a file whose
    survival is the assertion. Nothing about this test depends on production
    code still being able to create such a path."""
    _, clone = repo_pair
    # One level DEEPER than tmp_path: the `repo_pair` fixture builds the clone
    # directly under tmp_path, so tmp_path IS clone.parent and a sibling there
    # would be accepted for the right reason, testing nothing.
    outsider = tmp_path / "elsewhere" / f"{worktrees.VERIFY_PREFIX}merge-deadbeefcafe"
    outsider.mkdir(parents=True)
    (outsider / "precious.txt").write_text("must survive\n", encoding="utf-8")
    assert outsider.parent != clone.parent

    code, out = run(capsys, clone, "cleanup", "--worktree", str(outsider),
                    gh_run=FakeRunner(), run_id="run-test")
    assert code == 1, out
    assert "outside this checkout" in out["error"], out
    assert (outsider / "precious.txt").exists(), "the refusal must not have deleted anything"

    # ...and the SAME name inside the checkout's parent is still accepted, so
    # the conjunct narrowed the path rather than closing the documented exit.
    g = Git(clone)
    kept = worktrees.create(g, worktrees.verify_path(clone),
                            g.rev("master"))
    code, out = run(capsys, clone, "cleanup", "--worktree", str(kept),
                    gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(kept), out
    assert not kept.exists()


def test_cleanup_without_a_claim_record_is_still_allowed(capsys, repo_pair):
    """THE TOLERANCE, stated as its own test because it is what keeps the
    binding a narrowing rather than an outage. An operator cleaning up by hand
    after a crash has no `claim.json` -- and neither does a run whose evidence
    directory was already retained -- so an absent record must not refuse.
    `recovery._refuse_unowned_path` is what covers that case instead.

    PRECONDITION ASSERTED, not assumed: the record really is absent."""
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(8, 3)
    g.worktree_add(wt, "issue/8-x-a3", "master")
    commit_file(wt, "f.txt", "f\n", "work")
    merge_sha = g.merge_squash("issue/8-x-a3", "squash issue/8-x-a3")
    assert not (clone / ".agent" / "runs" / "run-test" / "claim.json").exists()
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-x-a3",
                    "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt), out
    assert not wt.exists()


def test_cleanup_refuses_foreign_lock_and_keeps_worktree(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(8, 2)
    g.worktree_add(wt, "issue/8-y-a1", "master")
    commit_file(wt, "issue2.txt", "y\n", "issue work 2")
    merge_sha = g.merge_squash("issue/8-y-a1", "squash issue/8-y-a1")
    lock.acquire(Ctx(Paths(clone), run_id="owner-run"), 8, "s", 1)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-y-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    assert wt.exists()


def test_cleanup_of_one_issue_is_not_fenced_by_a_siblings_lease_for_another(capsys, repo_pair):
    """Design test 36. The fence for an issue worktree is that ISSUE's lease:
    a sibling holding issue 9 does not stop the cleanup of issue 8's tree,
    while another run holding issue 8 does (the test above)."""
    _, clone = repo_pair
    g = Git(clone)
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "config.json").write_text(json.dumps({"slots": 2}), encoding="utf-8")
    wt = clone.parent / recovery.worktree_name(8, 2)
    g.worktree_add(wt, "issue/8-y-a1", "master")
    commit_file(wt, "issue2.txt", "y\n", "issue work 2")
    merge_sha = g.merge_squash("issue/8-y-a1", "squash issue/8-y-a1")
    lock.acquire(Ctx(Paths(clone), run_id="sibling-run"), 9, "s", 1)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-y-a1",
                    "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="other-run")
    assert code == 0, out
    assert not wt.exists()
    assert lock.for_issue(Ctx(Paths(clone)), 9).run_id == "sibling-run"


def test_recover_of_a_merged_stale_run_claims_the_lock_for_the_recovering_run(capsys, repo_pair, monkeypatch):
    # Review round 1, Important 6: recovering a merged stale run must not stop
    # at unlinking the old lock -- the recovering run needs its OWN lock and
    # budget/claim.json so post-merge/discoveries/cleanup/finish can run
    # fenced, exactly like any other claimed work.
    _, clone = repo_pair
    old_ctx = Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0)
    lk = lock.acquire(old_ctx, 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + 4000))
    gh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1",
                                               "headRefOid": "h", "mergeCommit": {"oid": "abc"}, "mergedAt": "x"}])})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0
    assert out["action"] == "merged-needs-post-merge" and out["merge_sha"] == "abc" and out["branch"] == "issue/8-x-a1"
    expected_worktree = str(clone.parent / recovery.worktree_name(8, 1))
    assert out["worktree"] == expected_worktree
    lk2 = lock.read(Ctx(Paths(clone)))
    assert lk2 is not None and lk2.run_id == "run-new" and lk2.issue == 8 and lk2.attempt == 1
    claim = json.loads((clone / ".agent" / "runs" / "run-new" / "claim.json").read_text())
    assert claim == {"issue": 8, "attempt": 1, "branch": "issue/8-x-a1", "worktree": expected_worktree}


def test_recover_refuses_to_mint_a_merge_record_from_a_previous_attempts_pr(
        capsys, repo_pair, monkeypatch):
    """THE CONSEQUENCE, at the call site: no trusted record is written.

    test_recovery.py pins `recover_stale`'s verdict; this one pins what
    `cmd_recover` does with it, because the verdict only matters through the
    `MergeRecord` it mints -- and that record is what makes
    `cmd_post_merge`'s provenance check pass BY CONSTRUCTION. A record minted
    from a previous attempt's long-merged PR aims a full gate re-run, and
    potentially a REVERT, at a months-old commit.

    PRECONDITION HAND-STATED: the stale lock is on attempt 2; GitHub reports
    attempt 1's PR as MERGED (ranked first) and attempt 2's as OPEN. The merged
    PR is dated AFTER `lk.started` so the ATTEMPT filter is the only thing that
    can refuse it -- an earlier date would let the recency guard refuse
    instead, and this test would then survive the removal of the filter it
    exists to pin (measured)."""
    _, clone = repo_pair
    old_ctx = Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0)
    lk = lock.acquire(old_ctx, 8, "s", 2)
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + 4000))
    gh = FakeRunner({"pr list *": json.dumps([
        {"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1", "headRefOid": "h",
         "mergeCommit": {"oid": "a" * 40}, "mergedAt": "1970-02-01T00:00:00Z"},
        {"number": 4, "state": "OPEN", "headRefName": "issue/8-x-a2", "headRefOid": "h2",
         "mergeCommit": None, "mergedAt": None}]),
        "issue comment 8 *": "", "issue edit 8 --add-label agent-blocked": "",
        "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0 and out["action"] == "blocked-crashed", out
    assert not (clone / ".agent" / "runs" / "run-new" / "merge.json").exists()
    assert "merge_sha" not in out and "worktree" not in out
    assert lock.read(Ctx(Paths(clone))) is None      # the stale lock is still freed


def test_recover_records_the_merge_it_adopted_and_post_merge_accepts_it(capsys, repo_pair, monkeypatch):
    """A recovering run performed no merge of its own, so it has no `merge` to
    mint the record. It must write the equivalent from the merged PR it just
    read -- otherwise the provenance check would silently retire the recovery
    follow-through, which is the one path that legitimately post-merges a
    commit this process did not create. Asserted BOTH ways: the record's exact
    contents, and that `post-merge` then accepts it."""
    _, clone = repo_pair
    g = Git(clone)
    # A REAL commit, unlike the sibling test's "abc": post-merge has to check
    # it out and diff it against its parent.
    merged = commit_file(clone, "src/thing.rs", "fn main() {}\n", "squash merge of #8")
    assert g.push("origin", "master")
    lk = lock.acquire(Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0), 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", str(lk.heartbeat + 4000))
    gh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "MERGED",
                                               "headRefName": "issue/8-x-a1", "headRefOid": "h",
                                               "mergeCommit": {"oid": merged}, "mergedAt": "x"}])})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0 and out["action"] == "merged-needs-post-merge"
    assert json.loads((clone / ".agent" / "runs" / "run-new" / "merge.json").read_text()) == {
        "issue": 8, "pr": 3, "merge_sha": merged, "run_id": "run-new", "source": "recover"}
    monkeypatch.setattr(cli, "gates", lambda: [("noop", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("noop-cdo", [sys.executable, "-c", "pass"], 1))
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merged,
                    gh_run=FakeRunner(), run_id="run-new")
    assert code == 0 and out["ok"] is True and out["revert"] is None


def test_a_recovery_follow_through_that_dies_mid_gates_is_recoverable_again(
        capsys, repo_pair, monkeypatch):
    """RECOVER -> post-merge dies mid-gates -> RECOVER. The chain no test in
    this suite performed, and the one that measures whether the self-heal
    `orchestrate.md` promises actually exists.

    THE DEFECT IT PINS. `recover_stale` refuses a PR whose `mergedAt`
    predates `lk.started`, and `cmd_recover` used to mint its follow-through
    lock with `started=now` -- i.e. AFTER the merge it had just adopted. On
    every recovery-minted lock the refusal then held BY CONSTRUCTION, so a
    post-merge killed mid-gates inside a follow-through degraded to
    `blocked-crashed` forever: `agent-blocked` on the issue, the conductor
    told to continue the tick, and master left carrying a merge nothing
    verified behind a `verifying` incident row that fails nothing.

    PRECONDITIONS HAND-STATED AS LITERALS, and the three timestamps are
    ORDERED rather than arbitrary -- that ordering is the whole test:

        claimed    1_000_000.0  the ORIGINAL claim
        merged_at  1_002_000.0  AFTER the claim   (so recover #1 adopts it)
        first_tick 1_004_000.0  AFTER the merge   (the recovering tick)

    A `mergedAt` later than `first_tick` would be adopted either way and the
    test would prove nothing; the real world puts the merge between the two,
    because the tick that recovers a dead run starts after the merge it is
    recovering. The literal `"mergedAt": "x"` the older follow-through
    fixtures carry is unparseable, which `_merged_before` treats as False --
    inert, and it would make this test inert too."""
    _, clone = repo_pair
    claimed = 1_000_000.0                  # 1970-01-12T13:46:40Z
    merged_at = "1970-01-12T14:20:00Z"     # 1_002_000.0
    first_tick = claimed + 4_000.0         # 1_004_000.0, and > lk.heartbeat + STALE
    lk = lock.acquire(Ctx(Paths(clone), run_id="old-run", now=lambda: claimed), 8, "s", 1)
    assert lk.started == claimed and lk.heartbeat == claimed   # the precondition, stated
    prs = json.dumps([{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1",
                       "headRefOid": "h", "mergeCommit": {"oid": "a" * 40},
                       "mergedAt": merged_at}])
    monkeypatch.setenv("AGENTFLOW_NOW", str(first_tick))
    code, out = run(capsys, clone, "recover", gh_run=FakeRunner({"pr list *": prs}), run_id="run-2")
    assert code == 0 and out["action"] == "merged-needs-post-merge", out

    follow = lock.read(Ctx(Paths(clone)))
    assert follow is not None and follow.run_id == "run-2" and follow.session == "recover"
    # The heartbeat is NOT carried back. `is_stale` reads the heartbeat
    # alone, so a lock born stale is one any concurrent tick's preflight would
    # report as recoverable while the follow-through is still running.
    assert follow.heartbeat == first_tick, follow
    assert not lock.is_stale(follow, first_tick)

    # POST-MERGE DIES MID-GATES. That leaves exactly this: the follow-through
    # lock still on disk, no HALT, no terminal incident. Nothing to simulate
    # beyond letting the clock run past the heartbeat.
    second_tick = first_tick + lock.STALE_SECONDS + 1.0
    assert lock.is_stale(follow, second_tick)
    monkeypatch.setenv("AGENTFLOW_NOW", str(second_tick))
    # The blocked-crashed gh calls are stubbed so a REGRESSION fails on the
    # assertion below rather than on a missing fake -- a 404 from FakeRunner
    # would be a different failure telling a different story.
    gh2 = FakeRunner({"pr list *": prs, "issue comment 8 *": "",
                      "issue edit 8 --add-label agent-blocked": "",
                      "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, clone, "recover", gh_run=gh2, run_id="run-3")
    assert code == 0, out
    assert out["action"] == "merged-needs-post-merge", out   # NOT blocked-crashed
    assert out["merge_sha"] == "a" * 40
    # THE CONSEQUENCE: the merge record post-merge demands is minted again, so
    # the follow-through can actually re-run.
    assert json.loads((clone / ".agent" / "runs" / "run-3" / "merge.json").read_text()) == {
        "issue": 8, "pr": 3, "merge_sha": "a" * 40, "run_id": "run-3", "source": "recover"}
    assert not any("agent-blocked" in c for c in gh2.calls), gh2.calls

    # AND THE THIRD, because "it works twice" is not "it works every time"
    # and the shape that failed here is a date being RE-STAMPED. The claim
    # date has to propagate through every follow-through, not just the first:
    # if recovery N ever re-dated the lock to `now`, recovery N+1 would refuse
    # its own work again and this whole defect would be back with the suite
    # green. Driven, not reasoned about.
    third_tick = second_tick + lock.STALE_SECONDS + 1.0
    monkeypatch.setenv("AGENTFLOW_NOW", str(third_tick))
    gh3 = FakeRunner({"pr list *": prs, "issue comment 8 *": "",
                      "issue edit 8 --add-label agent-blocked": "",
                      "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, clone, "recover", gh_run=gh3, run_id="run-4")
    assert code == 0 and out["action"] == "merged-needs-post-merge", out
    third = lock.read(Ctx(Paths(clone)))
    assert third is not None and third.started == claimed, third
    assert third.heartbeat == third_tick, third
    # THE FIELD THE FIX LIVES IN, asserted LAST on purpose: a regression must
    # fail on the OUTCOME above (an unrecoverable follow-through) rather than
    # on a field value, or the proof only shows that the field was read.
    assert follow.started == claimed, follow


# ---- recovery per issue (parallel-issues design, commit 7) -----------------

def _stale_pair(clone):
    """Two stale leases, 8 and 9, with a worktree each. Written by acquiring
    at time 1.0 under `slots: 2`, so both are stale at any later clock."""
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "config.json").write_text(json.dumps({"slots": 2}), encoding="utf-8")
    for run_id, issue in (("dead-8", 8), ("dead-9", 9)):
        lock.acquire(Ctx(Paths(clone), run_id=run_id, now=lambda: 1.0), issue, "s", 1)
        (clone.parent / recovery.worktree_name(issue, 1)).mkdir()


BLOCKED_9 = {"pr list *": "[]", "issue comment 9 *": "", "issue edit 9 --add-label agent-blocked": "",
             "issue edit 9 --remove-label agent-working": ""}


def test_recover_with_two_stale_leases_needs_an_issue_and_touches_only_that_one(
        capsys, repo_pair, monkeypatch):
    """Design test 27. With more than one stale lease, `recover` must be told
    which (exit 2, listing them). `recover --issue 9` then moves only issue
    9's worktree, labels only issue 9 and drops only its lease."""
    _, clone = repo_pair
    _stale_pair(clone)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    gh = FakeRunner(BLOCKED_9)
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-r")
    assert code == 2, out
    assert [lk["issue"] for lk in out["stale_locks"]] == [8, 9], out
    assert gh.calls == [] and len(lock.all_locks(Ctx(Paths(clone)))) == 2
    before_8 = lock.for_issue(Ctx(Paths(clone)), 8)
    code, out = run(capsys, clone, "recover", "--issue", "9", gh_run=gh, run_id="run-r")
    assert code == 0 and out["action"] == "blocked-crashed", out
    assert all(" 9 " in f" {c} " or c.startswith("pr list") for c in gh.calls), gh.calls
    assert lock.for_issue(Ctx(Paths(clone)), 9) is None
    assert lock.for_issue(Ctx(Paths(clone)), 8) == before_8                 # untouched
    assert (clone.parent / recovery.worktree_name(8, 1)).exists()
    assert not (clone.parent / recovery.worktree_name(9, 1)).exists()
    assert list(clone.parent.glob(f"{recovery.worktree_name(9, 1)}.crashed-*"))


def test_recover_of_a_live_lease_by_issue_is_lock_live(capsys, repo_pair, monkeypatch):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="alive", now=lambda: 1_000_000.0), 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    gh = FakeRunner({})
    code, out = run(capsys, clone, "recover", "--issue", "8", gh_run=gh, run_id="run-r")
    assert code == 1 and out["error"] == "lock-live:alive", out
    assert gh.calls == []


STEAL_CHILD = """
from agentflow import lock
from agentflow.state import Ctx, Paths
ctx = Ctx(Paths(ROOT), run_id=f"rec-{TAG}", now=lambda: 1_000_000.0)
stale = lock.for_issue(ctx, 8)          # both sessions see it stale, BEFORE the barrier
assert lock.is_stale(stale, ctx.now())
slow_reads("issue-8.json")
barrier()
try:
    lock.steal_for_recovery(ctx, stale)
    print("RESULT stolen")
except lock.LockHeld as e:
    print("RESULT", e.code)
"""


def test_two_sessions_recovering_one_stale_lease_at_once_one_proceeds(root):
    """Design test 28, two real processes. Both saw the lease stale; exactly
    one steals it. The steal makes it LIVE (heartbeat now), so the other gets
    `lock-live` and leaves it alone. The lease keeps the stale run's id, so
    `recover_stale`'s old-run gh fence still passes."""
    from agentflow.tests.conftest import run_concurrently
    lock.acquire(Ctx(Paths(root), run_id="dead-8", now=lambda: 1.0), 8, "s", 1)
    res = run_concurrently(root, STEAL_CHILD, ["a", "b"])
    results = sorted(o.split("RESULT")[-1].strip() for _, o, _ in res)
    assert results == ["lock-live:dead-8", "stolen"], res
    lk = lock.for_issue(Ctx(Paths(root)), 8)
    assert lk.run_id == "dead-8" and lk.recovering_by in ("rec-a", "rec-b")
    assert lk.heartbeat == 1_000_000.0


def test_a_merged_path_recovery_never_leaves_the_issue_without_a_lease(capsys, repo_pair, monkeypatch):
    """Design test 29. `recover_stale` used to unlink the stale lease and
    return, and `cmd_recover` acquired the follow-through lease only after:
    in between, issue 8 had no lease and a sibling could claim it. Checked
    INSIDE a wrapped `recover_stale` (before and after the real one) and at
    the end."""
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="old-run", now=lambda: 1_000_000.0), 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", str(1_000_000.0 + 4000))
    real, seen = recovery.recover_stale, []

    def watching(ctx, git, gh, lk, worktrees_parent):
        seen.append(lock.for_issue(ctx, 8))
        out = real(ctx, git, gh, lk, worktrees_parent=worktrees_parent)
        seen.append(lock.for_issue(ctx, 8))
        return out

    monkeypatch.setattr(recovery, "recover_stale", watching)
    gh = FakeRunner({"pr list *": json.dumps([{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1",
                                               "headRefOid": "h", "mergeCommit": {"oid": "abc"}, "mergedAt": "x"}])})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-new")
    assert code == 0 and out["action"] == "merged-needs-post-merge", out
    assert len(seen) == 2 and all(lk is not None for lk in seen), seen
    assert seen[0].recovering_by == "run-new" and seen[0].run_id == "old-run"   # stolen, not new
    final = lock.for_issue(Ctx(Paths(clone)), 8)
    assert (final.run_id, final.started, final.recovering_by) == ("run-new", 1_000_000.0, None)


def test_a_recovery_lock_still_refuses_a_merge_that_predates_the_original_claim(
        capsys, repo_pair, monkeypatch):
    """THE CONTRAST: carrying `started` forward must WIDEN nothing.

    A merge GitHub dates before the work was ever claimed still cannot be the
    merge of that work, and a recovery-minted lock must refuse it exactly like
    an original claim lock does. Without this, the sibling test above is
    satisfied by deleting the recency axis outright.

    PRECONDITION HAND-STATED: the lock is WRITTEN, not obtained by asking
    `cmd_recover` for one -- a test that has to drive production code to
    reach its own starting state stops working the moment that code changes
    shape, and a follow-through lock over a pre-claim merge is a state
    `cmd_recover` cannot produce at all (it would have refused the merge)."""
    _, clone = repo_pair
    claimed = 1_000_000.0
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "lock.json").write_text(json.dumps(
        {"issue": 8, "run_id": "run-2", "session": "recover", "started": claimed,
         "heartbeat": claimed, "attempt": 1}), encoding="utf-8")
    monkeypatch.setenv("AGENTFLOW_NOW", str(claimed + 4_000.0))
    gh = FakeRunner({"pr list *": json.dumps(
        [{"number": 3, "state": "MERGED", "headRefName": "issue/8-x-a1", "headRefOid": "h",
          "mergeCommit": {"oid": "a" * 40},
          "mergedAt": "1970-01-01T00:00:00Z"}]),          # 0.0 -- BEFORE the claim
        "issue comment 8 *": "", "issue edit 8 --add-label agent-blocked": "",
        "issue edit 8 --remove-label agent-working": ""})
    code, out = run(capsys, clone, "recover", gh_run=gh, run_id="run-3")
    assert code == 0 and out["action"] == "blocked-crashed", out
    assert "merge_sha" not in out
    assert not (clone / ".agent" / "runs" / "run-3" / "merge.json").exists()


def test_cleanup_spike_removes_a_commit_free_worktree_and_branch(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(9, 1)
    g.worktree_add(wt, "issue/9-spike-a1", "master")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a1", "--spike",
                     gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert not wt.exists() and "issue/9-spike-a1" not in g.out("branch", "--list")


def test_cleanup_spike_refuses_a_branch_with_a_commit(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(9, 2)
    g.worktree_add(wt, "issue/9-spike-a2", "master")
    commit_file(wt, "probe.txt", "code, not just a read-only probe result\n", "spike accidentally committed code")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a2", "--spike",
                     gh_run=FakeRunner(), run_id="run-test")
    assert code == 1 and "not spike-clean" in out["error"]
    assert wt.exists()


def test_cleanup_rejects_spike_and_merge_sha_together(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(9, 3)
    g.worktree_add(wt, "issue/9-spike-a3", "master")
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-spike-a3", "--spike",
                     "--merge-sha", "deadbeef", gh_run=FakeRunner(), run_id="run-test")
    assert code == 2 and "mutually exclusive" in out["error"]
    assert wt.exists()


def test_cleanup_succeeds_when_worktree_already_removed_by_hand(capsys, repo_pair):
    # Review round 2, New Breakage 5: a crash between cleanup deleting the
    # directory and `finish` running must not turn a re-run into a permanent
    # stuck lock -- an already-absent worktree is success.
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(8, 5)
    g.worktree_add(wt, "issue/8-vanished-a1", "master")
    commit_file(wt, "issue.txt", "x\n", "issue work")
    merge_sha = g.merge_squash("issue/8-vanished-a1", "squash issue/8-vanished-a1")
    shutil.rmtree(wt)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/8-vanished-a1",
                     "--merge-sha", merge_sha, gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert "issue/8-vanished-a1" not in g.out("branch", "--list")


def test_cleanup_spike_succeeds_when_worktree_already_removed_by_hand(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    wt = clone.parent / recovery.worktree_name(9, 6)
    g.worktree_add(wt, "issue/9-vanished-spike-a1", "master")
    shutil.rmtree(wt)
    code, out = run(capsys, clone, "cleanup", "--worktree", str(wt), "--branch", "issue/9-vanished-spike-a1",
                     "--spike", gh_run=FakeRunner(), run_id="run-test")
    assert code == 0 and out["removed"] == str(wt)
    assert "issue/9-vanished-spike-a1" not in g.out("branch", "--list")


def test_file_discoveries_refuses_a_file_carrying_a_customer_path(capsys, root, tmp_path, monkeypatch):
    # C1: the discoveries file is conductor-written and every field in it is
    # published verbatim to a PUBLIC repository, so the file is scanned before
    # it is even parsed -- nothing is filed when it carries a CDO_WS path.
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    f = tmp_path / "discoveries.json"
    f.write_text(json.dumps([{
        "subsystem": "resolve", "locator": "U:/Git/CDO/App/Src/Thing.al",
        "symptom": "edge dropped", "kind": "bug", "origin_issue": 8,
        "reproducer": "aldump --program-call-graph-stats", "pre_existing": True,
        "capability": "x resolves", "acceptance": "y",
    }]), encoding="utf-8")
    gh = FakeRunner({"issue list *": "[]", "issue create *": "https://github.com/SShadowS/al-sem/issues/90\n"})
    code, out = run(capsys, root, "file-discoveries", str(f), "--session", "https://s", gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert out["violations"][0]["kind"] == "cdo-path"
    assert gh.calls == []


def test_file_discoveries_scans_the_same_bytes_it_parses(capsys, root, tmp_path, monkeypatch):
    # N6: scanning one read and parsing another leaves the scanned bytes only
    # probably equal to the published ones. One read, one string.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    f = tmp_path / "discoveries.json"
    f.write_text("[]", encoding="utf-8")
    reads = []
    real_read_text = Path.read_text

    def counting(self, *a, **kw):
        if str(self) == str(f):
            reads.append(str(self))
        return real_read_text(self, *a, **kw)

    monkeypatch.setattr(Path, "read_text", counting)
    code, out = run(capsys, root, "file-discoveries", str(f), "--session", "https://s",
                     gh_run=FakeRunner({"issue list *": "[]"}))
    assert code == 0 and out["filed"] == []
    assert len(reads) == 1, reads


def test_finish_refuses_a_reason_that_leaks_a_dependency_path(capsys, root):
    # C1: a spike answer and a block reason are both posted as issue comments.
    # The refusal must land BEFORE any label or comment, so a rejected text
    # leaves no half-finished bookkeeping behind.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = FakeRunner({"issue edit 8 --add-label agent-blocked": "", "issue edit 8 --remove-label agent-working": "",
                     "issue comment 8 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "blocked",
                     "--reason", "fails only against .alpackages/Microsoft_Base Application.app",
                     gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert out["violations"][0]["kind"] == "alpackages-path"
    assert gh.calls == []
    assert lock.read(Ctx(Paths(root))) is not None  # nothing terminal happened


def test_finish_accepts_an_answer_from_a_reason_file(capsys, root, tmp_path, monkeypatch):
    # I9: a spike's `## Answer` is multi-line markdown. Passing it as an argv
    # argument is fragile through two shells and capped near 32 KB, so the
    # conductor hands over a file instead.
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    ctx = Ctx(Paths(root), run_id="run-test")
    lock.acquire(ctx, 8, "s", 1)
    ctx.run_dir.mkdir(parents=True)  # `claim` makes this; retention copies it
    answer = tmp_path / "answer.md"
    answer.write_text("## Answer\n\nYes -- `resolve_in_table_scope` already covers it.\n\n- one\n- two\n")
    gh = FakeRunner({"issue edit 8 --add-label agent-answered": "", "issue edit 8 --remove-label agent-working": "",
                     "issue comment 8 *": ""})
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "answered",
                     "--reason-file", str(answer), gh_run=gh)
    assert code == 0 and out["outcome"] == "answered"
    assert any(c.startswith("issue comment 8") for c in gh.calls)


def test_finish_regressed_releases_the_lock_without_relabelling(capsys, root, tmp_path, monkeypatch):
    # I8: post-merge's revert path has already labelled the issue
    # `agent-regressed` and set HALT. The tick still needs its terminal
    # bookkeeping -- release the lock, retain the evidence -- without a second
    # label transition and without being refused by the HALT it just set.
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    ctx = Ctx(Paths(root), run_id="run-test")
    lock.acquire(ctx, 8, "s", 1)
    ctx.run_dir.mkdir(parents=True)  # `claim` makes this; retention copies it
    lock.set_halt(ctx, "regression: merge deadbeef failed post-merge gates")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, root, "finish", "--issue", "8", "--outcome", "regressed", gh_run=gh)
    assert code == 0 and out["outcome"] == "regressed"
    assert gh.calls == []
    assert lock.read(Ctx(Paths(root))) is None
    assert (tmp_path / "home" / ".al-sem" / "agentflow" / "runs" / "run-test").exists()


# ---- I4: PR creation, PR comments and branch pushes belong to the executor --
# Before this, all three were conductor-side raw `gh`/`git`: outside the
# dry-run guard, outside the fence, outside the HALT check, and outside the
# sanitizer -- while the spec and CHANGELOG both claimed every mutation went
# through the executor.

def test_pr_create_draft_and_pr_ready(capsys, repo_pair):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner({"pr create*": "https://github.com/SShadowS/al-sem/pull/12\n",
                     f"pr ready 12 --repo {REPO}": ""})
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "issue/8-x", "--draft", gh_run=gh)
    assert code == 0 and out == {"pr": 12}
    assert gh.calls[-1].startswith("pr create") and gh.calls[-1].endswith("--draft")
    code, out = run(capsys, clone, "pr-ready", "--pr", "12", gh_run=gh)
    assert code == 0 and out == {"ready": 12} and gh.calls[-1] == f"pr ready 12 --repo {REPO}"
    # With the final body: edited first, then ready.
    gh.responses[f"pr edit 12 --repo {REPO} --body-file*"] = ""
    n = len(gh.calls)
    code, out = run(capsys, clone, "pr-ready", "--pr", "12", "--body-file", str(body), gh_run=gh)
    assert code == 0 and gh.calls[n].startswith("pr edit 12") and gh.calls[n + 1] == f"pr ready 12 --repo {REPO}"
    # A body the sanitizer refuses publishes nothing.
    body.write_text("fails only against .alpackages/Microsoft_Base Application.app\n")
    n = len(gh.calls)
    code, out = run(capsys, clone, "pr-ready", "--pr", "12", "--body-file", str(body), gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed" and gh.calls[n:] == []


def test_pr_ready_is_an_external_write(capsys, repo_pair):
    _, clone = repo_pair
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-ready", "--pr", "12", gh_run=gh, dry=True)
    assert code == 2 and "error" in out and gh.calls == []


def test_pr_create_refuses_master_as_head(capsys, repo_pair):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "master", gh_run=gh)
    assert code == 1 and out["error"] == "refusing to open a PR from master"
    assert gh.calls == []


def test_pr_create_refuses_the_cross_repo_and_malformed_head_spellings(capsys, repo_pair):
    # N5: `owner:master` is the cross-repo spelling of the same head, and a
    # head that is not a plain branch name at all is a conductor error worth
    # naming here rather than as a confusing error from GitHub.
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner(readonly=True)
    for head in ("SShadowS:master", "SShadowS:refs/heads/master", "refs/heads/master", "-x", "feat branch"):
        arg = [f"--head={head}"] if head.startswith("-") else ["--head", head]
        code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                         *arg, gh_run=gh)
        assert code == 1 and "error" in out, head
    assert gh.calls == []


def test_pr_create_refuses_a_body_that_leaks_a_customer_path(capsys, repo_pair, monkeypatch):
    _, clone = repo_pair
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("## Gate results\n\ncdo-gate log: U:/Git/CDO/App/Src/Thing.al\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 1 and out["error"] == "sanitize-failed"
    assert gh.calls == []


def test_pr_create_and_pr_comment_refuse_without_a_lock(capsys, repo_pair):
    _, clone = repo_pair
    body = clone / "body.md"
    body.write_text("ledger\n")
    gh = FakeRunner(readonly=True)
    code, out = run(capsys, clone, "pr-create", "--title", "T", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 1 and "FenceError" in out["error"]
    code, out = run(capsys, clone, "pr-comment", "--pr", "12", "--body-file", str(body), gh_run=gh)
    assert code == 1 and "FenceError" in out["error"]
    assert gh.calls == []


def test_pr_create_then_comment_under_the_lock(capsys, repo_pair):
    _, clone = repo_pair
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    body = clone / "body.md"
    body.write_text("## Ledger\n\nall gates green\n")
    gh = FakeRunner({"pr create *": "https://github.com/SShadowS/al-sem/pull/77\n",
                     "pr comment *": ""})
    code, out = run(capsys, clone, "pr-create", "--title", "c10: scope (#8)", "--body-file", str(body),
                     "--head", "issue/8-x-a1", gh_run=gh)
    assert code == 0 and out["pr"] == 77
    assert any(c.startswith("pr create --title c10: scope (#8)") and "--head issue/8-x-a1 --base master" in c
               for c in gh.calls)
    code, out = run(capsys, clone, "pr-comment", "--pr", "77", "--body-file", str(body), gh_run=gh)
    assert code == 0 and out["commented"] == 77
    assert any(c.startswith(f"pr comment 77 --repo {REPO} --body-file") for c in gh.calls)


def test_push_branch_refuses_the_master_literal(capsys, repo_pair):
    # B4: this test covers the literal spelling only. The refspec and full-ref
    # forms, and the "remote master is unmoved" property, are pinned by
    # test_push_branch_refuses_every_argument_that_is_not_a_plain_branch_name.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    before = g.rev("origin/master")
    code, out = run(capsys, clone, "push-branch", "--branch", "master", gh_run=FakeRunner(readonly=True))
    assert code == 1 and out["error"] == "refusing to push master"
    assert g.rev("origin/master") == before


def test_push_branch_refuses_a_case_variant_of_master(capsys, repo_pair):
    # B3: on a case-insensitive filesystem `refs/heads/Master` resolves to the
    # local `master` ref, so a case-sensitive comparison let the call through
    # to `git push`. Git then refused it on its own ref collision -- but the
    # thing that stopped it must be this guard, not the remote's luck: against
    # a case-sensitive remote the same call creates a stray branch carrying
    # master's commits.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    for spelling in ("Master", "MASTER"):
        before = g.out("ls-remote", "origin")
        code, out = run(capsys, clone, "push-branch", "--branch", spelling, gh_run=FakeRunner(readonly=True))
        assert code == 1 and out["error"] == "refusing to push master", spelling
        assert g.out("ls-remote", "origin") == before, spelling


def test_push_branch_refuses_every_argument_that_is_not_a_plain_branch_name(capsys, repo_pair):
    # N1 (Critical): a colon in a push argument means "push this local ref onto
    # THAT remote ref". `feat:master` is not a ref, so a name comparison sees
    # neither `master` nor anything resolving to it, and `git push origin
    # <arg>` then moves remote master -- with --force-with-lease, destroying
    # whatever was only there. The remote SHA is asserted around EVERY case,
    # because a refusal that still wrote is the failure being guarded against.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    g._run("checkout", "-q", "-b", "feat")
    commit_file(clone, "src/x.rs", "x\n", "work only on feat")
    g.checkout("master")
    for ref in ("refs/heads/master", "HEAD", "feat:master", "HEAD:master",
                "feat:refs/heads/master", "-x", "feat branch", "feat:feat"):
        # `--branch=-x` rather than `--branch -x`: argparse refuses the spaced
        # form as a usage error before the executor ever sees the value, so
        # only the `=` form actually reaches the validator under test.
        arg = [f"--branch={ref}"] if ref.startswith("-") else ["--branch", ref]
        before = g.out("ls-remote", "origin", "refs/heads/master")
        code, out = run(capsys, clone, "push-branch", *arg, gh_run=FakeRunner(readonly=True))
        after = g.out("ls-remote", "origin", "refs/heads/master")
        assert code == 1 and "error" in out, ref
        assert after == before, f"remote master moved for {ref!r}"


def test_push_branch_force_with_lease_cannot_reach_master_either(capsys, repo_pair):
    # The same argument under --force-with-lease is the destructive variant:
    # it would overwrite a commit that exists only on remote master.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    commit_file(clone, "precious.txt", "only on remote master\n", "precious")
    assert g.push("origin", "master")
    precious = g.out("ls-remote", "origin", "refs/heads/master")
    g._run("checkout", "-q", "-b", "feat", "HEAD~1")
    commit_file(clone, "other.txt", "x\n", "diverged work")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "feat:master", "--force-with-lease",
                     gh_run=FakeRunner(readonly=True))
    assert code == 1 and "error" in out
    assert g.out("ls-remote", "origin", "refs/heads/master") == precious


def test_push_branch_refuses_without_a_lock(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    head = commit_file(clone, "src/x.rs", "x\n", "work")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    assert code == 1 and "FenceError" in out["error"]
    assert not g.ok("rev-parse", "--verify", "origin/issue/8-x-a1")
    assert head  # the branch exists locally; only the push was refused


def test_push_branch_pushes_a_feature_branch_and_sets_upstream(capsys, repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    head = commit_file(clone, "src/x.rs", "x\n", "work")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["pushed"] == "issue/8-x-a1" and out["head"] == head
    g.fetch()
    assert g.rev("origin/issue/8-x-a1") == head


def test_push_branch_force_with_lease_after_a_rebase(capsys, repo_pair):
    # The force-with-lease carve-out becomes enforceable rather than
    # aspirational only if the force form is issued by code that refuses
    # `master` -- which is the whole point of routing it through here.
    _, clone = repo_pair
    g = Git(clone)
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    g._run("checkout", "-q", "-b", "issue/8-x-a1")
    commit_file(clone, "src/x.rs", "x\n", "work")
    run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", gh_run=FakeRunner(readonly=True))
    g._run("commit", "-q", "--amend", "-m", "work, rebased")  # history rewritten
    rewritten = g.rev("HEAD")
    g.checkout("master")
    code, out = run(capsys, clone, "push-branch", "--branch", "issue/8-x-a1", "--force-with-lease",
                     gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["head"] == rewritten
    g.fetch()
    assert g.rev("origin/issue/8-x-a1") == rewritten


def test_run_fences_against_a_foreign_lock_and_flags_unsupervised(capsys, root):
    # Pin I6: a lock owned by another run refuses `run` outright; no lock at
    # all runs unsupervised and says so in the JSON.
    lock.acquire(Ctx(Paths(root), run_id="owner-run"), 8, "s", 1)
    code, out = run(capsys, root, "run", "--name", "probe", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), run_id="other-run")
    assert code == 1 and "FenceError" in out["error"]
    lock.release(Ctx(Paths(root), run_id="owner-run"))
    code, out = run(capsys, root, "run", "--name", "probe2", "--timeout", "1", "--",
                     sys.executable, "-c", "print('x')", gh_run=FakeRunner(), run_id="solo-run")
    assert code == 0 and out["supervised"] is False


def test_run_with_sibling_leases_is_supervised_for_the_owner_and_fenced_for_anyone_else(
        capsys, root, monkeypatch):
    """With slots, a sibling's lease always exists. `run` is supervised for a
    run that owns a lease, a FenceError for a run that owns none while ANY
    lease exists (even when there are two, so the old single-lock `read`
    found none and let it run unsupervised), and unsupervised only when no
    lease exists at all."""
    from agentflow import budget
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    (root / ".agent" / "config.json").write_text(json.dumps({"slots": 3}), encoding="utf-8")
    owner = Ctx(Paths(root), run_id="run-a", now=lambda: 1_000_000.0)
    lock.acquire(owner, 8, "s", 1)
    budget.init(owner, claimed_at=1_000_000.0)
    lock.acquire(Ctx(Paths(root), run_id="run-b", now=lambda: 1_000_000.0), 9, "s", 1)
    lock.acquire(Ctx(Paths(root), run_id="run-c", now=lambda: 1_000_000.0), 10, "s", 1)
    argv = ("run", "--name", "probe", "--timeout", "1", "--", sys.executable, "-c", "print('x')")
    code, out = run(capsys, root, *argv, gh_run=FakeRunner(), run_id="run-a")
    assert code == 0 and out["supervised"] is True, out
    code, out = run(capsys, root, *argv, gh_run=FakeRunner(), run_id="run-lost")
    assert code == 1 and "FenceError" in out["error"], out


# ---- the gate semaphore (parallel-issues design, commit 11) -----------------

GATE_CHILD = """
import json
from agentflow import cli
cli.QUEUE_POLL_S = 0.05
barrier()
code = cli.main(["--root", str(ROOT), "--repo", "o/r", "--run-id", f"run-{TAG}", "run", "--name", ARGS[0],
                 "--timeout", "1", "--", sys.executable, "-c",
                 "import time; a = time.time(); time.sleep(2); print(a, time.time())"])
print("EXIT", code)
"""


def _gate_window(out_text):
    payload = json.loads(out_text.rsplit("EXIT", 1)[0])
    start, end = map(float, Path(payload["log"]).read_text().split())
    return payload, start, end


def test_two_heavy_gates_do_not_overlap_and_the_second_reports_its_queue(root):
    """Design test 15. Two processes run a heavy gate at once with
    `heavy_gates: 1`: the children's time windows do not overlap, and the
    one that waited says so in `queued_s`."""
    from agentflow.tests.conftest import run_concurrently
    res = run_concurrently(root, GATE_CHILD, ["a", "b"], extra=["check-goldens"])
    assert all(o.strip().endswith("EXIT 0") for _, o, _ in res), res
    (pa, sa, ea), (pb, sb, eb) = (_gate_window(o) for _, o, _ in res)
    assert ea <= sb or eb <= sa, (sa, ea, sb, eb)
    assert max(pa["queued_s"], pb["queued_s"]) > 1.0, (pa, pb)


def test_with_two_heavy_gate_slots_two_heavy_gates_run_at_once(root):
    """The other side of test 15: `heavy_gates: 2` lets both run together,
    so the semaphore is the config number, not a fixed one."""
    from agentflow.tests.conftest import run_concurrently
    (root / ".agent" / "config.json").write_text(json.dumps({"heavy_gates": 2}), encoding="utf-8")
    res = run_concurrently(root, GATE_CHILD, ["a", "b"], extra=["ci-steps-all"])
    (pa, sa, ea), (pb, sb, eb) = (_gate_window(o) for _, o, _ in res)
    assert sa < eb and sb < ea, (sa, ea, sb, eb)      # the windows overlap


def test_a_light_gate_never_queues_and_a_queued_heavy_gate_times_out_without_running(
        capsys, root, monkeypatch):
    """Design tests 16 and 19. Held by a sibling (here, this test), the gate
    semaphore leaves a light gate (`check-goldens-coverage`, `ci-wait`) alone.
    A heavy one waits; past `gate_wait_minutes` it is `failure:
    gate-queue-timeout` with `exit_code: null`, and its child never started.
    The heavy set is the names the conductor and attest already use, plus
    anything `-on-revert`."""
    from agentflow.state import mutex
    monkeypatch.setattr(cli, "QUEUE_POLL_S", 0.01)
    (root / ".agent" / "config.json").write_text(json.dumps({"gate_wait_minutes": 0}), encoding="utf-8")
    marker = root / "child-ran"
    child = ("--", sys.executable, "-c", f"open({str(marker)!r}, 'w').close()")
    with mutex(Ctx(Paths(root)), "gate-0"):
        for name in ("check-goldens-coverage", "ci-wait"):
            code, out = run(capsys, root, "run", "--name", name, "--timeout", "1", *child, gh_run=FakeRunner())
            assert code == 0 and out["queued_s"] == 0 and marker.exists(), out
            marker.unlink()
        for name in ("ci-steps-all", "check-goldens", "check-goldens-regen", "cdo-gate", "x-on-revert"):
            code, out = run(capsys, root, "run", "--name", name, "--timeout", "1", *child, gh_run=FakeRunner())
            assert code == 1 and out["failure"] == "gate-queue-timeout" and out["exit_code"] is None, (name, out)
            assert not marker.exists(), name


def test_a_queued_gate_beats_and_stops_on_halt_unless_told_not_to(ctx, monkeypatch):
    """Design tests 17 and 18, at the `_run_gate` level. While it waits, a
    run that owns a lease keeps its heartbeat moving. HALT written while it
    waits ends the wait as `halted-while-queued` with the child never
    started; `post-merge` passes `halt_aborts=False`, and then HALT does not
    end the wait (here it runs into `gate-wait` instead)."""
    from agentflow import budget
    from agentflow.state import mutex
    clock = iter(range(1_000_000, 2_000_000, 10))
    ticking = Ctx(paths=ctx.paths, run_id="run-test", now=lambda: float(next(clock)))
    lock.acquire(ticking, 8, "s", 1)
    budget.init(ticking, claimed_at=1_000_000.0)
    before = lock.own(ticking).heartbeat
    monkeypatch.setattr(cli, "QUEUE_POLL_S", 0.01)
    monkeypatch.setattr(cli, "QUEUE_BEAT_S", 0)
    ctx.paths.config.write_text(json.dumps({"gate_wait_minutes": 0}), encoding="utf-8")
    marker = ctx.paths.root / "child-ran"
    cmd = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
    with mutex(ctx, "gate-0"):
        with pytest.raises(cli.GateQueued) as e:
            cli._run_gate(ticking, "check-goldens", cmd, 1, ctx.paths.root)
        assert e.value.failure == "gate-queue-timeout"
        assert lock.own(ticking).heartbeat > before
        lock.set_halt(ctx, "operator")
        ctx.paths.config.write_text(json.dumps({"gate_wait_minutes": 1}), encoding="utf-8")
        with pytest.raises(cli.GateQueued) as e:
            cli._run_gate(ticking, "check-goldens", cmd, 1, ctx.paths.root)
        assert e.value.failure == "halted-while-queued"
        ctx.paths.config.write_text(json.dumps({"gate_wait_minutes": 0}), encoding="utf-8")
        with pytest.raises(cli.GateQueued) as e:
            cli._run_gate(ticking, "check-goldens", cmd, 1, ctx.paths.root, halt_aborts=False)
        assert e.value.failure == "gate-queue-timeout"
    assert not marker.exists()


def test_a_bench_takes_every_gate_slot(root, monkeypatch):
    """A CDO A/B bench measures wall clock and peaks, so nothing heavy may build
    beside it. With `heavy_gates: 2`, one held slot leaves the other free for a
    normal gate, but a `bench` waits for BOTH, and while it runs no gate starts."""
    import contextlib
    from agentflow.state import mutex
    ctx = Ctx(Paths(root), run_id="run-test")
    monkeypatch.setattr(cli, "QUEUE_POLL_S", 0.01)
    ctx.paths.config.write_text(json.dumps({"heavy_gates": 2, "gate_wait_minutes": 0}), encoding="utf-8")
    marker = ctx.paths.root / "child-ran"
    cmd = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
    with mutex(ctx, "gate-0"):
        with pytest.raises(cli.GateQueued):
            cli._run_gate(ctx, "bench", cmd, 1, ctx.paths.root)
        assert not marker.exists()
        cli._run_gate(ctx, "check-goldens", cmd, 1, ctx.paths.root)    # gate-1 is free
        assert marker.exists()
    marker.unlink()
    held = contextlib.ExitStack()
    cli._gate_slot(ctx, "bench", True, held)                           # both slots
    with held:
        with pytest.raises(cli.GateQueued):
            cli._run_gate(ctx, "check-goldens", cmd, 1, ctx.paths.root)
        assert not marker.exists()


def test_a_killed_holder_frees_the_gate_semaphore(root):
    """Design test 20. The semaphore is an OS lock, so a holder that dies
    frees it: a gate started after the kill does not wait."""
    holder = subprocess.Popen([sys.executable, "-c", (
        "import sys, time; from pathlib import Path; sys.path.insert(0, sys.argv[1])\n"
        "from agentflow.state import Ctx, Paths, mutex\n"
        "with mutex(Ctx(Paths(Path(sys.argv[2]))), 'gate-0'):\n"
        "    (Path(sys.argv[2]) / 'held').write_text('')\n"
        "    time.sleep(120)\n"), str(Path(cli.__file__).parents[1]), str(root)])
    try:
        for _ in range(600):
            if (root / "held").exists():
                break
            import time
            time.sleep(0.05)
        assert (root / "held").exists()
        (root / ".agent" / "config.json").write_text(json.dumps({"gate_wait_minutes": 0}), encoding="utf-8")
        ctx = Ctx(Paths(root), run_id="run-test")
        with pytest.raises(cli.GateQueued):
            cli._run_gate(ctx, "check-goldens", [sys.executable, "-c", "pass"], 1, root)
    finally:
        holder.kill()
        holder.wait()
    r = cli._run_gate(ctx, "check-goldens", [sys.executable, "-c", "pass"], 1, root)
    assert r.exit_code == 0 and r.queued_s < 1.0


def test_post_merge_routes_a_gate_queue_timeout_to_halt_only_even_under_halt(
        capsys, repo_pair, monkeypatch):
    """Design test 19, the post-merge half. `post-merge` runs under HALT, and
    so does its queue (`halt_aborts=False`): a HALT already present does not
    end the wait. A queue timeout is COULD NOT VERIFY, never PROVEN BAD: it
    takes the `gate-timed-out` HALT-only route and never the revert path."""
    from agentflow.state import mutex
    _, clone = repo_pair
    monkeypatch.setattr(cli, "QUEUE_POLL_S", 0.01)
    monkeypatch.setattr(cli, "gates", lambda: [("ci-steps-all", [sys.executable, "-c", "pass"], 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("cdo-gate", [sys.executable, "-c", "pass"], 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "config.json").write_text(json.dumps({"gate_wait_minutes": 0}), encoding="utf-8")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    lock.set_halt(Ctx(Paths(clone)), "a sibling's post-merge stopped")

    def no_revert(*a, **kw):
        raise AssertionError("revert path reached on a gate that never ran")

    monkeypatch.setattr(recovery, "post_merge_failure", no_revert)
    gh = FakeRunner({
        f"api repos/{REPO}/labels?per_page=100 --paginate --slurp":
            json.dumps([[{"name": n} for n in recovery.INCIDENT_LABELS]]),
        "issue edit 8 --add-label *": "", "issue edit 8 --remove-label agent-working": "",
        "issue comment 8 *": "",
    })
    with mutex(Ctx(Paths(clone)), "gate-0"):
        code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=gh)
    assert code == 1 and out["ok"] is False and out["revert"] is None, out
    assert out["halt"]["reason"] == "gate-timed-out", out["halt"]
    assert out["timeouts"] == ["ci-steps-all"] and out["gates"] == {}, out


def test_post_merge_under_halt_waits_for_the_gate_semaphore_and_verifies(capsys, repo_pair, monkeypatch):
    """The test above cannot tell `halt_aborts=False` from a queue that gives
    up on HALT: both end on the HALT-only route. Here the semaphore is held
    for about a second under HALT and then released. A post-merge whose
    queue ignored HALT waits, runs its gates and verifies the merge."""
    import threading
    from agentflow.state import mutex
    _, clone = repo_pair
    monkeypatch.setattr(cli, "QUEUE_POLL_S", 0.05)
    _noop_heavy = [sys.executable, "-c", "pass"]
    monkeypatch.setattr(cli, "gates", lambda: [("ci-steps-all", _noop_heavy, 1)])
    monkeypatch.setattr(cli, "cdo_gate", lambda: ("cdo-gate", _noop_heavy, 1))
    merge_sha = commit_file(clone, "src/thing.rs", "fn main() {}\n", "code change")
    assert Git(clone).push("origin", "master")
    lock.acquire(Ctx(Paths(clone), run_id="run-test"), 8, "s", 1)
    record_merge(clone, merge_sha)
    lock.set_halt(Ctx(Paths(clone)), "a sibling's post-merge stopped")
    held, release = threading.Event(), threading.Event()

    def sibling():
        with mutex(Ctx(Paths(clone)), "gate-0"):
            held.set()
            release.wait(10)

    t = threading.Thread(target=sibling)
    t.start()
    held.wait(10)
    threading.Timer(1.0, release.set).start()
    code, out = run(capsys, clone, "post-merge", "--issue", "8", "--merge-sha", merge_sha, gh_run=FakeRunner())
    t.join()
    assert code == 0 and out["ok"] is True, out
    assert out["gates"] == {"ci-steps-all": 0, "cdo-gate": 0} and out["timeouts"] == []


def test_each_gate_invocation_gets_its_own_log(ctx):
    # #43: `logs/<name>.log` was shared by every run under one name, so a
    # replacement run truncated a killed run's log and their output interleaved.
    a = cli._run_gate(ctx, "probe", [sys.executable, "-c", "print('first')"], 1, ctx.paths.root)
    b = cli._run_gate(ctx, "probe", [sys.executable, "-c", "print('second')"], 1, ctx.paths.root)
    assert a.log_path != b.log_path
    assert a.log_path.parent.name == b.log_path.parent.name == "probe"
    assert "first" in a.log_path.read_text() and "second" in b.log_path.read_text()


# ---- #45: a design converged at the spec-panel cap is handed off -----------

# #34's register, the shape the issue was filed over: an `important` finding
# one reviewer still re-raised against wording a later spec revision fixed.
# Stated literally, never produced by the code under test.
ISSUE34_REGISTER = [
    {"id": "A1", "severity": "important", "disposition": "fixed",
     "reviews": {"astra": "re-raised", "flash": "accepted"}},
    {"id": "A2", "severity": "minor", "disposition": "refuted",
     "reviews": {"astra": "accepted", "flash": "accepted"}},
    {"id": "A3", "severity": "minor", "disposition": "deferred",
     "reviews": {"astra": "accepted", "flash": "accepted"}},
]
HANDOFF_GH = {**READS, "issue edit 8 --add-label agent-working": "", "issue comment 8 *": "",
              "issue edit 8 --add-label agent-design-ready": "",
              "issue edit 8 --remove-label agent-working": "",
              "issue edit 8 --remove-label agent-design-ready": ""}
BRANCH_A1 = "issue/8-c10-scope-a1"


def handoff_branch(clone, register, *, branch=BRANCH_A1, ledger=True):
    """The previous attempt's branch carrying its spec and evidence, committed
    by hand. Built BEFORE any claim: checking master out again removes the
    committed `.agent/issue-8/` files from the working tree and nothing else."""
    g = Git(clone)
    g._run("checkout", "-q", "-b", branch)
    commit_file(clone, "docs/superpowers/specs/2026-10-13-issue-8-design.md", "# Design\n", "spec")
    if ledger:
        commit_file(clone, ".agent/issue-8/ledger.md", "## Design\n", "ledger")
    sha = commit_file(clone, ".agent/issue-8/findings.json", json.dumps(register), "register")
    g.checkout("master")
    return sha


def claim8(capsys, clone, gh, run_id="run-test"):
    return run(capsys, clone, "claim", "8", "--session", "https://s", "--title-slug", "c10-scope",
               gh_run=gh, run_id=run_id)


def finish_handoff(capsys, clone, sha, gh, run_id="run-test"):
    return run(capsys, clone, "finish", "--issue", "8", "--outcome", "handoff", "--handoff-ref", sha,
               "--reason", "spec-panel-cap", gh_run=gh, run_id=run_id)


def handoffs(clone):
    p = clone / ".agent" / "handoffs.json"
    return json.loads(p.read_text()) if p.exists() else {}


@pytest.fixture
def fake_home(tmp_path, monkeypatch):
    """`finish` retains the run under `Path.home()`. A refusal test that
    unexpectedly SUCCEEDS must not write into the real one."""
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")


def test_a_design_converged_at_the_cap_is_handed_off_and_the_next_claim_starts_from_it(
        capsys, repo_pair, tmp_path, monkeypatch):
    """#45. A tick stopped at the spec-panel cap with a converged register used
    to `finish --outcome blocked`, whose `agent-blocked` is in the exclusion
    list -- so `fetch` dropped the issue and the design waited for a human.
    Here the handoff labels `agent-design-ready` instead, the issue stays
    eligible, and the next claim carries the design and consumes the record."""
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    gh = FakeRunner(HANDOFF_GH)
    code, _ = claim8(capsys, clone, gh)
    assert code == 0
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 0, out
    assert "issue edit 8 --add-label agent-design-ready" in gh.calls
    assert not any("agent-blocked" in c for c in gh.calls), gh.calls
    assert lock.read(Ctx(Paths(clone))) is None
    rec = handoffs(clone)["8"]
    assert (rec["sha"], rec["branch"], rec["from_attempt"], rec["from_run"]) == (sha, BRANCH_A1, 1, "run-test")
    assert rec["body_hash"] == mergeops.body_hash("## Acceptance\nx")
    assert ".agent/issue-8/findings.json" in rec["files"]

    # The trap: the label the handoff leaves must not remove the issue from fetch.
    labelled = json.loads(ISSUES)
    labelled[0][0]["labels"] = [{"name": "agent-design-ready"}]
    code, out = run(capsys, clone, "fetch", gh_run=FakeRunner({**READS, READS_ISSUES_KEY: json.dumps(labelled)}),
                    run_id=None)
    assert [i["number"] for i in out["eligible"]] == [8], out

    gh2 = FakeRunner(HANDOFF_GH)
    code, out = claim8(capsys, clone, gh2, run_id="run-2")
    assert code == 0 and out["attempt"] == 2
    assert out["handoff"]["sha"] == sha
    claim = json.loads((clone / ".agent" / "runs" / "run-2" / "claim.json").read_text())
    assert claim["handoff"]["sha"] == sha
    assert "8" not in handoffs(clone)
    assert "issue edit 8 --remove-label agent-design-ready" in gh2.calls


def test_a_handoff_measures_its_files_from_origin_master_not_a_lagging_local_master(
        capsys, repo_pair, fake_home):
    """The shared checkout's local `master` lags while another session's merge waits
    for its post-merge. Measured from it, the handoff's `files` would also carry what
    that merge brought in, and the next attempt would `git restore` those files too."""
    _, clone = repo_pair
    g = Git(clone)
    lagging = g.rev("HEAD")
    commit_file(clone, "upstream.txt", "u\n", "another session's merge")
    assert g.push("origin", "master")
    sha = handoff_branch(clone, ISSUE34_REGISTER)          # cut from origin/master
    _git_raw(clone, "checkout", "-q", "--detach")
    _git_raw(clone, "branch", "-f", "master", lagging)     # local master one merge behind
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 0, out
    files = handoffs(clone)["8"]["files"]
    assert "upstream.txt" not in files and ".agent/issue-8/findings.json" in files, files


@pytest.mark.parametrize("fix, ok", [("spec", True), ("code", False)])
def test_the_handoff_check_applies_the_prose_rule_to_the_branchs_own_commits(
        capsys, repo_pair, fake_home, fix, ok):
    """P3 on the handoff path: an `accepted-by-rule` mark passes only for a fix commit on
    origin/master..sha that touches prose alone; a code fix leaves the mark unknown."""
    _, clone = repo_pair
    g = Git(clone)
    g._run("checkout", "-q", "-b", BRANCH_A1)
    shas = {"spec": commit_file(clone, "docs/superpowers/specs/2026-10-13-issue-8-design.md", "# D\n", "spec"),
            "code": commit_file(clone, "src/x.rs", "x\n", "code")}
    commit_file(clone, ".agent/issue-8/ledger.md", "## Design\n", "ledger")
    register = [{"id": "F1", "source": "astra", "severity": "minor", "disposition": "fixed",
                 "fix_commit": shas[fix], "reviews": {"astra": "accepted", "flash": "accepted-by-rule"}}]
    sha = commit_file(clone, ".agent/issue-8/findings.json", json.dumps(register), "register")
    g.checkout("master")
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    code, out = finish_handoff(capsys, clone, sha, gh)
    if ok:
        assert code == 0, out
    else:
        assert code == 1 and out["error"] == "handoff-not-ready" and out["entries"] == ["F1"], out


READS_ISSUES_KEY = f"api repos/{REPO}/issues?state=open&per_page=100 --paginate --slurp"


def _entry(eid, severity, disposition, astra="accepted", flash="accepted"):
    reviews = {k: v for k, v in (("astra", astra), ("flash", flash)) if v is not None}
    return {"id": eid, "severity": severity, "disposition": disposition, "reviews": reviews}


@pytest.mark.parametrize("bad", [
    _entry("X", "minor", "open"),
    _entry("X", "minor", "fixed", astra="re-raised", flash="re-raised"),
    _entry("X", "important", "deferred"),
    _entry("X", "critical", "fixed", flash="unreviewed"),
    _entry("X", "minor", "fixed", flash=None),
    _entry("X", "High", "fixed"),
    _entry("X", "minor", "wontfix"),
], ids=["open", "both-re-raised", "blocking-deferred", "unreviewed", "mark-missing",
        "severity-unknown", "disposition-unknown"])
def test_a_register_that_has_not_converged_is_not_handed_off(capsys, repo_pair, fake_home, bad):
    """Each case is ONE bad entry beside #34's handoff-ready entries, so the
    refusal can only come from that entry. Nothing leaves the machine and the
    lock is kept, so the conductor can fall back to `--outcome blocked`."""
    _, clone = repo_pair
    sha = handoff_branch(clone, [*ISSUE34_REGISTER, bad])
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 1 and out["error"] == "handoff-not-ready" and out["entries"] == ["X"], out
    assert gh.calls == []
    assert lock.read(Ctx(Paths(clone))).run_id == "run-test"
    assert handoffs(clone) == {}


def test_a_handoff_ref_that_is_short_or_off_branch_is_refused(capsys, repo_pair, fake_home):
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    # Not on the claimed branch: a commit that only exists on a sibling. Its
    # register DIFFERS, because identical content, message, parent and second
    # make git mint the very same SHA -- and then it IS on the branch.
    stray = handoff_branch(clone, ISSUE34_REGISTER[:1], branch="issue/8-other-a9")
    assert stray != sha and not Git(clone).is_ancestor(stray, BRANCH_A1)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    for ref, error in ((sha[:12], "sha-not-full"), (stray, "handoff-not-on-branch")):
        gh = FakeRunner(HANDOFF_GH)
        code, out = finish_handoff(capsys, clone, ref, gh)
        assert code == 1 and out["error"] == error, out
        assert gh.calls == [] and handoffs(clone) == {}
    assert lock.read(Ctx(Paths(clone))).run_id == "run-test"


def test_a_handoff_without_a_committed_ledger_is_refused(capsys, repo_pair, fake_home):
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER, ledger=False)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 1 and out["error"] == "handoff-evidence-missing", out
    assert gh.calls == [] and handoffs(clone) == {}


def test_a_handoff_is_void_when_the_issue_was_edited_since(capsys, repo_pair, tmp_path, monkeypatch):
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    assert finish_handoff(capsys, clone, sha, gh)[0] == 0
    edited = {**json.loads(ISSUES)[0][0], "body": "## Acceptance\nsomething else"}
    gh2 = FakeRunner({**HANDOFF_GH, f"api repos/{REPO}/issues/8": json.dumps(edited)})
    code, out = claim8(capsys, clone, gh2, run_id="run-2")
    assert code == 0, out
    assert "handoff" not in out and out["handoff_void"] == "issue-edited"
    assert "8" not in handoffs(clone)
    assert "issue edit 8 --remove-label agent-design-ready" in gh2.calls


def test_a_pre_ratified_attempt_cannot_hand_off_again(capsys, repo_pair, tmp_path, monkeypatch):
    """One handoff per issue: an attempt that started from a handed-off design
    never ran a spec panel, so it has no cap of its own to hand off from."""
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    assert finish_handoff(capsys, clone, sha, gh)[0] == 0
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH), run_id="run-2")[0] == 0
    gh3 = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff(capsys, clone, sha, gh3, run_id="run-2")
    assert code == 1 and out["error"] == "handoff-already-used", out
    assert gh3.calls == [] and handoffs(clone) == {}


def test_a_rolled_back_claim_does_not_consume_the_handoff(capsys, repo_pair, tmp_path, monkeypatch):
    monkeypatch.setattr("pathlib.Path.home", lambda: tmp_path / "home")
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    assert finish_handoff(capsys, clone, sha, gh)[0] == 0
    failing = FakeRunner({**HANDOFF_GH, "issue comment 8 *": (1, "", "HTTP 500: boom")})
    code, out = claim8(capsys, clone, failing, run_id="run-2")
    assert code == 1 and "claim rolled back" in out["error"]
    assert handoffs(clone)["8"]["sha"] == sha
    assert "issue edit 8 --remove-label agent-design-ready" not in failing.calls


# ---- #141: a handoff signed by declared stand-ins ---------------------------

# #34's register with its marks recorded under the stand-ins the operator
# named (sol for astra, opus for flash). Stated literally: the #141 reproducer
# is the first entry, `sol: re-raised, opus: accepted`.
STAND_IN_REGISTER = [
    {"id": "A1", "severity": "important", "disposition": "fixed",
     "reviews": {"sol": "re-raised", "opus": "accepted"}},
    {"id": "A2", "severity": "minor", "disposition": "refuted",
     "reviews": {"sol": "accepted", "opus": "accepted"}},
    {"id": "A3", "severity": "minor", "disposition": "deferred",
     "reviews": {"sol": "accepted", "opus": "accepted"}},
]
STAND_INS = ("--substitute", "astra=sol", "--substitute", "flash=opus")


def finish_handoff_with(capsys, clone, sha, gh, extra, run_id="run-test"):
    return run(capsys, clone, "finish", "--issue", "8", "--outcome", "handoff", "--handoff-ref", sha,
               "--reason", "spec-panel-cap", *extra, gh_run=gh, run_id=run_id)


def comment_bodies(gh):
    return [Path(c.split(" --body-file ", 1)[1]).read_text(encoding="utf-8")
            for c in gh.calls if c.startswith("issue comment 8 --body-file ")]


def test_a_register_marked_by_stand_ins_is_handed_off_with_substitute(capsys, repo_pair, fake_home):
    """#141. `attest` honours `--substitute`, but the handoff judged the rule
    against the fixed astra/flash roster, so a register the operator's
    stand-ins marked could never be handed off. With the same declaration
    the stand-ins' marks fill the slots, and the record and the public
    comment both say who signed."""
    _, clone = repo_pair
    sha = handoff_branch(clone, STAND_IN_REGISTER)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff_with(capsys, clone, sha, gh, STAND_INS)
    assert code == 0, out
    rec = handoffs(clone)["8"]
    assert rec["reviewers"] == {"astra": "sol", "flash": "opus"}
    assert "issue edit 8 --add-label agent-design-ready" in gh.calls
    [body] = [b for b in comment_bodies(gh) if "PRE-RATIFIED" in b]
    assert "sol (astra slot)" in body and "opus (flash slot)" in body, body


def test_a_register_marked_by_stand_ins_is_not_handed_off_without_substitute(capsys, repo_pair, fake_home):
    """The #141 bug, kept as a pin: without the declaration the astra and
    flash slots are empty for every entry, so a stand-in's marks never count
    by accident."""
    _, clone = repo_pair
    sha = handoff_branch(clone, STAND_IN_REGISTER)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 1 and out["error"] == "handoff-not-ready", out
    assert out["entries"] == ["A1", "A2", "A3"]
    assert out["reviewers"] == {"astra": "astra", "flash": "flash"}
    assert gh.calls == [] and handoffs(clone) == {}


@pytest.mark.parametrize("extra", [
    ("--substitute", "bob=sol"),                                     # no such slot
    ("--substitute", "astra=sol", "--substitute", "astra=x"),        # slot twice
    ("--substitute", "astra=sol", "--substitute", "flash=sol"),      # one reviewer, both slots
    ("--substitute", "sol"),                                         # malformed
], ids=["unknown-slot", "slot-twice", "one-reviewer-both-slots", "malformed"])
def test_a_handoff_with_a_bad_substitute_is_refused(capsys, repo_pair, fake_home, extra):
    _, clone = repo_pair
    sha = handoff_branch(clone, STAND_IN_REGISTER)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = finish_handoff_with(capsys, clone, sha, gh, extra)
    assert code == 1 and out["error"] == "bad-substitute", out
    assert gh.calls == []
    assert lock.read(Ctx(Paths(clone))).run_id == "run-test"
    assert handoffs(clone) == {}


def test_substitute_is_refused_on_an_outcome_other_than_handoff(capsys, repo_pair, fake_home):
    """A flag that is accepted and then ignored is a silent no-op: refuse it."""
    _, clone = repo_pair
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    gh = FakeRunner(HANDOFF_GH)
    code, out = run(capsys, clone, "finish", "--issue", "8", "--outcome", "blocked", "--reason", "x",
                    *STAND_INS, gh_run=gh)
    assert code == 2, out
    assert "--substitute" in out["error"]
    assert gh.calls == []
    assert lock.read(Ctx(Paths(clone))).run_id == "run-test"


def test_the_next_claim_carries_the_stand_ins_and_attest_fails_closed_without_them(
        capsys, repo_pair, fake_home):
    """The record's `reviewers` reach the next attempt through `claim`. Attest
    does NOT apply them on its own: the same declaration converges, and an
    attest without it is `register-not-converged`, never a silent pass."""
    _, clone = repo_pair
    sha = handoff_branch(clone, STAND_IN_REGISTER)
    assert claim8(capsys, clone, FakeRunner(HANDOFF_GH))[0] == 0
    assert finish_handoff_with(capsys, clone, sha, FakeRunner(HANDOFF_GH), STAND_INS)[0] == 0
    code, out = claim8(capsys, clone, FakeRunner(HANDOFF_GH), run_id="run-2")
    assert code == 0, out
    assert out["handoff"]["reviewers"] == {"astra": "sol", "flash": "opus"}
    claim = json.loads((clone / ".agent" / "runs" / "run-2" / "claim.json").read_text())
    assert claim["handoff"]["reviewers"] == {"astra": "sol", "flash": "opus"}

    # The final panel re-marked every carried entry under the same stand-ins.
    register = Path(claim["worktree"]) / ".agent" / "issue-8" / "findings.json"
    register.parent.mkdir(parents=True)
    register.write_text(json.dumps([{**e, "reviews": {"sol": "accepted", "opus": "accepted"}}
                                    for e in STAND_IN_REGISTER]))

    def attest2(*extra):
        return run(capsys, clone, "attest", "--issue", "8", "--B", "b" * 40, "--H", "c" * 40,
                   "--final-head", "f" * 40, "--register", str(register), "--gates", GREEN_GATES,
                   "--body-hash", claim["body_hash"], *extra, gh_run=FakeRunner(), run_id="run-2")

    code, out = attest2()
    assert code == 1 and out["error"] == "register-not-converged", out
    code, out = attest2(*STAND_INS)
    assert code == 0, out
    assert json.loads(Path(out["path"]).read_text())["reviewers"] == {"astra": "sol", "flash": "opus"}


def test_a_handoff_record_without_reviewers_still_claims(capsys, repo_pair, fake_home):
    """A record written before #141 has no `reviewers` key; it was signed by
    the named roster, and the next claim carries it as before."""
    _, clone = repo_pair
    old = {"from_run": "run-old", "from_attempt": 1, "branch": BRANCH_A1, "sha": "a" * 40,
           "body_hash": mergeops.body_hash("## Acceptance\nx"), "register_blob": "b" * 40, "files": []}
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "handoffs.json").write_text(json.dumps({"8": old}))
    code, out = claim8(capsys, clone, FakeRunner(HANDOFF_GH))
    assert code == 0, out
    assert out["handoff"] == old and "reviewers" not in out["handoff"]
    assert "8" not in handoffs(clone)


def test_a_claim_consuming_its_handoff_keeps_a_record_written_meanwhile(
        capsys, repo_pair, fake_home, monkeypatch):
    """`claim` reads `handoffs.json` early and consumes its own record only
    after its gh calls. A sibling run's `finish --outcome handoff` for ANOTHER
    issue can land in between, and writing back the early snapshot dropped it.

    PRECONDITION HAND-STATED: the sibling's record is written literally,
    inside `reconcile_pending`, which `claim` calls between its read and its
    consume."""
    from agentflow import discoveries
    _, clone = repo_pair
    old = {"from_run": "run-old", "from_attempt": 1, "branch": BRANCH_A1, "sha": "a" * 40,
           "body_hash": mergeops.body_hash("## Acceptance\nx"), "register_blob": "b" * 40, "files": []}
    sibling = {**old, "from_run": "run-sibling", "sha": "c" * 40}
    path = clone / ".agent" / "handoffs.json"
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps({"8": old}))

    def sibling_hands_off_issue_9(ctx, gh):
        path.write_text(json.dumps({**json.loads(path.read_text()), "9": sibling}))
        return []

    monkeypatch.setattr(discoveries, "reconcile_pending", sibling_hands_off_issue_9)
    code, out = claim8(capsys, clone, FakeRunner(HANDOFF_GH))
    assert code == 0, out
    assert out["handoff"] == old
    assert handoffs(clone) == {"9": sibling}


def test_finish_writes_the_handoff_record_while_holding_the_handoffs_mutex(
        capsys, repo_pair, fake_home, monkeypatch):
    """`finish --outcome handoff` adds its record to `handoffs.json`, which
    other runs also write. The write must happen INSIDE mutex `handoffs`, not
    merely after taking it once."""
    from contextlib import contextmanager
    from agentflow import state
    _, clone = repo_pair
    sha = handoff_branch(clone, ISSUE34_REGISTER)
    gh = FakeRunner(HANDOFF_GH)
    assert claim8(capsys, clone, gh)[0] == 0
    real_mutex, real_write, held, writes = state.mutex, state.write_json, set(), []

    @contextmanager
    def tracking(c, name, **kw):
        with real_mutex(c, name, **kw):
            held.add(name)
            try:
                yield
            finally:
                held.discard(name)

    def recording(c, p, data):
        writes.append((Path(p).name, "handoffs" in held))
        return real_write(c, p, data)

    monkeypatch.setattr(state, "mutex", tracking)
    monkeypatch.setattr(state, "write_json", recording)
    code, out = finish_handoff(capsys, clone, sha, gh)
    assert code == 0, out
    assert [w for w in writes if w[0] == "handoffs.json"] == [("handoffs.json", True)]
    assert handoffs(clone)["8"]["sha"] == sha


# ---- ci-rerun (flow speed-ups P8) -------------------------------------------

PR_HEAD = "a" * 40
CI_RUN_LIST = (f"run list --repo {REPO} --workflow ci.yml --commit {PR_HEAD} --limit 1 "
               f"--json databaseId,attempt,status,conclusion,headSha")


def ci_rerun_fakes(*, status="completed", conclusion="failure", head_sha=PR_HEAD, runs=None):
    """The two reads `ci-rerun` makes, plus both rerun spellings, stated literally."""
    if runs is None:
        runs = [{"databaseId": 555, "attempt": 1, "status": status, "conclusion": conclusion,
                 "headSha": head_sha}]
    return FakeRunner({
        f"pr view 12 --repo {REPO} --json headRefOid": json.dumps({"headRefOid": PR_HEAD}),
        CI_RUN_LIST: json.dumps(runs),
        f"run rerun 555 --repo {REPO} --failed": "",
        f"run rerun 555 --repo {REPO}": "",
    })


def ci_reruns(gh):
    return [c for c in gh.calls if c.startswith("run rerun")]


def gate_runs(root):
    p = root / ".agent" / "runs" / "run-test" / "gate-runs.jsonl"
    return [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []


def test_ci_rerun_reruns_only_the_failed_jobs_and_records_it(capsys, root):
    # #111: a flake on a head the diff cannot reach cost a `ci_fix`, a new head and a
    # second full CI run. A rerun keeps the head; the record names what was rerun.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes()
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 0, out
    assert ci_reruns(gh) == [f"run rerun 555 --repo {REPO} --failed"]
    [rec] = gate_runs(root)
    assert rec["name"] == "ci-rerun" and rec["head"] == PR_HEAD and rec["ci_run_id"] == 555
    assert rec["failed_only"] is True and rec["attempt"] == 1


def test_ci_rerun_of_a_green_run_reruns_all_of_it(capsys, root):
    # An acceptance that asks for repeat runs on an unchanged head (#190's D4): a green
    # run has no failed jobs, so `--failed` would rerun nothing.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes(conclusion="success")
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 0, out
    assert ci_reruns(gh) == [f"run rerun 555 --repo {REPO}"]


@pytest.mark.parametrize("fakes, error", [
    # Real `gh run list --json` gives an unfinished run `conclusion: ""`, not null.
    (dict(status="in_progress", conclusion=""), "ci-run-not-complete"),
    (dict(status="queued", conclusion=""), "ci-run-not-complete"),
    (dict(conclusion="cancelled"), "ci-run-conclusion-unknown"),
    (dict(conclusion=""), "ci-run-conclusion-unknown"),
    (dict(conclusion=None), "ci-run-conclusion-unknown"),
    (dict(runs=[]), "ci-run-not-found"),
    (dict(head_sha="b" * 40), "ci-run-not-found"),
])
def test_ci_rerun_fails_closed_on_anything_but_a_finished_run_of_this_head(capsys, root, fakes, error):
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes(**fakes)
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 1 and out["error"] == error, out
    assert ci_reruns(gh) == [] and gate_runs(root) == []


def test_ci_rerun_refuses_under_halt_dry_run_and_without_a_lock(capsys, root):
    # Guarded like `push-branch`: each refusal states its own precondition.
    gh = ci_rerun_fakes()
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)          # no lock
    assert code == 1 and "FenceError" in out["error"]
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh, dry=True)  # dry run
    assert code == 2 and "dry-run" in out["error"]
    lock.set_halt(Ctx(Paths(root), run_id="run-test"), "stop")
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)          # HALT
    assert code == 1 and "HaltError" in out["error"]
    assert ci_reruns(gh) == [] and gate_runs(root) == []


@pytest.mark.parametrize("head", ["abc123", "", None, "A" * 40, 42])
def test_ci_rerun_refuses_a_pr_head_that_is_not_a_full_sha(capsys, root, head):
    # The head is what the run lookup and the one-rerun record key on; a short,
    # missing, upper-case or non-string value must not reach either.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes()
    gh.responses[f"pr view 12 --repo {REPO} --json headRefOid"] = json.dumps({"headRefOid": head})
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 1 and out["error"] == "pr-head-unknown", out
    assert not any(c.startswith("run ") for c in gh.calls) and gate_runs(root) == []


def test_the_readonly_fake_counts_a_ci_rerun_as_a_write(capsys, root):
    # conftest's MUTATING list is what lets a write-freedom test see `gh run rerun`;
    # a readonly fake must trip on it rather than answer it.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes()
    gh.readonly = True
    with pytest.raises(AssertionError, match="mutating gh call under readonly: run rerun"):
        run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)


def set_run_conclusion(gh, conclusion):
    gh.responses[CI_RUN_LIST] = json.dumps([{"databaseId": 555, "attempt": 1, "status": "completed",
                                             "conclusion": conclusion, "headSha": PR_HEAD}])


def test_a_second_rerun_of_a_red_run_on_one_head_is_refused(capsys, root):
    # The rule allows ONE flake rerun per head; a second red is `blocked ci-flaky:<test>`.
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    gh = ci_rerun_fakes()
    assert run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)[0] == 0
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 1 and out["error"] == "rerun-already-used", out
    assert len(ci_reruns(gh)) == 1 and len(gate_runs(root)) == 1


def test_a_prior_rerun_record_of_unknown_shape_counts_as_a_red_rerun(capsys, root):
    # Fail closed: only `failed_only: false` proves a prior rerun was of a green run.
    from agentflow import identity
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    identity.append_record(Ctx(Paths(root), run_id="run-test"), {"name": "ci-rerun", "head": PR_HEAD})
    gh = ci_rerun_fakes()
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 1 and out["error"] == "rerun-already-used", out
    assert ci_reruns(gh) == []


def test_green_reruns_stay_allowed_and_are_counted(capsys, root):
    # Repeat runs an acceptance asks for (#190's D4) rerun a green run; they do not use
    # up the flake rerun, and a rerun on ANOTHER head does not count either.
    from agentflow import identity
    lock.acquire(Ctx(Paths(root), run_id="run-test"), 8, "s", 1)
    identity.append_record(Ctx(Paths(root), run_id="run-test"),
                           {"name": "ci-rerun", "head": "b" * 40, "failed_only": True})
    gh = ci_rerun_fakes(conclusion="success")
    for _ in range(2):
        assert run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)[0] == 0
    set_run_conclusion(gh, "failure")
    assert run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)[0] == 0
    code, out = run(capsys, root, "ci-rerun", "--pr", "12", gh_run=gh)
    assert code == 1 and out["error"] == "rerun-already-used"
    mine = [r for r in gate_runs(root) if r.get("head") == PR_HEAD]
    assert [(r["failed_only"], r["count"]) for r in mine] == [(False, 1), (False, 2), (True, 3)]
