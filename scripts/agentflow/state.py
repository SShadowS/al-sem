"""Paths, execution context, dry-run guard, atomic JSON.

Every executor write goes through `Ctx.write_guard` so `--dry-run` is provably
write-free: the guard raises before any file, label, comment, or push happens.
"""
from __future__ import annotations

import hashlib
import json
import os
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable


class DryRunViolation(RuntimeError):
    """A write was attempted under --dry-run."""


@dataclass(frozen=True)
class Paths:
    root: Path

    @property
    def agent(self) -> Path:
        return self.root / ".agent"

    @property
    def lock(self) -> Path:
        return self.agent / "lock.json"

    @property
    def halt(self) -> Path:
        return self.agent / "HALT"

    @property
    def audit(self) -> Path:
        return self.agent / "audit.jsonl"

    @property
    def runs(self) -> Path:
        return self.agent / "runs"

    @property
    def loop(self) -> Path:
        return self.runs / "loop.json"

    @property
    def discoveries_index(self) -> Path:
        return self.agent / "discoveries-index.json"

    @property
    def attempts(self) -> Path:
        return self.agent / "attempts.json"

    @property
    def handoffs(self) -> Path:
        return self.agent / "handoffs.json"

    @property
    def incidents(self) -> Path:
        return self.agent / "incidents.json"

    def run_dir(self, run_id: str) -> Path:
        return self.runs / run_id


@dataclass
class Ctx:
    paths: Paths
    dry_run: bool = False
    run_id: str | None = None
    now: Callable[[], float] = field(default=time.time)

    def write_guard(self, what: str) -> None:
        if self.dry_run:
            raise DryRunViolation(what)

    @property
    def run_dir(self) -> Path:
        if not self.run_id:
            raise RuntimeError("no run_id in context")
        return self.paths.run_dir(self.run_id)


def new_run_id(now: float) -> str:
    stamp = time.strftime("%Y%m%d-%H%M%S", time.gmtime(now))
    return f"{stamp}-{uuid.uuid4().hex[:6]}"


# Windows sharing errors. `os.replace` onto a file another process has open
# fails with `PermissionError`, and so does opening a file while another
# process replaces it. Both clear within milliseconds, so a read or a replace
# retries this many times, RETRY_DELAY_S apart (2 s in all), before it fails.
# Measured with two processes writing one file 200 times each: 1 to 5
# collisions per run, and 5 retries at 100 ms still lost about 1 run in 15.
SHARING_RETRIES = 40
RETRY_DELAY_S = 0.05


def read_json(path: Path, default: Any = None) -> Any:
    """Parse `path`, or `default` when it does not exist.

    A Windows sharing error is retried (see SHARING_RETRIES). A parse error
    gets ONE retry: every writer here is atomic, but the lease writer was
    in-place before, and a lease written by older code can still be on disk.
    A file that is still bad on the retry raises: the retry must never hide
    real corruption.
    """
    parse_retried, sharing_errors = False, 0
    while True:
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            return default
        except PermissionError:
            sharing_errors += 1
            if sharing_errors > SHARING_RETRIES:
                raise
        except json.JSONDecodeError:
            if parse_retried:
                raise
            parse_retried = True
        time.sleep(RETRY_DELAY_S)


def write_json(ctx: Ctx, path: Path, data: Any) -> None:
    """Atomic write: a temp file, then `os.replace` onto `path`.

    The temp name is UNIQUE per call (pid + uuid). A fixed `<name>.tmp` was
    shared by every writer of one file, so two processes wrote the same temp
    file and one `os.replace` moved away the file the other was replacing.
    A write that fails removes its temp file.
    """
    ctx.write_guard(f"write {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
        for attempt in range(SHARING_RETRIES + 1):
            try:
                os.replace(tmp, path)
                break
            except PermissionError:
                if attempt == SHARING_RETRIES:
                    raise
                time.sleep(RETRY_DELAY_S)
    finally:
        tmp.unlink(missing_ok=True)


def append_audit(ctx: Ctx, record: dict) -> None:
    """Append one JSON line to `.agent/audit.jsonl`.

    APPEND-ONLY, and never read back by the executor: nothing in this package
    may branch on its contents. It exists for the human reconstructing an
    incident afterwards -- who set HALT, who cleared it, who resolved which
    merge -- which is precisely the history a single overwritten `.agent/HALT`
    file cannot carry. A consumer that started reading it would turn an
    evidence log into a control input, and a truncated or hand-edited line
    would then change what the flow DOES rather than only what it reports.

    Goes through `write_guard` like every other write, so `--dry-run` stays
    provably write-free.
    """
    ctx.write_guard("append audit record")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    line = json.dumps({"at": ctx.now(), "run_id": ctx.run_id, **record}, sort_keys=True)
    with open(ctx.paths.audit, "a", encoding="utf-8") as f:
        f.write(line + "\n")


def tree_snapshot(root: Path) -> dict[str, str]:
    """Relative path -> sha256 of content, for 'nothing was written' assertions."""
    out: dict[str, str] = {}
    for p in sorted(root.rglob("*")):
        if ".git" in p.relative_to(root).parts:
            continue  # git's own bookkeeping (FETCH_HEAD etc.) is not a write we count
        if p.is_file():
            out[str(p.relative_to(root))] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out
