# Issue orchestrator — design

Date: 2026-09-13
Status: revision 5 (2026-09-15) — the post-merge design was rewritten against the SHIPPED executor after the 2026-09-14 incident; see "Post-merge verification". A correction of record, not a new review round.
Previously: revision 4 — three review rounds with gpt-6-astra + gemini-3.8-flash (the cap); round 3: gemini READY, astra NOT READY with three blockers, all addressed there without a fourth round
Owner: SShadowS

**Reading this document.** Everything outside "Post-merge verification" is the
design as reviewed on 2026-09-13 and is a statement of INTENT. That one section
is a statement of what the code does today, written by reading it. Where the two
disagree, the code is what shipped.

## Goal

A reusable, fully autonomous flow that takes GitHub issues on `SShadowS/al-sem` from
open to squash-merged on `master`, one at a time, with no human gate. It picks its own
issues, files issues it discovers, and stops on its own when the queue is empty, a cap
is hit, a merge regresses `master`, or a human drops the kill switch.

The flow is spec-driven (SDD), test-driven (TDD, red then green, with a discrimination
proof on every test), and reviewed by two independent model families at the spec stage
and again before merge. Every gate CI runs, plus the local-only goldens and CDO gates,
must be green before a merge.

## Non-goals and stated limits

- Parallel issues. One issue at a time on this machine (disk space, shared goldens
  gate, shared `target/`).
- Cloud or GitHub Actions runners. The flow depends on pi (Copilot billing), `CDO_WS`,
  and the local tree-sitter/goldens setup, none of which exist off this machine.
- Closing issues by judgment. Issues close only through a merged PR carrying
  `Closes #N`, and only when every acceptance item is met.
- Human review gates. Approval is replaced by two-model convergence plus the gates.
- Working on issues from strangers. The repo is public; see Eligibility.
- **Sandboxing (stated limit).** Workers run as the owner, on the owner's machine,
  with the owner's `gh` token and filesystem. Build scripts and tests from an
  eligible issue can read anything the owner can. Mitigation is eligibility (owner
  and collaborators only) and protected paths, not isolation. Revisit if issue
  authorship ever widens.
- **Spend ceiling (stated limit).** Token or dollar spend is not observable from
  inside the harness. The proxies are the subagent-dispatch cap and the pi-call cap
  below.

## Shape

Three parts:

| Part | Role |
|------|------|
| `scripts/agentflow/` (package; entry `python scripts/agentflow`) | The **executor**: every state mutation lives here (lock, claim, release, cleanup, merge, revert, discovery filing, HALT check, evidence sanitizing, gate supervision). Deterministic, unit-tested with a fake `gh`, has a `--dry-run` that performs no write anywhere. |
| `/orchestrate [--dry-run] [--max-issues N]` | One tick: preflight, fetch, filter, rank, pick, claim, run `/issue N`, post-merge check, file discoveries, reschedule. Re-fired by `/loop`. |
| `/issue N` | The per-issue pipeline: worktree, classify, probes, spec, spec panel, acceptance tests, plan, implement, gates, final panel, PR, merge. |

The commands are prose for the conductor; the executor is code for everything that
touches GitHub, git, or the filesystem outside the worktree. This split exists so
the mutation-bearing steps can be tested with fault injection and so `--dry-run`
is provably write-free.

**Bootstrapping and self-maintenance.** The executor and the commands are installed
and changed by hand by the owner. The flow never modifies itself: `scripts/` and
`.claude/` are protected paths (below). An issue that requires changing them is
`blocked` with reason `self-modification`, and the owner does that work in a
normal session.

Repo-specific gates are referenced by CLAUDE.md section, not copied, so porting the
flow to another repo means copying the three files and editing the gate list.

Skills used: `superpowers:brainstorming` (classification), `superpowers:writing-plans`,
`superpowers:subagent-driven-development`, `superpowers:test-driven-development`,
`discrimination-proof`, `panel-review` (protocol; roster `gpt-6-astra` +
`gemini-3.8-flash`), `code-review`, `golden-diff-triager` (agent), `triage-wave`
(when a new default detector ships).

## Model routing

| Task | Model | Why |
|------|-------|-----|
| Ranking (over the prefiltered eligible set, at most 10), discovery dedupe | sonnet 5 subagent | Cheap enough, better judgement than haiku on effort and dependencies; output is schema-validated JSON |
| Spec and plan authoring | conductor (fable 5.1) | Holds the design context |
| Spec review, final review | `gpt-6-astra` + `gemini-3.8-flash` via pi, thinking high | Uncorrelated families; `require_evidence` on |
| Acceptance tests, implementation | opus 5 subagents | Strongest coder available in-harness |
| Per-task review, review-fix edits | sonnet 5 subagents | Fast, adequate for bounded edits |
| Diff review before the final panel | `code-review` skill at high | Repo-aware findings, ranked |

## Trust boundary

The repo is public. Issue text is untrusted input and is treated as data, never as
instructions to the flow. Concretely:

- **Eligibility** (executor, deterministic, before any model sees the issue): author is
  the repo owner or a collaborator with write access; none of the labels
  `agent-blocked`, `agent-working`, `agent-answered`, `agent-regressed`,
  `agent-gates-green-unverified`, `agent-revert-landed`, `agent-revert-blocked`,
  `manual-only`, `epic`, `meta` (the incident vocabulary of "Post-merge
  verification" below is `recovery.INCIDENT_LABELS`: `agent-regressed`, which
  predates that section, plus the three it added —
  `agent-gates-green-unverified`, `agent-revert-landed` and
  `agent-revert-blocked`. Named rather than counted: an earlier draft said "the
  last three of that set", which points at `manual-only`/`epic`/`meta` instead,
  and a maintainer trimming this list against the spec would have been told the
  wrong three entries were the load-bearing ones. They fail CLOSED — a human who
  reopens an issue still carrying `agent-revert-blocked` is saying master may
  still be red, and the loop must not pick it up); body has an `## Acceptance` section (or the issue is
  a question, which can only be a spike); no open dependency. Dependencies are parsed
  by the executor from `Depends-on: #N` lines and `#N` references under a
  `Dependencies` heading; an issue in a dependency cycle is ineligible and gets one
  comment saying so.
- **Protected paths.** A diff touching any of these is `blocked`, no exceptions:
  `.github/`, `scripts/`, `.claude/`, `CLAUDE.md`, `.gitignore`, `Cargo.toml` version
  fields, the `tree-sitter-al` submodule pointer, and `.agent/` except exactly
  `.agent/issue-N/ledger.md` and `.agent/issue-N/findings.json` for the current
  issue. The flow cannot change its own gates, its own commands, or CI.
