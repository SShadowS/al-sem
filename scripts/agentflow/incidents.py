"""The durable, per-merge obligation that `preflight` refuses on.

HALT is ONE global file. Clearing it erases the obligation it stood for, and
nothing then remembers that a particular merge was never verified. So the
record kept here is per MERGE SHA, lives in `.agent/incidents.json`, and is
what `cmd_preflight` refuses on -- computed INDEPENDENTLY of `lock.halted`.
A careless `clear-halt` therefore cannot silently resume the loop over a
commit nothing ever verified: the incident row survives the clear and keeps
failing preflight until someone says, on the record, that it is closed.

Two ways to close one, deliberately kept as two separate functions with no
shared bypass flag:

  `mark_verified`  the ONLY automation path -- a post-merge run where every
                   gate returned 0 and the tree came back clean. Stamps
                   `resolved_by="post-merge:<run_id>"`.
  `resolve`        an operator asserting it by hand. Goes through
                   `lock.require_operator` first.

An `automated=True` argument on one function would have been shorter and is
exactly the shape that rots: the day someone needs "just this once" the
operator gate becomes a keyword argument any caller can pass.

THREE states, not two, because "nothing recorded" and "terminal obligation"
are not the only things that can be true:

  `verifying`  a post-merge that has opened its obligation and not yet
               reached a verdict. VISIBLE in `preflight`/`status`, but not a
               failure row -- a run killed mid-gates must still be
               recoverable by `recover` with no operator action.
  `open`       a verdict was reached and it stops the loop. `preflight`
               refuses on these and only these.
  `resolved`   closed, by `mark_verified` or by an operator's `resolve`.

COULD NOT VERIFY is not PROVEN BAD, and neither is PROVEN GOOD. Every state
transition here keeps those three apart.

Storage is a plain dict keyed by the FULL merge SHA (never a prefix -- the
12-char form in preflight's failure rows is for human eyes), written through
`state.write_json`, so it is atomic and key-sorted like every other record.
"""
from __future__ import annotations

from . import lock
from .state import Ctx, append_audit, read_json, write_json


class IncidentRefused(RuntimeError):
    """A refusal from this module. `.reason` is the machine-readable code."""

    def __init__(self, reason: str, detail: str = ""):
        super().__init__(f"{reason}: {detail}" if detail else reason)
        self.reason, self.detail = reason, detail


def all_records(ctx: Ctx) -> dict:
    return read_json(ctx.paths.incidents, default={}) or {}


def get(ctx: Ctx, merge_sha: str) -> dict | None:
    return all_records(ctx).get(merge_sha)


def _ordered(rows: list[dict]) -> list[dict]:
    """Oldest first, ties broken by SHA, so a caller that renders them
    (preflight's failure rows, `status`) is deterministic run to run."""
    return sorted(rows, key=lambda r: (r.get("opened_at") or 0.0, r.get("merge_sha") or ""))


def unresolved(ctx: Ctx) -> list[dict]:
    """Every TERMINAL open record -- the obligations `preflight` REFUSES on.

    A record still in `verifying` state is deliberately not here. See
    `open_incident` for why a verification in flight is not yet an obligation,
    and `visible` for how it stays on screen anyway.
    """
    return _ordered([r for r in all_records(ctx).values() if r.get("state") == "open"])


def verifying(ctx: Ctx) -> list[dict]:
    """Every record whose verification is still in flight: a `post-merge` that
    opened its obligation and has not yet reached a verdict, including one
    whose process was killed mid-gates.

    Visible, never fatal. A killed post-merge must remain recoverable by
    `recover` with NO operator action -- an in-flight record that failed
    preflight would gate the very recovery that discharges it, and the
    operator's only exit would be to record on the durable audit trail that a
    merge nothing verified is closed, purely to be allowed to go verify it.
    """
    return _ordered([r for r in all_records(ctx).values() if r.get("state") == "verifying"])


def visible(ctx: Ctx) -> list[dict]:
    """Terminal AND in-flight records, for the operator-facing `incidents`
    field of `preflight` and `status`. An in-flight row must be SHOWN --
    invisible is how an obligation gets forgotten -- it just must not fail the
    tick by itself."""
    return _ordered([r for r in all_records(ctx).values()
                     if r.get("state") in ("open", "verifying")])


