# Issue orchestrator — design

Date: 2026-09-13
Status: approved in chat, awaiting written review
Owner: SShadowS

## Goal

A reusable, fully autonomous flow that takes GitHub issues on `SShadowS/al-sem` from
open to merged, one at a time, with no human gate. It picks its own issues, files new
issues it discovers, and stops on its own when the queue is empty or a cap is hit.

The flow is spec-driven (SDD), test-driven (TDD, red then green, with a discrimination
proof on every test), and reviewed by two independent models at the spec stage and
again before merge. All repo gates (tests, clippy, goldens, CDO ratchets, CHANGELOG)
must be green before a merge.

## Non-goals

- Parallel issues. One issue at a time on this machine (disk space, shared goldens
  gate, shared `target/`).
- Cloud or GitHub Actions runners. The flow depends on pi (Copilot billing), `CDO_WS`,
  and the local tree-sitter/goldens setup, none of which exist off this machine.
- The orchestrator changing priority labels or closing issues by judgment. Issues close
  only through a merged PR carrying `Closes #N`.
- Human review gates. Approval is replaced by two-model convergence plus the gates.

## Shape

Two project slash commands in `.claude/commands/` (versioned, like `/triage-wave`),
composed from skills that already exist:

| Command | Role |
|---------|------|
| `/orchestrate [--dry-run] [--max-issues N]` | One tick: preflight, fetch, rank, pick, claim, run `/issue N`, post-merge check, file discoveries, reschedule. Re-fired by `/loop`. |
| `/issue N` | The per-issue pipeline: worktree, classify, acceptance tests, spec, spec panel, plan, implement, gates, final panel, PR, merge. |

Repo-specific gates are referenced by CLAUDE.md section, not copied, so porting the
flow to another repo means copying the two files and editing the gate list.

Skills used: `superpowers:brainstorming` (classification), `superpowers:writing-plans`,
`superpowers:subagent-driven-development`, `superpowers:test-driven-development`,
`discrimination-proof`, `panel-review` (protocol, with the roster changed to
`gpt-5.6-sol` + `gemini-3.8-flash`), `code-review`, `golden-diff-triager` (agent).

## Model routing

| Task | Model | Why |
|------|-------|-----|
| Ranking, dedupe, classification hints | haiku 4.5 subagent | Cheap; the expensive models never see the whole issue list |
| Spec and plan authoring | conductor (fable 5.1) | Holds the design context |
| Spec review, final review | `gpt-5.6-sol` + `gemini-3.8-flash` via pi, thinking high | Uncorrelated families; `require_evidence` on |
| Acceptance tests, implementation | opus 5 subagents | Strongest coder available in-harness |
| Per-task review, review-fix edits | sonnet 5 subagents | Fast, adequate for bounded edits |
| Diff review before the final panel | `code-review` skill at high | Repo-aware findings, ranked |

## `/orchestrate` — one tick

1. **Preflight.** Working tree clean on `master`; `master` equals `origin/master`;
   `pi_models` reachable; `CDO_WS` set and the directory exists. Any failure stops the
   tick with the reason printed. No worktree is created before preflight passes.
2. **Fetch.** `gh issue list --state open --json number,title,body,labels,updatedAt`.
   Drop `agent-blocked` and `agent-working`. A stale `agent-working` (older than the
   per-issue wall-clock cap) means a crashed run: comment on the issue, reset the label,
   remove the orphan worktree, then treat it as eligible.
3. **Rank.** One haiku subagent receives the eligible issues and the rubric below and
   returns ranked JSON. Written to `.agent/orchestrate/<timestamp>-ranking.json` and
   echoed to the terminal.
4. **Dry run stops here** and prints the ranking and the pick.
5. **Claim.** Label the pick `agent-working`; comment with the session link and the
   classification.
6. **Run `/issue N`.** Returns `merged`, `blocked`, or `spike-answered`.
7. **Post-merge check.** On `merged`: `git pull` on `master`, then `cargo test` and
   `scripts/ci-steps clippy` once. Red means label the issue `agent-regressed`, file a
   bug with the failing output, and stop the loop.
8. **File discoveries** (see below); relabel `agent-done` or `agent-blocked`; remove
   the worktree (`rm -rf` + `git worktree prune`, since `git worktree remove` fails on
   trees with submodules).
9. **Reschedule.** Under `/loop`, the next tick fires when the queue is non-empty and
   `--max-issues` is not exhausted. An empty queue ends the loop.

Labels `agent-working`, `agent-done`, `agent-blocked`, `agent-regressed`, `agent-filed`
are created on first run if missing.

### Ranking rubric

Per issue: value 1–3, effort S/M/L, depends-on (issue numbers named in the body),
blast radius (subsystems named), classification spike / bounded / architectural. Sort
by value desc, then effort asc, then age. An issue whose named dependency is still
open is skipped this tick. `agent-filed` issues rank like any other.

## `/issue N` — the pipeline

Everything writes to `.agent/issue-N/ledger.md` inside the worktree. The ledger is
committed and becomes the PR body.

1. **Worktree.** `git worktree add ../al-sem-issue-N -b issue/N-<slug> master`, then
   `git submodule update --init` inside it. Fails fast if the submodule is missing.
2. **Classify.** The brainstorming skill's three-path rule.
   - Spike: run the probe, comment the answer on the issue, return `spike-answered`.
     No PR.
   - Bounded: short design in the ledger; one reviewer (`gpt-5.6-sol`) for the design.
   - Architectural: full spec and two-model panel.
