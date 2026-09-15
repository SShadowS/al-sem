"""Git operations the executor needs. Thin, explicit, no porcelain parsing
beyond what the tests pin. Conflicting rebases and reverts are aborted so
the tree is always left clean.
"""
from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

# A full, unabbreviated commit id. `push_master` below refuses anything else:
# an abbreviation or a NAME would be re-resolved by git, which is the exact
# hazard that helper exists to remove.
_FULL_SHA = re.compile(r"[0-9a-f]{40}")


class GitError(RuntimeError):
    pass


@dataclass
class RebaseResult:
    ok: bool
    conflicts: list[str] = field(default_factory=list)


@dataclass(frozen=True)
class TreeState:
    """Two INDEPENDENT verdicts about one working tree, because `git status`
    answers a question nobody asked.

    `semantic_paths` -- tracked paths whose CONTENT differs from HEAD (staged
    and unstaged in one comparison, clean filters applied). This is the only
    thing that is evidence about the tree's contents.

    `status_lines` -- the raw porcelain lines (` M path`) for whatever
    porcelain still calls dirt AFTER that content comparison came back clean.
    An operationally odd checkout, never a changed one.

    Why both: on 2026-09-14 `cmd_post_merge` ran on merge 19f654e1, its three
    real gates returned 0, 0, 0, and it entered the REVERT path anyway. The
    single dirty file was `crates/al-syntax/src/raw/generated/node-types.sha256`
    and its content was BYTE-IDENTICAL -- it carried no line-ending attribute,
    so with `core.autocrlf=true` git materialised CRLF while the generator
    writes LF. `git status` reported it modified, permanently; `git diff`
    reported nothing. A probe that only asks porcelain cannot tell those two
    situations apart, and it reverted a fully-attested, CI-green commit for the
    difference.
    """
    semantic_paths: list[str]
    status_lines: list[str]

    @property
    def semantic(self) -> bool:
        return bool(self.semantic_paths)

    @property
    def status_only(self) -> bool:
        return bool(self.status_lines) and not self.semantic_paths