- **Never** weaken a gate, rebaseline a golden without a fully explained triage, or
  add `#[ignore]`, `--no-verify`, or a new `allow(...)` to get green. Any of these in
  a diff is `blocked`.
- **Evidence sanitizing** (executor). Everything that leaves the machine is scanned
  before it goes: issue bodies, comments, PR bodies, **and the two committed
  evidence files before every `git add`**. The scan rejects paths under `CDO_WS`,
  AL source excerpts longer than one line from dependency packages, and anything
  matching token patterns. A rejected commit is a `blocked` with reason
  `sanitize-failed`; the unsanitized originals stay in the local run directory
  (Retention, below). Limit: dependency-source excerpts are detected by path
  attribution (`.alpackages/` and `CDO_WS` paths), not by recognising AL source
  text.
- **Issue revision pinning.** The executor records a hash of the issue body at claim.
  Before `Closes #N` is written into the PR, the body is re-fetched; a changed hash
  is `blocked` with reason `issue-edited`, so acceptance edited during a run is never
  silently missed.

## Kill switch, budgets, and notification

- **HALT.** `.agent/HALT` in the main checkout. The executor checks it before every
  external write. Under HALT the only permitted writes are **terminal bookkeeping**
  (one comment and the label transition on the current issue) and **emergency
  rollback** (the revert path below). Claims, merges, pushes of new work, and issue
  filing are refused; so is `run` (gate execution), both under HALT and under a
  lock held by another run — `finish` is the one command permitted for terminal
  bookkeeping. The current step's local work finishes, the ledger is written,
  the issue is labeled `agent-blocked` with reason `halted`. Resume: the human removes
  the file and runs `python scripts/agentflow unblock N`, which removes `agent-blocked` and lets
  the issue re-enter eligibility (as a fresh attempt, see Locking).
- **Post-merge halts.** EVERY post-merge stop writes `.agent/HALT` itself with the
  reason, so no later tick or second session resumes until a human looks — not
  only the one that reverts. A stop where no gate returned a red verdict is not
  an `agent-regressed` outcome and never claims to be; it halts all the same,
  because the merge is unverified either way. See "Post-merge verification".
- **HALT is not the whole obligation.** HALT is one global file, so clearing it
  erases the thing it stood for. Each post-merge stop therefore also writes a
  durable incident record keyed by the MERGE SHA, and `preflight` refuses on
  that record independently of HALT. Clearing HALT alone does not resume the
  loop. Both `clear-halt` and `resolve-incident` are OPERATOR-ONLY, enforced in
  code (`lock.require_operator` refuses any call carrying a run id, which every
  conductor call does) rather than by a line in the command prompt, and both
  append to `.agent/audit.jsonl` before they act.
- **Notification.** On `agent-regressed`, `blocked` with reason `halted`, `sanitize-failed`,
  or `gh-unavailable`, the executor emits a push notification through the harness
  when that tool is available, and always posts the GitHub comment and prints to
  the terminal.
- **Per-issue budgets**, stored by the executor in `.agent/runs/<run-id>/budget.json`
  in the main checkout. Ownership convention, not isolation: only the executor
  writes it, and the executor rejects a value that does not match its own history;
  the ledger mirrors the counters for reading.

| Cap | Value | On hit |
|-----|-------|--------|
| Spec panel rounds | 3 | `blocked` |
| Plan tasks | 12 | `blocked` (issue too big; comment says split it) |
| Red-to-green attempts per task | 3 | `blocked` |
| Final panel rounds | 3, the last may be confirm-only | `blocked` |
| Rebase-driven re-gate | 1 | `blocked` |
| CI fix attempt | 1, independent of the rebase budget | `blocked` |
| Discoveries filed per issue | 5 | rest go to the ledger only |
| Wall-clock per issue | 4 h from claim | `blocked` |
| Subagent dispatches per issue | 60 | `blocked` |
| pi calls per issue | 14 (2 reviewers × up to 7 rounds) | `blocked` |
| Per-gate timeout (supervised, below) | 45 min | gate fails |

- **Per-loop budgets**: `--max-issues` (default 3), counted in
  `.agent/runs/loop.json` so re-fired ticks share one durable counter; the loop stops
  on the first `agent-regressed`.
- **GitHub API**: every `gh` call retries with exponential backoff on 403/429/5xx,
  three attempts, then the step fails as `blocked` with reason `gh-unavailable`.
  Listing uses `gh api --paginate`; issue bodies are truncated to 8 000 characters
  for ranking (the full body is used for the pipeline).
- **pi**: `pi_cleanup` and one retry on a hung or failed call, then `blocked` with
  reason `reviewer-unavailable`. Never downgrade to one reviewer.

## Locking, supervision, and crash recovery

Labels are a mirror for humans, not a lock. The lock is local:

- `.agent/lock.json` in the main checkout: `{issue, run_id, session, started,
  heartbeat}`. Taken atomically (create-exclusive) at claim. Both `/orchestrate`
  and a standalone `/issue N` take it; `/issue N` refuses to run without it.
- **Supervision.** Long-running work (every gate, every `cargo` invocation the
  pipeline runs) goes through `python scripts/agentflow run --timeout <min> -- <cmd>`, which
  refreshes the heartbeat every 60 s while the child runs, captures the exit code
  and log path, and kills the child's process tree on timeout. That is the
  watchdog: no gate can hang past its cap. Reviewer calls (pi) are launched as
  background tasks, and the conductor runs `python scripts/agentflow beat` while polling them,
  so a long model call cannot look like a death. Subagent dispatches are bounded by
  the harness; the conductor beats before and after each one.
- **Fencing.** Every mutating executor command takes `--run-id` and refuses to act
  if `lock.json` names a different run. A recovered or superseded run therefore
  cannot label, push, merge, or file anything after losing the lock.
- **Deadline.** The executor computes elapsed time since claim on every call and
  refuses new phases past the 4-hour cap, so the wall-clock budget is enforced by
  code, not by the conductor remembering it.
- **Stale** means heartbeat older than 30 minutes. There is no PID check: the
  conductor is a chat session, not a supervisable process, and a 30-minute silence
  with supervision in place means it is gone. Recovery (never under `--dry-run`):
  reconcile with GitHub first (open PR for `issue/N-*`? merged? partially pushed?).
  If the PR was already merged, recovery finishes the run instead of blocking it,
  under a lock the recovering run re-acquires for the crashed issue: post-merge
  check on the merge SHA, discoveries filed from the crashed run's ledger, cleanup,
  then `agent-done`. That follow-through lock carries the STALE lock's `started`
  rather than `now` — it holds the same work, not a new claim — because the
  adoption check below dates a merge against when the work was CLAIMED, and a
  lock stamped `now` is always later than the merge it just adopted. Its
  `heartbeat` is `now`: staleness is read from the heartbeat alone, so carrying
  that back would mint a lock that is born stale and let a concurrent tick steal
  a follow-through that is still running.
  If that follow-through fails partway, the issue is labeled
  `agent-blocked` with reason `recovery-followthrough-failed` and the tick stops.
  Otherwise: comment on the issue with what was found, rename the worktree to
  `../al-sem-issue-N.crashed-<ts>`, label `agent-blocked` with reason `crashed`,
  remove the lock. The human decides what to do with the crashed tree.
