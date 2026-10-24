# Issue #237 — `alsem diff`: a user rename `X→T` plus an auto pair `T→U`

## Claim

```json
{"run_id":"20261024-090424-e491a7","issue":237,"attempt":1,"branch":"issue/237-agent-discovery-gate-diff-rename-a1","worktree":"/work/al-sem-issue-237-a1","body_hash":"c21e2d5532a1f2e47b1bb5d69ddb6ed95a40de9650f7d4c3810b375f845dd835"}
```

Base: `830a150c` (master after #241). Issue body hash:
`c21e2d5532a1f2e47b1bb5d69ddb6ed95a40de9650f7d4c3810b375f845dd835`.
Hands off: `src/engine/l5/digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs` (#160).
None of them is touched by this design.

## Classification

**bounded.** All the code is in `src/engine/gate/diff/` (`renames.rs`, `indexes.rs`,
`mod.rs`), about 60 lines that build and apply one rename table. No id, schema or
snapshot change. The probes show that the issue's premise is wrong: neither option it
offers is correct, and today's one-hop output is the right one. The change is: state the
one-hop rule, add one diagnostic for the one case where two old routines still land on
one key (D1, folded in after spec round 1), and pin both with tests through the real
`alsem diff`. No finding changes and no golden moves (P18).

Terms used below:
- **user entry**: a line of the `--renames` overlay file, `old id → new id`.
- **auto entry**: a #156 pair. A routine only in old and a routine only in new, with the
  same object, name and ABI status, are treated as one routine whose signature changed.
- **normalize**: rewrite an OLD-side id through the rename table so its facts are
  compared with the NEW-side facts under the new id. Today it is one lookup (one hop).
- **live old id**: an id that has rows in the old snapshot.
- **coverage**: the snapshot's `coverage` records. The probe snapshots and the tests have
  none, so coverage is `unknown`, and under the CLI default `--coverage-policy loose` a
  lost capability is reported as kind `capability-lost-under-coverage`
  (`policy.rs:102-109`; `details.kind` stays `capability-lost`).

## Assumption probes

Run 2026-10-24 at base `830a150c`, debug `alsem` built in this worktree
(`TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al cargo build --bin alsem`). Scratch
inputs (not committed): `/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/r237`.
They are JSON snapshots made with `jq` from
`tests/cli-b-goldens/diff/snapshots/abi-signature-changed-old.snap.json`, with empty
`coverage`. Object `O = dd6a6b6i-…:Codeunit:50600`. Routines are member-less,
`id = O#sig`, public procedures unless stated:
- old: `X = O#aaaa` named `Alpha` (one capability fact, `insert` on table 50600), and
  `T = O#tttt` named `Beta`;
- new: `U = O#uuuu` named `Beta` (the same `insert` fact);
- overlay `{X: T}`.

Command: `alsem diff old.snap.json new.snap.json --deterministic --renames overlay.json --format json`.

| probe | result | evidence |
|---|---|---|
| P1 how the table is built | `run_diff_engine` (`mod.rs:158-166`): `build_rename_table(overlay)` makes the user entries (`auto: false`), and `renamesApplied` is counted then. Then `pair_signature_changes(old, new, &user_table)` adds auto entries into the SAME table (`auto: true`). `build_diff_indexes` uses that one table | `src/engine/gate/diff/mod.rs:158-166` |
| P2 how the table is applied | `indexes.rs:49-54` `normalize` is one lookup. It is applied to every OLD-side key: identities (display and origin), contract, schema, permission, event, capability and coverage facts, and `normalize_old` for ids read from fact bodies (`passes.rs:770,925,937`). NEW-side keys are never rewritten. `oldOriginalStableId` is set only for a user entry (`indexes.rs:103`). Every map is last-write-wins on its key | `src/engine/gate/diff/indexes.rs` |
| P3 what the auto pairing excludes | Old side: an id that is a user KEY (`renames.rs:173`), or that is also in new. New side: an id that is a user TARGET (`:178`, #156 FQ-05), or that is in the NORMALIZED old id set (`:167`). Nothing excludes an old id that is a user TARGET; that is the issue's shape | `src/engine/gate/diff/renames.rs:158-189` |
| P4 user-only chains | `build_rename_table` reports `A→B, B→C` as a `rename-overlay-chain` diagnostic and still applies one hop (al-sem parity, unit test `overlay_chain_diagnostic`). So user chains are never followed today | `renames.rs:73-89, 259-271` |
| P5 reproduce the issue shape (real CLI) | exit 0. Diagnostic `rename-overlay-stale` `not-in-new` for `T`. Findings: `procedure-removed` on `T` (display `Alpha`, `oldOriginalStableId` = `X`); `capability-lost-under-coverage` on `T` (display `Alpha`, origin `X`); `procedure-signature-changed` on `U` (old `tttt`, new `uuuu`, display `Beta`); `capability-gained-write` on `U`. `renamesApplied` 1. So `X`'s facts sit on `T`, as the issue says; old `T` is paired with `U` | `with.json` |
| P6 P5 with old `T` before `X` | Same five lines. The current output does not depend on snapshot order | `rev.json` |
| P7 every auto key is a live old id | An auto key comes from `old_rows` (`renames.rs:172`), so it always has rows in old. In the issue's shape the user target `T` is therefore ALWAYS a second, separate old routine | code read |
| P8 simulate following the chain (`X→T→U`) — approximate | Overlay `{X: U, T: U}`. This is NOT the exact chained table: there `T` would be `auto`, so `oldOriginalStableId` and `renamesApplied` differ (spec r1 O8). It shows the fact merge only. Result: one finding, `procedure-signature-changed` on `U`. `X` disappears: its contract row is overwritten by `T`'s, its `insert` fact is merged with `U`'s, and no finding names it. The real evidence for this option is proof (a)'s recorded run (acceptance matrix) | `trans.json` |
| P9 simulate refusing the auto pair | Old `T` renamed `Beta2` so it cannot pair (same table as the refusal, display aside). Result: `procedure-removed` on `T` WITHOUT `oldOriginalStableId` (display `Beta2`), `capability-lost-under-coverage` on `T` (also shown as `T`'s), `procedure-added` on `U`, `capability-gained-write` on `U`. `X`'s contract row is overwritten by `T`'s; `X`'s rename origin is lost. Strictly worse than P5 | `refuse.json` |
| P10 D1: the same collision with no auto entry | old `{X, T}`, new `{T}` (T unchanged), overlay `{X: T}`. `X` and `T` both normalize to `T`, no diagnostic. Old order X,T: only `capability-lost-under-coverage` on `T` (no origin); X's contract is silently dropped. Old order T,X: a false `procedure-signature-changed` on `T` (`aaaa`→`tttt`) with `oldOriginalStableId` `X`, plus `capability-lost-under-coverage` on `T` with origin `X`. The output depends on row order | `occ.json`, `occ2.json` |
| P11 tests and goldens on rename overlays | Unit: `renames.rs` tests `overlay_chain_diagnostic`, `overlay_overlap_diagnostic_targets_insertion_order`, `overlay_stale_diagnostic`, `identity_mapping_dropped`. Integration (`--test cli`): `cli_b_diff_differential.rs` `diff_rename_variant_from_overlay_byte_match` (composes both snapshots from `tests/fixtures/ws-diff-rename` and calls `run_diff_engine` + `format_diff` directly, `:153-186`; goldens `tests/cli-b-goldens/diff/rename-overlay.json`, `rename.renamed.json`, `rename.renamed.human.txt`), `issue156_a2d_user_overlay_wins_over_auto_pairing`, `issue156_fq05_user_target_never_auto_pairs` (both `run_diff_engine`); `cli_diff_strict.rs` `t9_gate_runs_before_rename_overlay_load`. **`t9` is the only test that passes `--renames` to the binary**, and it only checks the overlay is not loaded (spec r1 S3/O2) | `grep -rn "RenameOverlay::new\|--renames\|rename_overlay: Some" tests src` |
| P12 docs that state the limit | `CHANGELOG.md:1091-1093`, in the released `[2.0.1]` section, calls one-hop normalization a limit. The text describes the behaviour correctly. `.agent/issue-156/ledger.md:244` says the same | file read |
| P13 auto then user | old `A=O#aaaa` (`Beta`, `insert`), `B=O#bbbb` (`Gamma`); new `O#bbbb` (`Beta`, `insert`), `C=O#cccc` (`Gamma`); overlay `{B: C}`. Table: `B→C` user, `A→O#bbbb` auto. Here the auto TARGET `O#bbbb` is a user KEY. Result: `procedure-signature-changed` on `O#bbbb` (old `aaaa`, no origin) and on `C` (old `bbbb`, origin `B`); nothing else; `renamesApplied` 1 | `a3.json` |
| P14 user, auto, user (spec r1 S2/O5) | old `{X Alpha (insert), T Beta, B Gamma}`, new `{B=O#bbbb Beta, C Gamma}`, overlay `{X:T, B:C}`. Chain `X→T→B→C` with no `rename-overlay-chain` diagnostic (the user entries do not chain with each other). Result: stale `not-in-new` `T`; `procedure-removed` on `T` (origin `X`); `capability-lost-under-coverage` on `T` (origin `X`); `procedure-signature-changed` on `O#bbbb` (old `tttt`, no origin); `procedure-signature-changed` on `C` (old `bbbb`, origin `B`); `renamesApplied` 2. Each old routine keeps its own key | `s2.json` |
| P15 the swap (spec r1 O4) | `X` must be in BOTH snapshots: old `{X Alpha (insert), T Beta}`, new `{X=O#aaaa Beta}`, overlay `{X:T}`. Old `X` is a user key; new `X` is not in the normalized old set `{T}`, so old `T` auto-pairs to new `X`: a cycle `X→T` (user), `T→X` (auto). Result: stale `not-in-new` `T`; `procedure-removed` on `T` (origin `X`); `capability-lost-under-coverage` on `T` (origin `X`); `procedure-signature-changed` on `X` (old `tttt`); `renamesApplied` 1. No loop, no collision | `sw.json` |
| P16 D1 inside the issue's shape: ABI status differs | Issue shape, but new `U` is `local`. The pair does not form (`renames.rs:200`). `X` and `T` collide on `T`. X first: `procedure-removed` on `T` display `Beta`, no origin; T first: display `Alpha`, origin `X`. Both: stale `T`, `capability-lost-under-coverage` on `T`. Order-dependent, no diagnostic | `abi1.json`, `abi2.json` |
| P17 D1 inside the issue's shape: ambiguous group | old `{X Alpha (insert), T Beta, T2=O#t2t2 Beta}`, new `{U Beta}`. Two old `Beta`s → no pair. `X` and `T` collide on `T`; X first: `procedure-removed` `T` display `Beta`, no origin; X last: display `Alpha`, origin `X`. Plus `procedure-removed` `T2`, `procedure-added` `U`, `capability-gained-write` `U`, `capability-lost-under-coverage` `T`. Order-dependent, no diagnostic | `amb1.json`, `amb2.json` |
| P18 does any golden have a live overlay target? | The only overlay golden is `rename.renamed.json` (overlay `rename-overlay.json`: `Codeunit:50100→50200` and its routine). Old side `ws-diff-rename/a` declares only codeunit 50100 (`a/src/sales-post.al:1`); 50200 is only in `b`. Its diagnostics array is empty today. No other golden passes an overlay. A2d (`ux→gone`) and FQ-05 (`x→t`, `x` not in old) have no live target either. So the new diagnostic fires in none of them | `grep -rn "50100\|50200" tests/fixtures/ws-diff-rename/*/src`; `jq .payload.diagnostics rename.renamed.json` |

### Which option is right (Q1)

**Neither. Keep one hop; make it a stated rule, and pin it.** Both reviewers accepted
this in spec round 1.

- A rename entry maps an OLD id to a NEW id. The two sides are different id spaces. In the
  shape `X→T` (user) plus `T→U` (auto), the `T` the user wrote is a new-side id, and the
  `T` the auto entry starts from is a live old routine (P7).
- **Following the chain** puts `X` and old `T` on `U`. `X` vanishes from the report (P8,
  approximate; proof (a) is the exact check).
- **Refusing** the auto pair puts `X` and old `T` together on `T`: `X`'s origin is lost,
  its contract is overwritten, and `U` becomes an unrelated `procedure-added` (P9).
- **Today** every old routine keeps its own key (P5, P6, P13, P14, P15). This is the rule
  #156 A2d already pins: "an old id the overlay maps to a target missing from new stays
  a removal".

## Design

### Goal

One principle for the rename table: **no entry may put two live old routines on one
key.** Under it:
- normalization is one hop, for user and auto entries alike. An entry's target is a
  new-side id and is never looked up again as an old id;
- a user key never auto-pairs (`renames.rs:173`), and a user target never auto-pairs on
  the new side (`:178`): either would add a second routine to a key a user entry fills;
- an old routine that IS a user target may auto-pair. That moves it off the key the user
  entry fills, which is what keeps the issue's shape collision-free (P5);
- when the inputs still put two live old routines on one key (D1: P10, P16, P17), the
  report says so with a new diagnostic instead of staying silent.

The rule is written in the code, stated in CHANGELOG, and pinned by tests that run the
real `alsem diff` binary, each with a discrimination proof.

### Non-goals

- No chain-following and no refusal of the auto pair (P8, P9).
- No change to user-only chain or overlap handling (al-sem parity, P4).
- No change to which rows win on a collision: the maps stay last-write-wins. The new
  diagnostic announces the collision; it does not reorder or drop rows. That is the
  smallest honest fix: making the winner order-independent would need a rule for which
  routine's facts are "right", and the inputs do not say.
- No change to any finding, the output shape, `renamesApplied` or any golden.

### The new diagnostic

- **Code:** `rename-overlay-collision`.
- **Fields, in this order:** `kind` (`"rename-overlay-collision"`), `from` (the entry's
  old id), `to` (its target). Same shape as `rename-overlay-chain`. JSON:
  `{"kind":"rename-overlay-collision","from":"<X>","to":"<T>"}`. Human output prints it
  through the existing generic line, `  rename-overlay-collision: {…}`
  (`format.rs:40-43`); no formatter change.
- **When it fires:** in `validate_overlay_against_snapshots` (`renames.rs:115-130`),
  which gets the full table (user and auto) and the live old id set. For each entry
  `old → new`, in table order, after that entry's stale checks: fire when `old` is a
  live old id, `new` is a live old id, and `new` is not itself a table key (nothing maps
  that routine away). Then `old`'s facts and the unmapped old `new`'s facts share the
  key `new`.
- **Why this condition is the whole of "two old routines on one key" under one hop.**
  Two live old ids share a key only if (i) two entries have the same target, or (ii) an
  entry's target is a live old id that no entry maps away. (i) between user entries is
  already `rename-overlay-overlap`; an auto target is never a user target (`:178`) and
  two auto entries never share a target (one per group). So (ii) is the only silent
  case, and it is what the condition checks. An auto entry cannot meet it (its target is
  not in the normalized old set, `:178`), so in practice it fires for user entries only;
  the check reads the full table anyway, so it stays true if pairing changes.
- **Report:** findings unchanged (still last-write-wins, still order-dependent); the
  diagnostic names both ids, in the `diagnostics` array that the stale diagnostics
  already use. Exit code unchanged.

### Acceptance matrix

All tests go in `tests/cli/cli_b_diff_differential.rs` next to the `issue156_*` tests,
so they use its private helpers (`snap156`, `routine_fact`, `cap_fact`, `id156`). A small
local helper there writes the snapshots to a temp dir with `gate::cbor::encode` as
`.cbor` (the loader accepts it, `diff/cli.rs:73`), writes the overlay as JSON, and runs
`Command::new(env!("CARGO_BIN_EXE_alsem"))` with
`diff old.cbor new.cbor --deterministic --renames overlay.json --format json`.
Snapshots carry no `coverage` records, so a lost capability is
`capability-lost-under-coverage` (see Terms). Each test asserts exit 0, the exact sorted
set of `(kind, normalizedStableId, oldOriginalStableId)`, the exact diagnostics, and
`renamesApplied`. Ids below are `id156(sig)`; names as in the probes.

| id | input | expected | discrimination proof |
|---|---|---|---|
| A1 | issue shape (P5): old `X`(`aaaa` Alpha, insert), `T`(`tttt` Beta); new `U`(`uuuu` Beta, insert); `{X:T}` | diagnostics: only stale `not-in-new` `T`, **no** collision. Findings: `procedure-removed` `T` origin `X`; `capability-lost-under-coverage` `T` origin `X`; `procedure-signature-changed` `U`; `capability-gained-write` `U`. `renamesApplied` 1 | (a) `normalize` follows the table to a fixed point, bounded (stop on a repeated id or after `table.len()` hops) → fails (no `procedure-removed`, no `X` origin). (b) add `&& !user_targets.contains(id.as_str())` at `renames.rs:173` → fails (`procedure-added` `U`; and a collision diagnostic, since `T` is then unmapped). (c) drop the "target is not a table key" clause of the new check → fails (a collision diagnostic appears). Revert each; passes |
| A2 | A1 with old rows `T`, `X` | same as A1 | through A1's proofs; pins order independence (P6) |
| A3 | auto then user (P13) | exactly: `procedure-signature-changed` `O#bbbb` (no origin), `procedure-signature-changed` `C` (origin `B`); no diagnostics; `renamesApplied` 1 | (a) fails (`A` lands on `C` with `B`). Revert; passes |
| A4 | user, auto, user (P14) | stale `T`; `procedure-removed` `T` (origin `X`); `capability-lost-under-coverage` `T` (origin `X`); `procedure-signature-changed` `O#bbbb` (no origin); `procedure-signature-changed` `C` (origin `B`); no collision; `renamesApplied` 2 | (a) fails (`X` and `T` both land on `C`). Revert; passes |
| A5 | swap (P15): old `{X Alpha (insert), T Beta}`, new `{X Beta}`, `{X:T}` | stale `T`; `procedure-removed` `T` (origin `X`); `capability-lost-under-coverage` `T` (origin `X`); `procedure-signature-changed` `X` (old `tttt`); no collision; `renamesApplied` 1 | (a) as specified stops on the repeated id (an unbounded fixed point would loop here; the bound is part of the break) → `X→T→X` stops at `X` and `T→X→T` at `T`, so the signature change on `X` disappears → fails. Revert; passes |
| D1a | P10, old order `X`, `T`; new `{T}`; `{X:T}` | diagnostics: exactly one `rename-overlay-collision` `{from: X, to: T}`. Findings as today: only `capability-lost-under-coverage` `T` (no origin) | (d) remove the new diagnostic push → fails. Revert; passes |
| D1b | P10, old order `T`, `X` | the same single collision diagnostic; findings as today: `procedure-signature-changed` `T` (origin `X`), `capability-lost-under-coverage` `T` (origin `X`) | (d) fails. Revert; passes |
| D1c | P16 (new `U` local), old order `X`, `T` | stale `T` and collision `{X, T}`; findings as today (`procedure-removed` `T` no origin, `capability-lost-under-coverage` `T`) | (d) fails |
| D1d | P17 (ambiguous group), old order `X`, `T`, `T2` | stale `T` and collision `{X, T}`; findings as today (P17 X-first lines) | (d) fails |

D1a/D1b pin the diagnostic in both row orders and also pin that the findings are
unchanged (the stated non-goal). D1c/D1d are cheap variants of the same helper and show
the diagnostic covers the issue's own shape when the pair does not form.

Unit test in `renames.rs`, next to `overlay_stale_diagnostic`: table `{X→T}` with
`old_ids = {X, T}` → exactly one collision diagnostic; table `{X→T, T→U}` with the same
old ids → none. Optional; the CLI tests above are the acceptance.

### Design (the smallest root fix)

1. `src/engine/gate/diff/renames.rs` `validate_overlay_against_snapshots`: the
   collision check above (one `if`, one `DiffDiagnostic` with three fields), and a doc
   line stating the principle.
2. `src/engine/gate/diff/renames.rs` `pair_signature_changes` doc: state the principle
   and that both exclusions and the inclusion of old-side user targets follow from it.
3. `src/engine/gate/diff/indexes.rs` `normalize`: a one-line doc, one hop by design.
4. `CHANGELOG.md` `[Unreleased]`: under `### Added`, the `rename-overlay-collision`
   diagnostic (when it fires, that findings are unchanged); under `### Changed`, the
   one-hop rule, and that the 2.0.1 "limit" (`X`'s facts stay on `T`) is the intended
   behaviour. The released 2.0.1 text is left alone (spec r1 O6).
5. The tests A1–A5, D1a–D1d.

### False-positive and failure shapes

- **Cycle (P15, A5).** `X` in both snapshots, `X→T` user, `T→X` auto. One hop gives a
  clean swap, no loop, no collision diagnostic (`T` is an auto key, so it is mapped away).
- **Two sources onto one target.** User-user: `rename-overlay-overlap`, unchanged. If the
  shared target is also a live unmapped old id, each entry also gets a collision
  diagnostic; that is true and acceptable. User-auto and auto-auto: impossible (see the
  new diagnostic). User target that is a live unmapped old id: the new diagnostic.
- **False positive of the new diagnostic.** It needs both ids to have rows in the old
  snapshot and the target to be unmapped; then the shared key is certain. A stale key
  (`not-in-old`) never fires it. A user entry whose target is live in old AND in new
  with nothing mapping it away (D1a) is a real collision, not a false positive: two old
  routines are compared with one new one.
- **User and auto disagree.** The user always wins on the ids it names (`:173`, `:178`).
  The only overlap left is the issue's shape, which is not a disagreement (two routines
  sharing a string, P7).
- **A chain longer than two.** Mixed chains exist: an auto target can be a user key (P13,
  `O#bbbb`), and a user target can be an auto key (P5, `T`); so `X→T→B→C` is legal (P14)
  and gets no chain diagnostic. One hop is still right: no entry's target is ever looked
  up again, so each old routine lands on exactly its own entry's target (A4). Only
  user-only chains get `rename-overlay-chain`, as before.

### Measurement

No finding changes. The new diagnostic needs an overlay whose target is a live old id;
no golden has one (P18), so `scripts/check-goldens` should be green with no regen; the
implementer runs it once to confirm. Also `scripts/ci-steps task`. No CDO measurement:
`diff` reads snapshots only.

### Fallbacks, stated in advance

1. **A golden moves after all.** Stop and find out why before any regen: P18 says none
   should. Only a golden whose overlay target is live in old may gain the diagnostic.
2. **A discrimination proof comes back green.** That is evidence about the test: check
   that the break applied, record it, and fix the test before going on.
3. **A5's bounded break still hangs** (the bound was not applied). Kill it, record it,
   and run the break with the bound; never leave the run hanging.

## Discoveries

- **D1 (pre-existing, not from #156): a user target that is an unmapped live old id
  silently merges two routines.** Folded into this issue after spec round 1 (Q2). P10,
  P16, P17.

## Open questions and decisions

- **Q1:** accept the corrected acceptance (today's one-hop output, pinned) instead of
  "X's facts land on U"? **Decision (spec r1):** yes. Both reviewers accept; the
  chain-following fallback is dropped.
- **Q2:** fold D1 into #237 or file it? **Decision (conductor, spec r1):** fold it in.
  Opus showed D1 occurs in the issue's own shape whenever the auto pair does not form
  (P16, P17). sol preferred filing separately. Fix as a diagnostic only, under one
  principle (O7); no chain-following.

## Spec panel

Round 1 (sol, opus; reviews in the run dir `spec-r1-sol.md`, `spec-r1-opus.md`). Every
finding was checked against the code or the binary; register:
`.agent/issue-237/findings.json`.
- S1 = O1 (fixed): kind is `capability-lost-under-coverage` with no coverage records
  (`policy.rs:102-109`; all probes re-read). Terms and every probe row now say so.
- S2 = O5 (fixed): mixed chains exist; P14 run on the binary; A4 added; the chain
  argument rewritten.
- S3 = O2 (fixed): P11 corrected; the rename golden calls `run_diff_engine`.
- O3 (fixed): test location and runner stated.
- O4 (fixed): swap precondition corrected (`X` in both snapshots), run as P15, pinned as
  A5 with a bounded break.
- O6 (fixed): `[Unreleased]` entry; 2.0.1 text untouched.
- O7 (fixed): one principle stated in Goal and in the doc edit.
- O8 (fixed): P8 labelled approximate; proof (a) is the evidence.

## Spec round 2 (conductor)

- Both reviewers accepted S1-S3 and O1-O8 and approve; both confirm the `rename-overlay-collision` firing rule (no false positives; fires for user entries only). Opus raised O9 (minor): the "old id is live" clause is pinned by no test. Required addition for the implementer: a `renames.rs` unit test with `{Y→T}`, `old_ids = {T}`: expect a stale `not-in-old` diagnostic for `Y` and NO collision; discrimination proof: drop the clause → the test fails. Both reviewers mark O9 at the final panel.

## Implementation

Plan: `docs/superpowers/plans/2026-10-24-issue-237-one-hop-rename.md`, **one task**.

### Changes

- `src/engine/gate/diff/renames.rs`: `validate_overlay_against_snapshots` pushes
  `rename-overlay-collision {kind, from, to}` after each entry's stale checks when the
  old id and the target are both live old ids and the target is not a table key. Docs
  on it and on `pair_signature_changes` state the principle. Unit tests
  `overlay_collision_diagnostic` (`{X→T}`, old `{X,T}` → one collision, field order
  `kind, from, to`; `{X→T, T→U}` → none) and `overlay_stale_key_is_not_a_collision` (O9).
- `src/engine/gate/diff/indexes.rs`: `normalize` doc, one hop by design.
- `tests/cli/cli_b_diff_differential.rs`: A1–A5 (`issue237_a1`…`a5`), D1a+D1b in one test
  (`issue237_d1ab_…`, both row orders in a loop), D1c, D1d. Helper `diff237` writes
  `.cbor` snapshots with `gate::cbor::encode` and the overlay JSON (by hand, to keep
  order) to a tempdir and runs `CARGO_BIN_EXE_alsem diff … --deterministic --renames …
  --format json`; asserts exit 0, sorted `(kind, normalizedStableId,
  oldOriginalStableId)`, the exact `payload.diagnostics`, and `renamesApplied`.
- `CHANGELOG.md` `[Unreleased]`: Added (the diagnostic), Changed (one hop is the rule).
  2.0.1 text untouched.

### TDD red (before the implementation)

`cargo test -p al-sem --lib gate::diff::renames`: 5 passed, 1 failed —
`overlay_collision_diagnostic` (`left: 0, right: 1`). `cargo test --test cli issue237`:
5 passed, 3 failed — `d1ab` (`t_first=false`, diagnostics `[]` vs one collision), `d1c`
and `d1d` (only the stale diagnostic, collision missing). **A1–A5 and O9 passed at
once, as expected:** they pin today's one-hop behaviour; their value is their breaks
(table below). Every expected finding set matched the probes (P5, P6, P10, P13–P17) on
the first run.

### Discrimination proofs

Every break made with Edit; "applied" checked by `grep -c` of a `BREAK237*` marker and of
the removed/added text before each run; reverted with Edit; after all reverts `grep -rc
BREAK237` is 0 everywhere, `git diff --stat` is the same as before the breaks, and the
14 new tests pass.

| break | edit | result under the break | revert |
|---|---|---|---|
| (a) follow the chain | `indexes.rs` `normalize` loops `table.get` until a repeated id (a `seen` set; also `seen.len() > table.len()`) | A1 FAIL: findings only `[psc uuuu]` (X vanishes). A2 FAIL: only `[psc uuuu origin aaaa]` (and now order-dependent). A3 FAIL: `[cap-gained bbbb, cap-lost cccc origin bbbb, procedure-added bbbb, psc cccc origin bbbb]`. A4 FAIL: `[cap-lost cccc origin bbbb, procedure-added bbbb, psc cccc origin bbbb]`. **A5 FAIL, no hang (bound applied), actual findings: `[("capability-lost-under-coverage", aaaa, origin aaaa), ("procedure-removed", tttt, no origin)]`** — the signature change on `X` is gone. D1a–d pass (no multi-hop there) | pass |
| (b) refuse the pair | `renames.rs` old-side filter `&& !user_targets.contains(id.as_str())` | A1, A2, A4, A5 FAIL (a collision `{aaaa, tttt}` appears, since `T` is then unmapped). D1d FAIL too (old `T` excluded, so `T2` pairs with `U`: `psc uuuu` replaces `procedure-added uuuu` / `procedure-removed t2t2`). A3 passes (no user target that is an old id) | pass |
| (c) drop "target not mapped away" | remove `&& !table.contains_key(&entry.new_id)` | A1, A2 FAIL (collision `{aaaa,tttt}`); A4 FAIL (+ `{aaaa,tttt}`, `{tttt,bbbb}`); A5 FAIL (+ `{aaaa,tttt}`, `{tttt,aaaa}`); A3 FAIL (collision `{aaaa,bbbb}` from the auto entry); unit `overlay_collision_diagnostic` FAIL (second half) | pass |
| (d) remove the diagnostic | the `diagnostics.push` replaced by a comment | D1a FAIL (`t_first=false`: `[]`), D1c, D1d FAIL (stale only); unit `overlay_collision_diagnostic` FAIL (`0` vs `1`). D1a/D1b share one loop, so D1b was checked on its own with the loop order flipped to `[true, false]` (Edit, still under (d)): FAIL `t_first=true`, `[]` vs one collision. Loop order reverted | pass |
| O9 drop "old id is live" | condition becomes `old_ids.contains(&entry.new_id) && !table.contains_key(...)` | `overlay_stale_key_is_not_a_collision` FAIL: `left: 2, right: 1` (stale `Y` + collision `{Y, T}`) | pass |

Fallbacks: none fired. (3) did not apply — (a) carried its bound and A5 finished in
0.01 s. (2) no break came back green for a test it targeted.

### Goldens and gates

- `bash scripts/ci-steps task`: **green** (exit 0; nextest 4006 run, 4006 passed, 8
  skipped; fmt, dev clippy and gen-syntax clean).
- `bash scripts/check-goldens`: exit 0. It skipped its own run because this exact tree
  had just passed `ci-steps test`, which already includes every golden target.
- **No golden moved** (`git status` showed only the four edited source files), as P18
  predicted.
- `agentflow check-diff --base origin/master` (origin/master is still `830a150c`):
  `reasons: []` before and after each commit.

### Commits (not pushed)

- `5a34c9e7` diff: one-hop rename rule and rename-overlay-collision (#237): code,
  tests, CHANGELOG.
- `4be3da5d` plan: issue #237 one-hop rename (one task).

## Final round 1 fixes (implementer)

Both reviewers said ship (sol F1-F3, opus G1); the code review raised CR1-CR8. Each was
checked against the code first. The branch was rebased twice: onto `b028ec32`
(clean), then onto `317d3b34` (#278, the grammar bump). The second rebase conflicted
in `CHANGELOG.md` `[Unreleased] ### Changed`; both entries were kept (grammar first,
then #237). Before that rebase, `check-diff --base origin/master` reported
`protected-path:CLAUDE.md`. The cause was only master's own file: #278 changed
`CLAUDE.md` and the branch does not. With `--base b028ec32` (the merge-base then) the
result was `reasons: []`, and after the rebase `--base origin/master` is clean again.

| id | verdict | what was done |
|---|---|---|
| CR1 | real, fixed | Fact subjects without an identity row DO happen by construction: `derive_identity_table` (`src/engine/l5/snapshot.rs:1565-1588`) writes objects, tables and routines only, and the committed schema snapshots carry field and key facts (`Table:50100#1`, `#2`, `#K0`) with no identity row (`tests/cli-b-goldens/diff/snapshots/schema-field-removed-old.snap.json`). The new `indexes::live_ids` makes an id live when it has an identity row or is the subject of any fact `normalize` re-keys (contract, schema, event, capability, coverage, permission). It feeds BOTH the collision and the stale checks: one definition of "present". So a field rename is no longer called stale on both sides. Red: D1e got `[stale #1 not-in-old, stale #2 not-in-new]` and no collision. That is listed under CHANGELOG Fixed. The O9 unit test is unchanged (it passes the sets directly); its doc says what live means |
| CR2 | real, fixed | D1a-D1d now assert the exact diagnostics, exit 0, `renamesApplied`, and only the findings off the collided key (`findings_off237`). Its doc says the collided key's findings depend on row order and are deliberately not pinned (the stated limit) |
| CR3 = F2 | real, fixed | The doc names both ways two old ids meet on one key. Two entries sharing a target are reported by `rename-overlay-overlap`, which only user entries can trigger. A live, unmapped target gets the collision. The smaller honest option was taken: a doc change, not a second diagnostic for the overlap case |
| CR4 = F1 | real, fixed | `mod.rs:162-163` inserts each pair into the same table, so a pair on a user key would REPLACE the user's entry. The doc now lists the exclusions side by side (old side: user keys; new side: user targets and already-filled keys) and the old-side user-target inclusion |
| CR5 = G1 = F3 | real, fixed | CHANGELOG and the doc now cover any id kind (routine, object, table, field, key) and say "the two old ids' facts share one key". The text no longer says "compared with the new one" |
| CR6 | refuted | The flow commits `.agent/issue-237/ledger.md` in its evidence commit before the merge, so the plan's citation resolves on master |
| CR7 | real, fixed | The `validate_overlay_against_snapshots` doc says it began as an al-sem port (the stale checks). It names the two #237 parts that diverge from al-sem: the live-id definition and the collision |
| CR8 | fixed (reuse) | `cli_diff_strict::alsem_diff(args)` takes any args, so `diff237_ids` now calls it (made `pub(crate)`; members of the umbrella already call each other, e.g. `crate::cli_b_digest_differential::DIGEST_CORPUS`). That is slightly less code than the hand-built `Command` |

### Discrimination proofs (round 1 fixes)

Each break was made with Edit, checked as applied by `grep -c`, and reverted with
Edit. Afterwards `grep -rc BREAK237` finds nothing.

| break | result | revert |
|---|---|---|
| CR1: `live_ids` returns the identity rows only (`if true { return ids; }`) | D1e FAIL: `[stale #1 not-in-old, stale #2 not-in-new]`, no collision; the other 8 pass | pass |
| (d) again, on the reworked D1 tests: collision push removed | D1a (`t_first=false`), D1c, D1d, D1e FAIL, each through the diagnostic assertion | pass |
| (d) with the D1 loop flipped to `[true, false]` | D1b FAIL `t_first=true`: `[]` vs one collision. Loop order reverted | pass |

O9: accepted by both reviewers in final round 1.

### Gates on the rebased head

See the next block.

## Final panel (conductor)

- Final round 1 (head 4be3da5d): sol ship (F1-F3 minor docs), opus ship (G1 minor; O9 accepted by both); built-in code-review CR1-CR8 (CR1 important: fact subjects without identity rows). Fixed in d0660814 (CR6 refuted), rebased onto 317d3b34 (#278 grammar bump; CHANGELOG conflict only).
- Final round 2 (head d0660814): both accepted F1-F3, G1, CR1-CR8, including CR1's single "live id" definition for the stale and collision checks; both ship. Register: all entries accepted by both. final_rounds remaining 3.
- Step 11 gates on d0660814 (base 317d3b34, grammar c89c2ecb, supervised, run 20261024-090424-e491a7): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (305.4 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (276.3 s).
