# al-sem: handoff to the kraken-alsem container

Written 2026-11-10, at the end of the session that worked on al-sem on the Linux desktop. Master is at the commit
that added this file. **If you read this, you have no memory of that session. This note, `CLAUDE.md` and
`docs/engine-limits.md` are what you have.** Read `CLAUDE.md` first.

CDO names in this repo are fine: the owner confirmed on 2026-11-10 that CDO object, routine and file names may be
pushed to this public repo. Keep secrets, credentials and other private data out.

## 1. What is finished

- **Output validation, rounds 1–4**, accepted by both reviewers.
- **Engine limits waves 0, 1 and 2** (see `docs/engine-limits.md` and `CHANGELOG.md`):
  - Wave 0: the prove-filter crash (B1), isolated events (B3), small wrong outputs (B8).
  - Wave 1: `scripts/cdo-gate` builds once and runs in two lanes; a speed and memory check, `scripts/cdo-speed` (L14).
  - Wave 2 (master `4cf1910c`): the remaining wrong answers.
    - B4/L16: `[CommitBehavior]`.
    - B7/L23/L30: traps in the forward error answers.
    - B5/L19: FlowField reads.
    - B6/L20: the platform's report, query and XmlPort dataset operations.
    - B2: `#if` arms are alternatives, each with its own site ids.
    - One routine or object declared in several places (`#if` arms or files) is treated as alternative builds; a subscriber whose declarations subscribe differently becomes one routine per declaration.
    - An `[EventSubscriber]` that cannot be read gives `unknown`, never `no`.
- **tree-sitter-al 5.0.0** (master `658d5723`, grammar `main` at `18c988e8`). The old `lower_case_body` fallback is gone. No golden moved, and CDO parse and unknown numbers were unchanged.
- **Branches.** `continai-gaps-pre366` and `proto/dep-callers` are pushed as they were, not merged. Nothing else has unlanded commits.
- **`docs/Handoff/`** old files were checked, found applied (the intent-evidence spec and skill), and deleted.

## 2. What is next, in order

Start nothing without the owner's go-ahead. These are what is on the list.

1. **Crate split.** Design: `docs/superpowers/specs/2026-11-10-crate-split-design.md`. The owner wants this started in a new session in this container.
   - It is a design only. Nothing has been built or measured.
   - Phase 0 (baseline) and Phase 1 (cheap wins) come first and can stop the work; Phase 1 costs about 2 days.
   - Section 10 lists four owner decisions still open.
   - **Fable (Agent, model `fable`) and gpt-6-astra (`mcp__pi__pi_ask`) review it again before Phase 2 and review each phase's diff.** The owner asked for this.
   - The owner also asked for the design document to be copied into this container. It arrives here through git at the path above. A copy also sits at `/home/sshadows/al-sem-crate-split-design.md` on the desktop.
2. **Outside consumers update.** See section 3.
3. **Engine limits waves 3–8.** `docs/engine-limits.md` section 3 gives the order. Waves need the owner's go-ahead.
4. **Wrong answers still open.** See section 4. Decide with the owner when they go in.

## 3. Outside consumers (pending, owner's request)

Programs that read `alsem` output must be updated. The old shapes are kept behind `schema:` for one release only.

- **ConTinAI** `work/scripts/effects.py`: reads query rows' `via`, `origin.routine.*`, `origin.anchor.{file,line}`, `isUnknownBucket`, `tempState == "known(true)"`.
- **bc-dev-mcp** `risk.ts`, `alsem-writes.ts`: reads digest 2.x, pinned to alsem 1.7.1. It must join root refs with `payload.effects` / `crossings` / `pathTails` when the pin moves. It reads `alsem graph`, and should read the new optional `otherDeclarations` (until then it falls back by name).
- **al-lsp-for-agents**: parses none of the documents, but display strings changed; tell its owner.
- Also check anything reading prove or explain JSON fields added in V2–V4 (`pathTier`, `followUps`, `result.scope`, `role`, `qualifiedDisplay`, `selector`) and the MCP envelope (`outcome`, `index`, `summaryTruncated`).
- The moved fields are listed in `CHANGELOG.md`: the query and digest 5.0.0 consumer notices, and **"Outside consumers (wave 2)"** at the top of Unreleased, which is the current list for wave 2.
- Offer still open from the last session: file this as a GitHub issue.

## 4. Known open items

