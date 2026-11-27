"""Disposable verification worktrees: create one at a commit, run gates in it,
delete it.

WHY a gate must not run in the shared checkout. The post-merge path verifies a
merge commit and, if a gate goes red, builds a revert and RE-RUNS the gates to
decide whether that revert may be pushed to `origin/master`. When both runs
happen in the same working tree, everything the first run left behind -- a
regenerated golden, a touched `Cargo.lock`, a `gen-syntax` rewrite, a
line-ending materialisation -- is inherited by the run that decides the push.
A revert that is RED from a clean checkout can be green-lit by the leavings of
the run it is reverting. Each verification therefore gets a tree that was
checked out seconds ago and is deleted afterwards.

TEARDOWN is `rm -rf` plus `git worktree prune`, never `git worktree remove`:
that command REFUSES outright on a worktree created while `tree-sitter-al/`
was a submodule ("working trees containing submodules cannot be moved or
removed"). Since 2026-10-03 the grammar is a plain gitignored clone, so new
worktrees would allow it, but `rm -rf` works for both. See CLAUDE.md,
Prerequisites.

GATES CAN COMPILE IN THERE even though a worktree has no grammar clone of its
own (it is gitignored), because `cli._run_gate` derives `TREE_SITTER_AL_PATH` from
`ctx.paths.root` rather than from the gate's cwd, and `scripts/ci-steps`'s
`grammar_dir()` honours that env var over its own `git rev-parse
--show-toplevel` fallback. That is a real dependency between two files, so
`tests/test_cli_flow.py` pins it rather than leaving it to be rediscovered.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import time
from pathlib import Path

from .gitops import Git, GitError
from .state import MutexTimeout, mutex, read_json, write_json

# The name is the ownership proof. `destroy` below deletes a directory tree,
# and the ONLY thing standing between it and an arbitrary path is this prefix.
VERIFY_PREFIX = "al-sem-verify-"


def link_target(worktree: Path, target_root: Path) -> Path:
    """Make `<worktree>/target` a link into `<target_root>/<worktree name>`
    and return that folder. One folder PER worktree, so two runs never build
    into one directory (#60); only the drive moves. A folder left by an
    earlier attempt of the same name is reused: it is cargo's own cache.

    A junction on Windows (no admin right needed, unlike a symlink).
    `shutil.rmtree` of the worktree removes the junction and leaves the
    folder, which `unlink_target` deletes."""
    built = Path(target_root) / worktree.name
    built.mkdir(parents=True, exist_ok=True)
    link = worktree / "target"
    if os.name == "nt":
        import _winapi
        _winapi.CreateJunction(str(built), str(link))
    else:
        os.symlink(built, link, target_is_directory=True)
    return built


SEED_SKIP = {"flycheck0", "tmp"}   # rust-analyzer's own builds and scratch: never reused


def _reflink_copy(src: Path, dst_dir: Path) -> None:
    subprocess.run(["cp", "-a", "--reflink=always", str(src), str(dst_dir)],
                   check=True, capture_output=True)


def seed_target(root: Path, worktree: Path) -> bool:
    """Start `<worktree>/target` as a copy-on-write copy of the root
    checkout's `target/`, so a new worktree does not rebuild every dependency
    from scratch (deps build at opt-level 2, in up to four profiles).

    SAFE AGAINST STALE TEST BINARIES (see `verify_path`): a fresh checkout
    gives every workspace source a newer mtime than any copied artifact, so
    cargo rebuilds every workspace crate (and re-bakes `CARGO_MANIFEST_DIR`)
    and reuses only registry dependencies. `cp --reflink=always` fails rather
    than doing a full copy on a filesystem without reflinks; any failure
    leaves no `target/` and returns False, so cargo builds cold as before."""
    src, dst = root / "target", worktree / "target"
    if os.name == "nt" or not src.is_dir() or dst.exists():
        return False
    dst.mkdir()
    try:
        for entry in src.iterdir():
            if entry.name not in SEED_SKIP:
                _reflink_copy(entry, dst)
    except (OSError, subprocess.CalledProcessError):
        shutil.rmtree(dst, ignore_errors=True)
        return False
    return True


def unlink_target(worktree: Path, target_root: Path) -> None:
    """Delete `<target_root>/<worktree name>`, the build folder `link_target`
    made, and nothing else. Absent is fine (already cleaned, or never linked)."""
    root = Path(target_root).resolve()
    built = (root / worktree.name).resolve()
    if built.parent != root or not worktree.name:
        raise RuntimeError(f"{built} is not a build folder directly under {root}")
    if built.exists():
        shutil.rmtree(built)


def _guard(path: Path) -> None:
    """Refuse any path this module does not own. Called FIRST in both `create`
    and `destroy` -- it is the guard on an `rm -rf`, so it raises rather than
    returning a verdict a caller could ignore."""
    if not path.name.startswith(VERIFY_PREFIX):
        raise RuntimeError(f"{path} is not an agentflow verification worktree")


VERIFY_TREE = f"{VERIFY_PREFIX}tree"


def verify_path(root: Path) -> Path:
    """Where the verification worktree lives: a SIBLING of the repo root, which
    is where the flow's issue worktrees already live (`cli.cmd_claim` uses
    `ctx.paths.root.parent`). The distinct prefix keeps it out of
    `recovery.recover_stale`'s glob, which matches `al-sem-issue-{issue}-a*`.

    ONE FIXED PATH, for the merge check and the revert check alike, never one
    per merge SHA. Test binaries bake `env!("CARGO_MANIFEST_DIR")` (175 uses
    in `tests/` and `src/`), and cargo reuses a binary whose Rust sources did
    not change, so a binary built in a tree at another path resolves its
    fixtures and goldens under THAT path. With a per-SHA path, the next
    post-merge of a merge that changes only fixtures ran binaries pointing at
    the previous, deleted tree: a false red, and post-merge reverts a good
    merge. A fixed path makes every binary in the verify cache
    (`cli._verify_target_dir`) name the tree it runs in. Safe because at most
    one exists at a time: post-merge runs only under the merge lane (one
    holder, through post-merge), and the merge tree is torn down before the
    revert tree is made (`cmd_post_merge`'s `finally`). `create` removes any
    leftover first, so a later post-merge reclaims a tree an earlier one
    RETAINED as evidence; that only follows an operator clearing the HALT the
    retention set."""
    return root.parent / VERIFY_TREE


class NotOurs(GitError):
    """The verification tree's path holds something agentflow cannot prove it
    made (a symlink, or a directory git has not registered as a worktree of
    this repository). Nothing was removed."""


def _stat_start(pid: int) -> str:
    """A process's start time: field 22 of `/proc/<pid>/stat`. Raises
    FileNotFoundError when no such process exists, any other OSError or
    ValueError when it exists but cannot be read."""
    stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    try:
        return stat[stat.rindex(")") + 2:].split()[19]
    except IndexError as e:
        raise ValueError(f"unexpected /proc/{pid}/stat") from e


def _own_start() -> str | None:
    try:
        return _stat_start(os.getpid())
    except (OSError, ValueError):
        return None


# THIS INVOCATION (one CLI process): what a reservation names (sol r13).
_ME = {"pid": os.getpid(), "start": _own_start(), "nonce": os.urandom(8).hex()}

ALIVE, DEAD, UNKNOWN = "alive", "dead", "unknown"


def _liveness(owner) -> str:
    """Whether the invocation a reservation names still runs: ALIVE, DEAD or
    UNKNOWN (sol r14). Only DEAD is ever reclaimed, and only by a new `create`
    or an explicit cleanup.
    - With /proc: no `/proc/<pid>` is DEAD; a start time that differs is DEAD
      (the PID was reused); the same start time is ALIVE; a stat that cannot
      be read is UNKNOWN, never DEAD.
    - Without /proc only the PID can be asked about: gone is DEAD, present is
      UNKNOWN (it may be a reused PID).
    - A reservation that is not a well-formed record: UNKNOWN."""
    pid = owner.get("pid") if isinstance(owner, dict) else None
    if not isinstance(pid, int) or isinstance(pid, bool) or pid <= 0:
        return UNKNOWN
    if Path("/proc").is_dir():
        recorded = owner.get("start")
        if not (isinstance(recorded, str) and recorded.isascii() and recorded.isdigit()):
            # No valid start time (clock ticks: digits only) to tell a live
            # owner from a reused PID (sol r15, r16).
            return UNKNOWN
        try:
            start = _stat_start(pid)
        except FileNotFoundError:
            return DEAD
        except (OSError, ValueError):
            return UNKNOWN
        return ALIVE if start == recorded else DEAD
    if os.name == "nt":
        import ctypes                                    # no /proc: ask Windows about the PID
        handle = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid)
        if handle:
            ctypes.windll.kernel32.CloseHandle(handle)
            return UNKNOWN
        return DEAD if ctypes.windll.kernel32.GetLastError() == 87 else UNKNOWN   # 87: no such process
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return DEAD
    except OSError:
        return UNKNOWN
    return UNKNOWN


def _reservation_path(ctx, path: Path) -> Path:
    return ctx.paths.agent / "verify-reservations" / f"{path.name}.json"


def _check_and_detach_held(git: Git, path: Path) -> Path | None:
    """Refuse (`NotOurs`) a symlink or a directory git has not registered for
    this repository; otherwise move the tree aside. ASSUMES `registry` held."""
    if path.is_symlink():
        raise NotOurs(f"{path} is a symlink; refusing to remove it")
    if not path.exists():
        return None
    if not registered(git, path):
        raise NotOurs(f"{path} is not a worktree git registered for this repository; "
                      f"refusing to remove it")
    return detach(path)


def _take_owned_held(ctx, git: Git, path: Path) -> Path | None:
    """The guarded step a NEW `create` and an explicit cleanup take before
    they remove the tree. ASSUMES mutex `registry` is held.

    A reservation another invocation holds is reclaimed only when that
    invocation is DEAD (`_liveness`); ALIVE or UNKNOWN refuses (`NotOurs`,
    touching nothing, the message naming the owner and the state, so cleanup
    can report it). Then the tree is checked (`_check_and_detach_held`), the
    reservation cleared, and the tombstone returned (None if nothing was
    there)."""
    res_path = _reservation_path(ctx, path)
    owner = read_json(res_path)
    mine = isinstance(owner, dict) and all(owner.get(k) == _ME[k] for k in ("pid", "start", "nonce"))
    if owner is not None and not mine:          # this invocation's own reservation is its to take
        state = _liveness(owner)
        if state != DEAD:
            pid = owner.get("pid") if isinstance(owner, dict) else None
            raise NotOurs(f"{path} is reserved by another invocation (pid {pid}, {state}); "
                          f"refusing to touch it. If that invocation has ended, delete {res_path}")
    tomb = _check_and_detach_held(git, path)
    res_path.unlink(missing_ok=True)
    return tomb


def _log(msg: str) -> None:
    import sys
    print(f"agentflow: {msg}", file=sys.stderr)


def remove(ctx, git: Git, path: Path, token: dict | None) -> bool:
    """Tear down the verification tree THIS create attempt made: `token` is
    what `create` returned (sol r14). It acts only while the path's
    reservation still equals `token` exactly; with no token (the create was
    refused, or never ran) or another reservation, it touches nothing and
    returns False. Reclaiming a stale reservation is `create`'s and explicit
    cleanup's job, never a teardown's.

    Otherwise: checked and moved aside under mutex `registry`, deleted after
    the lock is released. Returns whether `path` is free afterwards. FULLY
    best-effort: no failure escapes, a busy `registry` or a `git worktree
    prune` timing out on the `git` mutex included (sol r13, r14); it is logged
    to stderr, so a `finally` that calls this keeps its own result."""
    if token is None:
        return False
    try:
        with mutex(ctx, "registry"):
            res_path = _reservation_path(ctx, path)
            if read_json(res_path) != token:
                return False
            tomb = _check_and_detach_held(git, path)
            res_path.unlink(missing_ok=True)
    except Exception as e:
        _log(f"verification tree {path} not removed: {e}")
        return False
    gone = True
    if tomb is not None:
        try:
            gone = destroy(git, tomb)
        except Exception as e:
            _log(f"verification tree moved aside to {tomb} but not deleted: {e}")
            gone = False
    # True only when the tree is really gone, so a teardown failure is reported.
    # The check itself is best-effort too (sol r15).
    try:
        return gone and not (path.exists() or path.is_symlink())
    except Exception as e:
        _log(f"could not check that {path} is free: {e}")
        return False


def create(ctx, git: Git, path: Path, commit: str) -> dict:
    """A pristine detached worktree at `commit`; returns the TOKEN for it (the
    exact reservation this attempt wrote), which `remove` needs. Raises
    GitError if it cannot be made, or if it does not land where it was asked
    to; a refused create returns no token.

    A leftover verification tree (registered: a crashed previous run's) is
    moved aside and deleted first: `git worktree add` over an existing path
    fails rc=128, so such a leftover must never make the next verification
    unrunnable. An occupant agentflow cannot vouch for, or a reservation whose
    owner is not provably dead (`_take_owned_held`), is REFUSED (`NotOurs`),
    never deleted (sol r12-r14). The check, the move and the reservation
    happen under mutex `registry`; `git worktree add` runs after it, and
    refuses a path that is taken again by then.

    IT CLEANS UP AFTER ITS OWN FAILURE. `worktree_add_detached` has already
    created a checked-out directory by the time the postcondition below is
    evaluated, so a raise from here used to leave that directory -- and its
    `git worktree` registration -- behind, on a path whose whole contract is
    that nothing survives it. `cmd_post_merge` then reported
    `verify_worktree_removed: true` off an initialiser it never reassigned.
    Teardown belongs in the function that created the thing, so BOTH call
    sites get it rather than only the one that remembered."""
    _guard(path)
    with mutex(ctx, "registry"):
        try:
            tomb = _take_owned_held(ctx, git, path)
        except OSError as e:
            raise GitError(f"could not move the old verification tree aside: {e}") from e
        # Reserved for THIS ATTEMPT before the add (sol r13, r14): another
        # invocation now finds a reservation it cannot reclaim and refuses.
        token = {**_ME, "attempt": os.urandom(8).hex(), "path": str(path)}
        write_json(ctx, _reservation_path(ctx, path), token)
    # Everything after the token is published runs inside its rollback (sol r15).
    try:
        if tomb is not None:
            destroy(git, tomb)
        try:
            # A teardown whose prune failed left git's registration of this
            # path behind ("missing but already registered"); clear it before
            # the add. Best-effort: a busy `git` mutex is logged, not raised.
            git.worktree_prune()
        except (GitError, MutexTimeout) as e:
            _log(f"git worktree prune before the add: {e}")
        git.worktree_add_detached(path, commit)
        landed = Git(path, run=git.run).rev("HEAD")
        want = git.rev(commit)
        if landed != want:
            raise GitError(f"verification worktree {path} landed on {landed}, wanted {want}")
    except BaseException:
        # `except BaseException` and not `except GitError`: the point is that
        # the directory does not outlive this call, whatever ended it -- a
        # GitError from either `rev`, the postcondition raise, or a
        # KeyboardInterrupt. `destroy` is best-effort and swallows its own
        # failures, so it cannot mask the error being re-raised. Through the
        # guarded `remove` with this attempt's token, like every other
        # teardown of this path; `remove` never raises.
        remove(ctx, git, path, token)
        raise
    return token


def registered(git: Git, path: Path) -> bool:
    """`path` is a worktree git has registered for THIS repository: the main
    checkout's `git worktree list --porcelain` names it, and its `.git` is a
    regular file whose `gitdir:` points into this repository's `worktrees/`
    directory (sol r11). Evidence from outside the tree, so a directory that
    merely sits at the path is not taken for agentflow's. Any git or read
    failure: False.

    THE CONTRACT this rests on: the verification-tree path, under the
    checkout's parent, is reserved for agentflow, and only agentflow writes it,
    under its `registry` lock. A writer outside that lock is outside the trust
    model, just as a hand edit of `.agent/` is."""
    try:
        listed = {os.path.realpath(line[len("worktree "):])
                  for line in git.out("worktree", "list", "--porcelain").splitlines()
                  if line.startswith("worktree ")}
        common = Path(git.out("rev-parse", "--path-format=absolute", "--git-common-dir"))
        dotgit = path / ".git"
        if dotgit.is_symlink() or not dotgit.is_file():
            return False
        first = dotgit.read_text(encoding="utf-8").splitlines()[0]
    except (GitError, OSError, IndexError, UnicodeDecodeError):
        return False
    if os.path.realpath(path) not in listed or not first.startswith("gitdir: "):
        return False
    gitdir = Path(os.path.realpath(first[len("gitdir: "):].strip()))
    return gitdir.parent == Path(os.path.realpath(common / "worktrees"))


def detach(path: Path) -> Path | None:
    """Move a verification worktree off its fixed path, to a sibling named
    `<name>.removing-<id>` (still under VERIFY_PREFIX, so `destroy` takes it),
    and return where it went; None when there is nothing at `path`. One
    rename: fast, so a caller can do it under mutex `registry` and run the slow
    `destroy` after releasing it (TLS-122 sol r5). The fixed path is free at
    once, so a new verification can start there; `git worktree prune` in
    `destroy` drops the registration of the old path. OSError when the
    rename fails (e.g. Windows holds a file open)."""
    _guard(path)
    tomb = path.with_name(f"{path.name}{TOMBSTONE_INFIX}{os.urandom(4).hex()}")
    try:
        path.rename(tomb)
    except FileNotFoundError:
        return None
    return tomb


TOMBSTONE_INFIX = ".removing-"


def leftover_tombstones(home: Path) -> list[str]:
    """Entries beside `home` named `<home name>.removing-...` (plain string
    operations, never a pattern): what a cleanup that died between its rename
    and its delete may have left. LISTED ONLY, never deleted (sol r10): nothing
    on disk can prove agentflow made one, and a check-then-delete would race
    with whatever replaced it. Removing one is the operator's decision."""
    prefix = f"{home.name}{TOMBSTONE_INFIX}"
    try:
        return sorted(str(e) for e in home.parent.iterdir() if e.name.startswith(prefix))
    except OSError:
        return []


def destroy(git: Git, path: Path) -> bool:
    """Delete the worktree directory and prune the stale registration. Returns
    whether the directory is gone.

    LOW LEVEL: only for a tree already moved aside by `detach` under mutex
    `registry` (a tombstone). The verification tree at its fixed path is
    removed through `remove` (or `create` / `cmd_cleanup`), which check and
    detach it under that lock first (sol r12).

    Everything after `_guard` is BEST EFFORT: a teardown failure must never
    change a gate verdict. Windows holds file locks on a tree a build just
    touched, so `shutil.rmtree` gets the same 3-attempt / 2-second retry
    `recovery.remove_worktree` uses -- and then gives up rather than raising,
    leaving the caller to report `verify_worktree_removed: false`."""
    _guard(path)
    for attempt in range(3):
        if not path.exists():
            break
        try:
            shutil.rmtree(path)
            break
        except OSError:
            if attempt == 2:
                break  # deliberately swallowed: see the docstring
            time.sleep(2)
    try:
        git.worktree_prune()
    except (GitError, MutexTimeout):
        pass                       # best-effort, a busy `git` mutex included (sol r15)
    return not path.exists()