- **Attempts.** Every claim gets an attempt number. Branch and worktree are
  `issue/N-<slug>-a<attempt>` and `../al-sem-issue-N-a<attempt>`, so a re-attempt
  after `unblock` never collides with a preserved branch.
- The wall-clock cap is enforced by the running session on itself, never by another
  session.

## `/orchestrate` — one tick

1. **Preflight** (executor). Working tree clean on `master`; `master` equals
   `origin/master`; no `.agent/HALT`; no live lock; `gh auth status` ok; `pi_models`
   lists both reviewer models; `CDO_WS` set and exists; `tree-sitter-al/src/node-types.json`
   present in the main checkout; at least 20 GB free on the worktree drive. Any failure
   stops the tick with the reason printed. Nothing is created before preflight passes.
2. **Fetch** (executor). Paginated list of open issues with number, title, body, labels,
   author, createdAt. Apply Eligibility. Detect the stale-lock case and recover as
   above (not under `--dry-run`).
3. **Rank.** If more than 10 eligible, the executor keeps the oldest 10 by createdAt.
   One sonnet subagent gets those and the rubric, returns JSON validated against a
   schema (issue numbers must be from the input set; unknown fields rejected). Under
   `--dry-run` the ranking goes to stdout only; otherwise to
   `.agent/runs/<run-id>/ranking.json`.
4. **Dry run stops here** and prints the ranking and the pick. No labels, comments,
   locks, worktrees, files, or recovery under `--dry-run`.
5. **Claim** (executor). `loop-tick` runs here — after ranking (step 3), before
   claim — charging the per-loop `--max-issues` counter (`.agent/runs/loop.json`),
   so an empty ranked queue burns no slot and exactly N issues are claimed per
   loop. Take the lock; record the issue body hash; label `agent-working`; comment
   with the session link, classification hint, run id, and attempt number.
6. **Run `/issue N`.** Returns `merged`, `blocked`, or `spike-answered`.
7. **Post-merge check** (executor). On `merged`: `post-merge --issue N
   --merge-sha <sha>`. It refuses an invocation it cannot vouch for, fast-forwards
   the main checkout's `master` to `origin/master` (it must fast-forward; anything
   else is a preflight failure next tick), materialises the **merge SHA** in a
   DISPOSABLE detached worktree — the shared checkout stays on `master` throughout
   — and runs `scripts/ci-steps all`, `scripts/check-goldens`, and
   `scripts/cdo-gate` (unless the diff was docs-only) in that worktree, which is
   deleted afterwards. There are three distinct stops, only one of which may
   revert, and the full rules live in "Post-merge verification" below.

   This check always runs after a merge, HALT or not — it is one of the two
   writes HALT permits, because a HALT pressed between the merge and the check
   must not leave an unverified commit on `master` unexamined. Nothing is FILED
   on any stop: HALT is set by then and issue filing is refused under HALT, so
   the comment `post-merge` leaves is the record and a human files any follow-up.
   The tick then runs `finish --outcome regressed`, which changes NO label —
   `post-merge` has already made the issue's label transition, on whichever axes
   applied — and only retains the run directory and releases the lock.
8. **Cleanup, then finish** (executor). File discoveries (see below). Cleanup runs
   **before** finish: `cleanup` refuses to act if `lock.json` names a different
   run (the same fencing as every mutating command). On `merged` it removes the
   worktree after four independent proofs: the path is a direct child of the
   expected parent AND carries the `al-sem-issue-` name this flow hands out (the
   location check alone is "is a sibling of the checkout", and the checkout has
   230 siblings, essentially all of them repositories); `--worktree`/`--branch`
   agree with `.agent/runs/<run-id>/claim.json`, the record THIS RUN wrote, so a
   conductor juggling two worktrees in one tick cannot aim it with the wrong one;
   the tree has no modified tracked file; and the branch's tree equals the merge
   commit's. An absent run id or claim file is deliberately still accepted — an
   operator cleaning up by hand after a crash must keep working — which is why
   the name check, not the claim check, is the guard that covers that case. Then
   `rm -rf` with two retries for Windows file locks, `git worktree prune`, and
   delete the local branch;
   on `blocked` the worktree stays; on `spike-answered` (the branch never carries
   a commit of its own) `cleanup --spike` removes the worktree and branch outright.
   Finish then relabels `agent-done` / `agent-blocked` / `agent-answered`, copies
   the run directory to retention, and releases the lock last.
9. **Reschedule.** Under `/loop`, the next tick fires when the eligible queue is
   non-empty and `--max-issues` is not exhausted. Empty queue, HALT, or
   `agent-regressed` ends the loop.

Labels `agent-working`, `agent-done`, `agent-blocked`, `agent-answered`,
`agent-filed`, `agent-regressed`, `agent-gates-green-unverified`,
`agent-revert-landed` and `agent-revert-blocked` are created on first run if
missing. The incident path re-runs that create-if-missing pass before it stamps
anything: `gh issue edit --add-label X` hard-FAILS when X does not exist on the
repo, and a run claimed before the two-axis names shipped did its claim-time
pass against the old list — the incident path is the worst possible place to
discover that.

### Ranking rubric

Per issue: value 1–3, effort S/M/L, blast radius (subsystems named), classification
spike / bounded / architectural, one-line reason. Sort by value desc, effort asc,
createdAt asc. The ranking is advisory: dependency and eligibility filtering already
happened deterministically, and the model cannot introduce an issue that was not in
its input.

## Post-merge verification

This section is the design of record for `post-merge`, rewritten on 2026-09-15
against the shipped executor. It supersedes the single paragraph step 7 used to
carry, which described a revert built in the shared checkout and one
unconditional `agent-regressed` label. The rewrite is a CORRECTION, not a fourth
review round: what changed is the code, over two hardening arcs that followed the
2026-09-14 incident below.

**The incident this section exists because of.** On 2026-09-14 `post-merge` ran
on merge `19f654e1`. Its three real gates returned 0, 0 and 0. One tracked file
then reported modified — `crates/al-syntax/src/raw/generated/node-types.sha256`,
whose content was BYTE-IDENTICAL: it carried no line-ending attribute, so with
`core.autocrlf=true` git materialised CRLF while the generator writes LF. The
harness read that as proof the commit was bad and entered the REVERT path on a
fully-attested, CI-green merge. It failed to push the revert only because it
happened to hit an unrelated `master-not-ff` guard. The rule every paragraph
below is an application of: **COULD NOT VERIFY is not PROVEN BAD.**

