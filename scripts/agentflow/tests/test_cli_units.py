import json
import re

import pytest

from agentflow import cli, lock
from agentflow.tests.conftest import FakeRunner

REPO = "SShadowS/al-sem"


def run_cli(capsys, root, *args, gh_run=None, dry=False, run_id="run-test"):
    argv = ["--root", str(root), "--repo", REPO]
    if dry:
        argv.append("--dry-run")
    if run_id:
        argv += ["--run-id", run_id]
    code = cli.main(argv + list(args), gh_run=gh_run)
    out = capsys.readouterr().out
    return code, json.loads(out)


def test_the_rule_mark_must_fill_every_slot_but_the_raisers():
    """P3's "rule mark only in the OTHER slots" check. With today's two-slot roster it
    follows from "the raiser accepted" plus "a rule mark is present", so no register
    test can see it; it decides only for a larger roster, stated here literally."""
    roster, entry = ("astra", "flash", "third"), {"source": "astra"}
    yes = lambda e: True  # noqa: E731 -- the prose check is not what this pins
    rule = cli.RULE_MARK
    assert cli._rule_marks(["accepted", rule, rule], "minor", False, entry, yes, roster) == ["accepted"] * 3
    for marks in (["accepted", rule, "re-raised"], ["accepted", rule, "accepted"], ["accepted", rule, None]):
        assert cli._rule_marks(marks, "minor", False, entry, yes, roster) == marks, marks


def test_slug():
    assert cli.slug("c10: Scope and elevation reachability across .app symbols") == "c10-scope-and-elevation-reachab"


# ---- `_verify_target_dir`'s precedence chain -------------------------------
# THREE RUNGS, THREE CASES, one per sentence of the docstring: "an explicit
# `AGENTFLOW_VERIFY_TARGET_DIR` wins, then an operator's inherited
# `CARGO_TARGET_DIR`, then the root's `target/`". None of it was pinned. The
# single assertion that touched this function lived in a CLI test that
# inherited the real process environment, so it FAILED on exactly the machine
# configuration rung 2 exists to support (reproduced:
# `CARGO_TARGET_DIR=U:/shared-cargo-target ... -k grammar` -> 1 failed) and
# proved nothing about precedence in either direction. Each case below
# delenvs every rung it is not asserting, so none of them can be answered by
# the machine the suite happens to run on.

def test_verify_target_dir_falls_back_to_its_own_cache_never_the_checkouts_target(ctx, monkeypatch):
    """RUNG 3 is `<root>/target-verify`. Sharing `<root>/target` let the main
    checkout's gates reuse test binaries built in the verify tree, whose baked
    CARGO_MANIFEST_DIR named that (deleted) tree (#190's post-merge, 81e858ac)."""
    monkeypatch.delenv("AGENTFLOW_VERIFY_TARGET_DIR", raising=False)
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    assert cli._verify_target_dir(ctx) == str(ctx.paths.root / "target-verify")
    assert cli._verify_target_dir(ctx) != str(ctx.paths.root / "target")


