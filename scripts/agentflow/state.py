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


def read_json(path: Path, default: Any = None) -> Any:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return default


def write_json(ctx: Ctx, path: Path, data: Any) -> None:
    ctx.write_guard(f"write {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=True), encoding="utf-8")
    os.replace(tmp, path)


def tree_snapshot(root: Path) -> dict[str, str]:
    """Relative path -> sha256 of content, for 'nothing was written' assertions."""
    out: dict[str, str] = {}
    for p in sorted(root.rglob("*")):
        if ".git" in p.relative_to(root).parts:
            continue  # git's own bookkeeping (FETCH_HEAD etc.) is not a write we count
        if p.is_file():
            out[str(p.relative_to(root))] = hashlib.sha256(p.read_bytes()).hexdigest()
    return out