def _save(ctx: Ctx, records: dict) -> None:
    write_json(ctx, ctx.paths.incidents, records)


def open_incident(ctx: Ctx, *, merge_sha: str, issue: int, gate_red: bool,
                  reason: str = "in-progress", verifying: bool = False) -> dict:
    """Create or re-open the record for `merge_sha`. IDEMPOTENT, and MONOTONIC
    on the evidence axis.

    A second call for the same SHA is the SAME incident, not a new one, so
    `opened_at` is preserved. That is what lets `cmd_post_merge` open the
    record before the gates run (`verifying=True`, "not verified yet") and
    `post_merge_failure` open it again with the real verdict, without the
    second call inventing a duplicate or resetting the clock.

    WHICH FIELDS MOVE, and in which direction -- "idempotent" alone was not a
    true description of this function and hid three defects:

      `gate_red`  MONOTONIC False -> True, never back. Nothing in
                  `cmd_post_merge` refuses a second invocation under the same
                  lock, and its pre-gate call passes `gate_red=False`. A
                  re-run therefore used to erase the record that a gate had
                  ever gone red -- unconditionally, even when the retry then
                  crashed -- and `mark_verified` would then close it.
      `reason`    a TERMINAL reason ("push-rejected") is kept. Only
                  `record_outcome` writes one, and it marks the record
                  `terminal`; an in-progress reason must never overwrite it.
      `state`     `verifying` is for a verification in flight. It NEVER
                  demotes a record that is already terminally `open` -- a
                  re-run must not be able to hide an obligation from
                  `preflight` -- so the demotion is refused rather than
                  written.
      the close   re-opening a RESOLVED record clears `resolved_at` /
                  `resolved_by` / `note` and audits the reopen. A record that
                  says `state: open` and `resolved_by: <a human>` at once
                  describes two different verifications as if they were one,
                  and if the second run also crashes that contradiction is the
                  entire forensic trail.
    """
    records = all_records(ctx)
    prev = records.get(merge_sha)
    rec = dict(prev or {})
    prev_state = rec.get("state")
    state = "verifying" if verifying and prev_state != "open" else "open"
    rec.update(merge_sha=merge_sha, issue=issue, state=state, run_id=ctx.run_id,
               gate_red=bool(rec.get("gate_red")) or gate_red,
               reason=rec.get("reason", reason) if rec.get("terminal") else reason)
    rec.setdefault("revert_landed", None)
    rec.setdefault("labels", [])
    rec.setdefault("resolved_at", None)
    rec.setdefault("resolved_by", None)
    rec.setdefault("note", "")
    if prev_state == "resolved":
        rec.update(resolved_at=None, resolved_by=None, note="")
    if prev is None:
        rec["opened_at"] = ctx.now()
    records[merge_sha] = rec
    _save(ctx, records)
    if prev is None:
        # `state` rides along because `.agent/audit.jsonl` is the trail that
        # survives a wiped `.agent/incidents.json`, and "an incident was
        # opened" without "and nothing had been decided yet" is half a record.
        append_audit(ctx, {"action": "incident-open", "merge_sha": merge_sha,
                           "issue": issue, "gate_red": gate_red, "state": state})
    elif prev_state == "resolved":
        # The human's close is SUPERSEDED, not erased: `.agent/audit.jsonl`
        # still names who closed it and what reopened it.
        append_audit(ctx, {"action": "incident-reopen", "merge_sha": merge_sha,
                           "issue": issue, "state": state,
                           "superseded": prev.get("resolved_by")})
    return rec


def record_outcome(ctx: Ctx, merge_sha: str, *, reason: str,
                   revert_landed: bool | None, labels: list[str]) -> dict:
    """Write the terminal fields onto an already-open record.

    Refuses an unknown SHA rather than inventing a record: a caller that
    reaches here without having opened one has a bug, and a silently minted
    row would hide it behind a plausible-looking incident.

    PROMOTES the record out of `verifying`: a verdict has been reached, so
    this is now a terminal obligation `preflight` refuses on. `terminal=True`
    is what stops a later in-progress `open_incident` overwriting the reason
    recorded here.
    """
    records = all_records(ctx)
    rec = records.get(merge_sha)
    if rec is None:
        raise IncidentRefused("unknown-merge-sha", f"no incident is open for {merge_sha}")
    rec.update(reason=reason, revert_landed=revert_landed, labels=list(labels),
               state="open", terminal=True)
    records[merge_sha] = rec
    _save(ctx, records)
    return rec


