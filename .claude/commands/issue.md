---
description: The per-issue pipeline — worktree, classify, probes, spec, two-model spec panel, acceptance tests, plan, TDD implementation with discrimination proofs, repo gates, two-model final panel, PR, gated squash-merge. Called by /orchestrate; standalone use requires a claim first.
---

Take GitHub issue `$ARGUMENTS` (a number) from claimed to merged, per
`docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md`. Requires an
existing claim (`python scripts/agentflow status` lists, in `locks`, a lease
for this issue naming your run id, and `AGENTFLOW_RUN_ID` is set). Standalone
use: run `claim` first, exactly as `/orchestrate` step 7 does. Another session
may be running another issue at the same time; never act on its run id or
issue. Never set `AGENTFLOW_NOW` — it exists only to freeze
the executor's clock for tests. An executor call that exits non-zero, or whose
JSON has a TOP-LEVEL `error` key, stops this step: print it and return `blocked
<error-or-reason>`. A per-item `error` field inside a list in an otherwise-ok
reply (e.g. `file-discoveries`'s `filed[]`, where a per-discovery `error` is
`null` on success) is informational, not a stop.

**Invocation.** Every executor call in this file is `python scripts/agentflow
--root <main checkout> --run-id "$AGENTFLOW_RUN_ID" <subcommand …>`, run FROM the
main checkout (`U:\Git\al-call-hierarchy`) — even for work that lives in the
worktree. A gate's own build/test commands run inside the worktree via that same
call's `--cwd <worktree>`; the `agentflow` invocation itself never runs from
inside the worktree. The lock, `HALT`, the budget file, and this run's directory
all live under the MAIN checkout's `.agent/`, never the worktree's.

Ledger: `<worktree>/.agent/issue-N/ledger.md`. Findings register:
`<worktree>/.agent/issue-N/findings.json`. Both are committed; nothing else under
`.agent/` may be. Write the ledger as you go, one timeline line per phase with the
executor's JSON outcome and the cap counters.

Reviewer roster for BOTH panels: `gpt-6-astra` and `gemini-3.8-flash` via
`mcp__pi__pi_ask`, `thinking: high`, `require_evidence` on, `output_file` under
`.agent/runs/$AGENTFLOW_RUN_ID/`. Launch both in one message. Charge `pi_calls`
for each call. While a pi call runs in the background, call
`python scripts/agentflow beat` at least every 10 minutes.

Return value (the last line you print): `merged <merge_sha>`, `blocked <reason>`,
`handoff <full sha>` (step 5 only), or `spike-answered`. Any other cap hit
(`charge` prints `exhausted`) is `blocked <cap-name>`.

## Steps

