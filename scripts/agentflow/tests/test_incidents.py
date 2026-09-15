"""T13. The incident module's own contracts.

Every input here is written LITERALLY. Nothing asks `post_merge_failure` to
produce a record so this file can read it back -- that is what the producer
tests in test_recovery.py are for, and a test that depends on production code
to build its own precondition dies the moment that code changes shape.
"""
import json

import pytest

from agentflow import incidents, lock
from agentflow.state import Ctx, DryRunViolation, Paths

SHA_A = "a" * 40
SHA_B = "b" * 40


def operator(ctx):
    """The same paths and clock, with NO run id: an operator at a shell."""
    return Ctx(paths=ctx.paths, run_id=None, now=ctx.now)


def test_opening_the_same_merge_twice_is_one_incident_not_two(ctx):
    """This idempotency is what lets `cmd_post_merge` open the record BEFORE
    the gates ("not verified yet", gate_red False) and `post_merge_failure`
    open it again with the real verdict, without minting a duplicate or
    resetting the clock a human reads to order the incidents."""
    first = incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                                    reason="post-merge verification has not completed")
    assert first["opened_at"] == 1_000_000.0
    later = Ctx(paths=ctx.paths, run_id="run-test", now=lambda: 2_000_000.0)
    second = incidents.open_incident(later, merge_sha=SHA_A, issue=8, gate_red=True,
                                     reason="post-merge gates went red")
    assert list(incidents.all_records(ctx)) == [SHA_A]      # ONE record
    assert second["opened_at"] == 1_000_000.0               # the clock was not reset
    assert second["gate_red"] is True                       # ...but the verdict updated
    assert second["reason"] == "post-merge gates went red"
    assert second["state"] == "open"


def test_unresolved_returns_only_open_records_in_a_deterministic_order(ctx):
    incidents.open_incident(ctx, merge_sha=SHA_B, issue=9, gate_red=True)
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=True)
    # Hand-stated: SHA_B is closed by editing the file directly, so the test
    # never depends on `resolve`/`mark_verified` working to set up `unresolved`.
    raw = json.loads(ctx.paths.incidents.read_text(encoding="utf-8"))
    raw[SHA_B]["state"] = "resolved"
    ctx.paths.incidents.write_text(json.dumps(raw, indent=2, sort_keys=True), encoding="utf-8")
    assert [r["merge_sha"] for r in incidents.unresolved(ctx)] == [SHA_A]
    # Both opened at the same frozen instant, so the tie-break by SHA is what
    # keeps preflight's failure rows from reordering between runs.
    incidents.open_incident(ctx, merge_sha=SHA_B, issue=9, gate_red=True)
    assert [r["merge_sha"] for r in incidents.unresolved(ctx)] == [SHA_A, SHA_B]


def test_reopening_an_incident_never_downgrades_a_red_gate(ctx):
    """"Idempotent" was not a true description of `open_incident`: every call
    rewrote `gate_red` and `reason`, so the in-progress call `cmd_post_merge`
    makes BEFORE the gates erased the record that a gate had ever gone red --
    unconditionally, even when the retry then crashed.

    PRECONDITION HAND-STATED BY CALL SEQUENCE, not by asking production for a
    collision it may one day stop producing: the three calls below are exactly
    what a red-gate run followed by an operator re-run performs, in order.
    """
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=True,
                            reason="post-merge gates went red")
    incidents.record_outcome(ctx, SHA_A, reason="push-rejected", revert_landed=False,
                             labels=["agent-regressed", "agent-revert-blocked"])
    # Byte for byte what cli.py's pre-gate call passes on the re-run.
    rec = incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                                  reason="post-merge verification has not completed",
                                  verifying=True)
    assert rec["gate_red"] is True                  # EVIDENCE only ever moves False -> True
    assert rec["reason"] == "push-rejected"         # a terminal reason is not overwritten
    assert rec["revert_landed"] is False
    assert rec["state"] == "open"                   # `verifying` never demotes a verdict
    assert incidents.get(ctx, SHA_A)["gate_red"] is True     # on disk, not just returned
    assert incidents.unresolved(ctx)[0]["merge_sha"] == SHA_A  # still refuses preflight


