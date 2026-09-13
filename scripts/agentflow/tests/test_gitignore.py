import subprocess
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]


def ignored(path: str) -> bool:
    r = subprocess.run(["git", "-C", str(REPO), "check-ignore", "-q", path], capture_output=True)
    return r.returncode == 0


def test_agent_dir_ignored_except_the_two_evidence_files():
    assert ignored(".agent/lock.json")
    assert ignored(".agent/runs/x/ranking.json")
    assert ignored(".agent/HALT")
    assert not ignored(".agent/issue-8/ledger.md")
    assert not ignored(".agent/issue-8/findings.json")
    assert ignored(".agent/issue-8/other.md")