1. **Worktree.** `python scripts/agentflow make-worktree` (per Invocation;
   never `git worktree add` yourself: the executor holds the shared checkout's
   git mutex, so a sibling session's git writes cannot collide with it). It
   takes the worktree and branch from this run's `claim.json`, fetches, and
   branches from `origin/master`, NOT the main checkout's `master` (which lags
   while another session's merge waits for its post-merge); it replies
   `{worktree, branch, base}`. Record `base` in the ledger. In every command run inside
   the worktree, export `TREE_SITTER_AL_PATH=<main checkout>/tree-sitter-al`. Create
   `.agent/issue-N/` there and start the ledger with the claim JSON and a hash of
   the issue body (`body_hash` from the claim).

   **Pre-ratified design (#45).** If the claim JSON carries `handoff`, an earlier
   attempt converged its design but hit the spec-panel cap, and handed it off.
   Restore the files it lists, in the worktree, from its commit:
   `git restore --source <handoff.sha> -- <every path in handoff.files>`
   (worktree only, nothing staged). Commit every restored path EXCEPT
   `.agent/issue-N/ledger.md` and `.agent/issue-N/findings.json` (they stay
   uncommitted until step 12, as always; a bounded design kept in the ledger
   leaves nothing to commit here) with the message
   `docs: pre-ratified design for #N from attempt <handoff.from_attempt>`.
   Add a ledger timeline line `Pre-ratified from attempt <k> at <sha>`, then
   SKIP steps 2–5 and continue at step 6; the carried spec (or the ledger's
   `## Design`) is the spec, and the carried `findings.json` is the register.
   A claim carrying `handoff_void: issue-edited` means the issue changed since
   the handoff: ignore it and run every step from 2.
2. **Classify** with the brainstorming skill's three-path rule (spike / bounded /
   architectural) and write the classification and reason to the ledger.
   - Spike: run the probe (read-only; a throwaway script under `.agent/runs/` if
     needed). Write the full answer under `## Answer` in the ledger and return
     `spike-answered`. Do not comment on the issue yourself; the orchestrator
     posts the answer and the label through `finish --outcome answered`.
   - Bounded: a short design (a few paragraphs) in the ledger under `## Design`,
     then continue at step 3.
   - Architectural: continue to step 3.
3. **Assumption probes.** List every fact the issue's Acceptance depends on (its
   Dependencies section and any "assumes" in the body). Verify each against real
   data with read-only commands (`aldump`, `alsem`, a fixture, `CDO_WS` if named).
   Record `probe | result | evidence` rows in the ledger. A falsified assumption
   shapes the spec and is a candidate discovery.

   **Probe in the acceptance's own shape.** A probe reproduces the
   acceptance's own invocation: the same script step (`scripts/ci-steps
   <step>`), the same parallelism and profile, pinned to the same core count
   when the target is CI. When the acceptance names the CI runner, this step
   may push ONE throwaway probe commit to a draft PR to read the runner's
   numbers: open the draft here (`push-branch`, then `pr-create --draft` as in
   step 9) instead of in step 9, record the runner's numbers as probe rows,
   then `git revert` the probe commit so it is not in the approved diff. A
   probe charges no `ci_fix`. Then state in the Design which acceptance items
   are feasible on the measured hardware and which are not, BEFORE the spec
   panel sees it. On #190 a serial probe of a test CI runs in parallel, and
   runner variance found only after the design, cost one spec round, two
   amendments and ~1 h.
4. **Spec.** Write `docs/superpowers/specs/<today>-issue-N-<slug>-design.md` with
   sections: Goal, Non-goals, Acceptance matrix (each Acceptance item →
   the test or measurement that proves it, or "not deliverable, because"),
   Design, False-positive and failure shapes, Measurement plan. For a bounded
   issue the ledger's `## Design` is the spec.

   **Pre-state every fallback.** Every rule of the form "if the measurement
   shows X, go back to design" (in the spec or the bounded `## Design`) states
   the alternative it falls back to (e.g. "keep the bound, state the
   acceptance as partly delivered, file the ratio-gate follow-up"). The spec
   panel reviews the fallback with the rule, so firing it needs no new round
   and no operator decision: apply it and record that it fired in the ledger.
   Ask the operator only when NO pre-stated fallback fits, and say in the same
   message what you will do if there is no answer within 15 minutes. On #190
   a bare "back to design" fired twice and cost ~30 min of operator wait and
   one spec round.
5. **Spec panel.** `charge spec_rounds` per round (cap 5). A round past a
   cap exists only after the operator runs `python scripts/agentflow grant
   --issue N --key spec_rounds --n 1 --by <operator>` (`final_rounds` for the
   final panel; no other cap can be granted). Never call `grant` yourself: it
   refuses a call carrying your run id, but this sentence, not the code, is
   what stops you dropping it. Then charge the round as usual. Never
   run a round the budget does not cover, and never record one "outside the
   budget" in the ledger instead (#190 did, and `status` under-counted it). Follow the
   `panel-review` skill: a briefing file with absolute paths and a
   confirm/reject checklist; both reviewers in parallel. Maintain
   `findings.json` as a list of `{id, source, round, file, line,
   severity: critical|important|minor, text,
   disposition: open|fixed|refuted|deferred, evidence, anchor, anchor_hash,
   reviews: {astra: accepted|re-raised|unreviewed, flash: …}}`. Both
   vocabularies are closed and the executor refuses an entry whose `severity`
   or `disposition` is missing or outside them — an unrecognised word is a
   malformed entry, never a silently non-blocking one. It treats `critical`
   and `important` — or an explicit `blocking: true` — as BLOCKING, which may
   not be merely `deferred`. Normalise a reviewer's own wording (a panel that
   says "High" or "Blocker") into the three words before writing the register.
   **Evidence that only exists after the panel.** An entry whose only
   remaining work is a record the flow produces AFTER the panel (a gate exit
   code, `B`/`H`/`final_head`, a `ci-wait` record) takes `disposition: fixed`
   with `closes_by: {"kind": "gate"|"attest"|"ci-wait", "key": "<gate name |
   B | H | final_head | ci-wait>"}` and needs no further mark: the reviewers
   mark the RULE (that the named record closes it) `accepted`, and the
   executor checks the record. `attest` refuses (`register-not-converged`) an
   unknown kind or key, an entry that is not `fixed`, a `gate` key that is
   not one of the gates `attest` requires (`ci-steps-all`,
   `check-goldens-coverage`, `check-goldens`, `cdo-gate`) or whose attested
   exit code is not 0, or an `attest` key that is not one of the
   three SHA fields; `merge-gate` refuses (`closes-by-unmet:<ids>`) a
   `ci-wait` entry until this run's latest `ci-wait` record is on the final
   head with exit code 0, not timed out, supervised, and of the step 13
   command (`gh pr checks <pr> --watch`). The attestation lists each one
   under `closed_by`.
   A repeat-run tuple an acceptance asks for is not a `closes_by` kind: record
   it in the ledger before the panel's last round.
   Round 1's briefing asks for EVERY finding at once, wording and stale
   references included, not only blocking ones; later rounds verify fixes and
   raise a new finding only for a real defect a fix introduced (the final
   panel's rule in step 10; on #160 step 2 the spec panel used all 5 rounds).
   **A later round's briefing carries the diff and the ids, nothing else:**
   the fix diff (a diff of the spec, or of the ledger's `## Design` for a
   bounded issue; `git diff <last-marked-commit>..HEAD` for code, or, when a
   rebase came in between, `git range-diff <old base>..<last-marked-commit>
   origin/master..HEAD`, since a plain diff across a rebase also shows
   everything `master` brought in) and the ids
   to mark (the entries whose marks were reset, plus new entries). Each
   reviewer marks those `accepted` or `re-raised`. Reviewers continue their
   round-1 thread (`continuation_id` for pi; the same Opus agent via
   `SendMessage` for an Opus stand-in). Do not re-link the ledger, the full
   register or the previous reviews. If a reviewer's thread is gone (pi
   threads do not survive a session), send that reviewer a full briefing
   instead. Diff-only rounds came back in 1-2 min (#190 r4, r6b; #111 r3),
   full-context rounds in 4-13 min (#160 final r1: 13 min).

   **Marks are scoped to what changed (per-entry anchors).** Each entry
   carries `anchor`: the file and line range (or the entry's own fix commit)
   its fix lives in, and `anchor_hash`: sha256 of those lines at the time of
   the last mark. A later Design or code edit resets the marks ONLY of
   entries whose anchor lines changed (`git diff` over the anchor range),
   plus any entry the edit's commit message names; a fix commit names the
   entry ids it addresses, and those are always reset. An entry whose anchor
   is unchanged keeps both marks. The reset entries plus the new ones are
   "the ids to mark" in a later round's briefing. (One hash over the whole
   Design reset every mark on every edit: 7 resets of up to 31 entries on
   #190.) Converged when: no `open`; every entry `accepted` by both against
   its own `anchor_hash`; every blocking entry is `fixed` or `refuted` with
   evidence. A `re-raised` stays unresolved even without new evidence. Not
   converged after 5 rounds: check the handoff rule below, else return
   `blocked spec-panel-cap`.

   **Raiser-only acceptance for a minor prose fix.** A `minor` entry whose fix
   touches only gate-inert files (`CHANGELOG.md`, `docs/**/*.md`) and the two
   evidence files is closed when the reviewer who raised it marks it
   `accepted`; write the other reviewer's mark as `accepted-by-rule` and set
   the entry's `fix_commit` to the full SHA of that fix. The briefing still
   lists it so the other reviewer may object, and an objection is a
   `re-raised` that reopens it. Code, tests, fixtures and `tests/**/*.md`
   never qualify. The entry's `source` names the raiser exactly as its marks
   are stored (a stand-in's own name under `--substitute`); an entry raised by
   anyone outside the two review slots (the code-review skill, a task
   reviewer, a proof) has no raiser here, and both reviewers accept it as
   before. The executor counts `accepted-by-rule` as `accepted` only when the
   entry is `minor` and not `blocking`, its `source` fills exactly one slot,
   that slot's mark is `accepted`, the rule mark is in the OTHER slot, and
   `fix_commit` is a commit on the branch (after `B`) whose
   own diff touches only those files; anywhere else it is an unknown mark and
   the entry fails. In 3 of 3 runs a final round existed only to confirm a
   wording fix.

   **Handoff at the cap (#45).** The design may pass to the next attempt as
   PRE-RATIFIED when the register meets the handoff rule: every `severity` and
   `disposition` from the closed vocabularies, no `open` entry, no blocking
   entry `deferred`, every reviewer mark either `accepted` or `re-raised` (none
   missing, none `unreviewed`), and every entry `accepted` by AT LEAST ONE
   reviewer — an entry both reviewers re-raised is a live dispute and blocks
   the handoff. Never handoff from an attempt whose claim carried `handoff`
   (the executor refuses `handoff-already-used`). To hand off: run `sanitize`
   on the ledger and register (as in step 12), commit the spec file, the
   ledger and `findings.json` on the issue branch — name each path; a spec
   left untracked is not carried — run `check-diff --base master --head HEAD
   --issue N --cwd <worktree>`, write the commit's FULL SHA into the ledger,
   and return `handoff <full sha>`. If the operator named stand-ins and the
   marks are stored under their names (e.g. `reviews: {sol: …, opus: …}`),
   also tell the orchestrator which `--substitute` flags the marks were made
   under (e.g. `--substitute astra=sol --substitute flash=opus`); `finish`
   judges the handoff rule against that roster (#141). Not handoff-ready:
   `blocked spec-panel-cap`, as before.
6. **Acceptance tests.** Call `python scripts/agentflow beat` immediately before
   AND immediately after dispatching this step's subagent — the same
   30-minute-staleness reason as step 8's dispatches. Dispatch an `opus`
   subagent with the acceptance matrix, the fixture conventions from CLAUDE.md
   "Adding New AL Constructs" and "Testing Philosophy & Goldens", and the rule
   that a new golden family needs a seed file. Tests must compile and fail for
   the stated reason; capture the failing output in the ledger. Do NOT commit
   them alone — the pre-commit hook
   runs `check-goldens` and rejects a red golden family, so a failing test
   committed by itself (before its implementation lands) would block every
   subsequent commit on this branch until they are committed together.
7. **Plan.** `superpowers:writing-plans` to
   `docs/superpowers/plans/<today>-issue-N-<slug>.md`. Charge `plan_tasks` once
   per task in the plan (`python scripts/agentflow charge plan_tasks`) so the cap
   is enforced by the executor, not by the conductor counting; the first
   `exhausted` result is `blocked plan-too-large` — comment "split this issue" in
   the ledger first.
8. **Implement.** `superpowers:subagent-driven-development`. `halt-check` first;
   if halted, write the ledger and return `blocked halted`. Per task: implementer
   `opus`, then an independent reviewer `sonnet`; the review's findings go BACK
   to the same implementer (`SendMessage` to its agent id, context intact)
   instead of a third, cold review-fix agent, which re-read the repo from
   scratch on every task. Two consecutive tasks the plan marks small may share
   one reviewer dispatch over both commits. **A bounded issue whose plan has
   at most two tasks dispatches no per-task reviewer:** the implementer's own
   TDD and discrimination proofs plus the final panel (the code-review skill
   at high and both reviewers) are its review. Architectural issues, and
   plans with three or more tasks, keep the per-task reviewer. (On #111 and
   #190 it found only minors the final panel would also read; on #160 it
   found an important gap at T1 that was cheaper to fix then.) Call `python scripts/agentflow
   beat` immediately before AND immediately after each dispatch and each
   `SendMessage` (not only around the pi calls above; an `opus` implementer
   dispatch routinely runs longer than the 30-minute staleness threshold).
   The implementer's per-task gate is `scripts/ci-steps task-affected` (fmt, dev
   clippy, gen-syntax, and the tests the changed paths can reach, from the map in
   `scripts/ci-steps`); the WHOLE suite runs in step 9 and step 11, as before, so
   every test still runs before the panel and before the merge. Every task prompt
   includes: TDD red then green; `discrimination-proof` for every new or changed
   test (record test, mutation patch, fail output, pass output, commit in the
   ledger's proof table); `rustfmt <file>` only; SOLID and DRY as review
   criteria; the protected-path list (`.github/ scripts/ .claude/ CLAUDE.md
   .gitignore Cargo.toml version tree-sitter-al .agent/`). `charge subagents`
   per dispatch; `charge task_attempts --sub <task-id>` per red-to-green attempt.
   Before each task's commit: `python scripts/agentflow check-diff --base master
   --head HEAD --issue N --cwd <worktree>`; any reason is `blocked <reason>`.
9. **Repo gates.** `halt-check` first; if halted, write the ledger and return
   `blocked halted`.

   **The full set below runs ONCE, in step 11, on the rebased head you will
   attest.** Here, before the final panel, and again after every panel fix, run
   only the task gate:
   `python scripts/agentflow run --name ci-steps-task --timeout 45 --cwd <worktree> -- bash scripts/ci-steps task`
   (fmt, dev clippy, gen-syntax, the whole suite including every golden target).
   Exit 0 and a clean tree (as below) let the branch go to the panel. Do the
   golden triage, `/triage-wave` and the CHANGELOG entry from this step now, not
   in step 11. Running the full set before the panel only verifies a head that
   is never attested: on #107 it ran 7 times for one issue.

   **Open a DRAFT PR now, so GitHub CI runs during the final panel** (unless
   step 3's runner probe already opened it; then just `push-branch`). After the
   first green task gate here: `push-branch` (step 13's command), then
   `python scripts/agentflow pr-create --draft --title "<issue title> (#N)" --body-file <a short "final panel in progress" body> --head <branch>`.
   CI (Linux, CI's exact bar) then runs while the panel reviews, and again on
   each later push; a red CI is fixed like a panel finding. This replaces any ad
   hoc Linux check (a WSL clone): CI IS the Linux check. Step 13 puts the final
   body in place and marks it ready (`pr-ready --body-file`); `merge-gate`
   refuses a draft (`pr-draft`).

   **Measurements (a CDO A/B bench) run ONCE per issue,** after the final panel
   has converged and before step 11's gates, through the supervisor so nothing
   else builds meanwhile:
   `python scripts/agentflow run --name bench --timeout 45 --cwd <worktree> -- python scripts/bench_ab.py run …`
   A `bench` takes EVERY gate slot (no other session's gate builds beside it).
   Re-measure only when a later fix changes a measured code path (the same
   `range-diff` test as step 11's extra panel round). On #160 step 2 three
   benches ran and a gate building beside one made its walls unreadable.

   The full set, each through the supervisor, from the worktree:
   `python scripts/agentflow run --name ci-steps-all --timeout 45 --cwd <worktree> -- bash scripts/ci-steps all`
   `python scripts/agentflow run --name check-goldens-coverage --timeout 5 --cwd <worktree> -- bash scripts/check-goldens --verify-coverage`
   `python scripts/agentflow run --name check-goldens --timeout 45 --cwd <worktree> -- bash scripts/check-goldens`
   then `git status --porcelain -- . ':!.agent'` in the worktree must be empty —
   this excludes the issue's own `.agent/issue-N/ledger.md` and
   `.agent/issue-N/findings.json`, which are written as you go and stay
   uncommitted until step 12, so the gates only assert that nothing ELSE moved.
   Editing those two files WHILE a gate runs is fine: a gate record's
   `clean_before` / `clean_after` ignore `.agent/`, so the post-merge identity
   skip still holds. COMMITTING them during a run is not: the record's tree is
   HEAD's, and a moved HEAD leaves the record naming no tree.
   Each `run` result carries `"supervised": true|false`; it must be `true` here
   (this run holds the claim's lock) — `false` means the lock was lost or never
   held and the gate did not run under supervision, which is `blocked lock-lost`,
   not a passed gate. A result with `"failure": "contended-build"` means another
   cargo build held this worktree's `target/` (a link step got `permission
   denied`) — not a verdict on the code: wait until nothing else builds there
   (no subagent, reviewer or proof running cargo in it) and run the gate again;
   do not diagnose it as a test failure.
   HEAVY gates (`ci-steps-all`, `check-goldens`, `check-goldens-regen`,
   `cdo-gate`) take a gate semaphore shared by every session, so one may first
   QUEUE behind another session's gate; `check-goldens-coverage` and `ci-wait`
   never queue. Run each heavy `run` in the background, as its 45-minute
   timeout already requires; the wait does not count toward that timeout, and
   the result reports it as `queued_s`. A result with `"exit_code": null` is
   not a verdict and its child never started: `"failure":
   "gate-queue-timeout"` (it waited `gate_wait_minutes`) — run it again once,
   then `blocked gate-queue-timeout`; `"failure": "halted-while-queued"` is
   `blocked halted`.
   Each run writes its own log under
   `logs/<name>/`; read the path the result names. A moved golden: dispatch `golden-diff-triager`; only when
   every line is explained run
   `python scripts/agentflow run --name check-goldens-regen --timeout 45 --cwd <worktree> -- bash scripts/check-goldens --regen`
   and commit the regenerated files with the triage summary in the message;
   an unexplained line is a discovery and the golden is not blessed. If the diff
   is not docs-only:
   `python scripts/agentflow run --name cdo-gate --timeout 45 --cwd <worktree> -- bash scripts/cdo-gate`
   — its exit code 0 is the check; `cdo-gate`'s own ratchets carry the north-star
   numbers (CLAUDE.md's "Resolution Coverage" table is a point-in-time snapshot
   the ratchets re-verify, not something to compare against directly). A new
   DEFAULT detector: run `/triage-wave` first; above 30% false positives it ships
   opt-in. Add the CHANGELOG entry under `## [Unreleased]`. Record every exit
   code and log path in the ledger.
10. **Final panel.** Run the `code-review` skill at high on the issue branch:
    the Skill tool with `skill: "code-review"`, `args: "high <issue branch>"`. It
    is a Claude Code BUILT-IN skill, so it is not under `.claude/skills/`. If the
    Skill tool does not offer it, spawn the `code-reviewer` agent over
    `master..HEAD` instead and record the substitution in the ledger by name;
    never let a stand-in read as the specified step. Either way its findings
    enter `findings.json`. Then both reviewers over the diff, spec, and
    ledger, `charge final_rounds` per round (cap 5), same convergence rule.
    On a pre-ratified attempt (step 1), round 1 also asks both reviewers to
    mark EVERY carried entry, and sets each one's `anchor_hash` afresh (anchors
    do not carry across attempts): a carried `re-raised`
    is still unresolved, and `attest` refuses until both reviewers have
    accepted every entry.
    Source-verify every reviewer code claim before editing. Every fix passes
    step 9's TASK gate (`ci-steps-task`) before the next round; the full set
    is step 11, which starts beside round 2 (see step 11).

    **Ask for everything in round 1; later rounds verify.** Round 1's briefing
    asks each reviewer for EVERY finding at once (code, tests, the numbers file,
    the CHANGELOG, spec wording, stale docs), not only blocking ones. Later
    rounds' briefings follow step 5's later-round rule (the fix diff
    `git diff <last-marked-commit>..HEAD`, a `git range-diff` across a
    rebase, and the ids to mark, in the round-1
    thread, nothing else) and ask each reviewer to mark those entries, and to
    raise a NEW finding only for a real defect the fixes introduced. On #160 step 2 rounds 2-4 were wording and record fixes that
    round 1 could have named.

    **Prose after the gates is free.** `CHANGELOG.md` and `docs/**/*.md` are
    gate-inert (no gate reads them; `protect.gate_inert`), so `freeze-check` lets
    them follow `H`: a panel fix that touches ONLY those files (and the evidence)
    after step 11's gates needs no new gate run and no new `H`. A fix that
    touches anything else, a test included, goes back through step 11.
11. **Rebase and gate.** Start this step as soon as final round 1's fixes
    have a green task gate, in the background, while round 2 runs: send round
    2's briefing first (its fix diff is taken before the rebase), then rebase
    and start the full set. If a later round yields a fix outside the
    gate-inert set (`CHANGELOG.md`, `docs/**/*.md`, the two evidence files),
    that run is discarded and this step runs again on the new head. A
    gate-inert fix is committed only once the running gates have finished
    (a commit during a run moves HEAD and voids the run's record); `attest`
    still needs the records on `H`'s tree, so a discarded run can never be
    attested. Exception: an issue that still has its CDO bench to run waits
    for the panel to converge and the bench to finish, as step 9 says. In 2 of
    3 runs every round after the first was prose only, and the ~10 min of
    gates sat after a round that could not change them. Note the old base
    (`git merge-base HEAD origin/master`) and the old head, then
    `git fetch origin && git rebase origin/master` in the worktree (the fetch
    updates refs every session shares: on `cannot lock ref`, another session is
    fetching — retry the fetch once). Conflicts: resolve once (`charge
    rebase_regate`), else `blocked rebase-conflict`; a clean rebase charges
    nothing. Commit messages that start with `#` are read as comments by git, so
    rebase with `git -c core.commentChar=';' rebase origin/master`.
    Then run step 9's FULL set at once on the rebased head, and go straight on
    to step 12: the less time between the rebase and `attest`, the less chance
    `master` moves again. If `origin/master` has moved by the time of step 12's
    `check-diff`, do this step again (no charge if the rebase is clean).

    **One more final-panel round only if the BRANCH's own change moved:** a
    conflict was resolved, the post-rebase gates needed a fix, or
    `git range-diff <old base>..<old head> origin/master..HEAD` shows a branch
    commit whose diff changed anywhere except CHANGELOG context lines and
    `.agent/` evidence. Code that `master` brought in does not count: the gates
    cover how it combines with the branch, and the register is about the
    branch. Record the range-diff verdict in the ledger either way. A red gate
    here: fix it, run one panel round over the fix, then the full set again.
    Record `B = origin/master` and `H = HEAD` in the ledger.

    **A rebase rewrites every branch SHA, so re-point the register.** Use the
    same `git range-diff <old base>..<old head> origin/master..HEAD` to map each
    old branch commit to its rebased one, then rewrite every `fix_commit` and
    every commit-shaped `anchor` in `findings.json` to the rebased SHA (an
    `accepted-by-rule` entry whose `fix_commit` is not on `B..final_head` fails
    `attest`). Recompute an `anchor_hash` only if the range-diff shows its
    lines changed, and then that entry's marks are reset as usual. Do this
    before `attest`, which hashes the register.
12. **Freeze.** `halt-check` first; if halted, write the ledger and return
    `blocked halted`. First re-run the protected-path check over the WHOLE
    approved diff:
    `python scripts/agentflow check-diff --base <B> --head <H> --issue N --cwd <worktree>`
    — step 8 only ran it before each task commit, and steps 10 (panel fixes),
    11 (rebase) and this step all add commits after that, so this is the only
    thing standing between a panel-fix edit that adds `#[allow(` or touches
    `scripts/` and the approved diff. Run it immediately after the rebase and
    BEFORE `attest`; any reason is `blocked <reason>`.

    Then commit the ledger and `findings.json` (first
    `python scripts/agentflow sanitize <worktree>/.agent/issue-N/ledger.md <worktree>/.agent/issue-N/findings.json`
    — absolute paths: `sanitize` resolves its arguments against the process's own
    working directory, which per Invocation above is the main checkout, not the
    worktree; a violation is `blocked sanitize-failed`). Then
    `python scripts/agentflow freeze-check --H <H> --issue N --cwd <worktree>` must be
    empty (after `H` it allows only the two evidence files and gate-inert prose:
    `CHANGELOG.md`, `docs/**/*.md`). `python scripts/agentflow attest --issue N --B <B> --H <H> --final-head
    <HEAD> --register <worktree>/.agent/issue-N/findings.json --gates '<json of
    step 9 exit codes>' --body-hash <claim body_hash>` — `attest` cross-checks
    `--body-hash` against this run's `claim.json` and refuses (`error`: body-hash
    mismatch) if the issue body changed since claim, so pass the claim's
    `body_hash` verbatim, never a freshly recomputed one. It refuses four more
    things, all of which are conductor errors rather than surprises:
    - a `--B`, `--H` or `--final-head` that is not a full 40-character SHA
      (`sha-not-full`, listing the values) — `merge-gate` compares them with
      `!=`, so an abbreviated SHA would later read as `head-moved`/`base-moved`;
    - no `claim.json` for this run (`no claim.json for this run`) — the run
      directory and `--run-id` must be the claim's;
    - `--gates` that is not all green (`gates-not-green`, listing the keys).
      The JSON must carry `ci-steps-all`, `check-goldens-coverage` and
      `check-goldens`, plus `cdo-gate` unless the diff was docs-only, in which
      case pass `--docs-only` and omit that key. Every value is the gate's own
      exit code and every one must be `0`; use the `--name` strings from step 9
      verbatim as the keys.
    - a findings register that has not converged (`register-not-converged`,
      listing the entry ids): any entry whose `severity` is missing or not one
      of `critical|important|minor`, any whose `disposition` is missing or not
      one of `open|fixed|refuted|deferred`, any entry still `open`, any entry
      not marked `accepted` by BOTH reviewers (an `accepted-by-rule` counts
      only as step 5 states), or any blocking entry
      (`severity` `critical` or `important`, or `blocking: true`) left
      `deferred`, or any `closes_by` entry whose named record is not there
      (step 5).

    **A reviewer stand-in is the operator's decision, never yours.** When a
    rostered reviewer is unreachable, `attest --substitute SLOT=REVIEWER`
    (e.g. `astra=fable`) lets a named stand-in's marks fill that slot. Pass it
    ONLY when the operator has told you to; an unreachable reviewer is otherwise
    `blocked reviewer-unavailable`. The stand-in's verdict replaces the slot's
    own (the original reviewer's marks stop counting), each slot can be
    substituted once, and one reviewer may not hold both slots
    (`bad-substitute`). The substitution is written into the attestation as
    `reviewers`, so the public PR comment says who actually signed.
    On a pre-ratified attempt the claim's `handoff.reviewers` records who
    signed the carried design (absent means the named roster); print it in the
    ledger. `attest` never applies it on its own: pass the same `--substitute`
    flags only if the operator's instruction still holds. A mismatch fails
    closed (`register-not-converged`), it is never a silent pass.

    `attest` also records the register's path — relative to the claim's
    worktree, because the attestation is posted verbatim as a public PR comment
    and must not carry a local absolute path — and `merge` joins it back onto
    that worktree and re-hashes the file. So pass a `--register` inside the
    worktree (anything else is `register-outside-worktree`) and do not edit
    `findings.json` after attesting, or the merge refuses with
    `register-changed`.
13. **PR and merge.** `halt-check` first; if halted, write the ledger and return
    `blocked halted` — never push, create a PR, or comment past this point while
    halted. The push, the PR, the PR comment and a CI rerun all go through the
    executor; never run `git push` or `gh` yourself here. Each of them refuses under
    `--dry-run`, under HALT, and unless the lock names this run, and the two
    that publish text scan it first (a violation is `blocked sanitize-failed`).

    Push the branch:
    `python scripts/agentflow push-branch --branch <branch> --cwd <worktree>`
    — it accepts only a plain branch name (no `:`, no `refs/`, no revision
    syntax; anything else is `not a plain branch name`), refuses `master` or
    anything resolving to it whatever the casing, and pushes an explicit
    `refs/heads/<name>:refs/heads/<name>` refspec.

    The final body is the sanitized ledger, with `Closes #N` ONLY if every
    acceptance-matrix row is met; otherwise return `blocked acceptance-unmet`
    (and leave the draft as it is). The PR already exists as a draft (step 9):
    `python scripts/agentflow pr-ready --pr <pr> --body-file <path to the PR body>`
    replaces its body (scanned like `pr-create`'s) and marks it ready. If no
    draft exists (an older run), create it now instead:
    `python scripts/agentflow pr-create --title "<issue title> (#N)" --body-file <path to the PR body> --head <branch>`
    (`--base` defaults to `master`). It replies `{"pr": <n>}`.

    Poll CI through the supervisor:
    `python scripts/agentflow run --name ci-wait --timeout 45 --cwd <worktree> -- gh pr checks <pr> --watch`.
    CI red: read the failing job. If every failing test is one the diff
    cannot reach (the `task-affected` map in `scripts/ci-steps` says the
    changed paths do not reach its test binary, or it is `perf_bounds` and
    the diff touches none of the code it times), it is a FLAKE:
    `python scripts/agentflow ci-rerun --pr <pr>` once (no `ci_fix` charge;
    the head and the attestation are unchanged), then `ci-wait` again. A
    second failure of the same test is `blocked ci-flaky:<test>` plus a
    discovery, never a `ci_fix`. Any other red is one fix (`charge ci_fix`),
    then steps 9–12 again with a new attestation. Repeat CI runs on an
    unchanged head are otherwise run only when the issue's own acceptance
    asks for them, and only through `ci-rerun` (a green run is rerun whole,
    a red one only its failed jobs; anything else is refused:
    `ci-run-not-found`, `ci-run-not-complete`, `ci-run-conclusion-unknown`).
    The executor allows ONE rerun of a red run per head: a second one is
    `rerun-already-used`, which is `blocked ci-flaky:<test>`. Green reruns
    are not limited, and each record carries the head's rerun `count`.
    On #111 a flake cost the one `ci_fix`, a second full CI run and a block,
    45 min with no defect. Then `python scripts/agentflow merge-gate --pr <pr>`.
    `merge-lane-busy` means another session has merged and its post-merge has
    not yet verified it: the base is about to move. Beat at least every 10
    minutes and poll `merge-gate` until that reason is gone — no budget
    charge — and only THEN handle `base-moved` (it will usually appear once
    the lane frees). `merge-lane-orphaned` means the lane's holder died after
    merging; only a `recover` moves it on: `blocked merge-lane-orphaned`.
    `base-moved` means one more step 11 (`charge rebase_regate`); any other
    reason is `blocked <reason>` — including `ci-workflow-unknown`, which means
    the executor could not read the required workflow's name from
    `.github/workflows/ci.yml` and so cannot tell a green PR from one whose CI
    has not started, `register-changed`, which means `findings.json` moved
    after the attestation, and `register-unresolvable`, which means this run's
    `claim.json` is gone so the attested register cannot be located at all.
    Both the CI-fix path and the `base-moved` path go
    back through step 11's rebase, which rewrites the ALREADY-pushed branch's
    history — a plain push would be rejected, so re-push with the one permitted
    force form, and only ever on this issue branch:
    `python scripts/agentflow push-branch --branch <branch> --cwd <worktree> --force-with-lease`.

    Finally `python scripts/agentflow merge --pr <pr>`, which returns
    `{"merge_sha": …}` only once the gate has passed a second time internally —
    a refused merge instead returns `{"reasons": […]}` with exit 1, which is
    `blocked <reasons joined>`. `merge` first takes the merge lane: an
    `{"error": "merge-lane-busy"}` reply is handled exactly like the
    `merge-gate` reason above (wait, then re-check), and `merge-lane-orphaned`
    is `blocked merge-lane-orphaned`. The lane stays this run's until its
    post-merge verifies the merge or its `finish` runs. On success, post the attestation JSON as a PR
    comment, using the `path` that `attest` replied with
    (`.agent/runs/$AGENTFLOW_RUN_ID/attestation.json`):
    `python scripts/agentflow pr-comment --pr <pr> --body-file <that path>`.
    Print `merged <merge_sha>`.

## Rules

- `halt-check` before steps 8, 9, 12, and 13 (each stated inline above too).
  Never push, create a PR, or comment when `halt-check` reports halted.

## Ledger sections (in this order)

Claim; Classification; Assumption probes; Base/Head SHAs (B, H per round);
Timeline; Acceptance matrix; Discrimination proofs; Findings register summary;
Gate results (exit code, log, test counts, CDO numbers, toolchain and grammar
commit, CDO_WS identity as a hash); Discoveries (in the exact JSON shape
`/orchestrate` step 9 files); Not delivered.