def test_mark_verified_refuses_a_record_that_says_a_gate_went_red(ctx):
    """`mark_verified` is the ONE automation close, and it checked only that a
    record existed. A record whose own content says "a gate went red" or "the
    revert could not be pushed" (`agent-revert-blocked` -- MASTER MAY STILL BE
    RED) was auto-closed by any later green pass, and `preflight` stopped
    refusing. That is automation deciding what only an operator's `resolve` is
    allowed to decide.

    PRECONDITIONS STATED BY ASSIGNMENT: both records are written literally.
    Nothing asks `post_merge_failure` to produce one.
    """
    red = {"merge_sha": SHA_A, "issue": 8, "state": "open", "gate_red": True, "opened_at": 1.0,
           "reason": "push-rejected", "revert_landed": False, "terminal": True,
           "labels": ["agent-regressed", "agent-revert-blocked"], "run_id": "run-old",
           "resolved_at": None, "resolved_by": None, "note": ""}
    # The OTHER axis on its own: no gate went red, but the revert conflicted,
    # so master is still carrying the commit.
    blocked = dict(red, merge_sha=SHA_B, gate_red=False, reason="revert-conflict")
    ctx.paths.incidents.write_text(json.dumps({SHA_A: red, SHA_B: blocked}, indent=2, sort_keys=True),
                                   encoding="utf-8")
    with pytest.raises(incidents.IncidentRefused) as e:
        incidents.mark_verified(ctx, SHA_A, "run-new")
    assert e.value.reason == "evidence-against-close"
    # OUTSIDE the `with`: `pytest.raises` aborts its body at the raise, so a
    # second assertion inside it never runs and silently proves nothing.
    assert incidents.get(ctx, SHA_A) == red         # not one field moved
    with pytest.raises(incidents.IncidentRefused) as e2:
        incidents.mark_verified(ctx, SHA_B, "run-new")
    assert e2.value.reason == "evidence-against-close"
    assert incidents.get(ctx, SHA_B) == blocked
    # ...and the refusal is not vacuous: a record carrying neither mark still
    # closes, which is the whole point of having an automation path at all.
    incidents.open_incident(ctx, merge_sha="d" * 40, issue=8, gate_red=False)
    assert incidents.mark_verified(ctx, "d" * 40, "run-new")["state"] == "resolved"


def test_reopening_a_resolved_incident_clears_the_operators_close_and_audits_it(ctx):
    """A record that says `state: "open"` and `resolved_by: "torben"` at once
    describes two different verifications as if they were one. If the second
    run also crashes, that contradiction is the entire forensic trail.

    PRECONDITION HAND-STATED BY CALL SEQUENCE: open, an operator's close, then
    the re-open a second `post-merge` on the same SHA performs.
    """
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                            reason="tree-dirty-after-gates")
    incidents.resolve(operator(ctx), SHA_A, by="torben", note="CRLF only, checked by hand")
    rec = incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                                  reason="post-merge verification has not completed")
    assert rec["state"] == "open"
    assert rec["resolved_by"] is None and rec["resolved_at"] is None and rec["note"] == ""
    assert rec["opened_at"] == 1_000_000.0          # still ONE incident, not a new one
    # The human's close is SUPERSEDED, not erased -- recoverable from the
    # audit trail, which is what makes clearing it honest rather than silent.
    audit = [json.loads(l) for l in ctx.paths.audit.read_text(encoding="utf-8").splitlines()]
    reopen = [a for a in audit if a["action"] == "incident-reopen"]
    assert len(reopen) == 1, audit
    assert reopen[0]["merge_sha"] == SHA_A and reopen[0]["superseded"] == "torben"


def test_a_verifying_record_is_visible_but_does_not_block(ctx):
    """The three states, at the module's own boundary. `unresolved` is what
    `preflight` REFUSES on; `visible` is what it and `status` SHOW. An
    in-flight record must appear in the second and not the first -- invisible
    is how an obligation gets forgotten, and fatal is how a killed post-merge
    deadlocks the loop it was meant to protect."""
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                            reason="post-merge verification has not completed", verifying=True)
    assert incidents.get(ctx, SHA_A)["state"] == "verifying"
    # `.agent/audit.jsonl` is the trail that survives a wiped incidents.json,
    # so it has to say WHICH kind of open this was.
    audit = [json.loads(l) for l in ctx.paths.audit.read_text(encoding="utf-8").splitlines()]
    assert audit[-1]["action"] == "incident-open" and audit[-1]["state"] == "verifying"
    assert incidents.unresolved(ctx) == []
    assert [r["merge_sha"] for r in incidents.verifying(ctx)] == [SHA_A]
    assert [r["merge_sha"] for r in incidents.visible(ctx)] == [SHA_A]
    # A verdict PROMOTES it: `record_outcome` is the transition, so the same
    # record now blocks.
    incidents.record_outcome(ctx, SHA_A, reason="tree-dirty-after-gates", revert_landed=None,
                             labels=["agent-gates-green-unverified"])
    assert [r["merge_sha"] for r in incidents.unresolved(ctx)] == [SHA_A]
    assert incidents.verifying(ctx) == []
    assert [r["merge_sha"] for r in incidents.visible(ctx)] == [SHA_A]


