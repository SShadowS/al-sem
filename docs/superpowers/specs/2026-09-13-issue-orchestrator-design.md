# Issue orchestrator — design

Date: 2026-09-13
Status: revision 4 — three review rounds with gpt-6-astra + gemini-3.8-flash (the cap); round 3: gemini READY, astra NOT READY with three blockers, all addressed here without a fourth round
Owner: SShadowS

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
  `manual-only`, `epic`, `meta`; body has an `## Acceptance` section (or the issue is
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
- **Regression halts.** An `agent-regressed` outcome writes `.agent/HALT` itself
  with the reason, so no later tick or second session resumes until a human looks.
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
  then `agent-done`. If that follow-through fails partway, the issue is labeled
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
7. **Post-merge check** (executor). On `merged`: `git fetch` and fast-forward the
   main checkout's `master` to `origin/master` (it must fast-forward; anything else
   is a preflight failure next tick), then check out the **merge SHA** itself
   (detached, so a moving `master` is not what gets tested), then `scripts/ci-steps
   all`, `scripts/check-goldens`, and `scripts/cdo-gate` unless the diff was docs-only.
   Red means, in this order: write `.agent/HALT` with the reason first (durable
   before any fallible remote action); if the main checkout's local `master` has
   diverged from `origin/master` (unpushed human work sitting on it), leave
   `master` untouched, comment with the reason (`master-not-ff`), and stop the loop
   without reverting — HALT is already set; otherwise create the revert commit
   locally (`git revert --no-edit <merge-sha>`); rerun on it the gate that failed plus
   `scripts/ci-steps test`; push it to `master` only if those pass and
   `origin/master` still equals the merge SHA (a push rejected because `master`
   advanced is reported, not forced); reopen the issue; label it `agent-regressed`;
   comment with the failing output and the revert SHA; file a bug if the failure is
   outside the issue's diff; notify; stop the loop. A revert that conflicts, fails
   its own gates, or cannot be pushed leaves `master` as is, posts exactly that,
   notifies, and stops the loop; HALT is already set.
8. **Cleanup, then finish** (executor). File discoveries (see below). Cleanup runs
   **before** finish: `cleanup` refuses to act if `lock.json` names a different
   run (the same fencing as every mutating command). On `merged` it removes the
   worktree after verifying the path is under the expected parent, the tree is
   clean, and the branch is merged into `master` (`rm -rf` with two retries for
   Windows file locks, then `git worktree prune`, then delete the local branch);
   on `blocked` the worktree stays; on `spike-answered` (the branch never carries
   a commit of its own) `cleanup --spike` removes the worktree and branch outright.
   Finish then relabels `agent-done` / `agent-blocked` / `agent-answered`, copies
   the run directory to retention, and releases the lock last.
9. **Reschedule.** Under `/loop`, the next tick fires when the eligible queue is
   non-empty and `--max-issues` is not exhausted. Empty queue, HALT, or
   `agent-regressed` ends the loop.

Labels `agent-working`, `agent-done`, `agent-blocked`, `agent-answered`,
`agent-regressed`, `agent-filed` are created on first run if missing.

### Ranking rubric

Per issue: value 1–3, effort S/M/L, blast radius (subsystems named), classification
spike / bounded / architectural, one-line reason. Sort by value desc, effort asc,
createdAt asc. The ranking is advisory: dependency and eligibility filtering already
happened deterministically, and the model cannot introduce an issue that was not in
its input.

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
    The only commits allowed after `H` are evidence commits touching nothing but
    the two `.agent/issue-N/` files; the executor asserts
    `git diff H..HEAD --name-only` is a subset of those two paths, so recording the
    approval cannot invalidate it. The committed ledger records `B` and `H` only;
    `final_head` (the evidence commit) lives solely in the attestation
    `{issue, B, H, final_head, register_hash, gates, body_hash}`, which the executor
    writes to the run directory and posts as a PR comment.
13. **PR and merge** (executor). `gh pr create` with the sanitized ledger as body
    and `Closes #N` only if the acceptance matrix is fully met. Required CI: the
    `ci.yml` workflow's jobs all `success`; a `skipped`, `cancelled`, or missing
    check is not green. One CI fix attempt allowed, independent of the rebase
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
cited file and line, severity as the reviewer stated it, the conductor's disposition
(`fixed` with commit SHA, `refuted` with source evidence, `deferred` as discovery
with the reproducer, or `open`), the artifact hash it was raised against, and a
per-reviewer disposition field (`accepted`, `re-raised`, `unreviewed`).

Each round's prompt shows every entry with its conductor disposition and asks each
reviewer to mark each one `accepted` or `re-raised` against the current hash. A
`re-raised` stays unresolved whether or not it cites new evidence: the conductor
must fix it or produce a new refutation for the next round. Silence is
`unreviewed`, never approval. A dispute that survives the round cap is `blocked`
for a human to arbitrate; the register never converts a rejection into approval
on its own (fail closed, at the cost of an occasional blocked-by-dispute issue).

Convergence means: no `open` entries; every entry is `accepted` by **both**
reviewers against the current hash (including the originating reviewer's own);
every entry a reviewer marked blocking is `fixed` or `refuted` with evidence, never
`deferred`. A reviewer downgrading its own blocking finding without a refutation is
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
- Red `master` after merge: validated revert, reopen, `agent-regressed`, HALT,
  notify, loop stops.
- HALT: terminal bookkeeping only, ledger written, `blocked` with reason `halted`.

## Repo changes

- `scripts/agentflow/` (package) and `scripts/agentflow/tests/` (fake `gh`, fault
  injection before and after every side effect, dry-run write-free assertion, lock
  races, stale recovery preserving the tree, sanitizer rejection, protected-path
  detection, freeze-boundary assertion, pending-discovery reconciliation, gitignore
  exception).
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
5. A seeded issue whose fix breaks a test only visible on `master` after merge.
   Expected: revert, reopen, `agent-regressed`, HALT present, notification sent,
   loop stops, `master` green again.