### Aiming: only a merge record may point this command at a commit

`--issue` and `--merge-sha` are argv, and the lock fence proves only that the
LOCK names this run — not that this run merged that SHA. Together they could aim
the revert path at any commit, a human's sitting at the tip included.

- `merge` writes `.agent/runs/<run-id>/merge.json`: `{issue, pr, merge_sha,
  run_id, source}`. The issue comes from the ATTESTATION, never from argv —
  `merge` has no `--issue` of its own to be wrong about.
- The stale-run recovery writes the equivalent (`source: "recover"`) for a merge
  GitHub performed on the stale run's behalf, so that path stays supported
  without loosening the check for anyone else.
- `post-merge` refuses unless a record exists for this run and its `issue`,
  `merge_sha` and `run_id` ALL match argv. A record in an older field set (the
  bare `{pr, merge_sha}` this file used to hold) reads as ABSENT and refuses too,
  rather than being half-trusted on the two fields it happens to carry.
- Provenance alone proves only that SOME record names this SHA, so a second,
  independent check follows the fetch: the merge SHA must be reachable from
  `refs/remotes/origin/master`. Both sides fully qualified, because `git
  rev-parse master` prefers `refs/tags/master` over `refs/heads/master`; the
  check runs through git's own exit code, so an unknown or unfetched SHA is
  fail-closed.
- Stale-run adoption is scoped to the stale lock's OWN attempt on two
  independent axes: the PR's `headRefName` must end `-a<attempt>` (a prefix
  match alone adopts any attempt the issue ever had, and merged PRs outrank open
  ones), and a `mergedAt` that parses and predates the lock's `started` is
  refused. An absent or unparseable `mergedAt` does NOT refuse — that axis is
  defence in depth, and refusing a self-heal because a timestamp did not parse
  would be this section's own rule broken in a smaller form. `started` is the
  date the WORK was claimed, which is the whole reason `recover` mints its
  follow-through lock with the stale lock's own (see "Locking, supervision, and
  crash recovery"): a follow-through lock stamped `now` would make this axis
  refuse its own work by construction, so a post-merge killed mid-gates inside a
  recovery could never be recovered a second time.

All of those refusals happen BEFORE anything is written *by this flow*: no HALT,
no incident, no label, no comment, and nothing reaches GitHub. Be precise about
the one thing that is NOT true of the last of them: the fetch and the `--ff-only`
fast-forward of local `master` run before the reachability check, so by the time
that refusal fires the shared checkout's ref, index and working tree have already
moved. The tick stops and the lock is deliberately left to go
stale, so the next tick's `recover` re-mints the record from GitHub's own answer
and runs the check properly. A HALT here would close that self-heal and demand a
human for what the flow can finish itself.

### Isolation: the gates never run in the shared checkout

The shared checkout stays on `master` for the whole command. The merge SHA is
materialised in a disposable detached worktree (`../al-sem-verify-merge-<sha12>`,
a sibling of the repo root, deliberately outside the glob that finds issue
worktrees), and that directory is torn down in a `finally`.

Why it must be disposable: when the verification run and the revert's validation
run share a tree, everything the first left behind — a regenerated golden, a
touched `Cargo.lock`, a `gen-syntax` rewrite, a line-ending materialisation — is
inherited by the run that decides whether to PUSH. A revert that is red from a
clean checkout can be green-lit by the leavings of the run it is reverting. The
revert therefore gets its own second disposable worktree, created only after the
first is destroyed, so peak extra disk is one tree rather than two.

Three consequences worth writing down, because each is a real dependency between
two files rather than an implementation detail:

- Teardown is `rm -rf` plus `git worktree prune`, never `git worktree remove` —
  that command refuses outright in a repository with a submodule, and this one
  has `tree-sitter-al/`.
- A worktree gets no submodule checkout of its own, so gates would not compile
  there. They do because the gate runner derives `TREE_SITTER_AL_PATH` from the
  repo ROOT, not from the gate's cwd, and `scripts/ci-steps` honours that env var
  over its own `rev-parse --show-toplevel` fallback.
