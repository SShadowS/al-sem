# Issue #73 — `alsem graph` accepts an `app.json` without an `id`, every gate-backed leaf rejects it

## Claim

```json
{"run_id":"20261015-182358-9c58ab","issue":73,"attempt":1,"session":"claude-code-session:cc1781f3-fe4e-414a-aea6-4919dc8d314c","branch":"issue/73-agent-discovery-graph-snapshot-a1","worktree":"U:\\Git\\al-sem-issue-73-a1","body_hash":"6e92eaa90e3a7ec07a3937be540e8120667dd0fa669c84123e9a1905d755de7f"}
```

Issue body sha256 `6e92eaa9…de7f` (from the claim). Worktree from `master` @
`d382d58761c11efb9ade7ab5e93a437b1c6d1338`. Grammar `8682cb5f` via `TREE_SITTER_AL_PATH`.

Reviewer roster (operator instruction, 2026-10-15): `gpt-6.1-sol` fills the `astra` slot (`sol`)
and `claude-opus-5.5` fills the `flash` slot (`opus`); attested with `--substitute astra=sol
--substitute flash=opus`.

## Classification

**BOUNDED.** One identity rule, one entry point. The gate-backed leaves get the workspace identity
from `compute_gate_model_instance_id` (`src/engine/gate/model_instance_id.rs:75`), which requires
`source_text::read_root_app_guid` (`app.json` readable, JSON, with a non-empty string `id`) and
otherwise fails closed, so each leaf exits 1 with "could not compute modelInstanceId". `alsem graph`
goes through `impact_graph::export_workspace` (`src/program/impact_graph/mod.rs:191`) to the
snapshot builder, which reads the id with `unwrap_or("")` (`src/snapshot/snapshot.rs:199-210`) and
continues with `guid = ""`.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| P1 — the reproducer holds on `master` | **HOLDS** | release-fast `alsem` from `b77025e4` (graph and digest unchanged since); a copy of `tests/r0-corpus/ws-d8-commit-in-tx` with `app.json` = `{"name":"x"}`: `alsem graph` → exit 0, 6 JSONL lines; `alsem digest … --file src/posting.al` → exit 1, `digest: could not compute modelInstanceId` (`$SCRATCH/73/ws.*`) |
| P2 — an EMPTY string id behaves the same | yes: `"id": ""` → graph exit 0 (6 lines), digest exit 1 | `$SCRATCH/73/ws2.*` |
| P3 — the gate's rule is one function | `source_text::read_root_app_guid` (`src/source_text.rs:129-133`) is what the gate's `compute_gate_model_instance_id` uses (`model_instance_id.rs:29, 75-76`): missing file, unparseable JSON, a missing / non-string / empty `id` → `None`. (`engine/snapshot.rs:282` has a separate reader with the same rule and its own test, `read_root_app_guid_requires_nonempty_string_id`; corrected, R1-02) | source |
| P4 — graph's error path | `run_graph_cmd` prints `al-sem graph: {msg}` to stderr and exits 1 on `Err` | `src/bin/alsem.rs:709-717` |
| P5 — #64's S08 row | it lives ONLY in #64's attempt-1 draft spec, an UNTRACKED file in #64's worktree (`al-sem-issue-64-a1/docs/superpowers/specs/2026-10-14-issue-64-shared-toolbox-envelope-design.md:167`): `S08 | graph | 0 | ok | - | an app.json with no id is accepted (D3)`. Not on `master`, not in the issue body | `grep -rn S08` over the repo (none) and that worktree |

## Design

