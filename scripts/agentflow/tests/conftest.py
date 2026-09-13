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