def mark_verified(ctx: Ctx, merge_sha: str, run_id: str | None) -> dict:
    """The one automation close: a post-merge pass where every gate returned 0
    and the tree came back clean.

    `resolved_by` carries the `post-merge:<run_id>` form so an automated close
    is never indistinguishable from an operator's initials in the record.

    REFUSES a record whose own content argues against closing it. `gate_red:
    True` means a gate actually went red on this merge; `revert_landed: False`
    means the revert could NOT be pushed, i.e. `agent-revert-blocked`, which
    `orchestrate.md` defines as MASTER MAY STILL BE RED. A later green pass
    proves something about a later run, not about either of those, so closing
    on it is the automation deciding what only an operator's `resolve` is
    allowed to decide -- the exact distinction this module exists to keep.
    """
    records = all_records(ctx)
    rec = records.get(merge_sha)
    if rec is None:
        raise IncidentRefused("unknown-merge-sha", f"no incident is open for {merge_sha}")
    if rec.get("gate_red") or rec.get("revert_landed") is False:
        raise IncidentRefused(
            "evidence-against-close",
            f"{merge_sha[:12]} records gate_red={rec.get('gate_red')!r}, "
            f"revert_landed={rec.get('revert_landed')!r} -- only an operator's "
            f"`resolve-incident` may close that")
    rec.update(state="resolved", resolved_at=ctx.now(),
               resolved_by=f"post-merge:{run_id}",
               note="every gate returned 0 and the tree was clean")
    records[merge_sha] = rec
    _save(ctx, records)
    return rec


def resolve(ctx: Ctx, merge_sha: str, *, by: str, note: str) -> dict:
    """An operator closing an incident by hand.

    `write_guard` first (so `--dry-run` refuses before anything is read), then
    the operator gate, then the refusals, then the audit line BEFORE the
    write -- the same order, and for the same reasons, as `lock.clear_halt`.

    CLOSES A `verifying` ROW TOO, and that is not a loosening -- it is the
    only exit that state has. A `verifying` record is what a post-merge killed
    mid-gates leaves behind: shown by `preflight`/`status`, failing nothing,
    and closable by nothing. `mark_verified` needs a post-merge run, and
    post-merge needs both a merge record and a fence that no operator
    invocation can satisfy once the lock is gone. So the row sat on the
    dashboard forever while `resolve` told the operator `"<sha12> was closed
    by None"` -- a merge nobody verified, reported to a human as a state
    somebody established. That sentence is this arc's own error (COULD NOT
    VERIFY rendered as ESTABLISHED) in the operator-facing surface.

    An unverified merge IS an obligation, and discharging an obligation is
    exactly what an operator is for. The record keeps `resolved_by` and the
    audit line, so an operator's close stays distinguishable from
    `mark_verified`'s `post-merge:<run>` form -- nothing about who closed what
    is lost by allowing it.

    REFUSALS, each naming only what it can vouch for: `already-resolved`
    carries the real closer, and any other state is reported as the state it
    actually found rather than being described as a close.
    """
    ctx.write_guard("resolve incident")
    lock.require_operator(ctx, "resolve incident")
    records = all_records(ctx)
    rec = records.get(merge_sha)
    if rec is None:
        raise IncidentRefused("unknown-merge-sha", f"no incident recorded for {merge_sha}")
    state = rec.get("state")
    if state == "resolved":
        raise IncidentRefused("already-resolved",
                              f"{merge_sha[:12]} was closed by {rec.get('resolved_by')!r}")
    if state not in ("open", "verifying"):
        # Only reachable through a hand-edited `incidents.json`. Say what was
        # found; do not invent a closer for it.
        raise IncidentRefused("unknown-state",
                              f"{merge_sha[:12]} is in state {state!r}, which this command "
                              f"does not know how to close")
    append_audit(ctx, {"action": "incident-resolve", "merge_sha": merge_sha,
                       "by": by, "note": note})
    rec.update(state="resolved", resolved_at=ctx.now(), resolved_by=by, note=note)
    records[merge_sha] = rec
    _save(ctx, records)
    return rec