**Choice: `graph` rejects an id-less `app.json` the way the gate does** (the issue's first option).
The other choice, making the gate accept it, would give every gate leaf an empty app identity, which
the model instance id and every app-qualified routine id depend on. The fail-closed rule is the
project's existing identity contract.

(Implementation note, final round 1: the check reads the snapshot's own `workspace_app.guid`, which
is `""` under exactly the `read_root_app_guid` rule, instead of re-reading `app.json`, so the id
checked is the id exported, with no race; F1-02.)
Change: `impact_graph::export_workspace` checks the id IMMEDIATELY after the
`build_cross_app_program(...)?` call (R1-03, R2-01), before any projection work or stderr output
(such as the coverage warning at `mod.rs:261-267`), with `crate::source_text::read_root_app_guid(workspace_root)`, the rule the gate uses,
so there is one rule and no copy. A workspace whose `app.json` is missing or is not JSON fails in the
build first and keeps today's `read …` / `parse …` error, unchanged. On `None` after a good build
(the `id` is missing, empty or not a string) it returns
`Err(format!("could not compute modelInstanceId for workspace '{}': app.json has no non-empty string
id", workspace_root.display()))`. `run_graph_cmd` prints it as `al-sem graph: …` and exits 1, like
the gate leaves. A workspace with a valid id is unchanged: same output, and no golden moves
(`graph_export_golden` and the other graph tests use fixtures with ids).

CHANGELOG `### Fixed` text (R1-07): "`alsem graph` now rejects an `app.json` without a non-empty
string `id` (exit 1, `could not compute modelInstanceId`), like every gate-backed command; before, it
exported the graph with an empty app id and exited 0 (#73). A missing or unparseable `app.json` keeps
its existing error. The LSP and `aldump` are unchanged."

Not changed: the snapshot builder's `unwrap_or("")` (the LSP and `aldump` also use it, and they need
no gate identity); `aldump --graphify-export` (a developer dump, not a gate leaf).

S08 (R1-01, R1-04): #64's draft spec (untracked, in #64's worktree) already has a row for this exact
case, S43 (`CONFIGURATION_ERROR`, trigger `{"name":"x"}`), listing every leaf except `graph`. So the
update is: delete row S08; add `graph` to S43's Leaves; in §6.5, replace "it has no `id` check, so
graph accepts … #64 does not add the check" with "graph's id check (added by #73) maps to the same
variant as `read_root_app_guid_checked`'s id failure"; strike §7 item 6; and mark discovery D3
"fixed by #73". These edits were made in spec round 1; every edited line is listed in the
Timeline (R2-02). The durable record is THIS ledger, committed with the PR. The flow does
not comment on other issues (R1-05).

Tests (`tests/cli/cli_graph_export.rs`, driving `CARGO_BIN_EXE_alsem`):
- **T1** `graph_rejects_an_app_json_without_an_id`: tempdir copies of `ws-d8-commit-in-tx` with
  `app.json` = `{"name":"x"}`, with `"id": ""`, and with `"id": 5` (R1-06). Each gives exit 1, EMPTY
  stdout, and stderr starting `al-sem graph: could not compute modelInstanceId`. Mutation: delete the
  `read_root_app_guid` check → exit 0 with 6 lines → fails.
- **T2** `graph_and_digest_agree_on_workspace_identity` (a consistency check, R1-06): on the
  `{"name":"x"}` copy, `alsem digest` also exits 1 with `could not compute modelInstanceId`; and on
  the original fixture both exit 0 (precondition that the fixture is valid). It adds no
  discrimination beyond T1; it pins the agreement the issue asks for.
- **T3** `graph_keeps_its_errors_for_a_missing_or_unparseable_app_json` (R1-03): a copy with no
  `app.json` and a copy whose `app.json` is `not json`. Each exits 1; stderr contains
  `al-sem graph: snapshot build failed: read` (no `app.json`) or `al-sem graph: snapshot build failed: parse`
  (`not json`), as today, and does NOT contain `could not compute modelInstanceId` (R2-03). Mutation:
  move the id check before the build → fails.

## Base/Head SHAs

- Base: `d382d58761c11efb9ade7ab5e93a437b1c6d1338` (`master`)

## Timeline

- Claimed (attempt 1), worktree created; probes P1-P5; bounded design.
- Spec round 1 (sol + opus): sol 1 important + 1 minor; opus 2 important + 3 minor. All were
  source-checked. Fixed: the id check moved AFTER the build (T3 keeps the missing/unparseable
  errors); S08 → S43; P3 corrected; `"id": 5` added; T2 restated as a consistency check; CHANGELOG
  text written. Refuted: the request to comment on #64 (this flow may not comment on issues; this
  committed ledger is the durable record).
- #64's draft spec edited in #64's worktree
  (`al-sem-issue-64-a1/docs/superpowers/specs/2026-10-14-issue-64-shared-toolbox-envelope-design.md`,
  untracked there):
  - line 167, row S08 (`graph | 0 | ok | … no id is accepted (D3)`): deleted;
  - row S43: `graph` added to its Leaves;
  - §6.5 (was lines 527-528, "It has no `id` check … #64 does not add the check"): replaced by
    "Graph's `id` check (added by #73, after a successful build …) maps to the same variant as
    `read_root_app_guid_checked`'s `id` failure (S43; D3, fixed by #73)";
  - §7 item 6 (was lines 684-685): struck through, "No longer true: #73 made graph reject it …
    S08 is removed";
  - discovery D3 (line 698): "**Fixed by #73.**" appended;
  - §3 line 128: marked as a measurement from before #73.
- Spec round 2: sol accepted all 7, "ready"; opus accepted all 7, "ready", plus 3 minors (R2-01…R2-03),
  all fixed.
- Spec round 3 (`spec_rounds` 2 left): both accepted all 10, "ready". **SPEC CONVERGED.**
- Step 7: plan `docs/superpowers/plans/2026-10-15-issue-73-graph-app-json-id.md`, 1 task. Steps 6+8
  run as ONE opus dispatch (red, then green with proofs). Commit **`450e5459`**
  (`$SCRATCH/73/impl.md`).