3. **Acceptance to tests first.** An opus subagent turns the issue's Acceptance section
   into failing tests (fixtures under `tests/fixtures/` or the relevant golden family,
   with a seed golden where the family requires one). Tests must compile and fail. This
   is the first commit, so the spec is reviewed against runnable acceptance.
4. **Spec.** `docs/superpowers/specs/YYYY-MM-DD-issue-N-<slug>-design.md`, with two
   required sections: "False-positive and failure shapes" and "Measurement plan".
5. **Spec panel.** `panel-review` protocol: briefing file with absolute paths, both
   models in parallel, `require_evidence` on, `output_file` per model. Cap 3 rounds via
   `continuation_id`. Converged means both say ready, or every residual is marked
   non-blocking by both. Not converged after 3 rounds means `blocked`.
6. **Plan.** `writing-plans` skill writes `docs/superpowers/plans/…`.
7. **Implement.** `subagent-driven-development`. Every task prompt carries: TDD red then
   green; `discrimination-proof` on every new or changed test with both outcomes
   recorded in the ledger; `rustfmt <file>` only, never `cargo fmt`; SOLID and DRY as
   explicit per-task review criteria. Implementer opus 5, per-task reviewer sonnet 5,
   review-fix edits sonnet 5. Cap 3 red-to-green attempts per task.
8. **Repo gates**, in order, each redirected to a log file and grepped (never piped
   through `tail`): `cargo test`; `scripts/ci-steps clippy`; `scripts/check-goldens`.
   Goldens moved means `golden-diff-triager` runs and every changed line must classify
   as explained before a regen is accepted; an unexplained line is a discovery and the
   golden is not blessed. Touching `src/program/resolve/`, `crates/al-syntax/`, or
   `src/snapshot/` adds `scripts/cdo-gate` and the north-star numbers in CLAUDE.md
   "Resolution Coverage" must hold. A CHANGELOG entry is required.
9. **Final panel.** `code-review` skill at high on the diff; then `gpt-5.6-sol` and
   `gemini-3.8-flash` in parallel over the diff, spec, and ledger. Same cap and
   convergence rule. Every reviewer claim about code is source-verified before an
   edit is made.
10. **PR and merge.** `gh pr create` with the ledger as body and `Closes #N`; wait for
    CI green; `gh pr merge --squash`. Return `merged`. If `master` moved during the
    run: rebase once, rerun the gates, then merge or `blocked`. CI red after one fix
    attempt means `blocked`.

### Caps

| Cap | Value | On hit |
|-----|-------|--------|
| Spec panel rounds | 3 | `blocked` |
| Red-to-green attempts per task | 3 | `blocked` |
| Final panel rounds | 3 | `blocked` |
| CI fix attempts | 1 | `blocked` |
| Wall-clock per issue | 4 h | `blocked` |

`blocked` means: comment on the issue with the ledger, label `agent-blocked`, leave the
branch and worktree in place for a human. A golden is never rebaselined to reach green.

## Discoveries

Appended to the ledger's `## Discoveries` during any phase, never acted on mid-issue:

- A defect with a reproducer: a failing test shape, a command with its output, or a
  file and line plus the wrong behaviour.
- A gap the issue's Acceptance needs but the spec excluded, with the reason.
- A gate finding the issue did not cause: a flaky test, a stale doc claim, a golden
  line the triager classed unexplained.

Not filed: style nits, refactors without a defect, reviewer suggestions without
evidence.

At the end of the tick a haiku subagent dedupes each discovery against open issues
(`gh issue list --search` on title and body) and files survivors with `gh issue create`,
labeled `agent-filed` plus `bug` or `enhancement`. The body carries the reproducer, the
origin issue number, and the session link. Filed issues enter ranking on the next tick.

## Ledger

`.agent/issue-N/ledger.md`, committed, becomes the PR body. Sections:

- Classification and why.
- Timeline: one line per phase with outcome and cap counters.
- Review file paths (`pi-sol-*.md`, `pi-flash-*.md`) with each round's verdict.
- Discrimination-proof table: test, break applied, fail seen, pass seen.
- Gate results with log paths; CDO numbers when that gate ran.
- Discoveries.
- What the issue asked for and was not delivered, with the reason.

`.agent/` is gitignored except `.agent/*/ledger.md`, so pi review files and gate logs
stay local.

## Failure handling

- Crashed session mid-issue: handled by the stale `agent-working` rule in the next tick.
- pi unreachable mid-panel: `pi_cleanup`, retry once, then `blocked`. The flow never
  silently downgrades to one reviewer.
- Auto-merge with a moved `master`: rebase once, gates rerun, then merge or `blocked`.
- Red `master` after merge: `agent-regressed`, a filed bug, loop stops.

## Repo changes

- `.claude/commands/orchestrate.md`, `.claude/commands/issue.md`.
- `.claude/commands/README.md` (new): one entry per command, plus `/triage-wave`.
- `.gitignore`: `.agent/**` except `.agent/*/ledger.md`.
- CLAUDE.md: one paragraph recording the auto-merge exception for this flow and the
  exact conditions (all gates green, both final reviewers converged, CI green).
- CHANGELOG: Added entry.

## Proving the flow

1. `/orchestrate --dry-run` on the current open issues. Expected: a ranking with
   classifications and one pick; nothing else touched.
2. A seeded bounded smoke issue (a known one-line gap with an Acceptance block) run
   end to end. Expected: a merged PR, a ledger with a discrimination proof, and the
   label transitions `agent-working` → `agent-done`.
3. A deliberately unsatisfiable issue. Expected: `agent-blocked`, a ledger comment,
   branch left in place, and the next tick skips it.