def test_verify_target_dir_honours_an_operators_inherited_cargo_target_dir(ctx, monkeypatch):
    """RUNG 2 BEATS RUNG 3: an operator who has already pointed cargo somewhere
    with room is the case it must not override."""
    monkeypatch.delenv("AGENTFLOW_VERIFY_TARGET_DIR", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", "U:/shared-cargo-target")
    assert cli._verify_target_dir(ctx) == "U:/shared-cargo-target"


def test_an_explicit_verify_target_dir_beats_the_inherited_cargo_one(ctx, monkeypatch):
    """RUNG 1 BEATS RUNG 2 -- the direction with no test at all, so the chain
    could have been reordered, or the agentflow-specific rung dropped
    outright, with the whole suite green."""
    monkeypatch.setenv("CARGO_TARGET_DIR", "U:/shared-cargo-target")
    monkeypatch.setenv("AGENTFLOW_VERIFY_TARGET_DIR", "U:/agentflow-only-target")
    assert cli._verify_target_dir(ctx) == "U:/agentflow-only-target"


def test_halt_check_and_set(capsys, root):
    code, out = run_cli(capsys, root, "halt-check")
    assert code == 0 and out["halted"] is None
    code, out = run_cli(capsys, root, "set-halt", "manual stop")
    assert code == 0 and out["halted"] == "manual stop"
    code, out = run_cli(capsys, root, "halt-check")
    assert code == 1 and out["halted"] == "manual stop"
    code, _ = run_cli(capsys, root, "halt-check", "--terminal")
    assert code == 0


def halt_by_hand(root, text):
    """State the precondition BY ASSIGNMENT: `.agent/HALT` with this exact
    text, no lock, nothing else. Nothing here asks `post_merge_failure` to
    produce a HALT for these tests to clear."""
    from agentflow.state import Ctx, Paths
    lock.set_halt(Ctx(Paths(root), run_id=None, now=lambda: 1_000_000.0), text)


def test_clear_halt_refuses_an_ordinary_invocation_carrying_a_run_id(capsys, root, monkeypatch):
    """T6. The kill switch must not be reachable by the loop that the kill
    switch exists to stop. Documenting "the conductor never calls clear-halt"
    is not what enforces that; `lock.require_operator` is.

    Part (c) is mandatory, not decoration: without it, the refusals in (a) and
    (b) would be satisfied just as well by a command that never works at all.
    """
    halt_by_hand(root, "incident A")
    before = (root / ".agent" / "HALT").read_bytes()
    # (a) the explicit flag.
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident A", "--by", "me",
                        run_id="run-test")
    assert code == 1 and out["refused"] == "operator-only" and out["cleared"] is None
    assert (root / ".agent" / "HALT").read_bytes() == before
    # (b) the ENV route, which is how the conductor actually carries it: the
    # parser's default is evaluated inside `main` on every call, so a shell
    # that still exports it from an earlier tick is automation too.
    monkeypatch.setenv("AGENTFLOW_RUN_ID", "tick-1")
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident A", "--by", "me",
                        run_id=None)
    assert code == 1 and out["refused"] == "operator-only"
    assert (root / ".agent" / "HALT").read_bytes() == before
    # (c) an operator, at a shell with nothing exported.
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident A", "--by", "me",
                        run_id=None)
    assert code == 0 and out["cleared"] == "incident A"
    assert out["unresolved_incidents"] == []
    assert not (root / ".agent" / "HALT").exists()


def test_grant_is_operator_only_fails_closed_and_extends_the_live_run(capsys, root, monkeypatch):
    """P9: an operator override past a cap goes through the budget, not around it.
    Preconditions by assignment: run `run-a` holds issue 8 and has a budget."""
    from agentflow import budget
    from agentflow.state import Ctx, Paths
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    owner = Ctx(Paths(root), run_id="run-a", now=lambda: 1_000_000.0)
    lock.acquire(owner, 8, "s", 1)
    budget.init(owner, claimed_at=1_000_000.0)
    path = root / ".agent" / "runs" / "run-a" / "budget.json"
    before = path.read_bytes()
    grant = ("grant", "--issue", "8", "--key", "spec_rounds", "--n", "1", "--by", "me")
    # (a) automation: a run id on the call, by flag or by the environment.
    code, out = run_cli(capsys, root, *grant, run_id="run-a")
    assert code == 1 and out["refused"] == "operator-only"
    monkeypatch.setenv("AGENTFLOW_RUN_ID", "tick-1")
    code, out = run_cli(capsys, root, *grant, run_id=None)
    assert code == 1 and out["refused"] == "operator-only"
    monkeypatch.delenv("AGENTFLOW_RUN_ID")
    # (b) fail closed on what it is asked to grant.
    # Only panel rounds: `ci_fix`, `pi_calls` and the rest are real caps but not grantable.
    for args, refused in ((("--key", "spec-rounds"), "key-not-grantable"),
                          (("--key", "ci_fix"), "key-not-grantable"),
                          (("--key", "pi_calls"), "key-not-grantable"),
                          (("--key", "rebase_regate"), "key-not-grantable"),
                          (("--n", "0"), "bad-n"), (("--issue", "9"), "no-lease")):
        argv = list(grant)
        i = argv.index(args[0])
        argv[i + 1] = args[1]
        code, out = run_cli(capsys, root, *argv, run_id=None)
        assert code == 1 and out["refused"] == refused, args
    assert path.read_bytes() == before
    # (c) an operator at a clean shell, while the run is live: that is when a grant is needed.
    code, out = run_cli(capsys, root, *grant, run_id=None)
    assert code == 0 and out["granted"]["run_id"] == "run-a" and out["granted"]["cap"] == 6
    assert budget.snapshot(owner)["grants"] == {"spec_rounds": 1}


