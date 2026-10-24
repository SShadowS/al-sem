# Issue #250 — program graph: two spellings for one object name (`""`)

## Claim

```json
{"run_id":"20261023-215149-03b203","issue":250,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/250-agent-discovery-program-graph-a1","worktree":"/work/al-sem-issue-250-a1","body_hash":"250028aa085191baf48805d86057f8ca77bd002d9f2c1ae75216c3368e928246","reconciled":[]}
```

Issue body hash (claim `body_hash`): `250028aa085191baf48805d86057f8ca77bd002d9f2c1ae75216c3368e928246`.
Base (`make-worktree`): `678d654080857769e5c027cf730c9641437453a9`.
Reviewers (operator instruction, 2026-10-23): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator queue #256 → #250 → #252; claimed with `--loop-max 3`.

## Classification

**Architectural.** The fix changes how object identity is keyed in the program graph (every `ObjKey::Name` construction site and the name-lookup paths: the resolver's name index and reference normalisation), across `node_extract`, `resolve/*`, `model`, `lsp/snapshot` and `impact_graph`. A first read suggests the split also affects RESOLUTION (a source reference to a dependency object whose name contains `""`), not only ids. A spec document goes to the spec panel.

## Assumption probes

Run by the design author (opus), probe crate `/work/bench/250/probe`, outputs `/work/bench/250/p1p2-v2.out`, workspaces `/work/bench/250/ws/s{1,2,3}-*`.

| probe | result | evidence |
|---|---|---|
| P1 reproducer (P3 vs P8) | **reproduced** | symbol-only dependency `interface "A""""B"` (logical `A""B`, no Id): graph key `Name("a\"\"b")` → P3 `model_object_id` = `…/Interface/@a%22b`; P8 projected `ModelObject` = `…/Interface/@a%22%22b`. Shape of #83's T5b (`tests/cli/cli_numberless_object_ids.rs:487`). `p1p2-v2.out` S1 |
| P2 resolution | **two misses for a symbol-only dependency**; none source↔source, none for an embedded-source dependency | S1: `X.Go()` on `Interface "A""""B"` → `routes=[]` (control `P.Go()` on `IPlain` reaches `Impl.Go`, same `Impl`); `C.Run2()` on `Codeunit "C""""D"` (50200, logical `C""D`) → `Unknown(ObjectNotInGraph)`. S2 all source: all resolve. S3 embedded source: all resolve. Cause: the name indexes key on `ObjectNode.name` (`graph.rs:28`, `index.rs:562`) — raw for source, logical for ABI; queries raw too (`unquote_identifier`). `ObjKey` plays no part in it |
| P3 population | **0 everywhere real** | CDO baseline @ `bc3ccb1`: 741 `.al` files, 0 object declarations with `""` (9 `""` lines are string literals); its 11 `.app` SymbolReference files: 1,250,923 `Name` values, 498 contain `"`, all page/request-page control names; objects/fields/methods/parameters 0. Fixtures 666 `.al`: 0. Goldens: 0 files with `%22` or escaped quotes. `""` names exist only in inline test workspaces (`cli_numberless_object_ids.rs` T3, `cli_escaped_object_names.rs`, `cli_object_display.rs`, `graphify_export.rs:1565-1579`) |
| P4 consumers that print/hash the key | model ids unchanged for source (fold∘unescape = unescape∘fold); ABI `""` keys become correct; `alsem graph` node ids change only for numberless source names with `""` | `model_key`; `impact_graph/mod.rs:320-323, 409-416`; `lsp/def_surface.rs:319-330` (in-memory hash). Unmoved: `abi_check.rs:103-106, 373-376`, `resolver.rs:157-160, 2174-2177`, `edge.rs:72`, `applicability.rs:286`, `semantic_golden.rs:1145, 993`. Nothing persists a graph key (`pack::encode/decode` no caller outside `pack/`; `DepCache` in-memory) |
| P5 other identifiers | same split, zero population, out of scope → discovery | field/routine/event/enum-value/dataitem/control names keep raw `""` on the source side (`FieldNode.name_lc`, `RoutineNodeId.name_lc`; member lookups via `unquote_identifier` at `receiver.rs:1007, 1810, 2466, 2486`, `arg_dispatch.rs:784, 1101, 1120`), logical on the ABI side; CDO `.app` files: 0 field/method names with `"` |

Spec: `docs/superpowers/specs/2026-10-23-issue-250-objkey-logical-names-design.md`.

## Base/Head SHAs

- Claim base (`make-worktree`) and **B** = `678d654080857769e5c027cf730c9641437453a9` (`origin/master` did not move; no rebase).
- **H** = `b36baf85178c66395aaf35718c2f05e8877cc17e`.

## Timeline

- 21:51 claim #250 attempt 1 (`--loop-max 3`), worktree at `678d6540`.
- Classified architectural; design author dispatched (opus; subagents 59). Probes P1–P5 and spec (rev 1, 261 lines).
- Spec round 1 (`spec_rounds` remaining 4; pi_calls 19; subagents 58): **astra reviewer unavailable** — pi: `Codex error: The usage limit has been reached` (every pi model is on the same openai-codex provider); NOTIFY line written to the run's `notify.log`, push notification sent. Operator (asked 22:05 UTC): **wait for the limit**, no stand-in. Opus (flash) round 1: approve with fixes, 10 findings (5 important). Spec revised by the author (rev 2, 416 lines; 8 accepted, O4 partly refuted with evidence, O10 → discovery). Register O1–O10. astra to give a full round-1 review of rev 2 when the limit clears (its thread was never opened).
- Retries 22:36 and 23:06 UTC: limit still in place; 23:37 UTC: available.
- Spec round 2 (remaining 3; pi_calls 18): sol's first review of rev 2 — **rework** (8 findings: R5 unreachable, R12 mixed shapes, T2/T3 assertions blind to object-name resolution, R12 fixture wrong producer, R7b fixture, ABI-type-text claim, LSP discovery, LSP impact accounting); register O1–O4, O6, O8, O9 accepted, O5/O7/O10 re-raised. opus: O1–O10 accepted, approve; new minors N1/N2.
- **Scope decision (conductor):** the resolution half (D3–D8, R1–R12, T2/T3) fixes a shape with zero population on CDO and every fixture (P3) and needs more design than the issue's acceptance asks for (CLAUDE.md: measure the population before building for it). #250 is narrowed to exactly its acceptance (D1 key helper + differential comparison, D2 `model_key`, T1 P3 == P8, a source/embedded-source control, CDO unchanged); the resolution design moves into a discovery carrying every reviewer point. Spec rev 3 dispatched to the author (subagents 56).
- Spec rev 3 (254 lines): scope = D1 (`ObjKey::from_source` at 8 constructions + 3 comparison sites: differential, applicability, semantic_golden — the latter two found by the self-consistency check) + D2 + T1/T2/T3; resolution design → discovery D-A; D-B member identifiers; D-C LSP narrowed.
- Spec round 3 (remaining 2; pi_calls 17): opus approve 21/21 + minors N3–N5; sol approve-with-fixes (S1/S2/S4 re-raised: D-A must spell out the regression requirements; S9 D2 doc). Conductor text fixes → rev 3.1.
- Spec round 4 (remaining 1; pi_calls 16): S1 S2 S4 S9 O13 O14 O15 — both approve 7/7. **Converged**: 25 entries, all accepted by both.
- Plan `docs/superpowers/plans/2026-10-23-issue-250-objkey-logical-names.md`, 1 task (`plan_tasks` 11). Bounded plan size → no per-task reviewer.
- CDO before-measurement (bench) on unchanged master code.
- Steps 6+8, opus implementer (subagents 55; task_attempts T1): Phase A red — T1, T3, T3b, the `model_key` and graphify ABI vectors failed for the stated reason; T2 (control) passed on master (no fallback); the `from_source` vectors are a compile error before the helper exists (their failure is proof row 3). Commits `47dea15f` (spec + plan), `969ecff9` (change). task-affected `3937 passed, 8 skipped`; check-goldens `1044 passed, 4 skipped`, zero files changed; check-diff `{"reasons": []}`. Site check: outside `#[cfg(test)]` only `node.rs:92` (`from_abi`) and `:103` (`from_source`) build a key; the rest are read-only arms, comments or test code. Implementer notes: T2 also catches a re-added `model_key` unescape; a stale doc on `target_is_on_table_or_extension` (semantic_golden.rs) names a `differential` helper that no longer exists.
- CDO after-measurement: byte-identical (Gate results).
- Task gate green (44.5 s); `push-branch` → `969ecff9`; draft PR **#266**.
- Final round 1 (`final_rounds` remaining 4; pi_calls 15; subagents 54): built-in `code-review` at high (forked skill, 9 candidates, no correctness bug), sol approve-with-fixes (1 important: gate evidence; 3 minor), opus approve-with-fixes (7 minor). Deduplicated into FR1–FR13. Fix `b36baf85` by the implementer (task_attempts F1): `from_source(&ObjectDecl)` crate-only via `ir_object_name`; `ProgramGraph::object` binary search + one shared `target_is_on_table_or_extension`; T2's S3 precondition on the interface node; docs; CHANGELOG plain English (Changed + Fixed). Proofs recorded. task-affected `3937 passed`; check-goldens `1044 passed`, zero files changed. CDO stats re-measured on `b36baf85`: byte-identical; probe diff unchanged. Task gate green (34.6 s).
- Final round 2 (remaining 3; pi_calls 14): FR1–FR13 — sol approve 13/13, opus approve 13/13. **Converged**: 38 entries. `origin/master` unchanged (`678d6540`), no rebase. Full gate set started on `b36baf85`.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| T1 (+ `model_key` vector, graphify ABI vector) | `model_key` unescapes again | cli:607 left `…/Interface/@a%22b`; node.rs:372 left `@a%22b`; graphify:1596 left `@a%22b` | 11/11 cli, 6/6 lib ok | 969ecff9 |
| T2 | same `model_key` unescape (source key unescaped twice) | cli:634 left `[]` (id `@a%22b`) | 11/11 ok | 969ecff9 |
| T2 (+ `from_source` vector, restated graphify, #83 T3) | `from_source` keys on `fold(raw)`, D2 kept | cli:634 (id `@a%22%22%22%22b`); cli:273 #83 T3 `@…%22%22q%22%22…`; node.rs:371 left `Name("a\"\"\"\"b")`; graphify:1578 left `@i%22%22foo` | ok | 969ecff9 |
| T3 | applicability `Name` arm returns the key text | applicability.rs:774 panicked (T3b ok) | ok | 969ecff9 |
| T3b | semantic_golden `Name` arm returns the key text | semantic_golden.rs:2223 left 1 right 0 (T3 ok) | ok | 969ecff9 |
| FR1 + restated #83 T3 | `from_source` → `o.name.fold_identifier()` (no unescape) | node.rs:379 left `Name("a\"\"\"\"b")`; graphify:1580 `@i%22%22foo`; T2 cli:646 `[]`; #83 T3 cli:284 `%22%22q%22%22` | pass | b36baf85 |
| T3 + T3b (shared helper) | helper reads the `Name` key text instead of `table.name` | applicability.rs:786 and semantic_golden.rs:2190 (left 1) | pass | b36baf85 |
| T3, T3b, trigger_applicable_insert_table_extension_true | `ProgramGraph::object` never finds | all three fail | pass | b36baf85 |
| T2 S3 precondition | drop `src/AB.Interface.al` from S3's embedded source | cli:739 left `[]` right `[EmbeddedSource]` (the old any-object check would pass) | pass | b36baf85 |

## Acceptance result

Every issue acceptance item is met. 1 one helper at every construction and comparison site: `ObjKey::from_source(&ObjectDecl)` (crate-only) at all 9 sites, plus the two table-extension lookups through one shared helper; the `rg` site check (outside `#[cfg(test)]` only `from_abi` and `from_source` build a key) and the `node.rs` unit vectors. 2 `model_object_id` stops unescaping: `model_key_encodes_a_logical_name_key_as_it_is` and the graphify vectors. 3 P3 == P8 for a dependency name holding `""`: T1 `dependency_interface_with_escaped_quotes_graph_id_equals_its_model_object_id`, with a recorded proof. 4 resolution on CDO unchanged: stats byte-identical before and after (twice), and the cdo-gate record on H (Gate results). Both final reviewers marked every item met. Also held: source ids keep their bytes and S2/S3 resolution is unchanged (T2); #83's tests stay green.

## Findings register summary

38 entries. Spec panel (4 rounds): O1–O15, S1–S9, C1 (the conductor's scope decision). Final panel (2 rounds; the built-in `code-review` at high ran as the specified step): FR1–FR13. 36 `fixed`, 1 `refuted` (FR12, the accepted spec limit), and FR11 closes by the cdo-gate record. Every entry accepted by both reviewers (`sol` = gpt-6.1-sol in the astra slot, `opus` = Opus in the flash slot); none open, no blocking entry deferred. The astra slot was unavailable for about 90 minutes (Codex usage limit); the operator chose to wait, and no stand-in was used.

## Gate results

**CDO measurement (spec Measurement plan 1 and 4).** Through the supervisor as `bench`: before at master `678d6540` (`logs/bench/20261023T233008-1cab5947.log`, 79.9 s), after at `969ecff9` (`logs/bench/20261023T234534-ad030ddb.log`, 46.0 s); `cdo`-profile `aldump` on the pinned CDO workspace. `--program-call-graph-stats` sha256 `c032c8db…1594ef` before and after, `--dependency-bodies-stats` `7eacefbb…5261e8` before and after: **byte-identical**. Probe crate re-run (`/work/bench/250/probe-after.out` vs `p1p2-v2.out`, spans stripped): only three key/id lines differ — S1 symbol-only `model_object_id` `@a%22b` → `@a%22%22b` (the fix), S2/S3 source keys `Name("a\"\"\"\"b")` → `Name("a\"\"b")` with their ids unchanged (`@a%22%22b`); every route line identical (S1's two misses remain: discovery D-A). Re-measured on `b36baf85` after the final-round fixes (`logs/bench/20261024T000225-bdc6b31a.log`): still byte-identical; probe diff unchanged.

**Attested gate set on H = `b36baf85`** (all supervised, queued 0 s):

| gate | exit | seconds | log |
|------|------|---------|-----|
| ci-steps-all | 0 | 354.7 | `.agent/runs/20261023-215149-03b203/logs/ci-steps-all/20261024T000628-c2177eed.log` |
| check-goldens-coverage | 0 | 0.0 | `.agent/runs/20261023-215149-03b203/logs/check-goldens-coverage/20261024T001223-6cb0e90e.log` |
| check-goldens | 0 | 0.1 (tree-stamp trust of the green `ci-steps test` on this exact tree) | `.agent/runs/20261023-215149-03b203/logs/check-goldens/20261024T001223-3d73ff54.log` |
| cdo-gate | 0 | 243.4 | `.agent/runs/20261023-215149-03b203/logs/cdo-gate/20261024T001223-daf0eb88.log` |

`git status --porcelain -- . ':!.agent'` empty after the gates. Task gates: 44.5 s and 34.6 s, both 0.

## Discoveries

The three discoveries are the JSON array in the spec's `## Discoveries` section (`docs/superpowers/specs/2026-10-23-issue-250-objkey-logical-names-design.md`), filed from there in step 10:
- **D-A** (bug): a source reference to a symbol-only dependency object whose name holds `""` does not resolve (P2's two misses); its acceptance carries every reviewer point of the removed resolution design.
- **D-B** (bug): the same split for member identifiers (fields, routines, events, enum values, dataitems, controls).
- **D-C** (bug): LSP `eventReferenceAtPosition` compares a logical ABI publisher name with raw source text.

## Not delivered

- Nothing from the issue's acceptance. Deliberately out of scope (conductor's scope decision, accepted by both reviewers as C1): making source references to symbol-only dependency objects whose name holds `""` resolve. Probe P2 shows two misses. The population is zero on CDO and in every fixture (P3). It is filed as discovery D-A, with every reviewer point in its acceptance.
