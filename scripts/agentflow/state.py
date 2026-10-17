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
from contextlib import contextmanager
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Iterator

if os.name == "nt":
    import msvcrt
else:
    import fcntl


class DryRunViolation(RuntimeError):
    """A write was attempted under --dry-run."""


class MutexTimeout(RuntimeError):
    """`mutex` could not take its lock within the timeout."""


class ConfigError(ValueError):
    """`.agent/config.json` holds something `config` refuses to guess about."""


# `.agent/config.json`, read by `config`. These defaults ARE today's
# behaviour, so a checkout without the file acts exactly as before.
# `min_free_gb`: `preflight` refuses a tick below this much free disk. One
# issue run builds a worktree target (up to ~30 GB) and post-merge builds
# another, so 20 GB was not enough: on 2026-10-14 a tick started above it,
# the drive hit 35 MB, and the post-merge check failed at `gen-syntax` on a
# green merge (#68). The other keys are read by later steps of the
# parallel-issues design.
CONFIG_DEFAULTS: dict[str, int] = {
    "slots": 1, "heavy_gates": 1, "min_free_gb": 40, "reserve_gb_per_run": 30,
    "gate_wait_minutes": 180,
}


@dataclass(frozen=True)
class Paths:
    root: Path

    @property
    def agent(self) -> Path:
        return self.root / ".agent"

    @property
    def lock(self) -> Path:
        """The LEGACY single lease; new leases live in `locks_dir`."""
        return self.agent / "lock.json"

    @property
    def locks_dir(self) -> Path:
        return self.agent / "locks"

    def lease(self, issue: int) -> Path:
        return self.locks_dir / f"issue-{issue}.json"

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

    def mutex(self, name: str) -> Path:
        return self.agent / "mutex" / f"{name}.lock"

    @property
    def config(self) -> Path:
        return self.agent / "config.json"

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


def config(ctx: Ctx) -> dict[str, int]:
    """`.agent/config.json` over CONFIG_DEFAULTS. A missing file or key is the
    default.

    A FILE, not a flag or an env var: two sessions must not disagree about
    these numbers. For the same reason a value it would otherwise ignore (an
    unknown key, a non-integer, a negative) raises `ConfigError` instead of
    quietly falling back to the default.
    """
    data = read_json(ctx.paths.config, default={})
    if not isinstance(data, dict):
        raise ConfigError(f"{ctx.paths.config} must hold a JSON object")
    for key, value in data.items():
        if key not in CONFIG_DEFAULTS:
            raise ConfigError(f"{ctx.paths.config}: unknown key {key!r}")
        if type(value) is not int or value < 0:
            raise ConfigError(f"{ctx.paths.config}: {key} must be a whole number >= 0, not {value!r}")
    return {**CONFIG_DEFAULTS, **data}


MUTEX_POLL_S = 0.05


def _try_lock(f) -> None:
    """Take the OS lock on `f` without waiting; `OSError` when it is held."""
    if os.name == "nt":
        f.seek(0)
        msvcrt.locking(f.fileno(), msvcrt.LK_NBLCK, 1)   # byte 0, from position 0
    else:
        fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)


def _unlock(f) -> None:
    if os.name == "nt":
        f.seek(0)
        msvcrt.locking(f.fileno(), msvcrt.LK_UNLCK, 1)
    else:
        fcntl.flock(f.fileno(), fcntl.LOCK_UN)


@contextmanager
def mutex(ctx: Ctx, name: str, timeout: float = 30.0) -> Iterator[None]:
    """Hold the OS file lock `.agent/mutex/<name>.lock` for the `with` body.

    An OS lock, not a lease: the OS drops it when the process dies, so it can
    never go stale. It is for short critical sections only. It waits in
    MUTEX_POLL_S steps and raises `MutexTimeout` after `timeout` seconds.

    NOT RE-ENTRANT. Taking a mutex this process already holds waits for
    itself until the timeout. The rule: only public entry points take a
    mutex, and helpers assume it is held.

    A no-op under `--dry-run`: it creates no lock file, and every write it
    would protect is refused by `write_guard` anyway.
    """
    if ctx.dry_run:
        yield
        return
    path = ctx.paths.mutex(name)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a+b") as f:
        deadline = time.monotonic() + timeout
        while True:
            try:
                _try_lock(f)
                break
            except OSError:
                if time.monotonic() >= deadline:
                    raise MutexTimeout(f"mutex {name!r} still held after {timeout} s") from None
                time.sleep(MUTEX_POLL_S)
        try:
            yield
        finally:
            _unlock(f)


def update_json(ctx: Ctx, path: Path, fn: Callable[[Any], Any], *, mutex_name: str,
                default: Any = None) -> Any:
    """The one read-modify-write of a shared JSON file.

    Holds mutex `mutex_name`, reads `path` (`default` when missing), writes
    what `fn` returns, and returns it. When `fn` raises, nothing is written.
    Never call it while holding `mutex_name` (see `mutex`).
    """
    ctx.write_guard(f"update {path}")
    with mutex(ctx, mutex_name):
        data = fn(read_json(path, default))
        write_json(ctx, path, data)
    return data


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
    provably write-free. Written under mutex `audit`: `open(..., "a")` from
    two processes is not an atomic append on Windows.
    """
    ctx.write_guard("append audit record")
    ctx.paths.agent.mkdir(parents=True, exist_ok=True)
    line = json.dumps({"at": ctx.now(), "run_id": ctx.run_id, **record}, sort_keys=True)
    with mutex(ctx, "audit"):
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