def test_clear_halt_refuses_a_stale_reason_and_a_live_lock(capsys, root, monkeypatch):
    """T7. Three refusals whose preconditions are all stated literally."""
    from agentflow.state import Ctx, Paths
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    # (a) REASON DRIFT: a newer incident overwrote HALT while the operator was
    # reading the older one. Clearing with the text they were looking at would
    # silently discard the newer stop.
    halt_by_hand(root, "incident A")
    halt_by_hand(root, "incident B")
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident A", "--by", "me",
                        run_id=None)
    assert code == 1 and out["refused"] == "reason-mismatch"
    assert lock.halted(cli_ctx(root)) == "incident B"
    # (b) a LIVE lock: a run is still acting on this checkout.
    lock.acquire(Ctx(Paths(root), run_id="other", now=lambda: 1_000_000.0), 8, "s", 1)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident B", "--by", "me",
                        run_id=None)
    assert code == 1 and out["refused"] == "operator-only"
    assert lock.halted(cli_ctx(root)) == "incident B"
    # (c) a STALE lock: allowed through, deliberately. Refusing it would
    # deadlock the operator for 30 minutes, since `recover` is the only thing
    # that clears one and it is itself reachable under HALT.
    monkeypatch.setenv("AGENTFLOW_NOW", str(1_000_000.0 + lock.STALE_SECONDS + 10))
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident B", "--by", "me",
                        run_id=None)
    assert code == 0 and out["cleared"] == "incident B"
    assert lock.halted(cli_ctx(root)) is None


def test_charge_and_status(capsys, root, ctx, monkeypatch):
    from agentflow import budget
    # AGENTFLOW_NOW is the clock seam cli._ctx honours: it lets this test hand
    # cli.main the SAME frozen clock the `ctx` fixture used to write
    # claimed_at, so budget.charge's wall-clock-deadline check compares a
    # frozen `now()` against a frozen `claimed_at`, not a frozen `claimed_at`
    # against the real wall clock (which would always exceed the wall-clock cap).
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    budget.init(ctx, claimed_at=ctx.now())
    code, out = run_cli(capsys, root, "charge", "ci_fix")
    assert code == 0 and out["remaining"] == 0
    code, out = run_cli(capsys, root, "charge", "ci_fix")
    assert code == 1 and out["exhausted"] == "ci_fix"
    code, out = run_cli(capsys, root, "status")
    assert out["budget"]["counts"] == {"ci_fix": 1}


def _two_leases(root, *, stale_issue=None):
    from agentflow.state import Ctx, Paths
    (root / ".agent" / "config.json").write_text(json.dumps({"slots": 2}), encoding="utf-8")
    for run_id, issue in (("run-a", 8), ("run-b", 9)):
        beat_at = 1.0 if issue == stale_issue else 1_000_000.0
        lock.acquire(Ctx(Paths(root), run_id=run_id, now=lambda t=beat_at: t), issue, "s", 1)


def test_status_lists_every_lease_and_the_runs_own(capsys, root, monkeypatch):
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    _two_leases(root, stale_issue=9)
    code, out = run_cli(capsys, root, "status", run_id="run-a")
    assert code == 0
    assert [(lk["issue"], lk["run_id"], lk["stale"]) for lk in out["locks"]] == [
        (8, "run-a", False), (9, "run-b", True)]
    assert out["lock"]["run_id"] == "run-a"
    code, out = run_cli(capsys, root, "status", run_id=None)
    assert out["lock"] is None and len(out["locks"]) == 2      # no single lease to show


