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
    # T15: local operator state. Committing either would publish merge SHAs
    # and operator names. They land under `.agent/` precisely so the EXISTING
    # `.agent/*` rule already covers them -- this assertion also pins that no
    # `.gitignore` edit was needed for the incident record or the audit log.
    assert ignored(".agent/incidents.json")
    assert ignored(".agent/audit.jsonl")
    assert not ignored(".agent/issue-8/ledger.md")
    assert not ignored(".agent/issue-8/findings.json")
    assert ignored(".agent/issue-8/other.md")
