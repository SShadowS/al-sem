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
4. **Spec.** Write `docs/superpowers/specs/<today>-issue-N-<slug>-design.md` with
   sections: Goal, Non-goals, Acceptance matrix (each Acceptance item →
   the test or measurement that proves it, or "not deliverable, because"),
   Design, False-positive and failure shapes, Measurement plan. For a bounded
   issue the ledger's `## Design` is the spec.
5. **Spec panel.** `charge spec_rounds` per round (cap 5). Follow the
   `panel-review` skill: a briefing file with absolute paths and a
   confirm/reject checklist; both reviewers in parallel. Maintain
   `findings.json` as a list of `{id, source, round, file, line,
   severity: critical|important|minor, text,
   disposition: open|fixed|refuted|deferred, evidence, hash,
   reviews: {astra: accepted|re-raised|unreviewed, flash: …}}`. Both
   vocabularies are closed and the executor refuses an entry whose `severity`
   or `disposition` is missing or outside them — an unrecognised word is a
   malformed entry, never a silently non-blocking one. It treats `critical`
   and `important` — or an explicit `blocking: true` — as BLOCKING, which may
   not be merely `deferred`. Normalise a reviewer's own wording (a panel that
   says "High" or "Blocker") into the three words before writing the register.
   Each later round
   sends the spec diff plus the register and asks each reviewer to mark every
   entry `accepted` or `re-raised`. Converged when: no `open`; every entry
   `accepted` by both against the current hash; every blocking entry is `fixed`
   or `refuted` with evidence. A `re-raised` stays unresolved even without new
   evidence. Not converged after 5 rounds: check the handoff rule below, else
   return `blocked spec-panel-cap`.

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
   `opus`, reviewer `sonnet`, review-fix `sonnet` — call `python scripts/agentflow
   beat` immediately before AND immediately after each of these three dispatches
   (not only around the pi calls above; an `opus` implementer dispatch routinely
   runs longer than the 30-minute staleness threshold). Every task prompt
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

   The full set, each through the supervisor, from the worktree:
   `python scripts/agentflow run --name ci-steps-all --timeout 45 --cwd <worktree> -- bash scripts/ci-steps all`
   `python scripts/agentflow run --name check-goldens-coverage --timeout 5 --cwd <worktree> -- bash scripts/check-goldens --verify-coverage`
   `python scripts/agentflow run --name check-goldens --timeout 45 --cwd <worktree> -- bash scripts/check-goldens`
   then `git status --porcelain -- . ':!.agent'` in the worktree must be empty —
   this excludes the issue's own `.agent/issue-N/ledger.md` and
   `.agent/issue-N/findings.json`, which are written as you go and stay
   uncommitted until step 12, so the gates only assert that nothing ELSE moved.
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
    mark EVERY carried entry against the current hash: a carried `re-raised`
    is still unresolved, and `attest` refuses until both reviewers have
    accepted every entry.
    Source-verify every reviewer code claim before editing. Every fix passes
    step 9's TASK gate (`ci-steps-task`) before the next round; the full set
    waits for step 11.
11. **Rebase and gate.** Only once the panel has converged. Note the old base
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
    empty. `python scripts/agentflow attest --issue N --B <B> --H <H> --final-head
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
      not marked `accepted` by BOTH reviewers, or any blocking entry
      (`severity` `critical` or `important`, or `blocking: true`) left
      `deferred`.

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
    halted. The push, the PR and the PR comment all go through the executor;
    never run `git push` or `gh` yourself here. Each of the three refuses under
    `--dry-run`, under HALT, and unless the lock names this run, and the two
    that publish text scan it first (a violation is `blocked sanitize-failed`).

    Push the branch:
    `python scripts/agentflow push-branch --branch <branch> --cwd <worktree>`
    — it accepts only a plain branch name (no `:`, no `refs/`, no revision
    syntax; anything else is `not a plain branch name`), refuses `master` or
    anything resolving to it whatever the casing, and pushes an explicit
    `refs/heads/<name>:refs/heads/<name>` refspec.

    Create the PR with the sanitized ledger as body, title `<issue title> (#N)`,
    and `Closes #N` ONLY if every acceptance-matrix row is met; otherwise return
    `blocked acceptance-unmet` (no PR):
    `python scripts/agentflow pr-create --title "<issue title> (#N)" --body-file <path to the PR body> --head <branch>`
    (`--base` defaults to `master`). It replies `{"pr": <n>}`.

    Poll CI through the supervisor:
    `python scripts/agentflow run --name ci-wait --timeout 45 --cwd <worktree> -- gh pr checks <pr> --watch`.
    CI red: one fix (`charge ci_fix`), then steps 9–12 again with a new
    attestation. Then `python scripts/agentflow merge-gate --pr <pr>`.
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