def test_operator_commands_are_refused_while_any_lease_is_live(capsys, root, monkeypatch):
    """`require_operator` refuses while ANY run is acting, not only when the
    one and only lease is live. With a stale lease and a live one, the old
    single-lock read found no single lease and let a clear-halt through."""
    from agentflow.state import Ctx, Paths
    monkeypatch.delenv("AGENTFLOW_RUN_ID", raising=False)
    monkeypatch.setenv("AGENTFLOW_NOW", "1000000.0")
    _two_leases(root, stale_issue=9)
    halt_by_hand(root, "incident B")
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident B", "--by", "me", run_id=None)
    assert code == 1 and out["refused"] == "operator-only" and "run-a" in out["detail"], out
    lock.release(Ctx(Paths(root), run_id="run-a"))            # only the stale lease is left
    code, out = run_cli(capsys, root, "clear-halt", "--reason", "incident B", "--by", "me", run_id=None)
    assert code == 0 and out["cleared"] == "incident B", out


def test_charge_unknown_key_yields_json_not_traceback(capsys, root):
    code, out = run_cli(capsys, root, "charge", "bogus_key")
    assert code == 2 and "KeyError" in out["error"]


def test_sanitize_command(capsys, root, tmp_path, monkeypatch):
    monkeypatch.setenv("CDO_WS", r"U:\Git\CDO")
    f = tmp_path / "ledger.md"
    f.write_text("ok\nsee U:/Git/CDO/x.al\n")
    code, out = run_cli(capsys, root, "sanitize", str(f))
    assert code == 1 and out["violations"][str(f)][0]["kind"] == "cdo-path"


def test_unblock_removes_label_without_prior_lock(capsys, root, monkeypatch):
    """`unblock` runs under its own `unblock-<6 hex>` id (two unblocks must not
    share one) and does not take a slot."""
    ids, real = [], lock.acquire

    def recording(ctx, *a, **kw):
        ids.append((ctx.run_id, kw.get("count_slot", True)))
        return real(ctx, *a, **kw)

    monkeypatch.setattr(lock, "acquire", recording)
    r = FakeRunner({"issue edit 8 --remove-label agent-blocked": ""})
    for _ in range(2):
        code, out = run_cli(capsys, root, "unblock", "8", gh_run=r, run_id=None)
        assert code == 0 and out["unblocked"] == 8 and lock.read(cli_ctx(root)) is None
    assert len(ids) == 2 and ids[0][0] != ids[1][0]
    assert all(re.fullmatch(r"unblock-[0-9a-f]{6}", i) and not slot for i, slot in ids), ids


def test_unblock_refuses_an_issue_another_run_holds(capsys, root):
    """It used to lose to ANY live lock. Now it refuses `issue-held` for its
    own issue, and no label is touched."""
    from agentflow.state import Ctx, Paths
    lock.acquire(Ctx(Paths(root), run_id="run-holder", now=lambda: 1_000_000.0), 8, "s", 1)
    r = FakeRunner({"issue edit 8 --remove-label agent-blocked": ""})
    code, out = run_cli(capsys, root, "unblock", "8", gh_run=r, run_id=None)
    assert code == 1 and out["error"] == "issue-held:run-holder", out
    assert r.calls == []
    assert lock.for_issue(cli_ctx(root), 8).run_id == "run-holder"


def cli_ctx(root):
    from agentflow.state import Ctx, Paths
    return Ctx(Paths(root))


def test_loop_tick(capsys, root):
    run_cli(capsys, root, "loop-reset")
    code, out = run_cli(capsys, root, "loop-tick", "--max", "1")
    assert code == 0 and out["remaining"] == 0
    code, out = run_cli(capsys, root, "loop-tick", "--max", "1")
    assert code == 1


