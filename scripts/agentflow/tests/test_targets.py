"""Build output off the worktrees' drive (`target_root` in `.agent/config.json`).

Each issue worktree keeps its OWN `target/` (one shared directory would make
two runs' builds contend, #60), but that directory is a junction into
`<target_root>/<worktree name>`. The disk guard then reserves each live run's
build space on the drive that holds it."""
import json
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

from agentflow import cli, lock, recovery, worktrees
from agentflow.gitops import Git
from agentflow.state import Ctx, Paths
from agentflow.tests.conftest import FakeRunner
from agentflow.tests.test_cli_flow import CLAIM_GH, READS, _claim, preflight_ready, run


def _config(clone, **cfg):
    (clone / ".agent").mkdir(exist_ok=True)
    (clone / ".agent" / "config.json").write_text(json.dumps(cfg), encoding="utf-8")


def test_link_target_makes_the_worktree_target_a_link_into_the_target_root(tmp_path):
    wt = tmp_path / "al-sem-issue-9-a1"
    wt.mkdir()
    built = worktrees.link_target(wt, tmp_path / "targets")
    assert built == tmp_path / "targets" / "al-sem-issue-9-a1"
    (wt / "target" / "artifact.rlib").write_text("x")
    assert (built / "artifact.rlib").read_text() == "x"      # writes land in the target root


def test_unlink_target_deletes_the_build_folder_and_nothing_else(tmp_path):
    wt = tmp_path / "al-sem-issue-9-a1"
    wt.mkdir()
    built = worktrees.link_target(wt, tmp_path / "targets")
    (built / "artifact.rlib").write_text("x")
    (tmp_path / "targets" / "al-sem-issue-10-a1").mkdir()    # a sibling run's build
    worktrees.unlink_target(wt, tmp_path / "targets")
    assert not built.exists()
    assert (tmp_path / "targets" / "al-sem-issue-10-a1").exists()


def test_make_worktree_links_target_when_a_target_root_is_configured(capsys, repo_pair, tmp_path):
    _, clone = repo_pair
    _config(clone, target_root=str(tmp_path / "targets"))
    code, claim = _claim(capsys, clone, 8, FakeRunner(CLAIM_GH), "run-test")
    assert code == 0, claim
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 0, out
    name = Path(claim["worktree"]).name
    (Path(claim["worktree"]) / "target" / "probe").write_text("x")
    assert (tmp_path / "targets" / name / "probe").exists()
    assert out["target"] == str(tmp_path / "targets" / name)


def test_make_worktree_without_a_target_root_makes_no_link(capsys, repo_pair):
    _, clone = repo_pair
    code, claim = _claim(capsys, clone, 8, FakeRunner(CLAIM_GH), "run-test")
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 0 and not (Path(claim["worktree"]) / "target").exists()
    assert out.get("target") is None


def _fake_reflink(src, dst_dir):   # stands in for `cp --reflink=always`, which needs btrfs/xfs
    shutil.copytree(src, dst_dir / src.name)


def _root_target(root):
    for d in ("debug", "release", "flycheck0", "tmp"):
        (root / "target" / d).mkdir(parents=True)
        (root / "target" / d / "dep.rlib").write_text(d)


def test_seed_target_copies_the_root_build_cache_except_rust_analyzer_and_scratch(tmp_path, monkeypatch):
    root, wt = tmp_path / "root", tmp_path / "al-sem-issue-9-a1"
    _root_target(root)
    wt.mkdir()
    monkeypatch.setattr(worktrees, "_reflink_copy", _fake_reflink)
    assert worktrees.seed_target(root, wt) is True
    assert sorted(p.name for p in (wt / "target").iterdir()) == ["debug", "release"]
    assert (wt / "target" / "debug" / "dep.rlib").read_text() == "debug"


def test_seed_target_leaves_no_target_when_reflinks_are_unsupported(tmp_path, monkeypatch):
    root, wt = tmp_path / "root", tmp_path / "al-sem-issue-9-a1"
    _root_target(root)
    wt.mkdir()

    calls = []

    def failing_cp(src, dst_dir):   # `cp --reflink=always` on overlayfs/ext4 exits 1
        calls.append(src.name)
        if len(calls) == 2:
            raise subprocess.CalledProcessError(1, ["cp"])
        _fake_reflink(src, dst_dir)
    monkeypatch.setattr(worktrees, "_reflink_copy", failing_cp)
    assert worktrees.seed_target(root, wt) is False
    assert len(calls) == 2 and not (wt / "target").exists()   # half a copy is removed; cargo builds cold


def test_make_worktree_seeds_target_from_the_root_checkout(capsys, repo_pair, monkeypatch):
    _, clone = repo_pair
    (clone / "target" / "debug").mkdir(parents=True)
    monkeypatch.setattr(worktrees, "_reflink_copy", _fake_reflink)
    code, claim = _claim(capsys, clone, 8, FakeRunner(CLAIM_GH), "run-test")
    code, out = run(capsys, clone, "make-worktree", gh_run=FakeRunner(readonly=True))
    assert code == 0 and out["seeded"] is True
    assert (Path(claim["worktree"]) / "target" / "debug").is_dir()


def test_spike_cleanup_removes_the_build_folder_too(repo_pair, tmp_path):
    _, clone = repo_pair
    _config(clone, target_root=str(tmp_path / "targets"))
    wt = clone.parent / "al-sem-issue-9-a1"
    subprocess.run(["git", "-C", str(clone), "worktree", "add", "-q", "-b", "issue/9", str(wt)],
                   check=True, capture_output=True)
    built = worktrees.link_target(wt, tmp_path / "targets")
    (built / "artifact.rlib").write_text("x")
    ctx = Ctx(Paths(clone), run_id="run-test")
    recovery.remove_spike_worktree(ctx, Git(clone), wt, "issue/9", clone.parent)
    assert not wt.exists() and not built.exists()


def test_the_run_reserve_is_charged_to_the_target_root_drive(capsys, repo_pair, monkeypatch, tmp_path):
    """With a target root, the worktrees' drive keeps only the floor
    (`min_free_gb`) and each live run's reserve is asked of the drive that
    holds the builds."""
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    preflight_ready(clone, monkeypatch, tmp_path)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    targets = tmp_path / "targets"
    targets.mkdir()
    _config(clone, slots=3, target_root=str(targets))
    lock.acquire(Ctx(Paths(clone), run_id="run-live", now=lambda: 1_000_000.0), 8, "s", 1)

    def disk(worktrees_gb, targets_gb):
        def usage(p):
            gb = targets_gb if Path(p) == targets else worktrees_gb
            return SimpleNamespace(free=gb * 2**30)
        monkeypatch.setattr(cli.shutil, "disk_usage", usage)

    disk(50, 10)                                  # U: above the 40 GB floor, H: below one 30 GB reserve
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert "disk-free:targets:10GB" in out["failures"], out["failures"]
    assert not any(f.startswith("disk-free:") and "targets" not in f for f in out["failures"])
    assert (out["target_free_gb"], out["target_required_gb"]) == (10, 30)
    code, out = _claim(capsys, clone, 9, FakeRunner(CLAIM_GH), "run-new")
    assert code == 1 and out["error"] == "disk-low:targets:10GB<30GB", out
    disk(50, 500)
    code, out = run(capsys, clone, "preflight", gh_run=FakeRunner(READS), run_id=None)
    assert not any(f.startswith("disk-free") for f in out["failures"]), out["failures"]
    assert out["disk_required_gb"] == 40
    code, out = _claim(capsys, clone, 9, FakeRunner(CLAIM_GH), "run-new")
    assert code == 0, out
