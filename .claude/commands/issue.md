---
description: The per-issue pipeline — worktree, classify, probes, spec, two-model spec panel, acceptance tests, plan, TDD implementation with discrimination proofs, repo gates, two-model final panel, PR, gated squash-merge. Called by /orchestrate; standalone use requires a claim first.
---

Take GitHub issue `$ARGUMENTS` (a number) from claimed to merged, per
`docs/superpowers/specs/2026-09-13-issue-orchestrator-design.md`. Requires an
existing claim (`python scripts/agentflow status` shows a lock for this issue and
`AGENTFLOW_RUN_ID` is set). Standalone use: run `claim` first, exactly as
`/orchestrate` step 6 does. Never set `AGENTFLOW_NOW` — it exists only to freeze
the executor's clock for tests. Any executor call that returns JSON with an
`error` key is a stop: print it and return `blocked <error>`.

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
or `spike-answered`. Any cap hit (`charge` prints `exhausted`) is
`blocked <cap-name>`.

## Steps

1. **Worktree.** From the main checkout:
   `git worktree add <worktree> -b <branch> master`. In every command run inside
   the worktree, export `TREE_SITTER_AL_PATH=<main checkout>/tree-sitter-al`. Create
   `.agent/issue-N/` there and start the ledger with the claim JSON and a hash of
   the issue body (`body_hash` from the claim).
2. **Classify** with the brainstorming skill's three-path rule (spike / bounded /
   architectural) and write the classification and reason to the ledger.
   - Spike: run the probe (read-only; a throwaway script under `.agent/runs/` if
     needed). Write the full answer under `## Answer` in the ledger and return
     `spike-answered`. Do not comment on the issue yourself; the orchestrator
     posts the answer and the label through `finish --outcome answered`.
   - Bounded: a short design (a few paragraphs) in the ledger under `## Design`.
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
5. **Spec panel.** `charge spec_rounds` per round (cap 3). Follow the
   `panel-review` skill: a briefing file with absolute paths and a
   confirm/reject checklist; both reviewers in parallel. Maintain
   `findings.json` as a list of `{id, source, round, file, line, severity,
   text, disposition: open|fixed|refuted|deferred, evidence, hash,
   reviews: {astra: accepted|re-raised|unreviewed, flash: …}}`. Each later round
   sends the spec diff plus the register and asks each reviewer to mark every
   entry `accepted` or `re-raised`. Converged when: no `open`; every entry
   `accepted` by both against the current hash; every blocking entry is `fixed`
   or `refuted` with evidence. A `re-raised` stays unresolved even without new
   evidence. Not converged after 3 rounds: return `blocked spec-panel-cap`.
6. **Acceptance tests.** Dispatch an `opus` subagent with the acceptance matrix,
   the fixture conventions from CLAUDE.md "Adding New AL Constructs" and
   "Testing Philosophy & Goldens", and the rule that a new golden family needs a
   seed file. Tests must compile and fail for the stated reason; capture the
   failing output in the ledger. Do NOT commit them alone.
7. **Plan.** `superpowers:writing-plans` to
   `docs/superpowers/plans/<today>-issue-N-<slug>.md`. Count tasks; more than 12:
   return `blocked plan-too-large` after commenting "split this issue" in the
   ledger.
