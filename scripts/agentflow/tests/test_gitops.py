import re
import subprocess
from pathlib import Path

import pytest

from agentflow.gitops import Git, GitError
from agentflow.tests.conftest import commit_file


def test_basic_queries(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    assert g.branch() == "master" and g.is_clean()
    a = g.rev("HEAD")
    b = commit_file(clone, "src/x.rs", "fn x(){}\n", "add x")
    assert g.changed_files(a, b) == ["src/x.rs"]
    assert "+fn x(){}" in g.diff(a, b)
    assert g.is_ancestor(a, b) and not g.is_ancestor(b, a)


def test_tracked_dirty_ignores_untracked_but_sees_content_changed_tracked_files(repo_pair):
    # RENAMED, body unchanged, and it must keep passing byte-for-byte: that is
    # the proof that reimplementing `tracked_dirty()` on `tree_state()` is
    # behaviour-preserving. Only the NAME over-claimed afterwards -- "modified"
    # now means content-modified, and a status-modified-but-byte-identical file
    # is deliberately not dirty (see the line-ending test below).
    origin, clone = repo_pair
    g = Git(clone)
    assert not g.tracked_dirty()
    (clone / "untracked.txt").write_text("x\n")
    assert not g.tracked_dirty()  # untracked: not tracked-dirty
    assert not g.is_clean()       # but the plain porcelain check DOES see it
    (clone / "untracked.txt").unlink()
    (clone / "README.md").write_text("changed\n")
    assert g.tracked_dirty()      # a modified TRACKED file: dirty


def test_worktree_add_and_prune(repo_pair, tmp_path):
    origin, clone = repo_pair
    g = Git(clone)
    wt = tmp_path / "wt-a1"
    g.worktree_add(wt, "issue/8-x-a1", "master")
    assert Git(wt).branch() == "issue/8-x-a1"
    import shutil
    shutil.rmtree(wt)
    g.worktree_prune()
    g.branch_delete("issue/8-x-a1")
    assert "issue/8-x-a1" not in g.out("branch", "--list")


def test_rebase_ok_and_conflict_aborts(repo_pair, tmp_path):
    origin, clone = repo_pair
    g = Git(clone)
    g.out("checkout", "-q", "-b", "feat")
    commit_file(clone, "a.txt", "feat\n", "feat a")
    g.out("checkout", "-q", "master")
    commit_file(clone, "b.txt", "master\n", "master b")
    g.out("push", "-q", "origin", "master")
    g.out("checkout", "-q", "feat")
    r = g.rebase("origin/master")
    assert r.ok and r.conflicts == []
    commit_file(clone, "b.txt", "feat edit\n", "feat b")
    g.out("checkout", "-q", "master")
    commit_file(clone, "b.txt", "master edit\n", "master b2")
    g.out("push", "-q", "origin", "master")
    g.out("checkout", "-q", "feat")
    r = g.rebase("origin/master")
    assert not r.ok and r.conflicts == ["b.txt"] and g.is_clean() and g.branch() == "feat"


def test_revert_push_and_ff(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    bad = commit_file(clone, "c.txt", "bad\n", "bad")
    assert g.push("origin", "master")
    assert g.revert(bad)
    assert not (clone / "c.txt").exists()
    assert g.push("origin", "master")
    g.out("reset", "-q", "--hard", "HEAD~1")
    assert g.ff("origin/master") and not (clone / "c.txt").exists()


def test_merge_squash_helper(repo_pair):
    origin, clone = repo_pair
    g = Git(clone)
    g.out("checkout", "-q", "-b", "feat")
    commit_file(clone, "d.txt", "d\n", "d")
    g.out("checkout", "-q", "master")
    sha = g.merge_squash("feat", "squash feat")
    assert g.rev("HEAD") == sha and (clone / "d.txt").exists()


def test_push_master_sends_the_named_commit_not_whatever_master_points_at(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    first = commit_file(clone, "first.txt", "1\n", "first")
    second = commit_file(clone, "second.txt", "2\n", "second")
    assert g.rev("master") == second          # precondition, stated not assumed
    assert g.push_master(first)
    assert g.out("ls-remote", "origin", "refs/heads/master").split()[0] == first


def test_push_master_refuses_anything_that_is_not_a_full_commit_id(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    sha = commit_file(clone, "x.txt", "x\n", "x")
    before = g.out("ls-remote", "origin", "refs/heads/master")
    # Every case asserts the remote is UNMOVED as well as that it raised: a
    # refusal that still wrote is the failure actually being guarded against.
    for bad in ("master", "refs/heads/master", "HEAD", sha[:12], sha.upper(),
                f"{sha}:refs/heads/master", f"{sha} ", ""):
        with pytest.raises(GitError):
            g.push_master(bad)
        assert g.out("ls-remote", "origin", "refs/heads/master") == before, bad


def test_push_master_never_forces(repo_pair):
    _, clone = repo_pair
    g = Git(clone)
    ahead = commit_file(clone, "ahead.txt", "a\n", "ahead")
    assert g.push_master(ahead)
    behind = g.rev(f"{ahead}~1")
    assert not g.push_master(behind)          # non-fast-forward: rejected, not forced
    assert g.out("ls-remote", "origin", "refs/heads/master").split()[0] == ahead


def test_no_production_module_issues_a_one_sided_push():
    # `Git.push` takes a caller-supplied refspec, so a bare name reaches git as
    # a ONE-SIDED push -- source re-resolved, destination inferred. Only test
    # setup may do that. Production must go through `push_branch`/`push_master`,
    # which build both halves themselves. Pinning only the one call site would
    # go green the moment the next module reinvents the call, so this pins the
    # package-wide property instead.
    pkg = Path(__file__).resolve().parents[1]
    offenders = [f"{f.name}:{n}: {line.strip()}"
                 for f in sorted(pkg.glob("*.py"))
                 for n, line in enumerate(f.read_text(encoding="utf-8").splitlines(), 1)
                 if re.search(r"\.push\(", line)]
    assert offenders == [], offenders


# ---- tree_state: what counts as dirt --------------------------------------
# EVERY test below asserts its PRECONDITION with raw git before asserting the
# verdict. The reverse CRLF direction (worktree CRLF, blob LF, autocrlf=true)
# reads CLEAN, so a fixture built the wrong way round would pass vacuously and
# look exactly like a passing test.

def _raw(cwd, *args):
    return subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True)


def test_tree_state_calls_a_line_ending_only_difference_clean_not_dirty(repo_pair):
    """THE 2026-09-14 INCIDENT, hand-stated end to end. `core.autocrlf=true`,
    a file committed from CRLF bytes, then rewritten as LF by a generator.
    `git status` calls it modified forever; its content is byte-identical."""
    _, clone = repo_pair
    g = Git(clone)
    _raw(clone, "config", "core.autocrlf", "true")
    # write_BYTES, never write_text: write_text translates newlines on Windows
    # and would silently build the fixture the other way round.
    (clone / "gen.sha256").write_bytes(b"deadbeef\r\n")
    _raw(clone, "add", "gen.sha256")
    _raw(clone, "commit", "-q", "-m", "sidecar committed from CRLF bytes")
    assert _raw(clone, "status", "--porcelain", "-uno").stdout.strip() == ""   # clean to start
    (clone / "gen.sha256").write_bytes(b"deadbeef\n")                          # what gen-syntax does

    # PRECONDITION, with raw git, independent of the code under test:
    assert _raw(clone, "status", "--porcelain", "-uno").stdout.strip() != ""   # porcelain: modified
    assert _raw(clone, "diff", "--name-only", "HEAD", "--").stdout.strip() == ""  # content: identical

    st = g.tree_state()
    assert st.semantic is False and st.semantic_paths == []
    assert st.status_only is True
    assert any("gen.sha256" in l for l in st.status_lines), st.status_lines
    assert g.tracked_dirty() is False


def test_tree_state_reports_a_real_content_change_as_semantic(repo_pair):
    """The control. Without it the probe could be trivially "always clean" and
    the incident test above would still pass."""
    _, clone = repo_pair
    g = Git(clone)
    (clone / "README.md").write_bytes(b"genuinely different\n")
    assert _raw(clone, "diff", "--name-only", "HEAD", "--").stdout.split() == ["README.md"]  # precondition
    st = g.tree_state()
    assert st.semantic is True and st.semantic_paths == ["README.md"]
    assert st.status_only is False
    assert g.tracked_dirty() is True


def test_tree_state_sees_a_staged_only_change(repo_pair):
    """Pins the `HEAD` argument. A staged-but-uncommitted change is invisible
    to a bare `git diff --name-only`; dropping `HEAD` would make staged dirt
    vanish from the probe entirely."""
    _, clone = repo_pair
    g = Git(clone)
    (clone / "README.md").write_bytes(b"staged but not committed\n")
    _raw(clone, "add", "README.md")
    # PRECONDITION -- this empty result IS the point of the test:
    assert _raw(clone, "diff", "--name-only").stdout.strip() == ""
    assert _raw(clone, "diff", "--name-only", "HEAD", "--").stdout.split() == ["README.md"]
    st = g.tree_state()
    assert st.semantic is True and "README.md" in st.semantic_paths


def _init_repo(d):
    d.mkdir(parents=True)
    _raw(d, "init", "-q", "-b", "master")
    _raw(d, "config", "user.email", "t@example.com")
    _raw(d, "config", "user.name", "T")
    return d


def _super_with_submodule(tmp_path, name, sub):
    """A superproject carrying `sub` as a submodule, committed and clean.

    Each half of the test below gets its OWN superproject rather than one
    being restored into the other: once a submodule worktree has been dirtied,
    `git -C sub checkout` refuses ("local changes would be overwritten") and
    only a FORCED checkout gets past it, which would be discarding state to
    make a test pass. Two fixtures is the honest way to state two
    preconditions."""
    sup = _init_repo(tmp_path / name)
    (sup / "README.md").write_bytes(b"hi\n")
    _raw(sup, "add", "README.md"); _raw(sup, "commit", "-q", "-m", "init")
    # `-c protocol.file.allow=always` is REQUIRED: git >= 2.38 blocks file://
    # submodules by default, and without it this fails with a protocol error
    # rather than an assertion.
    r = _raw(sup, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(sub), "sub")
    assert r.returncode == 0, r.stderr
    _raw(sup, "commit", "-q", "-m", "add sub")
    st = Git(sup).tree_state()
    assert st.semantic is False and st.status_lines == [], (name, st)   # clean to start
    return sup


def test_tree_state_ignores_a_dirty_submodule_worktree_but_not_a_moved_pointer(tmp_path):
    """A live false-revert hazard in this repo: a gate dropping a build
    artifact inside `tree-sitter-al/` must not revert a merge, while a MOVED
    submodule pointer IS this repo's own content and must."""
    sub = _init_repo(tmp_path / "subrepo")
    (sub / "a.txt").write_bytes(b"one\n")
    _raw(sub, "add", "a.txt"); _raw(sub, "commit", "-q", "-m", "c1")
    (sub / "a.txt").write_bytes(b"two\n")
    _raw(sub, "add", "a.txt"); _raw(sub, "commit", "-q", "-m", "c2")

    # HALF A: dirty submodule WORKTREE, pointer unchanged -> NOT this repo's
    # content, so not dirt.
    sup_a = _super_with_submodule(tmp_path, "super_a", sub)
    (sup_a / "sub" / "junk.txt").write_bytes(b"a gate's build artifact\n")
    (sup_a / "sub" / "a.txt").write_bytes(b"modified inside the submodule\n")
    # PRECONDITION: without the policy this is INDISTINGUISHABLE from a real
    # change -- both plain probes report the submodule path.
    assert _raw(sup_a, "status", "--porcelain", "-uno").stdout.split() == ["M", "sub"]
    assert _raw(sup_a, "diff", "--name-only", "HEAD", "--").stdout.split() == ["sub"]
    st = Git(sup_a).tree_state()
    assert st.semantic is False and st.status_lines == []

    # HALF B: pointer MOVED on an otherwise clean submodule -> the gitlink SHA
    # recorded in the superproject changed, which IS this repo's content.
    sup_b = _super_with_submodule(tmp_path, "super_b", sub)
    r = _raw(sup_b / "sub", "checkout", "-q", "HEAD~1")
    assert r.returncode == 0, r.stderr        # the precondition really applied
    st = Git(sup_b).tree_state()
    assert st.semantic is True and st.semantic_paths == ["sub"]
