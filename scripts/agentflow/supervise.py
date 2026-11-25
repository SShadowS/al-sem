"""Run a long child (a gate, a cargo build) under supervision.

Refreshes the lock heartbeat every `beat_every` seconds while the child runs,
enforces a timeout by killing the whole process tree, and writes stdout+stderr
to a log file whose path is returned with the exit code. The environment is
sanitized: REGEN_TEMP_GOLDENS is removed (a verification must never
regenerate), so are BASH_ENV/ENV, CDO_GATE_NO_SPEED and every declared test
hook (`declared_test_hooks`), TREE_SITTER_AL_PATH set, and
CARGO_TARGET_DIR set when the caller names one.
"""
from __future__ import annotations

import os
import re
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .state import Ctx, MutexTimeout

# BASH_ENV / ENV (TLS-63 review r1, sol): a startup file a non-interactive bash
# runs before the gate script, so one with `exit 0` would turn any gate green
# under its exact command. No gate needs one.
DROP_ENV = ("REGEN_TEMP_GOLDENS", "AGENTFLOW_NOW", "BASH_ENV", "ENV",
            # ⟨TLS-113⟩ An operator's speed skip is not the gate's: a gate run
            # here runs the whole of it (cmd_gate refuses the variable outright).
            "CDO_GATE_NO_SPEED")

SCRIPTS = Path(__file__).resolve().parents[1]


def script_tuple(script: str, name: str) -> tuple[str, ...]:
    """The string tuple `name = (...)` assigned at the top level of the Python
    script `scripts/<script>`, read with `ast` (the script is not run)."""
    import ast
    tree = ast.parse((SCRIPTS / script).read_text(), script)
    for node in tree.body:
        if isinstance(node, ast.Assign) and any(getattr(t, "id", None) == name for t in node.targets):
            return tuple(ast.literal_eval(node.value))
    raise LookupError(f"scripts/{script} assigns no {name}")


def declared_test_hooks() -> tuple[str, ...]:
    """⟨TLS-113⟩ Every test hook the gate scripts declare: cdo-speed's
    `TEST_HOOKS`, and each script's `# test-hooks: NAME...` lines. A real gate
    sets none: scripts/land refuses to start with one set, and a gate run here
    never passes one on."""
    names = set(script_tuple("cdo-speed", "TEST_HOOKS"))
    for path in sorted(SCRIPTS.iterdir()):
        if path.is_file():
            try:
                text = path.read_text()
            except UnicodeDecodeError:
                continue
            for line in text.splitlines():
                if line.startswith("# test-hooks:"):
                    names.update(line.split(":", 1)[1].split())
    return tuple(sorted(names))


@dataclass
class Result:
    exit_code: int
    log_path: Path
    timed_out: bool
    seconds: float
    # A failure whose log shows another build holding this worktree's target/
    # (#60), not a fault in the code under test. Re-run once nothing else builds.
    contended_build: bool = False
    # Seconds a heavy gate waited for the gate semaphore before it started
    # (`cli._run_gate`); not part of `seconds` or of the gate's timeout.
    queued_s: float = 0.0


# Signatures of a build that lost a race for target/ to another process in the
# same worktree (#60). Each was seen as a whole gate cycle wasted on diagnosis.
_CONTENTION = (
    re.compile(r"failed to write output .*: permission denied", re.IGNORECASE),
    re.compile(r"being used by another process", re.IGNORECASE),
    re.compile(r"failed to remove file .*Access is denied", re.IGNORECASE),
)


def contended(log_text: str) -> bool:
    """True when a failed run's log carries a build-contention signature."""
    return any(rx.search(log_text) for rx in _CONTENTION)


def sanitized_env(base: dict, tree_sitter_path: str | None,
                  cargo_target_dir: str | None = None) -> dict:
    drop = set(DROP_ENV) | set(declared_test_hooks())
    env = {k: v for k, v in base.items() if k not in drop}
    if tree_sitter_path:
        env["TREE_SITTER_AL_PATH"] = tree_sitter_path
    if cargo_target_dir:
        env["CARGO_TARGET_DIR"] = cargo_target_dir
    return env


def _kill_tree(proc: subprocess.Popen) -> None:
    if sys.platform == "win32":
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(proc.pid)], capture_output=True)
    else:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


def run(ctx: Ctx, cmd: list[str], *, cwd: Path, log_path: Path, timeout_s: float,
        beat: Callable[[Ctx], None] | None = None, beat_every: float = 60.0,
        env: dict | None = None) -> Result:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    start = time.monotonic()
    popen_kw: dict = {}
    if sys.platform == "win32":
        popen_kw["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        popen_kw["start_new_session"] = True
    with open(log_path, "w", encoding="utf-8", errors="replace") as log:
        proc = subprocess.Popen(cmd, cwd=str(cwd), stdout=log, stderr=subprocess.STDOUT,
                                env=env if env is not None else os.environ.copy(), **popen_kw)
        timed_out = False
        while True:
            try:
                code = proc.wait(timeout=beat_every)
                break
            except subprocess.TimeoutExpired:
                if beat is not None:
                    try:
                        beat(ctx)
                    except MutexTimeout:
                        pass        # `registry` busy (e.g. a cleanup's delete): beat next round

                if time.monotonic() - start > timeout_s:
                    _kill_tree(proc)
                    code = proc.wait()
                    timed_out = True
                    break
    exit_code = code if not timed_out else (code or 124)
    is_contended = False
    if exit_code != 0 and not timed_out:
        is_contended = contended(log_path.read_text(encoding="utf-8", errors="replace"))
    return Result(exit_code=exit_code, log_path=log_path, timed_out=timed_out,
                  seconds=time.monotonic() - start, contended_build=is_contended)