- The SOURCE tree is isolated; the build CACHE deliberately is not
  (`AGENTFLOW_VERIFY_TARGET_DIR`, else an inherited `CARGO_TARGET_DIR`, else the
  root's `target/`). Measured: `target/` is 65G here and the drive has 55G free
  while the issue worktree is still on disk. A per-worktree cache does not merely
  cost time, it does not fit — and the dirt that can green-light a bad revert is
  SOURCE dirt, which a fresh checkout already eliminates.

Teardown is best effort and is REPORTED, never promoted to a verdict:
`verify_worktree_removed: false` in the payload means either that Windows still
held a file lock or that this stop RETAINED the tree on purpose (below), and it
does not change what the gates said.

Teardown is also SKIPPED on exactly one stop — `tree-dirty-after-gates`, where
the dirt is the evidence. The flag that skips it is set inside that one branch,
never derived from "a halt was written": the gate-timed-out stop also writes a
halt and must keep destroying its tree, because a killed gate leaves a partial
build rather than evidence about a file. The retained tree is NOT permanent:
the verification path is a pure function of the repo root and the merge SHA, and
worktree creation destroys whatever sits there before checking out, so a second
`post-merge` of the SAME SHA reclaims it. That can only follow an operator clearing HALT — someone who has
been told the tree is there.

### Three stops, and only one of them may revert

| What happened | Route | Payload |
|---------------|-------|---------|
| A gate RETURNED non-zero | validated revert | `revert` |
| A gate was KILLED at its timeout | HALT only | `halt`, `reason: gate-timed-out`, gate named in `timeouts` |
| Every gate returned 0, tracked CONTENT differs | HALT only | `halt`, `reason: tree-dirty-after-gates`, `dirty_total`, `retained_worktree`, and `verify_worktree_removed: false` |

The second row is not a refinement of the first. The supervisor folds a kill into
`exit_code or 124`, so reading the exit code alone makes a clock running out
indistinguishable from a verdict at the one decision point where that distinction
IS the rule. A gate that never finished returned no verdict at all, and any gate
after it never ran. It is materially reachable rather than exotic: verification
runs in a worktree cargo has never built at, so a cold `ci-steps all` crossing 45
minutes is a normal outcome.

The third row is the 2026-09-14 class, and the probe that decides it asks the
tree the gates ACTUALLY RAN IN, comparing tracked CONTENT against the commit
rather than reading `git status`. A path whose content MATCHES and whose
porcelain merely disagrees is a fourth outcome, not a stop: it is reported as
`tree_anomaly` / `tree_anomaly_total`, notified, and changes no verdict. The same
content-aware probe is what `preflight` and worktree cleanup now use, so the
incident's own checkout no longer fails every tick or strands its worktree.

Both halt-only stops leave `master`, the remote, and whatever the working trees
hold exactly as found. On the third row the dirt IS the evidence: the old revert
path's `reset --hard` would have wiped the very CRLF clue that root-caused the
incident. So that row KEEPS the worktree the gates ran in, and says where it is
— `halt.retained_worktree`, the HALT reason, and the issue comment, which spells
out the `git -C <path> status` against `git -C <path> diff` comparison that
root-caused 2026-09-14. This is stated as a behaviour rather than an intention
because for two rounds it was neither: the HALT was written and the tree was
`rm -rf`'d one `finally` later, so the operator was sent to the shared checkout,
which no gate ran in and which is clean, with only a path list cut at 20 to work
from. A path list is not the dirt — the root cause was found by comparing a
file's on-disk BYTES against the committed blob.

`agentflow cleanup --worktree <retained path>` is that tree's supported exit and
the only one: the tree is detached, so it has no branch and no merge SHA to prove
anything about, and the two issue-worktree removers refuse its name outright. The
command routes it to the module that owns the `al-sem-verify-` prefix instead,
and refuses `--branch`, `--merge-sha` and `--spike` for it rather than ignoring
them. A retained directory with no supported way to remove it would be an
obligation nobody can discharge — the same shape as the `verifying` row below.

The function that handles both stops takes no `Git` at all — it has no way to
revert, reset or push, and that is the point; `retained` reaches it as a STRING
it reports, never as something it can touch.

### The revert path

Reached only on an actual non-zero gate result. In order:

1. HALT first, with a SHA-only reason, before any git call that can raise; the
   durable incident is opened immediately after, before the fetch.
2. Refuse if local `master` is not an ancestor of `origin/master` (read-only, and
   fully qualified on both sides — the old form answered "can this fast-forward?"
   by PERFORMING the fast-forward). Local `master` is never discarded.
3. Build the revert in its own disposable worktree at the merge commit. A
   conflict stops here.
4. Re-run EVERY gate, not only the one that failed, plus `scripts/ci-steps test`,
   in that worktree. The closure that runs them is handed the tree and never
   chooses one, because a closure that chose its own tree is exactly how the
   validation ends up in a contaminated one.
5. Refuse if `origin/master` no longer equals the merge SHA.
6. Push `<revert-sha>:refs/heads/master` — an explicit two-sided refspec, pinned
   to a full 40-hex id captured before the re-run, never a name and never forced.
   A one-sided `git push origin master` would send whatever the local ref points
   at after tens of minutes of gate time, and would re-resolve the name through
   rules that prefer `refs/tags/master`.

The issue is reopened on exactly ONE exit: the one where the revert actually
landed. `master-not-ff`, `revert-conflict`, `revert-failed-gates`,
`master-advanced` and `push-rejected` all leave `master` red, set HALT, label,
comment and stop — without reopening, because the work did not come off master.
On the exit that did push, the durable record is written BEFORE the reopen and
the comment, so a GitHub outage can cost the labels but never the record that
`origin/master` was rewritten.

### Labels: two independent axes

`incident_labels` is the one place that decides them, and it answers two
questions that are not the same question:

- **EVIDENCE — did a gate actually go RED?** `agent-regressed` when yes;
  `agent-gates-green-unverified` when no gate returned a red verdict, which
  covers both halt-only stops. This axis is the 2026-09-14 correction: the old
  code stamped `agent-regressed` unconditionally, so a merge whose every gate
  passed was labelled a regression on the strength of the other axis alone.
- **ACTION — did a validated revert land?** `agent-revert-landed`, or
  `agent-revert-blocked`, which means MASTER MAY STILL BE RED. Absent entirely
  when no revert was attempted, because there is then nothing true to say.

The naive inverse — label only when a revert lands — is wrong in the other
direction: a red gate IS a regression whether or not the revert could be pushed,
which is exactly when a human most needs to know. `agent-blocked` is deliberately
not reused for any of this; it already means "the work never landed", which on a
closed issue whose code is sitting on `master` reads as a contradiction.

### The durable incident record

HALT is one global file, and clearing it erases the obligation it stood for. So
each stop also writes `.agent/incidents.json`, keyed by the FULL merge SHA, and
`preflight` refuses on that record INDEPENDENTLY of HALT.

THREE states, because "nothing recorded" and "terminal obligation" are not the
only things that can be true:

- `verifying` — a `post-merge` that opened its obligation and has not reached a
  verdict. Shown by `preflight` and `status`, a failure row in neither. A
  post-merge killed mid-gates — the longest window in the tick — must stay
  recoverable by `recover` with no operator action; a terminal row here would
  gate the very recovery that discharges it, and the operator's only exit would
  be to record on the audit trail that a merge nothing verified is closed,
  purely to be allowed to go verify it.
- `open` — a verdict was reached. `preflight` refuses on these and only these,
  as an `incident:<sha12>` failure row.
- `resolved` — closed, by the one automation path or by an operator.

Re-opening the same SHA is the SAME incident, and the record is MONOTONIC on the
evidence axis: `gate_red` moves False → True and never back, and a terminal
reason is never overwritten by the in-progress one. Without that, an operator
re-running `post-merge` under the same lock erased the record that a gate had
ever gone red. Re-opening a RESOLVED record clears the human's close and appends
an `incident-reopen` line naming who it superseded, so no record ever reads
`state: open` and `resolved_by: <a human>` at once.

Two ways to close one, kept as two functions with no shared bypass flag:

- **`mark_verified`** — the only automation close, on a pass where every gate
  returned 0 and the tree came back clean. It REFUSES a record carrying
  `gate_red: True` or `revert_landed: False`: a later green pass proves something
  about a later run, not about either of those.
- **`resolve-incident --merge-sha S --by <name>`** — an operator asserting it by
  hand, through the operator gate. It closes an `open` row and a `verifying` one,
  and the second is not a loosening: it is the only exit that state has. A
  `verifying` row is what a post-merge killed mid-gates leaves, `mark_verified`
  needs a post-merge run, and post-merge needs a merge record plus a fence no
  operator invocation can satisfy once the lock is gone — so the row sat on the
  dashboard forever while this command told the operator "`<sha12>` was closed by
  `None`", about a merge nobody verified and nobody closed. An unverified merge
  is an obligation, and discharging an obligation is what an operator is for; the
  record keeps `resolved_by` and the audit line, so an operator's close stays
  distinguishable from `mark_verified`'s `post-merge:<run>` form. Its refusals
  each name only what they can vouch for: `unknown-merge-sha` (no record),
  `already-resolved` (naming the REAL closer), and `unknown-state` — reachable
  only through a hand-edited `incidents.json` — which reports the state it found
  rather than describing it as a close.

An `automated=True` argument on one function would have been shorter and is
exactly the shape that rots.

`clear-halt` and `resolve-incident` are OPERATOR-ONLY, enforced by
`lock.require_operator`: a run id in the context is refused, and every conductor
call carries one. A live lock is refused too; a STALE lock is deliberately let
through, or the operator would be deadlocked for 30 minutes. Both append to
`.agent/audit.jsonl` — append-only, and never read back by the executor, because
a consumer that started branching on it would turn an evidence log into a control
input. `unblock` is the stated exception: it sets its own run id, so the gate
cannot apply to it and the command prompt really is the only guard.

**Stated limit.** `.agent/incidents.json` is local per-checkout state. An
operator working from a different clone, or one who wipes `.agent/`, loses the
obligation silently. HALT has exactly the same property, so this is not a
regression — but it does mean the incident record is a stronger backstop against
a careless CLEAR than against a careless DELETE, and `.agent/audit.jsonl` helps
reconstruction rather than enforcement.

**Historical note.** `docs/superpowers/plans/2026-09-13-issue-orchestrator.md`
still shows `merge.json` being written in its original bare `{pr, merge_sha}`
shape. That is harmless as history, and worth knowing: a record in that shape is
now read as ABSENT and refuses, deliberately.

## `/issue N` — the pipeline

Everything writes to `.agent/issue-N/ledger.md` inside the worktree. A sanitized
render of the ledger becomes the PR body.

1. **Worktree.** `git worktree add ../al-sem-issue-N-a<k> -b issue/N-<slug>-a<k> master`.
   Export `TREE_SITTER_AL_PATH=<main checkout>/tree-sitter-al` for every command run in
   the worktree (no second grammar checkout). The pre-commit hook runs from the main
   checkout by design; that is correct because `scripts/` is a protected path.
2. **Classify.** The brainstorming skill's three-path rule, recorded with the reason.
   - Spike: run the probe, comment the answer on the issue, label `agent-answered`,
     return `spike-answered`. No PR, no closing.
   - Bounded: short design in the ledger; the spec panel still runs with both
     reviewers but a single round is expected to converge. Same final panel.
   - Architectural: full spec and panel.
3. **Assumption probes.** Before the spec: every fact the issue's Acceptance depends on
   (the battleplan issues list them under Dependencies, e.g. "SymbolReference.json
   carries Scope per symbol") is checked against real data, and the result goes in the
   ledger. A falsified dependency does not block; it shapes the spec and is a
   candidate discovery.
4. **Spec.** `docs/superpowers/specs/YYYY-MM-DD-issue-N-<slug>-design.md`, with three
   required sections: "Acceptance matrix" (each Acceptance item → the test or
   measurement that proves it, or "not deliverable, because"), "False-positive and
   failure shapes", and "Measurement plan".
5. **Spec panel.** `panel-review` protocol: briefing file, both models in parallel,
   `require_evidence` on, `output_file` per model. Convergence is defined by the
   **findings register** (below). Cap 3 rounds via `continuation_id`; each round's
   prompt carries the diff of the spec since the last round and the register.
6. **Acceptance to tests.** After the spec converges, an opus subagent turns the
   acceptance matrix into failing tests. Tests must compile and fail for the right
   reason (asserted output captured in the ledger). Not committed on their own: the
   pre-commit hook would reject a red golden family. They are committed with the
   task that turns them green; the red evidence lives in the ledger.
7. **Plan.** `writing-plans` skill writes `docs/superpowers/plans/…`. More than 12
   tasks means `blocked` with a "split this issue" comment.
8. **Implement.** `subagent-driven-development`. Every task prompt carries: TDD red
   then green; `discrimination-proof` on every new or changed test with both outcomes
   and the mutation patch recorded in the ledger; `rustfmt <file>` only; SOLID and DRY
   as explicit per-task review criteria; the protected-path list. Implementer opus 5,
   per-task reviewer sonnet 5, review-fix edits sonnet 5. Cap 3 red-to-green attempts
   per task.
9. **Repo gates**, each run under the supervisor in a sanitized environment
   (`REGEN_TEMP_GOLDENS` unset, `ALSEM_NO_PREFLIGHT_CACHE=1`), each with its exit
   code captured and its log retained (never piped through `tail`):
   `scripts/ci-steps all` (fmt, clippy, gen-syntax, test, build, perf-bounds,
   exactly CI), `scripts/check-goldens --verify-coverage`, `scripts/check-goldens`,
   then `git status --porcelain` must be empty (a verification that changed the tree
   is a failure). Goldens moved means `golden-diff-triager` runs and every changed
   line must classify as explained before `check-goldens --regen` is accepted; an
   unexplained line is a discovery and the golden is not blessed. `scripts/cdo-gate`
   runs for every diff that is not docs-only (so `Cargo.toml`/`Cargo.lock` dependency
   changes are covered), and the north-star numbers in CLAUDE.md "Resolution Coverage"
   must hold. A new detector that would ship DEFAULT runs `/triage-wave` first and
   ships opt-in above 30% false positives. A CHANGELOG entry is required.
10. **Final panel.** `code-review` skill at high on the diff (its findings enter the
    register); then `gpt-6-astra` and `gemini-3.8-flash` in parallel over the diff,
    spec, and ledger, against the head SHA recorded in the register. Same
    convergence rule and cap. Every reviewer claim about code is source-verified
    before an edit.
11. **Rebase and re-gate** (executor). Fetch, rebase onto `origin/master`. After
    **any** rebase the gates rerun (a clean rebase can still change behaviour
    through untouched files); the panel reruns one round only if the code diff
    changed. One rebase re-gate is budgeted. The validated pair is `(B, H)`: `B` the
    `origin/master` commit rebased onto, `H` the head the gates and panel passed on.
12. **Freeze.** The freeze boundary is set **after** step 11, so `H` is post-rebase.
    The protected-path check runs once more over the whole approved diff (`B..H`)
    immediately after the rebase and **before** the attestation — step 8's
    per-task check cannot see the commits that steps 10 and 11 add, so this is
    the only thing stopping a panel-fix edit from landing a protected path inside
    the approved diff. The only commits allowed after `H` are evidence commits
    touching nothing but the two `.agent/issue-N/` files; the executor asserts
    `git diff H..HEAD --name-only` is a subset of those two paths, so recording the
    approval cannot invalidate it. The committed ledger records `B` and `H` only;
    `final_head` (the evidence commit) lives solely in the attestation
    `{issue, B, H, final_head, register_hash, register_path, gates, body_hash}`,
    which the executor writes to the run directory and posts as a PR comment.
    The attestation is not a transcript of conductor assertions: `attest` refuses
    to write one unless this run's `claim.json` exists and its body hash matches,
    every required gate key is present with exit code `0` (`cdo-gate` included
    unless the diff was docs-only), and the findings register has converged — a
    `disposition` from the closed set on every entry, none `open`, both reviewers
    `accepted` on every entry, and no blocking entry left `deferred`. Blocking is
    derived from the severity vocabulary the panel writes (`critical` or
    `important`) or an explicit `blocking: true`, so the rule fires against the
    registers the commands actually produce. It binds the register's path as
    well as its hash — relative to
    the claim's worktree, since the attestation is published as a PR comment and
    must not carry a local absolute path — and the merge joins that path back
    onto the worktree and re-hashes the file, so a register edited afterwards is
    `register-changed`.
13. **PR and merge** (executor). PR creation, the attestation comment, and every
    branch push go through the executor (`pr-create`, `pr-comment`,
    `push-branch`), so all three are inside the dry-run guard, the HALT check and
    the run-id fence, and their bodies are sanitized. `push-branch` takes a plain
    branch NAME — never a refspec, an option, or a full ref — and pushes an
    explicit `refs/heads/<name>:refs/heads/<name>`, so the destination ref is
    built by the executor and can never be inferred from caller text;
    `--force-with-lease` is the only force form in the flow and reaches only that
    validated branch.
    The PR body is the sanitized ledger, with `Closes #N` only if the acceptance
    matrix is fully met. Required CI: the `ci.yml` workflow's checks must be
    PRESENT on the PR and all `success` — a `skipped`, `cancelled`, or missing
    check is not green, and neither is a PR where only an unrelated workflow has
    reported. The required workflow is identified by the `name:` of
    `.github/workflows/ci.yml`; if that cannot be read, the gate refuses
    (`ci-workflow-unknown`) rather than passing. One CI fix attempt allowed,
    independent of the rebase
    budget; a CI fix is a code change, so it returns to step 11 (gates, one panel
    round, new `H`, new attestation). Immediately before merging, the executor
    re-fetches and requires **all** of: `origin/master == B` (the base is bound,
    not just the head; if it moved, that is a rebase, budgeted as such), the PR head
    `== final_head`, and the issue body hash `== body_hash` (re-checked here, not
    only at PR creation). Then `gh pr merge --squash --match-head-commit <final_head>`.
    The post-merge check on the merge SHA (orchestrate step 7) is the backstop for
    the unavoidable window between the check and GitHub's merge. Return `merged`.

Unmet mandatory acceptance is `blocked`, never a partial merge. `blocked` means:
comment on the issue with the sanitized ledger, label `agent-blocked` with the
reason code, leave the branch and worktree in place for a human.

### Findings register

`.agent/issue-N/findings.json`: one entry per finding from any reviewer or the
`code-review` skill, with a stable id, source (which reviewer, which round), the
cited file and line, severity normalised to `critical` | `important` | `minor`,
the conductor's disposition
(`fixed` with commit SHA, `refuted` with source evidence, `deferred` as discovery
with the reproducer, or `open`), the artifact hash it was raised against, and a
per-reviewer disposition field (`accepted`, `re-raised`, `unreviewed`).

Severity is a closed vocabulary rather than the reviewer's own wording, because
the merge gate pattern-matches it: `critical` and `important` are BLOCKING, so
they may not be merely `deferred`. The conductor normalises a reviewer's phrasing
into the three words when writing the entry; the executor refuses a register
carrying anything else, rather than treating an unrecognised word as non-blocking.

Each round's prompt shows every entry with its conductor disposition and asks each
reviewer to mark each one `accepted` or `re-raised` against the current hash. A
`re-raised` stays unresolved whether or not it cites new evidence: the conductor
must fix it or produce a new refutation for the next round. Silence is
`unreviewed`, never approval. A dispute that survives the round cap is `blocked`
for a human to arbitrate; the register never converts a rejection into approval
on its own (fail closed, at the cost of an occasional blocked-by-dispute issue).

Convergence means: every entry carries a severity and a disposition from the two
closed vocabularies; no `open` entries; every entry is `accepted` by **both**
reviewers against the current hash (including the originating reviewer's own);
every blocking entry — `critical`, `important`, or explicitly `blocking: true` —
is `fixed` or `refuted` with evidence, never `deferred`. A reviewer downgrading its own blocking finding without a refutation is
recorded but does not count. The final round may be confirm-only (no changes since
the previous round, only dispositions). Three rounds without convergence is
`blocked` with the register attached; the standard is not relaxed.

## Discoveries

Appended to the ledger's `## Discoveries` during any phase, never acted on
mid-issue:

- A defect with a reproducer: a failing test shape, a command with its output, or a
  file and line plus the wrong behaviour.
- A falsified assumption from step 3 that changes what the issue can deliver.
- A gate finding the issue did not cause: a flaky test, a stale doc claim, a golden
  line the triager classed unexplained.

Not filed: style nits, refactors without a defect, reviewer suggestions without
evidence, anything under `CDO_WS` that cannot be reproduced on a committed fixture.

Filing (executor, end of tick): each discovery gets a deterministic fingerprint
(subsystem + normalized failing test name or file path + normalized first line of
the symptom). Filing is crash-safe: the executor writes a `pending` entry to
`.agent/discoveries-index.json` first, creates the issue with the fingerprint
embedded as an HTML comment marker (`<!-- agentflow-fp: … -->`) in the body, then
marks the entry `filed` with the number. On startup, every `pending` entry is
reconciled by listing `agent-filed` issues (state all) and scanning bodies for the
marker, because GitHub search does not reliably index HTML comments. A `pending`
entry whose marker is not found in that scan is `pending-not-found` (retryable — a
new creation is attempted); one matched by more than one issue is
`pending-ambiguous` (reported to the human, never recreated). Discoveries are
attributed to a baseline: a gate failure that also reproduces on `master` at `B` is
filed as pre-existing (and does not block the issue); one that does not is the
issue's own defect. Titles are `[agent-discovery][<subsystem>] <symptom>`, labels
`agent-filed` + `bug` or `enhancement`, body in the same shape `/issue` expects
(Capability, Acceptance, origin issue, reproducer, session link). At most 5 per
issue, the cap charged only after a successful create — a crash before creation
completes does not consume budget. Filed issues enter ranking on the next tick
like any other; `agent-filed` grants no special eligibility.

## Ledger and retention

`.agent/issue-N/ledger.md`, committed with the branch after sanitizing. Sections:

- Classification and why; assumption-probe results.
- Base `B`, head SHA at each panel round, `H`. The final head and merge SHA live in
  the attestation and the executor's PR comments, since the committed ledger cannot
  contain its own SHA.
- Timeline: one line per phase with outcome, exit code, and cap counters.
- Acceptance matrix with the proving test per item.
- Discrimination-proof table: test, mutation patch, fail output, pass output, commit.
- Findings register summary (full file committed alongside).
- Gate results with exit codes, test counts, skip counts, and CDO numbers when that
  gate ran; toolchain, grammar commit, and `CDO_WS` identity (a hash, not the path).
- Discoveries.
- What the issue asked for and was not delivered, with the reason.

The PR body is the sanitized render: reviewer outputs are embedded as short
summaries with verdicts, never as local paths. `.agent/` is gitignored except
`.agent/*/ledger.md` and `.agent/*/findings.json`; the gitignore rule is tested by
the executor's test suite (a parent-directory rule silently defeats the exception).

**Retention.** The full reviewer outputs, gate logs, unsanitized originals,
attestation, and budget file for a run are copied to
`~/.al-sem/agentflow/runs/<run-id>/` before the worktree is removed, so a review
can be reproduced after cleanup. Nothing in that directory is ever pushed.

## Failure handling

- Crashed session: stale-heartbeat recovery above; the worktree is preserved, never
  deleted by recovery.
- pi unreachable: retry once after `pi_cleanup`, then `blocked`.
- Moved `master`: rebase, gates rerun always, panel rerun if the diff changed; the
  rebased issue branch is re-pushed with `git push --force-with-lease` — the only
  permitted force form, used only on the issue branch, never on `master`; the
  merge requires `origin/master == B` and `--match-head-commit`, so neither a
  stale base nor a stale head can merge.
- Red `master` after merge (a gate RETURNED non-zero): validated revert in a
  disposable worktree, `agent-regressed` plus the action label the revert earned,
  HALT, a durable incident, notify, loop stops. The issue is reopened only when
  the revert actually landed.
- Merge NOT VERIFIED after merge (a gate was killed at its timeout, or every gate
  returned 0 and the tree came back with tracked content modified): HALT, a
  durable incident, `agent-gates-green-unverified`, notify, loop stops — and
  **nothing reverted, nothing pushed, the evidence left as found**. See
  "Post-merge verification"; this is the 2026-09-14 class, and treating it like
  the row above is the defect that section exists to prevent.
- A `post-merge` that cannot be aimed (no merge record, a record that disagrees
  with argv, a merge SHA not reachable from `origin/master`): refuse before
  anything is written, stop the tick, and let the lock go stale so the next
  tick's `recover` can re-mint the record and try again.
- HALT: terminal bookkeeping only, ledger written, `blocked` with reason `halted`.

## Repo changes

- `scripts/agentflow/` (package) and `scripts/agentflow/tests/` (fake `gh`, fault
  injection before and after every side effect, dry-run write-free assertion, lock
  races, stale recovery preserving the tree, sanitizer rejection, protected-path
  detection, freeze-boundary assertion, pending-discovery reconciliation, gitignore
  exception). The post-merge hardening added two modules to that package:
  `worktrees.py` (disposable verification worktrees, with the name prefix as the
  ownership proof on an `rm -rf`) and `incidents.py` (the durable per-merge
  obligation and its three states), plus `.agent/incidents.json` and the
  append-only `.agent/audit.jsonl` alongside the existing `.agent/` state.
- Executor subcommands added for the operator, not for the flow: `clear-halt`,
  `resolve-incident`, `incidents`. The first two are gated in code; see
  "Post-merge verification".
- `.claude/commands/orchestrate.md`, `.claude/commands/issue.md`.
- `.claude/commands/README.md` (new): one entry per command, plus `/triage-wave`.
- `.gitignore`: `.agent/*` with per-directory re-includes for the two evidence
  files (a fully-ignored directory cannot be re-included, so the pattern is
  single-level).
- CLAUDE.md: one paragraph recording the doctrine exception for this flow. It
  authorizes exactly two writes to `master` without a human request: the gated
  squash-merge (every gate in step 9 green, register converged, CI green, head SHA
  matched at merge) and the validated revert of a commit this flow itself merged.
  It also lists the protected paths. `master` carries no branch protection today;
  if that changes, the merge and revert paths must go through the protection's
  own API, which is a spec change.
- CHANGELOG: Added entry.

## Proving the flow

1. Executor unit tests (above) green, including: a claim races a live lock and loses;
   a stale lock recovers without deleting the tree; `--dry-run` performs zero writes
   under fault injection; the revert path is exercised on a fake merge SHA and refuses
   to push a revert whose tests fail; a protected path in a diff yields `blocked`;
   the sanitizer rejects a `CDO_WS` path in a ledger; a commit after `H` outside the
   two evidence files fails the freeze assertion; a `pending` discovery is
   reconciled by marker instead of refiled.
2. `/orchestrate --dry-run` on the current open issues. Expected: eligibility
   filtering shown, a ranking with classifications, one pick; nothing else touched
   (verified by `git status`, label listing, lock absence, and an empty
   `.agent/runs/` before and after).
3. A seeded bounded smoke issue (a known one-line gap with an Acceptance block) run
   end to end. Expected: a merged PR, a ledger with a discrimination proof and a
   converged register, an attestation comment, and the label transitions
   `agent-working` → `agent-done`.
4. A deliberately unsatisfiable issue. Expected: `agent-blocked`, a ledger comment,
   branch left in place, and the next tick skips it.
5. A seeded issue whose fix breaks a test only visible on `master` after merge —
   i.e. a gate that RETURNS non-zero. Expected: the revert built and validated in
   its own disposable worktree and pushed under a sha-pinned two-sided refspec,
   the issue reopened, `agent-regressed` plus `agent-revert-landed`, HALT
   present, an `open` incident keyed to the merge SHA, notification sent, loop
   stops, `master` green again. Then: `preflight` still refuses on the incident
   after `clear-halt`, and only `resolve-incident` clears it.
6. The same seeded merge, but with a gate arranged to leave a tracked file
   modified while every gate returns 0 — the 2026-09-14 shape. Expected:
   **nothing reverted and nothing pushed**, `origin/master` unmoved, HALT
   present, `agent-gates-green-unverified` and NO `agent-regressed`, an `open`
   incident, loop stops — and the dirty file left exactly as found IN THE
   RETAINED VERIFICATION WORKTREE, whose path the payload names in
   `halt.retained_worktree` and whose existence `verify_worktree_removed: false`
   reports. Check it there, not in the shared checkout: the gates never ran in
   the shared checkout, and this scenario could not pass as written while that
   tree was still being torn down. Then `cleanup --worktree <that path>` removes
   it, with no `--branch` (it is detached). The same expectation with a gate
   killed at its timeout instead, whose halt reason is `gate-timed-out`, whose
   gate is named in `timeouts`, and whose tree IS destroyed — a killed gate
   leaves a partial build, not evidence about a file.
