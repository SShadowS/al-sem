"""⟨TLS-113⟩ No test inherits its caller's gate environment.

ENG-159's landing exported CDO_GATE_NO_SPEED=1 (an approved skip of cdo-gate's
speed phase); `ci-steps agentflow`, run by that landing, inherited it, and
`gate --name cdo-gate` refused in two tests that set nothing. conftest's
`no_gate_environment` clears every gate variable before each test.

Discrimination proofs (2026-11-21, each break made with Edit and reverted):
- `no_gate_environment`'s delenv loop removed (body `pass`):
  `test_a_callers_gate_environment_reaches_no_test` FAILED (the inner run's
  `test_no_gate_variable_reaches_a_test` and the ENG-159 test both red);
  reverted: passed.
- "LAND_" dropped from GATE_ENV_PREFIXES:
  `test_every_variable_a_script_reads_is_cleared_or_kept` FAILED, naming the
  LAND_* reads of scripts/land; reverted: passed.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

from agentflow.tests.conftest import GATE_ENV_KEPT, SCRIPTS, is_gate_env

# `${NAME:-x}` / `${NAME-x}` / `${NAME:?}` / `${NAME:+x}` / `${NAME:=x}`: how a
# bash script under `set -u` reads its environment; a bare `$NAME` would fail
# when unset, so those are its own variables. Python: os.environ / getenv.
BASH_READ = re.compile(r"\$\{([A-Z][A-Z0-9_]*):?[-=?+]")
PY_READ = re.compile(r"""(?:environ(?:\.get)?\(?\[?|getenv\(|setdefault\()"([A-Z][A-Z0-9_]*)"|"([A-Z][A-Z0-9_]*)" in os\.environ""")


def _script_reads():
    """{variable: [files]} for every environment read under scripts/ (the suite
    itself and the Windows-only PowerShell aside)."""
    reads = {}
    for path in sorted(SCRIPTS.rglob("*")):
        if not path.is_file() or "tests" in path.relative_to(SCRIPTS).parts[:2] \
                or "__pycache__" in path.parts or path.suffix == ".ps1":
            continue
        try:
            text = path.read_text()
        except UnicodeDecodeError:
            continue
        names = set(BASH_READ.findall(text))
        names |= {a or b for a, b in PY_READ.findall(text)}
        for n in names:
            reads.setdefault(n, []).append(str(path.relative_to(SCRIPTS)))
    return reads


def test_every_variable_a_script_reads_is_cleared_or_kept():
    reads = _script_reads()
    assert "CDO_GATE_NO_SPEED" in reads and "LAND_WORKTREE" in reads, "the scan found nothing"
    loose = {n: f for n, f in reads.items() if not is_gate_env(n) and n not in GATE_ENV_KEPT}
    assert not loose, ("a script reads a variable conftest's no_gate_environment neither clears "
                       f"nor keeps on purpose; add it to GATE_ENV_NAMES or GATE_ENV_KEPT: {loose}")


def test_no_gate_variable_reaches_a_test():
    # CDO_WS is a_cdo_workspace's own empty folder, set after the clearing.
    leaked = sorted(n for n in os.environ if is_gate_env(n) and n != "CDO_WS")
    assert not leaked, leaked
    assert "cdo-ws" in os.environ["CDO_WS"], os.environ["CDO_WS"]


def test_a_callers_gate_environment_reaches_no_test():
    """The use, not the helper: a pytest run under ENG-159's environment (and
    more) passes the test that failed then, and sees no gate variable."""
    env = dict(os.environ, CDO_GATE_NO_SPEED="1", ENFORCE_CDO_WS="1", LAND_REQUIRE_PRESUBMIT="1",
               CDO_WS="/nonexistent/cdo", CDO_SLOT_TOKEN="caller", CARGO_TARGET_DIR="/nonexistent/target")
    here = Path(__file__).resolve().parent
    r = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider",
         f"{here / 'test_gate_env.py'}::test_no_gate_variable_reaches_a_test",
         f"{here / 'test_identity.py'}::test_cdo_gate_record_carries_the_cdo_identity"],
        cwd=SCRIPTS, env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout + r.stderr
    assert "2 passed" in r.stdout, r.stdout
