"""⟨TLS-113⟩ No test inherits its caller's gate environment.

ENG-159's landing exported CDO_GATE_NO_SPEED=1 (an approved skip of cdo-gate's
speed phase); `ci-steps agentflow`, run by that landing, inherited it, and
`gate --name cdo-gate` refused in two tests that set nothing. conftest's
`no_gate_environment` clears every gate variable before each test.

WHAT THE SCAN COVERS (and what it does not). Bash and sh scripts under
scripts/ (the suite and the Windows-only PowerShell aside), comment lines
skipped:
- `${NAME:-x}`, `${NAME-x}`, `${NAME:?}`, `${NAME:+x}`, `${NAME:=x}`,
  `[[ -v NAME ]]` and `printenv NAME`: always a read of the environment;
- plain `${NAME}` and bare `$NAME`: a read unless the same file assigns NAME
  (`NAME=`, `export`/`local`/`declare`/`readonly NAME`, `for NAME in`,
  `read ... NAME`) or NAME is a bash variable (BASH_*, SECONDS, ...). A bare
  read of an unset name is legal outside `set -u` (`scripts/compiler-graph`
  reads `$USERPROFILE` so), so bare reads count.
Python scripts: `environ["N"]`, `environ.get("N")`, `environ.pop("N")`,
`environ.setdefault("N")`, `getenv("N")` and `"N" in environ`, with either
quote. Not covered: a name built at run time. Every file that reads by
computed key (`for k in os.environ`, `os.environ.items()`, a non-literal
key, bash `${!name}`) must be listed in COMPUTED_READS, which says what it
reads and how that is classified; a new one fails
`test_every_computed_read_is_classified`. Lowercase names (a script's own
variables by convention) and the Rust test helpers are outside the scan.

Discrimination proofs (2026-11-21, each break made with Edit and reverted):
- `no_gate_environment`'s delenv loop removed (body `pass`):
  `test_a_callers_gate_environment_reaches_no_test` FAILED (the inner run's
  `test_no_gate_variable_reaches_a_test` and the ENG-159 test both red);
  reverted: passed.
- "LAND_" dropped from GATE_ENV_PREFIXES:
  `test_every_variable_a_script_reads_is_cleared_or_kept` FAILED, naming the
  LAND_* reads of scripts/land; reverted: passed.
- (r2) "HOME" dropped from GATE_ENV_KEPT: the same test FAILED, naming
  kraken-setup.sh's bare `$HOME` (a read only the widened bash scan sees);
  reverted: passed.
- (r2) "bench_ab.py" dropped from COMPUTED_READS:
  `test_every_computed_read_is_classified` FAILED; reverted: passed.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

from agentflow.tests.conftest import GATE_ENV_KEPT, SCRIPTS, is_gate_env

N = r"([A-Z][A-Z0-9_]*)"
BASH_SURE = [re.compile(r"\$\{" + N + r":?[-=?+]"), re.compile(r"\[\[\s+-v\s+" + N),
             re.compile(r"\bprintenv\s+" + N)]
BASH_MAYBE = [re.compile(r"\$\{" + N), re.compile(r"\$" + N)]
BASH_SETS = [
    re.compile(r"(?:^|[\s;(&|{])(?:export\s+|local\s+(?:-\w+\s+)*|declare\s+(?:-\w+\s+)*|readonly\s+)?" + N + r"\+?="),
    re.compile(r"\bfor\s+" + N + r"\s+in\b"),
    re.compile(r"\b(?:local|declare|export|readonly)\s+(?:-\w+\s+)*((?:[A-Z][A-Z0-9_]*\s*)+)(?:$|;)", re.M),
    re.compile(r"\bread\s+(?:-\w+\s+(?:\S+\s+)?)*((?:[A-Z][A-Z0-9_]*\s*)+)"),
]
BASH_BUILTINS = re.compile(
    r"^(BASH\w*|SECONDS|EPOCHREALTIME|EPOCHSECONDS|SRANDOM|RANDOM|PIPESTATUS|LINENO|FUNCNAME|PWD|"
    r"OLDPWD|IFS|PPID|UID|EUID|GROUPS|HOSTNAME|HOSTTYPE|OSTYPE|MACHTYPE|OPTIND|OPTARG|OPTERR|REPLY|"
    r"SHLVL|SHELLOPTS|COLUMNS|LINES|COPROC)$")
PY_READS = [re.compile(r"""environ(?:\.get|\.pop|\.setdefault)?\(?\[?\s*["']""" + N + "[\"']"),
            re.compile(r"""getenv\(\s*["']""" + N + "[\"']"),
            re.compile(r"""["']""" + N + r"""["']\s+(?:not\s+)?in\s+(?:os\.)?environ""")]
PY_COMPUTED = re.compile(r"""for\s+\w+(?:,\s*\w+)?\s+in\s+os\.environ|environ\.(?:items|keys|values)\(|"""
                         r"""environ\[(?!\s*["'])|environ\.(?:get|pop)\((?!\s*["'])|getenv\((?!\s*["'])""")
BASH_COMPUTED = re.compile(r"\$\{![A-Za-z_]|\bcompgen\s+-e\b|\bprintenv\s*($|[|;)])", re.M)

# Every file that reads the environment by computed key, and what it reads.
COMPUTED_READS = {
    "cdo-speed": "build_overrides: its BUILD_ENV_EXACT / BUILD_ENV_PREFIXES build inputs "
                 "(cleared: conftest reads both tuples from the script); the TEST_HOOKS "
                 "report (cleared: CDO_ names); child_env only drops ALSEM_* and C1_CONE_CENSUS",
    "bench_ab.py": "only drops ALSEM_* and C1_CONE_CENSUS from a child's environment",
    "nextest-priority-check": "only drops CDO_WS and ENFORCE_CDO_WS from a child's environment",
    "land": "the declared test hooks (agentflow.supervise.declared_test_hooks; cleared: "
            "CDO_ and CI_STEPS_ names), and with `compgen -e` the build overrides "
            "CARGO_PROFILE_*, CARGO_BUILD_*, RUSTFLAGS, RUSTDOCFLAGS, CARGO_ENCODED_RUSTFLAGS, "
            "CARGO_TARGET_DIR (cleared: cdo-speed's build inputs and GATE_ENV_NAMES) and the engine "
            "seams ALSEM_TEST_*, ALSEM_BATCH_CLAIMS (cleared), to refuse one set",
}


def _kind(path):
    first = path.read_text().split("\n", 1)[0]
    if path.suffix == ".py" or "python" in first:
        return "py"
    if path.suffix == ".sh" or (first.startswith("#!") and "sh" in first):
        return "sh"
    return None


def _sources():
    """(path relative to scripts/, kind, text without comment lines)."""
    for path in sorted(SCRIPTS.rglob("*")):
        if not path.is_file() or "tests" in path.relative_to(SCRIPTS).parts[:2] \
                or "__pycache__" in path.parts:
            continue
        try:
            kind = _kind(path)
        except UnicodeDecodeError:
            continue
        if kind:
            text = "\n".join(l for l in path.read_text().splitlines() if not l.lstrip().startswith("#"))
            yield str(path.relative_to(SCRIPTS)), kind, text


def _bash_reads(text):
    sets = set()
    for rx in BASH_SETS:
        for m in rx.findall(text):
            sets.update(m.split())
    maybe = {m for rx in BASH_MAYBE for m in rx.findall(text)} - sets
    sure = {m for rx in BASH_SURE for m in rx.findall(text)}
    return {n for n in maybe | sure if not BASH_BUILTINS.match(n)}


def _script_reads():
    """{variable: [files]} for every environment read the scan sees."""
    reads = {}
    for rel, kind, text in _sources():
        names = _bash_reads(text) if kind == "sh" else {m for rx in PY_READS for m in rx.findall(text)}
        for n in names:
            reads.setdefault(n, []).append(rel)
    return reads


def test_every_variable_a_script_reads_is_cleared_or_kept():
    reads = _script_reads()
    assert "CDO_GATE_NO_SPEED" in reads and "LAND_WORKTREE" in reads, "the scan found nothing"
    loose = {n: f for n, f in reads.items() if not is_gate_env(n) and n not in GATE_ENV_KEPT}
    assert not loose, ("a script reads a variable conftest's no_gate_environment neither clears "
                       f"nor keeps on purpose; add it to GATE_ENV_NAMES or GATE_ENV_KEPT: {loose}")


def test_every_computed_read_is_classified():
    found = sorted(rel for rel, kind, text in _sources()
                   if (PY_COMPUTED if kind == "py" else BASH_COMPUTED).search(text))
    assert found == sorted(COMPUTED_READS), (
        "a script reads the environment by computed key; say what it reads and how conftest "
        f"treats it in COMPUTED_READS: found {found}, classified {sorted(COMPUTED_READS)}")


def test_no_gate_variable_reaches_a_test():
    # CDO_WS is a_cdo_workspace's own empty folder, set after the clearing.
    leaked = sorted(n for n in os.environ if is_gate_env(n) and n != "CDO_WS")
    assert not leaked, leaked
    assert "cdo-ws" in os.environ["CDO_WS"], os.environ["CDO_WS"]


def test_a_callers_gate_environment_reaches_no_test():
    """The use, not the helper: a pytest run under ENG-159's environment (and
    more) passes the test that failed then, and sees no gate variable."""
    env = dict(os.environ, CDO_GATE_NO_SPEED="1", ENFORCE_CDO_WS="1", LAND_REQUIRE_PRESUBMIT="1",
               CDO_WS="/nonexistent/cdo", CDO_SLOT_TOKEN="caller", CARGO_TARGET_DIR="/nonexistent/target",
               CI_STEPS_PERF_BOUNDS_BIN="/bin/true", RUSTFLAGS="-Cdebug-assertions")
    here = Path(__file__).resolve().parent
    r = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider",
         f"{here / 'test_gate_env.py'}::test_no_gate_variable_reaches_a_test",
         f"{here / 'test_identity.py'}::test_cdo_gate_record_carries_the_cdo_identity"],
        cwd=SCRIPTS, env=env, capture_output=True, text=True, timeout=300)
    assert r.returncode == 0, r.stdout + r.stderr
    assert "2 passed" in r.stdout, r.stdout