- Task review (sonnet): approve (`$SCRATCH/73/review.md`).
- Final panel round 1 (sol + opus) plus built-in `code-review high`:
  - sol re-raised R1-05, and opus raised F1-01 (minor): the ledger is not in the reviewed commit. It
    is committed at freeze (step 12), before the PR; R1-05's refutation stands.
  - code-review found three things. Re-reading `app.json` after the build is a race and a second
    source of truth (F1-02/F1-03). The CHANGELOG over-claims (F1-04). T1's `starts_with` is
    brittle (F1-05).
  - Fixed in **`6b4d30ae`**: the check uses the snapshot's own `workspace_app.guid` (the exported
    value; same rule, `snapshot.rs:199-208`); CHANGELOG exact, including the slow-path and
    read/parse-error limits; T1 uses `contains`; T1 re-proved.
  - Deferred (minor): fail-fast before the build (F1-06) and the test-helper duplication (F1-07).
  - The gate run on `450e5459` was stopped before any verdict; gates rerun on `6b4d30ae`.
- Final panel round 2 (sol + opus, delta `450e5459..6b4d30ae`): all 17 register entries accepted by
  both, no new findings, both `VERDICT: ready` (`final-r2-sol.md`, `final-r2-opus.md`).
  `final_rounds` 3 left; `pi_calls` 10 left. (The plan's Architecture line still names
  `read_root_app_guid`; it is the plan as written before F1-02, and this ledger records the change.)

## Acceptance matrix

| Acceptance | proof |
|---|---|
| graph rejects an id-less `app.json` the way the gate does, the choice stated | Design (choice and reason); T1, T2; T3 keeps the other errors |
| a CHANGELOG entry | `### Fixed` entry |
| a test | T1, T2 |
| issue #64's S08 row updated | #64's draft spec (P5: it exists only there) edited as Design states (S08 deleted, graph added to S43, §6.5, §7 item 6, D3); every edited line listed in Timeline |

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| T1 `graph_rejects_an_app_json_without_an_id` | delete the `read_root_app_guid` check (`grep -c` = 0) | `cli_graph_export.rs:775: {"name":"x"}: exit … left: 0 right: 1` | ok | `450e5459` |
| T2 `graph_and_digest_agree_on_workspace_identity` (consistency) | same as T1 | `:812: graph on an id-less app.json … left: 0 right: 1` | ok | `450e5459` |
| T3 `graph_keeps_its_errors_for_a_missing_or_unparseable_app_json` | move the check BEFORE `build_cross_app_program` | `:831: None: stderr="al-sem graph: could not compute modelInstanceId…"` | ok | `450e5459` |
| T1 (re-proved after F1-02: the check now reads `ctx.snapshot().workspace_app.guid`) | `if false && ctx.snapshot()…` (`grep -c` = 1) | `cli_graph_export.rs:775` panicked, `0 passed; 1 failed` | `7 passed` | `6b4d30ae` |

RED on base (`$SCRATCH/73/impl.md`): T1 and T2 fail (graph exit 0 on an id-less `app.json`); T3 passes
on base by design. GREEN: `cli_graph_export::` 7 passed, `--test cli` 350 passed, `--lib` 2031
passed, clippy clean, no golden moved.

## Findings register summary

17 entries (R1-01…R1-07, R2-01…R2-03, F1-01…F1-07): 13 fixed, 2 refuted (R1-05, F1-01: the ledger
is the durable record, committed at freeze), 2 deferred minors (F1-06, F1-07). Every entry accepted
by sol and opus (final round 2).

## Gate results

All on H = `6b4d30ae573197effae02ee4232a0be55cd4f97c`, B = `d382d58761c11efb9ade7ab5e93a437b1c6d1338`
(`origin/master` unmoved; no rebase). Toolchain rustc 1.96.0; grammar `8682cb5f`; CDO_WS identity
sha256 `1e512deb11c28d513ade51d45a7e4cbd2f8a3fb6f145ba94387ec8e5b288bce7`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 894.5 | `logs/ci-steps-all/20261015T185520-0b2c11b2.log` |
| check-goldens-coverage | 0 | 0.8 | `logs/check-goldens-coverage/20261015T191017-a44df310.log` |
| check-goldens | 0 | 108.7 | `logs/check-goldens/20261015T191018-676fb2c2.log` |
| cdo-gate | 0 | 419.7 | `logs/cdo-gate/20261015T191208-745fcde6.log` |

All supervised. `git status --porcelain -- . ':!.agent'` empty after the gates.

## Discoveries

```json
[]
```

## Not delivered

- The snapshot builder still reads a missing id as `""` for the LSP and `aldump` (no gate identity needed there).
