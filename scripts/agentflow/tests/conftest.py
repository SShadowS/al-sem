import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[2]
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

from agentflow.state import Ctx, Paths  # noqa: E402


@pytest.fixture
def root(tmp_path):
    (tmp_path / ".agent").mkdir()
    return tmp_path


@pytest.fixture
def ctx(root):
    return Ctx(paths=Paths(root), dry_run=False, run_id="run-test", now=lambda: 1_000_000.0)


@pytest.fixture
def dry_ctx(root):
    return Ctx(paths=Paths(root), dry_run=True, run_id=None, now=lambda: 1_000_000.0)


import subprocess  # noqa: E402

MUTATING = ("issue edit", "issue comment", "issue create", "pr create", "pr merge", "issue reopen",
            "-X POST", "-X PATCH", "-X DELETE", "--method POST", "--method PATCH", "--method DELETE")


class FakeRunner:
    """Stand-in for subprocess.run limited to `gh`. Key = argv joined by spaces."""

    def __init__(self, responses=None, fail_at=None, readonly=False):
        self.responses = dict(responses or {})
        self.fail_at = fail_at
        self.readonly = readonly
        self.calls = []

    def __call__(self, argv, capture_output=True, text=True, **kw):
        assert argv[0] == "gh", argv
        key = " ".join(argv[1:])
        self.calls.append(key)
        if self.readonly and any(m in key for m in MUTATING):
            raise AssertionError(f"mutating gh call under readonly: {key}")
        if self.fail_at is not None and len(self.calls) == self.fail_at:
            return subprocess.CompletedProcess(argv, 1, "", "HTTP 503: boom")
        resp = self.responses.get(key)
        if isinstance(resp, Exception):
            raise resp
        if resp is None:
            for k, v in self.responses.items():
                if k.endswith("*") and key.startswith(k[:-1]):
                    resp = v
                    break
        if resp is None:
            return subprocess.CompletedProcess(argv, 1, "", f"HTTP 404: no fake for {key}")
        if isinstance(resp, tuple):  # (returncode, stdout, stderr)
            return subprocess.CompletedProcess(argv, *resp)
        return subprocess.CompletedProcess(argv, 0, resp, "")


@pytest.fixture
def runner():
    return FakeRunner()