def test_resolve_bash_prefers_the_env_override(monkeypatch, tmp_path):
    # I1: an operator who knows which bash is right must be able to say so,
    # and nothing else may second-guess it.
    override = tmp_path / "my-bash.exe"
    override.write_text("")
    monkeypatch.setenv("AGENTFLOW_BASH", str(override))
    assert cli.resolve_bash() == str(override)


def test_resolve_bash_ignores_an_override_that_is_not_a_file(monkeypatch):
    # N4: a typo'd override used to be trusted, so preflight passed and the
    # failure surfaced later as a FileNotFoundError out of Popen. An override
    # that is not there is not an override.
    monkeypatch.setenv("AGENTFLOW_BASH", r"D:\nope\bash.exe")
    monkeypatch.setattr(cli, "_git_exec_path", lambda: None)
    monkeypatch.setattr(cli.shutil, "which", lambda name: "/usr/bin/bash")
    assert cli.resolve_bash() == "/usr/bin/bash"
    monkeypatch.setattr(cli.shutil, "which", lambda name: None)
    with pytest.raises(RuntimeError, match="no usable bash"):
        cli.resolve_bash()


def test_resolve_bash_rejects_the_wsl_launcher_on_path(monkeypatch):
    # I1: `which bash` under a PowerShell-inherited PATH finds
    # C:\WINDOWS\system32\bash.exe (the WSL launcher) first. A gate spawned
    # under it fails as a RED GATE rather than as an environment error, so it
    # must never be chosen. With the git derivation unavailable too there is
    # no usable bash, and that is an error rather than a silent fallback.
    monkeypatch.delenv("AGENTFLOW_BASH", raising=False)
    monkeypatch.setattr(cli, "_git_exec_path", lambda: None)
    monkeypatch.setattr(cli.shutil, "which", lambda name: r"C:\WINDOWS\system32\bash.exe")
    with pytest.raises(RuntimeError, match="no usable bash"):
        cli.resolve_bash()


def test_resolve_bash_accepts_a_path_bash_outside_the_rejected_dirs(monkeypatch):
    # The rejection above must be a targeted filter, not a blanket refusal of
    # whatever `which` returns.
    monkeypatch.delenv("AGENTFLOW_BASH", raising=False)
    monkeypatch.setattr(cli, "_git_exec_path", lambda: None)
    monkeypatch.setattr(cli.shutil, "which", lambda name: "/usr/bin/bash")
    assert cli.resolve_bash() == "/usr/bin/bash"


def test_resolve_bash_derives_from_git_exec_path(monkeypatch, tmp_path):
    # The primary route on Windows: Git for Windows ships the bash the gates
    # need beside its own exec-path, and that installation is the one `git`
    # itself is already running from.
    monkeypatch.delenv("AGENTFLOW_BASH", raising=False)
    git_root = tmp_path / "Git"
    (git_root / "usr" / "bin").mkdir(parents=True)
    (git_root / "usr" / "bin" / "bash.exe").write_text("")
    monkeypatch.setattr(cli, "_git_exec_path", lambda: str(git_root / "mingw64" / "libexec" / "git-core"))
    monkeypatch.setattr(cli.shutil, "which", lambda name: r"C:\WINDOWS\system32\bash.exe")
    assert cli.resolve_bash() == str(git_root / "usr" / "bin" / "bash.exe")


def test_preflight_reports_bash_when_it_cannot_be_resolved(capsys, repo_pair, monkeypatch):
    # I1: an unresolvable bash must surface at preflight as an environment
    # failure, not later as every gate failing red. (A real checkout is needed
    # here: preflight's first checks are git ones, and they fail loudly outside
    # a repository, which would mask the row this test is about.)
    _, clone = repo_pair
    monkeypatch.delenv("AGENTFLOW_BASH", raising=False)
    monkeypatch.setattr(cli, "_git_exec_path", lambda: None)
    monkeypatch.setattr(cli.shutil, "which", lambda name: r"C:\WINDOWS\system32\bash.exe")
    code, out = run_cli(capsys, clone, "preflight", gh_run=FakeRunner(), run_id=None)
    assert code == 1 and "bash" in out["failures"]
