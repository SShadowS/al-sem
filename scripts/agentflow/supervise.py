"""Run a long child (a gate, a cargo build) under supervision.

Refreshes the lock heartbeat every `beat_every` seconds while the child runs,
enforces a timeout by killing the whole process tree, and writes stdout+stderr
to a log file whose path is returned with the exit code. The environment is
sanitized: REGEN_TEMP_GOLDENS is removed (a verification must never
regenerate), ALSEM_NO_PREFLIGHT_CACHE=1, TREE_SITTER_AL_PATH set, and
CARGO_TARGET_DIR set when the caller names one.
"""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from .state import Ctx

DROP_ENV = ("REGEN_TEMP_GOLDENS", "AGENTFLOW_NOW")


@dataclass
class Result:
    exit_code: int
    log_path: Path
    timed_out: bool
    seconds: float


def sanitized_env(base: dict, tree_sitter_path: str | None,
                  cargo_target_dir: str | None = None) -> dict:
    env = {k: v for k, v in base.items() if k not in DROP_ENV}
    env["ALSEM_NO_PREFLIGHT_CACHE"] = "1"
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
                    beat(ctx)
                if time.monotonic() - start > timeout_s:
                    _kill_tree(proc)
                    code = proc.wait()
                    timed_out = True
                    break
    return Result(exit_code=code if not timed_out else (code or 124), log_path=log_path,
                  timed_out=timed_out, seconds=time.monotonic() - start)
