"""Git operations the executor needs. Thin, explicit, no porcelain parsing
beyond what the tests pin. Conflicting rebases and reverts are aborted so
the tree is always left clean.
"""
from __future__ import annotations

import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


class GitError(RuntimeError):
    pass


@dataclass
class RebaseResult:
    ok: bool
    conflicts: list[str] = field(default_factory=list)


class Git:
    def __init__(self, cwd: Path | str, run: Callable = subprocess.run):
        self.cwd, self.run = str(cwd), run

    def _run(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        r = self.run(["git", "-C", self.cwd, *args], capture_output=True, text=True)
        if check and r.returncode != 0:
            raise GitError(f"git {' '.join(args)}: {r.stderr.strip()}")
        return r

    def out(self, *args: str) -> str:
        return self._run(*args).stdout.strip()

    def ok(self, *args: str) -> bool:
        return self._run(*args, check=False).returncode == 0

    def rev(self, ref: str) -> str:
        return self.out("rev-parse", ref)

    def is_clean(self) -> bool:
        return self.out("status", "--porcelain") == ""

    def branch(self) -> str:
        return self.out("rev-parse", "--abbrev-ref", "HEAD")

    def toplevel(self) -> Path:
        return Path(self.out("rev-parse", "--show-toplevel"))

    def fetch(self, remote: str = "origin") -> None:
        self._run("fetch", "-q", remote)

    def changed_files(self, a: str, b: str) -> list[str]:
        s = self.out("diff", "--name-only", f"{a}..{b}")
        return [l for l in s.splitlines() if l]

    def diff(self, a: str, b: str) -> str:
        return self._run("diff", f"{a}..{b}").stdout

    def worktree_add(self, path: Path, branch: str, base: str) -> None:
        self._run("worktree", "add", "-q", str(path), "-b", branch, base)

    def worktree_prune(self) -> None:
        self._run("worktree", "prune")

    def branch_delete(self, name: str) -> None:
        self._run("branch", "-D", name)

    def is_ancestor(self, a: str, b: str) -> bool:
        return self.ok("merge-base", "--is-ancestor", a, b)

    def rebase(self, onto: str) -> RebaseResult:
        r = self._run("rebase", onto, check=False)
        if r.returncode == 0:
            return RebaseResult(True)
        conflicts = [l for l in self.out("diff", "--name-only", "--diff-filter=U").splitlines() if l]
        self._run("rebase", "--abort", check=False)
        return RebaseResult(False, conflicts)

    def revert(self, sha: str) -> bool:
        r = self._run("revert", "--no-edit", sha, check=False)
        if r.returncode != 0:
            self._run("revert", "--abort", check=False)
            return False
        return True

    def push(self, remote: str, refspec: str) -> bool:
        return self.ok("push", "-q", remote, refspec)

    def ff(self, ref: str) -> bool:
        return self.ok("merge", "-q", "--ff-only", ref)

    def checkout(self, ref: str) -> None:
        self._run("checkout", "-q", ref)

    def commit_all(self, msg: str) -> str:
        self._run("add", "-A")
        self._run("commit", "-q", "-m", msg)
        return self.rev("HEAD")

    def merge_squash(self, branch: str, msg: str) -> str:
        """Test helper: what GitHub's squash button does, locally."""
        self._run("merge", "--squash", "-q", branch)
        self._run("commit", "-q", "-m", msg)
        return self.rev("HEAD")