**Wrong answers found and written down, not fixed** (`docs/engine-limits.md`):
- B9: `FieldRef.Validate` does not raise a table's validate events.
- In L20: a write in a report's request-page `OnOpenPage` answers `no` through `Report.Run`; an XmlPort import's existing-record error is not an error fact.
- L16 item 2: a checked `Codeunit.Run` with no explicit `Commit()` answers `may-commit no`.
- L23 item 6: `DisableWriteInsideTryFunctions`.
- L28: a checked run with a write pending is itself a run-time error, and the run's own error is not an error site.
- L35, L36: `GetBySystemId` and record array elements produce no record operation. Not yet in a wave.

**Owner decision pending:** L34. Seven test-library apps are missing from the pinned baseline's `Test/.alpackages`, so every CDO `alsem tests` answer is incomplete.

**Verification gaps:** `docs/engine-limits.md` section 4.

**Worth watching:**
- Wave 2 raised model-build CPU to about 1.20× (speed check, 2026-11-10; limit 1.30). It is real, and within the limit. Prove and memory stayed close to flat.
- `scripts/cdo-speed` builds the merge base's binary. **If grammar `main` moves after that commit, the build stops on the hash check and the speed phase fails with exit 1.** That happened on the grammar upgrade branch. The fix then was to pass `--base-exe <a binary of the previous code>`. Now that master builds on the new grammar, this only returns at the next grammar bump.
- `scripts/cdo-speed` reads `/proc`, so it is Linux only; `cdo-gate` needs `--no-speed` on Windows.
- A SIGBUS was seen twice in the session tests under plain `cargo test`. It did not reproduce on master or the branch, and it fits a test binary being overwritten by a concurrent build. Unconfirmed.

**Earlier offer, still open:** an artifact page of the output-validation results (it needed a login to another org or a new link).

## 5. Working rules from the last session (still in force)

Everything else is in `CLAUDE.md`, which applies as written.

- **Landing:** only `scripts/merge-and-push <branch>`, from the main checkout on master, which must be clean. Do not improvise `git push` lines. If it refuses, report why. Never push or merge to master without a request.
- **Gates once per landing:** branches run `scripts/ci-steps task` and `scripts/check-goldens --verify-coverage`. `scripts/ci-steps all` and `scripts/cdo-gate` run once, on the exact tree about to merge.
- **Re-mint CDO goldens with plain cargo test**, not nextest: `REGEN_TEMP_GOLDENS=1 CDO_WS=<ws> cargo test --profile cdo --test cli output_cdo::`. Set `CDO_WS` for that one command only. Read every diff. Explain every moved pin; on wave 2, a switch-off experiment proved each cause.
- **Reinstall the MCP binary once per batch of merges**, after the last landing, and before recording a validation round. Recipe: a detached worktree of the commit; `CARGO_TARGET_DIR=<a stable dir> cargo build --profile release-ship --bin alsem`; copy to `alsem.new` and `mv -f`.
- **Build throughput (owner's rules, 2026-11-10):**
  1. At most 2–3 agents building at once.
  2. A fix round reuses the branch's existing warm worktree. Reviewers probe with the implementer's built binary and do not build their own.
  3. Iterate with `scripts/ci-steps task-affected`, and run the full `task` gate once at the end.
  4. Nothing else builds or runs beside `scripts/cdo-gate`.
  5. Remove finished worktrees promptly.
- **Larger work:** use workflows, one worktree per implementing agent, after a spec is accepted. Keep heavy CDO-running agents to two or three at once.
- **Reviews:** independent review of every fix round found real wrong answers in every round of wave 2, so keep it. Give a reviewer the code and probes, not your conclusions.
- **Workflow subagents cannot write report files;** results come back through structured output only.
- **A gate's exit code:** when a background command ends with `echo`, the notification reports the echo's status. Read the printed text.
- **Never build uncommitted work into `target/release-fast` while a peer may be using it.** Use `CARGO_TARGET_DIR=target2`. Rebuild it only when a peer is idle.
- **Re-proving many claims one `alsem prove` at a time takes over an hour.** Run them in parallel (4–8 processes) or only re-prove the claims a fix touched.
- **Environment before any build or commit** (the pre-commit hook builds) on a worktree without its own grammar clone: `TREE_SITTER_AL_PATH=<main checkout>/tree-sitter-al`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## 6. Half-done work

None. Waves 0–2 and the grammar upgrade are landed and pushed. There are no open worktrees with changes.

On the desktop, the local branches `integrate/*`, `fix/w0*`, `fix/w1*`, `fix/w2*` and `chore/grammar-5` are merged and can be deleted. The worktree `../al-sem-w2` is clean and can be removed. The `target*` folders and build logs in the main checkout were left alone on purpose.