def test_record_outcome_on_an_unknown_merge_refuses_rather_than_inventing_one(ctx):
    """A caller that reaches `record_outcome` without having opened a record
    has a bug. A silently minted row would hide it behind a plausible-looking
    incident that no one ever decided to open."""
    with pytest.raises(incidents.IncidentRefused) as e:
        incidents.record_outcome(ctx, SHA_A, reason="reverted", revert_landed=True, labels=["x"])
    assert e.value.reason == "unknown-merge-sha"
    assert incidents.all_records(ctx) == {}


def test_mark_verified_names_the_run_that_verified_it(ctx):
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False)
    rec = incidents.mark_verified(ctx, SHA_A, "run-test")
    assert rec["state"] == "resolved"
    # The `post-merge:` prefix is the point: an automated close must never be
    # indistinguishable from an operator typing their own initials into `--by`.
    assert rec["resolved_by"] == "post-merge:run-test"
    assert rec["resolved_at"] == 1_000_000.0
    assert incidents.unresolved(ctx) == []


def test_resolve_and_mark_verified_are_two_functions_with_no_shared_bypass(ctx):
    """The operator gate must not be reachable as an argument. If these ever
    merge into one function with an `automated=True` flag, the day someone
    needs it "just this once" the gate becomes a keyword any caller can pass."""
    import inspect
    assert incidents.resolve is not incidents.mark_verified
    resolve_params = set(inspect.signature(incidents.resolve).parameters)
    verified_params = set(inspect.signature(incidents.mark_verified).parameters)
    assert not (resolve_params & {"automated", "bypass", "force", "by_automation"})
    assert not (verified_params & {"automated", "bypass", "force", "by_automation"})
    # ...and only one of them consults the operator gate.
    assert "require_operator" in inspect.getsource(incidents.resolve)
    assert "require_operator" not in inspect.getsource(incidents.mark_verified)


def test_resolve_is_operator_only_and_refuses_a_second_close(ctx):
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=True)
    # (a) automation asking: `ctx` carries run_id="run-test".
    with pytest.raises(lock.OperatorOnly):
        incidents.resolve(ctx, SHA_A, by="the loop", note="")
    assert incidents.get(ctx, SHA_A)["state"] == "open"
    # (b) an operator asking.
    op = operator(ctx)
    rec = incidents.resolve(op, SHA_A, by="operator", note="CRLF, not a regression")
    assert rec["state"] == "resolved" and rec["resolved_by"] == "operator"
    assert rec["note"] == "CRLF, not a regression"
    audit = [json.loads(l) for l in ctx.paths.audit.read_text(encoding="utf-8").splitlines()]
    assert audit[-1]["action"] == "incident-resolve" and audit[-1]["merge_sha"] == SHA_A
    # (c) again: a no-op must never read as success.
    with pytest.raises(incidents.IncidentRefused) as e:
        incidents.resolve(op, SHA_A, by="operator", note="again")
    assert e.value.reason == "already-resolved"
    # (d) a SHA nobody ever opened.
    with pytest.raises(incidents.IncidentRefused) as e:
        incidents.resolve(op, SHA_B, by="operator", note="")
    assert e.value.reason == "unknown-merge-sha"