8. **Implement.** `superpowers:subagent-driven-development`. Per task:
   implementer `opus`, reviewer `sonnet`, review-fix `sonnet`. Every task prompt
   includes: TDD red then green; `discrimination-proof` for every new or changed
   test (record test, mutation patch, fail output, pass output, commit in the
   ledger's proof table); `rustfmt <file>` only; SOLID and DRY as review
   criteria; the protected-path list (`.github/ scripts/ .claude/ CLAUDE.md
   .gitignore Cargo.toml version tree-sitter-al .agent/`). `charge subagents`
   per dispatch; `charge task_attempts --sub <task-id>` per red-to-green attempt.
   Before each task's commit: `python scripts/agentflow check-diff --base master
   --head HEAD --issue N --cwd <worktree>`; any reason is `blocked <reason>`.
9. **Repo gates**, from the worktree, each through the supervisor:
   `python scripts/agentflow run --name ci-steps-all --timeout 45 --cwd <worktree> -- bash scripts/ci-steps all`
   `python scripts/agentflow run --name check-goldens-coverage --timeout 5 --cwd <worktree> -- bash scripts/check-goldens --verify-coverage`
   `python scripts/agentflow run --name check-goldens --timeout 45 --cwd <worktree> -- bash scripts/check-goldens`
   then `git status --porcelain` in the worktree must be empty. Each `run` result
   carries `"supervised": true|false`; it must be `true` here (this run holds the
   claim's lock) — `false` means the lock was lost or never held and the gate did
   not run under supervision, which is `blocked lock-lost`, not a passed gate. A
   moved golden: dispatch `golden-diff-triager`; only when every line is explained
   run
   `python scripts/agentflow run --name check-goldens-regen --timeout 45 --cwd <worktree> -- bash scripts/check-goldens --regen`
   and commit the regenerated files with the triage summary in the message;
   an unexplained line is a discovery and the golden is not blessed. If the diff
   is not docs-only:
   `python scripts/agentflow run --name cdo-gate --timeout 45 --cwd <worktree> -- bash scripts/cdo-gate`
   and the north-star numbers in CLAUDE.md "Resolution Coverage" must hold. A new
   DEFAULT detector: run `/triage-wave` first; above 30% false positives it ships
   opt-in. Add the CHANGELOG entry under `## [Unreleased]`. Record every exit
   code and log path in the ledger.
10. **Final panel.** Run the `code-review` skill at high on `master..HEAD`; its
    findings enter `findings.json`. Then both reviewers over the diff, spec, and
    ledger, `charge final_rounds` per round (cap 3), same convergence rule.
    Source-verify every reviewer code claim before editing. Every fix returns to
    step 9's gates.
11. **Rebase and re-gate.** `git fetch origin && git rebase origin/master` in the
    worktree. Conflicts: resolve once (`charge rebase_regate`), else `blocked
    rebase-conflict`. After ANY rebase rerun step 9. If `git diff <old H> HEAD`
    on non-evidence files is non-empty, run one more final-panel round. Record
    `B = origin/master` and `H = HEAD` in the ledger.
12. **Freeze.** Commit the ledger and `findings.json` (first
    `python scripts/agentflow sanitize .agent/issue-N/ledger.md .agent/issue-N/findings.json`;
    a violation is `blocked sanitize-failed`). Then
    `python scripts/agentflow freeze-check --H <H> --issue N --cwd <worktree>` must be
    empty. `python scripts/agentflow attest --issue N --B <B> --H <H> --final-head
    <HEAD> --register <worktree>/.agent/issue-N/findings.json --gates '<json of
    step 9 exit codes>' --body-hash <claim body_hash>` — `attest` cross-checks
    `--body-hash` against this run's `claim.json` and refuses (`error`: body-hash
    mismatch) if the issue body changed since claim, so pass the claim's
    `body_hash` verbatim, never a freshly recomputed one.
13. **PR and merge.** Push the branch (`git push -u origin <branch>`). Create the
    PR with the sanitized ledger as body, title `<issue title> (#N)`, and
    `Closes #N` ONLY if every acceptance-matrix row is met; otherwise return
    `blocked acceptance-unmet` (no PR). Poll `gh pr checks <pr> --watch` through
    the supervisor (`run --name ci-wait --timeout 45`). CI red: one fix
    (`charge ci_fix`), then steps 9–12 again with a new attestation. Then
    `python scripts/agentflow merge-gate --pr <pr>`; `base-moved` means one more
    step 11 (`charge rebase_regate`); any other reason is `blocked <reason>`.
    Finally `python scripts/agentflow merge --pr <pr>`, which returns
    `{"merge_sha": …}` only once the gate has passed a second time internally —
    a refused merge instead returns `{"reasons": […]}` with exit 1, which is
    `blocked <reasons joined>`. On success, post the attestation JSON as a PR
    comment via `gh pr comment`. Print `merged <merge_sha>`.

## Ledger sections (in this order)

Claim; Classification; Assumption probes; Base/Head SHAs (B, H per round);
Timeline; Acceptance matrix; Discrimination proofs; Findings register summary;
Gate results (exit code, log, test counts, CDO numbers, toolchain and grammar
commit, CDO_WS identity as a hash); Discoveries (in the exact JSON shape
`/orchestrate` step 9 files); Not delivered.
