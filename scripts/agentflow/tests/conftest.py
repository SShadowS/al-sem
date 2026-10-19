import sys
from pathlib import Path

import pytest

SCRIPTS = Path(__file__).resolve().parents[2]
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

from agentflow.state import Ctx, Paths  # noqa: E402
from agentflow.gitops import Git  # noqa: E402


PLENTY_OF_DISK = 500 * 2**30


@pytest.fixture(autouse=True)
def plenty_of_disk(monkeypatch):
    """`claim` and `preflight` refuse on low free disk, and free disk is a
    fact about the machine running the suite (this one has had 31 GB free on
    C:, under the 40 GB floor). Every test sees 500 GB unless it patches
    `shutil.disk_usage` itself to state the number it is about."""
    import shutil
    from types import SimpleNamespace
    monkeypatch.setattr(shutil, "disk_usage", lambda p: SimpleNamespace(free=PLENTY_OF_DISK))


@pytest.fixture(autouse=True)
def a_cdo_workspace(monkeypatch, tmp_path_factory):
    """`preflight` and `post-merge` refuse without a CDO workspace (#107: an
    unset CDO_WS once read as a red gate). Every test gets an empty one unless
    it unsets or replaces it to state what it is about."""
    monkeypatch.setenv("CDO_WS", str(tmp_path_factory.mktemp("cdo-ws")))


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

MUTATING = ("issue edit", "issue comment", "issue create", "pr create", "pr comment", "pr merge", "pr ready", "pr edit",
            "issue reopen", "-X POST", "-X PATCH", "-X DELETE",
            "--method POST", "--method PATCH", "--method DELETE")


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


def _git(cwd, *args):
    subprocess.run(["git", "-C", str(cwd), *args], check=True, capture_output=True, text=True)


@pytest.fixture
def repo_pair(tmp_path):
    origin = tmp_path / "origin.git"
    subprocess.run(["git", "init", "--bare", "-b", "master", str(origin)], check=True, capture_output=True)
    clone = tmp_path / "clone"
    subprocess.run(["git", "clone", "-q", str(origin), str(clone)], check=True, capture_output=True)
    _git(clone, "config", "user.email", "t@example.com")
    _git(clone, "config", "user.name", "T")
    _git(clone, "checkout", "-q", "-b", "master")
    (clone / "README.md").write_text("hello\n")
    _git(clone, "add", "README.md")
    _git(clone, "commit", "-q", "-m", "init")
    _git(clone, "push", "-q", "-u", "origin", "master")
    return origin, clone


CHILD_PROLOGUE = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
ROOT = Path(sys.argv[2])
TAG = sys.argv[3]
ARGS = sys.argv[4:]

def barrier():
    (ROOT / f"ready-{TAG}").write_text("")
    while not (ROOT / "go").exists():
        time.sleep(0.005)

def slow_reads(name, seconds=0.3):
    # Widen a read-modify-write: every read of a file called `name` sleeps
    # AFTER it has read, so a second process reads the same old value unless
    # something makes it wait.
    real = Path.read_text
    def slow(self, *a, **kw):
        text = real(self, *a, **kw)
        if self.name == name:
            time.sleep(seconds)
        return text
    Path.read_text = slow
"""


def run_concurrently(root: Path, body: str, tags, extra=(), timeout: float = 180):
    """Run `body` in one real `python` process per tag, released together.

    Each child gets `ROOT`, `TAG`, `ARGS` and `barrier()` from
    `CHILD_PROLOGUE`, and must call `barrier()` right before the racy part.
    The parent starts every child, waits until all are at the barrier, then
    releases them. Returns `[(returncode, stdout, stderr), ...]` in tag order.
    """
    import textwrap
    import time
    code = CHILD_PROLOGUE + textwrap.dedent(body)
    procs = [subprocess.Popen([sys.executable, "-c", code, str(SCRIPTS), str(root), str(t), *extra],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
             for t in tags]
    deadline = time.monotonic() + timeout
    while not all((root / f"ready-{t}").exists() for t in tags):
        if any(p.poll() is not None for p in procs) or time.monotonic() > deadline:
            break  # a child died before the barrier; its output says why
        time.sleep(0.01)
    (root / "go").write_text("")
    out = []
    for p in procs:
        o, e = p.communicate(timeout=timeout)
        out.append((p.returncode, o, e))
    return out


def commit_file(repo: Path, name: str, text: str, msg: str) -> str:
    (repo / name).parent.mkdir(parents=True, exist_ok=True)
    (repo / name).write_text(text)
    _git(repo, "add", name)
    _git(repo, "commit", "-q", "-m", msg)
    return Git(repo).rev("HEAD")