class Git:
    # Measured 2026-09-14 across all three submodule cases. The gitlink SHA
    # recorded in the superproject IS this repo's content, so a MOVED POINTER
    # stays semantic and still reverts; the submodule's own working tree is
    # not this repo's content, so build artifacts a gate drops inside
    # `tree-sitter-al/` never revert a merge. Without this flag BOTH probes
    # report the submodule path for a dirty-worktree/unchanged-pointer
    # submodule, which is indistinguishable from a real change.
    _IGNORE_SUB = "--ignore-submodules=dirty"

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

    def tree_state(self, refresh: bool = True) -> TreeState:
        """Ask the working tree BOTH questions -- see `TreeState`.

        `refresh` runs `git update-index -q --refresh` first, which settles a
        pure STAT difference (identical bytes rewritten, an mtime touched) so
        it never reaches the status probe below. Two things about it are
        deliberate. Its exit code carries NO information -- `-q` makes it exit
        0 even when a file genuinely differs -- so it is never read. And it
        WRITES `.git/index` (the stat cache only, never content), which is the
        whole reason `refresh=False` exists: a caller running under
        `--dry-run` must be able to ask without writing.

        Measured, and the reason the content comparison below is what actually
        fixes the incident: `update-index --refresh` does NOT heal the
        CRLF-vs-LF class. Status still reports ` M path` afterwards, forever.

        `HEAD` in the diff is load-bearing. A STAGED-only change is invisible
        to a bare `git diff --name-only` and IS listed by
        `git diff --name-only HEAD --` (measured). `-z` avoids git's path
        quoting. Untracked paths are invisible to `git diff` at all, which is
        what preserves `tracked_dirty`'s existing contract below.

        Note `git diff --name-only HEAD --` exits 0 whether or not it lists
        paths (measured on git 2.55.0.windows.3), which is what makes the
        `check=True` call safe. Do NOT "fix" that with `--exit-code`: it would
        make `_run` raise GitError on every dirty tree.

        Status is read from `.stdout` DIRECTLY rather than through `out()`,
        which `.strip()`s -- that would eat the leading space of a ` M path`
        entry on the first line only, mangling the XY prefix inconsistently.
        The prefix (` M` unstaged, `M ` staged, `MM` both) is the diagnostic
        value, so it must survive verbatim.

        Raises GitError on an unborn HEAD (a repo with no commits). Reporting
        "clean" for a tree the probe cannot actually read would be the worse
        failure, so that is not defended against.
        """
        if refresh:
            self._run("update-index", "-q", "--refresh", check=False)
        raw = self._run("diff", "-z", "--name-only", self._IGNORE_SUB, "HEAD", "--").stdout
        semantic = [p for p in raw.split("\0") if p]
        status = [l for l in self._run("status", "--porcelain", "--untracked-files=no",
                                       self._IGNORE_SUB).stdout.splitlines() if l]
        return TreeState(semantic_paths=semantic, status_lines=status)

    def tracked_dirty(self) -> bool:
        """True only when a TRACKED path's CONTENT differs from HEAD.

        Ignores untracked paths -- the caller's own tooling routinely creates
        them (gate logs under `.agent/runs`, `__pycache__`, ...) and they are
        not evidence about anything. Also ignores a path whose content matches
        but whose checkout state is merely odd (line endings, attributes): see
        `tree_state`, which is the one implementation of the probe."""
        return self.tree_state().semantic

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

    def worktree_add_detached(self, path: Path, commit: str) -> None:
        """A worktree at `commit` with a DETACHED HEAD and no branch of its own.

        Branchless on purpose: a verification worktree is disposable and torn
        down with `rm -rf`, and a branch would outlive that as a ref -- one
        that then has to be deleted, and that something could push. Nothing
        may survive the directory.

        Measured on git 2.55.0.windows.3: `--detach` leaves a genuinely
        detached HEAD (`rev-parse --abbrev-ref HEAD` -> `HEAD`), and
        `worktree add` over an EXISTING directory fails rc=128
        (`fatal: '<path>' already exists`) -- which is why `worktrees.create`
        destroys any leftover first."""
        self._run("worktree", "add", "--detach", "-q", str(path), commit)

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
        """Low-level push with a caller-supplied refspec. NO production module
        may call this, and `tests/test_gitops.py` pins that emptiness: a bare
        name like `"master"` is a ONE-SIDED refspec, whose source git
        re-resolves (rev-parse prefers `refs/tags/<n>` over `refs/heads/<n>`)
        and whose destination is inferred from that source. `push_branch` and
        `push_master` below are the two forms the flow actually uses. This one
        survives for TEST SETUP, where a one-sided push to a scratch origin is
        the point."""
        return self.ok("push", "-q", remote, refspec)

    def push_branch(self, branch: str, force_with_lease: bool = False) -> bool:
        """Push one branch to `origin` under an EXPLICIT two-sided refspec, so the
        destination ref is built here and can never be inferred from caller text.
        A one-sided `git push origin <arg>` reads a colon in `<arg>` as
        "local:remote", which is how `feat:master` reaches remote `master`;
        `refs/heads/<b>:refs/heads/<b>` cannot name anything but `<b>`.
        `branch` must already be a validated plain branch name (see
        `cli._validate_branch_name`) -- a name carrying a colon would otherwise
        produce a malformed refspec. `--force-with-lease` is the only force form
        the flow may ever use."""
        refspec = f"refs/heads/{branch}:refs/heads/{branch}"
        args = ["push", "-q", "-u"] + (["--force-with-lease"] if force_with_lease else []) + ["origin", refspec]
        return self.ok(*args)

    def push_master(self, sha: str) -> bool:
        """Push ONE already-validated commit onto `origin`'s `master` under an
        EXPLICIT two-sided refspec, `<sha>:refs/heads/master`.

        `push_branch` above explains why the DESTINATION half must be built
        here. The revert push was the last place in the package still using the
        one-sided `git push origin master`, and it needs a second guarantee on
        top of that one: the SOURCE must be a commit id, not the name `master`.

        * A one-sided push re-resolves `master` through rev-parse's
          disambiguation rules, which prefer `refs/tags/master` over
          `refs/heads/master`. A name is not a fact. Measured: with a tag named
          `master` present, `git push origin master` fails outright with
          "error: src refspec master matches more than one", while
          `<sha>:refs/heads/master` is unaffected.
        * `git push origin master` sends whatever the local ref points at AT
          PUSH TIME. `recovery.post_merge_failure` reaches its push only after
          a full gate re-run lasting tens of minutes on the shared checkout,
          so "local master" is no longer something it owns -- anything that
          moved the checkout in that window would ride along, ungated, onto
          `master`. Its existing pre-push checks guard `origin/master`, never
          the local ref. Measured: `<sha>:refs/heads/master` sends exactly that
          commit even with local `master` two commits ahead.

        `sha` must be a full 40-character lowercase hex id. An abbreviation or
        a name reintroduces the re-resolution this exists to remove, and that
        is a programming error rather than a push failure -- so it RAISES
        instead of returning False, which would misreport it to the caller as
        a rejected push. Never forces: measured, a non-fast-forward
        `<sha>:refs/heads/master` is rejected by git on its own, and
        `--force-with-lease` on a feature branch stays the only force form the
        flow may use.
        """
        if not _FULL_SHA.fullmatch(sha):
            raise GitError(f"push_master needs a full 40-hex commit id, got {sha!r}")
        return self.ok("push", "-q", "origin", f"{sha}:refs/heads/master")

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