def test_resolve_closes_a_verifying_record_and_names_the_operator(ctx):
    """THE EXIT `verifying` DID NOT HAVE. A post-merge killed mid-gates leaves
    this row: visible in every `preflight`/`status`, failing nothing, and
    closable by nothing -- `mark_verified` needs a post-merge run, and
    post-merge needs a merge record plus a fence no operator invocation can
    satisfy once the lock is gone. `resolve` refused it as `already-resolved`
    and told the human "<sha12> was closed by None": a merge nobody verified,
    reported as a state somebody established.

    PRECONDITION HAND-STATED: the record is opened `verifying=True` and the
    state is asserted BEFORE the close, so this test says what it is about
    rather than depending on which state `open_incident` happens to default
    to."""
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=False,
                            reason="post-merge verification has not completed", verifying=True)
    opened = incidents.get(ctx, SHA_A)
    assert opened is not None and opened["state"] == "verifying"
    assert incidents.unresolved(ctx) == []                    # it never failed preflight
    assert [r["merge_sha"] for r in incidents.visible(ctx)] == [SHA_A]   # ...but it was on screen
    op = operator(ctx)
    rec = incidents.resolve(op, SHA_A, by="operator", note="looked; CRLF, not a regression")
    assert rec["state"] == "resolved"
    # WHO closed it, kept distinguishable from `mark_verified`'s
    # `post-merge:<run>` form -- allowing the close must not cost the record.
    assert rec["resolved_by"] == "operator"
    assert rec["note"] == "looked; CRLF, not a regression"
    assert rec["resolved_at"] == 1_000_000.0
    # THE CONSEQUENCE: it is off the dashboard, and the audit trail says who.
    assert incidents.visible(ctx) == [] and incidents.verifying(ctx) == []
    audit = [json.loads(l) for l in ctx.paths.audit.read_text(encoding="utf-8").splitlines()]
    assert audit[-1]["action"] == "incident-resolve" and audit[-1]["merge_sha"] == SHA_A
    assert audit[-1]["by"] == "operator"
    # ...and it is still a one-shot: the second close is refused, and NOW the
    # message can name a real closer.
    with pytest.raises(incidents.IncidentRefused) as e:
        incidents.resolve(op, SHA_A, by="operator", note="again")
    assert e.value.reason == "already-resolved"
    assert "'operator'" in str(e.value), str(e.value)


def test_resolve_never_reports_a_closer_that_does_not_exist(ctx):
    """The refusal message is the thing a human reads, so it must not invent
    one. Two rows, two different reason codes, and the text of each is
    asserted -- a single code for both states is how "`<sha12>` was closed by
    None" got printed about a merge nobody had closed.

    PRECONDITIONS HAND-STATED, including a state no production path can
    produce: `incidents.json` is a local, gitignored file an operator can
    hand-edit, and the refusal for a state this command does not understand
    must still name what it FOUND rather than describe it as a close."""
    incidents.open_incident(ctx, merge_sha=SHA_A, issue=8, gate_red=True)
    op = operator(ctx)
    incidents.resolve(op, SHA_A, by="torben", note="reverted by hand")
    with pytest.raises(incidents.IncidentRefused) as resolved_err:
        incidents.resolve(op, SHA_A, by="torben", note="")
    assert resolved_err.value.reason == "already-resolved"
    assert "'torben'" in str(resolved_err.value)
    # A state written by hand, stated literally.
    records = json.loads(ctx.paths.incidents.read_text(encoding="utf-8"))
    records[SHA_B] = {"merge_sha": SHA_B, "issue": 9, "state": "wedged", "resolved_by": None}
    ctx.paths.incidents.write_text(json.dumps(records), encoding="utf-8")
    with pytest.raises(incidents.IncidentRefused) as odd_err:
        incidents.resolve(op, SHA_B, by="torben", note="")
    assert odd_err.value.reason == "unknown-state"
    assert "'wedged'" in str(odd_err.value)
    # THE POINT: neither refusal claims a closer that does not exist.
    assert "closed by None" not in str(odd_err.value)
    assert odd_err.value.reason != resolved_err.value.reason


def test_resolve_refuses_under_dry_run_before_it_reads_anything(dry_ctx):
    """`write_guard` is the FIRST statement, so the refusal a `--dry-run`
    caller gets is DryRunViolation rather than OperatorOnly -- the write-
    freedom guarantee is not allowed to depend on which other check happened
    to fire first."""
    with pytest.raises(DryRunViolation):
        incidents.resolve(dry_ctx, SHA_A, by="operator", note="")
    assert not dry_ctx.paths.incidents.exists() and not dry_ctx.paths.audit.exists()
