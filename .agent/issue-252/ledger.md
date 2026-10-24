# Issue #252 — per-detector loose subjects for looseFingerprint

## Claim

```json
{"run_id":"20261024-002419-754270","issue":252,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/252-agent-discovery-l5-analyze-loose-a1","worktree":"/work/al-sem-issue-252-a1","body_hash":"21403b6fa157d1bb2106b21416a99374c930e8d4da74c7066110dbc41e4d7ec6","reconciled":[]}
```

Issue body hash (claim `body_hash`): `21403b6fa157d1bb2106b21416a99374c930e8d4da74c7066110dbc41e4d7ec6`.
Base (`make-worktree`): `ae3439c557b3c0392b9e6c4c06f03459df2ad6aa`.
Reviewers (operator instruction, 2026-10-23): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator queue #256 → #250 → #252. The shared loop budget showed 3 of 3 used (my #256 and #250, plus another session's new claim), so #252 was claimed with `--loop-max 4` on the operator's explicit queue instruction, the same way #254 was handled.

## Classification

**Architectural.** The change touches how 16 detectors (d4, d7, d12, d16, d18, d22, d39–d42, d43, d45, d48, d57, d59, d60) state the routine-independent part of their key, and how `fingerprint.rs` builds `looseFingerprint` from it, across `src/engine/l5/detectors/*` and `src/engine/l5/fingerprint.rs`. It affects every consumer that compares reports (cli-a goldens). A spec document goes to the spec panel.

## Assumption probes

Run by the design author (opus). Scratch: `/work/bench/252/` (master binary `before/alsem`, test workspaces `p2/`, CDO outputs `p3/`).

| probe | result | evidence |
|---|---|---|
| P1 d4 | key `d4/{rid}/{loopId}/{recVar}[/{literal}]` (d4.rs:150-158); record variable is routine-bound; strip leaves `d4/~/~/{recVar}[/{literal}]` | code |
| P1 d7 | `d7/{sorted SCC ids}` (d7.rs:243); every member routine-bound; only the anchor (first sorted id, d7.rs:206) stripped | code |
| P1 d12 | `d12/{publisherObj}/event/{name}` (d12.rs:72); no routine id, strip does nothing; the event name is the anchor's own name | code |
| P1 d16 | `d16/{from}/{cs}/{to}` (d16.rs:75); caller stripped, callee kept; a self-call becomes `d16/~/~/~` (all self-calls in one object share a key) | code |
| P1 d18 | `d18/{rid}/{loopId}/{recVar}/{field}` (d18.rs:161-168); record variable routine-bound | code |
| P1 d22 | `d22/{rid}/{recVar}/{field}` (d22.rs:143); confirmed: a variable named like a site id (`op2`, `cs1`) is stripped with the routine id | code |
| P1 d39–d42 | `dNN/{caller}/{cs}/{calleeParamIdx}` (d39.rs:245, d40.rs:194, d41.rs:199, d42.rs:277); `affectedTables` empty; strip leaves `dNN/~/~/{idx}` — no callee, no table: confirmed | code + P2 d41 |
| P1 d43 | `d43/{event}\|{caller}\|{caller}/csN\|{setter}\|{table}` (d43.rs:508); anchor is the setter, so the caller and its call site stay | code |
| P1 d45 | `d45/{publisher}\|{table}` (d45.rs:169); a primary publisher is the anchor and is stripped: confirmed, two primary publishers on one table share a key | code + P2 + P3 |
| P1 d48 | `d48/{loopRoutine}` (d48.rs:374, 525); direct anchors on the loop routine (stripped), transitive on the IO routine (loop routine stays) | code |
| P1 d57 | `d57/{rid}/{globalVar}` (d57.rs:214); a global named `op1`/`op2` is stripped: confirmed | code + P2 |
| P1 d59 | `d59/{rid}/{paramIdx}` (d59.rs:66); parameter position routine-bound | code |
| P1 d60 | `d60/{rid}/{loopId}/{recVar}` (d60.rs:223); record variable routine-bound | code |
| P2 d60 (issue reproducer) | **reproduced**: loop moved `OnUpgradePerCompany`→`Helper` and `P1`→`Itm`: fingerprint `43a4b595…`→`0d78ed49…`, loose `abdecf80…`→`31764dfe…`; control (moved, not renamed): loose stays `abdecf80…` | `before/alsem analyze p2/d60{a,b,c} --format json --deterministic --detector d60-upgrade-loop-should-be-datatransfer` |
| P2 d57 | globals `op1`, `op2` in two subscribers: both loose `6915b514d2725038` | `p2/d57` |
| P2 d45 | `OnFirst` and `OnSecond` both insert Customer: both loose `14739fcbd0e1bc9e` | `p2/d45` |
| P2 d41 | Customer and Item via two Reset helpers: both loose `390600826583e83b`, `affectedTables: []` | `p2/d41` |
| P3 CDO (findings / distinct loose today → predicted) | primary: d12 23/23→23, d16 71/40→40, d18 10/10→10, d39 3/1→1, d40 143/57→87, d42 2/1→1, d43 5/5→5, d45 150/147→150, d48 78/28→~4–5; all: d18 64/60→59, d22 86/58→52, d43 4573/4573→4573, d48 79/29→~5, d59 72/59→72; d4, d7, d41, d57, d60: 0 findings | pinned CDO `bc3ccb18`, 0 dirty, `--detector <16>` at both scopes; jq over `p3/before-*.json` ("after" is a jq prediction) |
| P3 d45 pairs | the 3 real CDO collisions are `OnAfterEnableCDOFeature` vs `OnAfterDisableCDOFeature` on tables 471, 472, 9178 | jq group_by |
| P4 looseFingerprint readers | computed once (projection.rs:111), emitted once (format_json.rs:211); 8 test literals `""`; only cli-a goldens (19 files); SARIF, HTML, baselines, r4 do not read it | grep |
| P4 cli-a lines that move | 4: d45 in `ws-d8-commit-in-tx.{all,default}.json`, d48 ×2 in `ws-txn-d48-pos.all.json` (d12's subject equals today's key) | jq over the goldens |
| P4 rootCauseKey readers / fingerprint fixed | `fingerprint_of`, path_merge (d1/d2), sort ties, the d45 cap, policy JSON, r4 StableFinding read rootCauseKey (unchanged); `fingerprint_of` (fingerprint.rs:157-162) never reads the new field → `fingerprint` bytes fixed | code |

Spec: `docs/superpowers/specs/2026-10-24-issue-252-loose-subjects-design.md`.

## Base/Head SHAs

- Claim base (`make-worktree`) and **B** = `ae3439c557b3c0392b9e6c4c06f03459df2ad6aa` (`origin/master` did not move; no rebase).
- **H** = `47acbc0b48f24f934a8156fc815dbe58bf75ea95`.

## Timeline

- 00:24 claim #252 attempt 1 (`--loop-max 4`), worktree at `ae3439c5`.
- Classified architectural; design author dispatched (opus; subagents 59). Probes P1–P4, spec rev 1 (247 lines).
- Spec round 1 (`spec_rounds` remaining 4; pi_calls 19; subagents 58): sol **rework** (5 important: d39–d42 generic `Record`, d7/dependency-d45 anchor object, distinctions need production tests incl. d48, per-detector move preconditions, fallbacks); opus approve-with-fixes (3 important test gaps + 7 minor). Author rev 2 (346 lines; subagents 57): all accepted; new probes P5 (generic `Record` collides) and P6 (d7 anchor moves object on a rename) confirm sol 1 and 2; typed `LooseSubject { object_id, key }`; d39–d42 state the caller table.
- Spec round 2 (remaining 3; pi_calls 18): opus 15/15 accepted; sol 14/15, O8 re-raised (no overload discriminator in `encode_event_id`). Conductor: O8 stated as an accepted limit (rev 2.1) + a test comment.
- Spec round 3 (remaining 2; pi_calls 17): O8 accepted by both; opus minor O11 (the README bullet must say it) → fixed in rev 2.2, to be marked in final round 1. Spec converged (O11 pending its marks).
- Plan `docs/superpowers/plans/2026-10-24-issue-252-loose-subjects.md`, 2 tasks (`plan_tasks` 10). Architectural → per-task reviewer (sonnet). Commit `150bb2f1` docs: spec and plan.
- T1 (opus; subagents 56; task_attempts T1): **stop-and-revise** at Step 1 — the generic-`Record` fixture fires only for d41 (d39, d40, d42: 0 findings); new probe P7 (table mismatch, compile-invalid but parses clean) fires all four with a shared loose key on master. Plan Task 1 had also omitted `d48_move_transitive` and `d45_publishers_distinct`. Spec rev 2.3 / plan amended (register I1).
- Spec round 4 (remaining 0; pi_calls 16): I1, O11 — both approve (both say keep the compile-invalid fixture as a wiring proof; no table limit). **Converged**: 17 entries, all accepted by both.
- T1 resumed (task_attempts T1 → 1 left): commit `4a7beefb` (66 files). Phase A: U1, U4, dNN_callees_distinct ×4, dNN_same_callee_tables_distinct ×4 + d41 generic, d45_publishers_distinct, d45_dep_anchor_switch, d48_move_transitive, d48_direct_to_transitive, d7_move, d7_anchor_object_switch failed on master on the target assertion; dNN_move_rename ×4, d7_events_distinct, d48_loop_objects_distinct passed on master and rely on their breaks. After: cli_loose_fingerprint 25/25, r4 d45 2/2, 54 fingerprint unit tests. task-affected: fmt/clippy/gen-syntax green; full nextest 3960 run, 3959 passed, 1 failed (`cli_a_json_byte_match`, the expected golden). check-goldens (no regen): exactly the 4 predicted looseFingerprint lines (d45 ×2 `060eaa3798168a33`→`7a82b719b4f5a7f5`; d48 `7a456ba3…`→`2339d7a9…`, `5479f435…`→`f5e7d33d…`), no `fingerprint` line, no other family. Implementer notes: d41's callee is read from `evidencePath[1].note` (its steps all carry the caller id); `d7_anchor_object_switch` needs P6's app id (…0252) for the switch; the d45 dependency switch pair is `OnThing`/`H6`. Note: this checkout has no pre-commit hook configured (`core.hooksPath` unset), so the commit was not hook-checked; the gates are run by the flow regardless.
- T1 per-task review (sonnet; subagents 55): approve, 3 minor → fixed in `53de9a63` (d45 key expression; `binding_source_table` takes `&[ModelRecordVariable]` + unit test for id/name/`?` arms; `dnn_callee` pins d41's note wording).
- T2 (same implementer; task_attempts T2): commit `d0d84084`. Phase A: d4_move_rename, d4_literal_distinct, d16_self_calls_distinct, d18_move_rename, d22_move_rename, d43_move_caller, d57_globals_distinct, d59_events_distinct, d59_param_reorder, d60_move_rename, T6 failed on master; d12_events_distinct, d16_move, d18_field_distinct, d22_field_distinct, d57_move, d59_params_distinct pass on master and rely on their breaks. check-goldens (no regen) before regenerating: exactly the 4 predicted lines (Task 2 moved nothing; d12 unchanged). `--regen`, then check-goldens 1081/1081; regenerated diff = exactly those 4 `looseFingerprint` values (the second d48 line is :55, not :59 as the spec said). task-affected: 3977 passed, 8 skipped. Deviation: `d59_param_reorder` asserts same routineName + object (internal ids hash the signature, which a reorder changes). The regen guard needed an untracked `.agent/golden-triage.md` receipt (not committed).
- T2 per-task review (sonnet; subagents 54): approve, 1 minor (d18 subject used the raw, possibly quoted field text) → fixed in `bf952e08` with `d18_quoted_field_same_key` (unquoted field name in both arms; rootCauseKey/id/fingerprint unchanged). check-goldens 1082/1082, no golden moved; task-affected 3978 passed. CDO re-measured (unchanged). Task gate green (27.8 s).
- `push-branch` → `bf952e08`; draft PR **#270**.
- Final round 1 (`final_rounds` remaining 4; pi_calls 15; subagents 53): built-in `code-review` at high (forked skill, 9 candidates, no blocking bug), sol approve-with-fixes (4 minor), opus approve-with-fixes (5 minor). Deduplicated into FR1–FR15. Fixes by the implementer (task_attempts F1): `544506e0` (code/tests/CHANGELOG: FR2 pin, FR3 d43 caller object, FR5–FR8, FR11, FR12) and `47acbc0b` (README/spec rev 2.4/plan: FR1, FR3, FR9, FR10). FR2, FR4, FR14, FR15 refuted with evidence. check-goldens 1085/1085 (no golden moved); task-affected 3982 passed. Both CDO measurements re-run (`logs/bench/20261024T022042-2288c0f8.log`): fingerprint multiset identical; d43 counts unchanged; non-listed detectors identical. Two discoveries recorded (d43 does not walk variable-typed calls; implicit-Rec bindings never resolve). Task gate green (32.1 s).
- Final round 2 (remaining 3; pi_calls 14): FR1–FR15 — sol approve 15/15, opus approve 15/15. **Converged**: 32 entries. `origin/master` unchanged (`ae3439c5`), no rebase. Full gate set started on `47acbc0b`.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| d39_move_rename / d40_move / d41_move / d42_move | caller id back into the key (one detector per round) | the move test red on its final loose-key assertion | restored green (40/40) | 4a7beefb |
| d39–d42 `_callees_distinct` | drop the callee from the key | the callees test red | green | 4a7beefb |
| d39–d42 `_same_callee_tables_distinct` (+ d41_generic_record_tables_distinct) | drop the caller table from the key | the tables test(s) red | green | 4a7beefb |
| d39–d42 (all their tests) | subject `None` | callees + tables red (move stays green, by design: the strip drops the caller) | green | 4a7beefb |
| d7_anchor_object_switch | `object_id: None` | red | green | 4a7beefb |
| d7_move + d7_anchor_object_switch | subject `None` | both red | green | 4a7beefb |
| d7_events_distinct | drop the event ids | red | green | 4a7beefb |
| d45_dep_anchor_switch | `object_id: None` | red | green | 4a7beefb |
| d45_publishers_distinct + d45_dep_anchor_switch | subject `None` | both red | green | 4a7beefb |
| d48_loop_objects_distinct + d48_direct_to_transitive | transitive builder drops the loop object | both red | green | 4a7beefb |
| d48_direct_to_transitive | direct builder drops the loop object | red | green | 4a7beefb |
| d48_move_transitive + d48_direct_to_transitive | subject `None` in both builders | both red | green | 4a7beefb |
| U1 / U4 | strip the subject key / unknown object id → `""` | U1 / U4 red | green | 4a7beefb |
| binding_source_table_matches_id_then_folded_name | drop the name fallback / return `""` for an unresolved table | red: `None` vs `Some("app/table/18")` / `Some("")` vs `None` | green | 53de9a63 |
| Task 2 subjects (d4, d16, d18, d22, d43, d57, d59, d60 → `None`; d12 key → `"d12"`) | round 1 | d4_move_rename, d4_literal_distinct, d12_events_distinct, d16_self_calls_distinct, d18_move_rename, d22_move_rename, d43_move_caller, d57_globals_distinct, d59_param_reorder, d59_events_distinct, d60_move_rename, T6 red | green (56/56) | d0d84084 |
| Task 2 components | round 2: d4 drops the literal; d16 caller into key; d18/d22 drop the field; d57 anchor id into key; d59 drops the parameter name; d60 variable into key | d4_literal_distinct, d16_move, d18_field_distinct, d22_field_distinct, d57_move, d59_params_distinct, T6, d60_move_rename red | green | d0d84084 |
| d59_events_distinct | round 3: d59 drops the event id | red | green | d0d84084 |
| d18_quoted_field_same_key | raw `field_name` text back in the subject | `810d27b77182ecd9` vs `3a9976119df08518` (also red before the fix) | green | bf952e08 |
| d43_caller_objects_distinct | subject without the caller object | red (equal loose keys) | green | 544506e0 |
| d43_move_within_caller_object, d43_move_caller | caller routine id in place of its object | both red at the move assertion | green | 544506e0 |
| callee_param_table_subject_marks_an_unresolved_table | `unwrap_or("?")` → `unwrap_or("")` | red (`"d41/callee/2/"` vs `"d41/callee/2/?"`) | green | 544506e0 |
| d41_implicit_rec_binding_is_not_reported | calls.rs makes implicit-rec bindings upgradable | red (2 findings, expected 1) | green | 544506e0 |
| A3 (fingerprint bytes) | `fingerprint_of` hashes the subject key | check-goldens: `cli_a_json_byte_match` (fingerprint lines), `r4_differential`, `gate_sarif_goldens_byte_match`, plus d7_move, d48_move_transitive | green | 4a7beefb |

## Acceptance result

Every issue acceptance item is met. 1 each listed detector keeps its looseFingerprint when a finding moves unchanged to another routine of the same object, or with its routine-bound part renamed where the subject allows. Proving tests: `d4/d18/d22/d60_move_rename`, `d7_move`, `d7_anchor_object_switch`, `d16_move`, `d39–d42_move_rename`, `d43_move_caller`, `d43_move_within_caller_object`, `d48_move_transitive`, `d48_direct_to_transitive`, `d57_move`, `d59_param_reorder`, T6. For d12, primary d45 and d59, a move does not apply, because the anchor is the event; dependency d45 is covered by `d45_dep_anchor_switch`. 2 d39–d42 findings on different record tables get different loose keys: `d39–d42_same_callee_tables_distinct` (P7 shape) and `d41_generic_record_tables_distinct`. 3 `fingerprint` bytes and every non-cli-a golden are unchanged: the golden diff is exactly the 4 cli-a `looseFingerprint` lines, the CDO fingerprint multiset is identical in both the 16-detector and default runs, and the A3 break is red. 4 the tests drive the analyze JSON with discrimination proofs (proof table). Both final reviewers marked every item met.

## Findings register summary

32 entries. Spec panel (4 rounds): S1–S5, O1–O11, I1 (the implementer's stop-and-revise). Final panel (2 rounds; the built-in `code-review` at high ran as the specified step): FR1–FR15. 28 `fixed`, 4 `refuted` (FR2 implicit Rec never reaches d39–d42, pinned by a test; FR4 a publisher's object is always indexed; FR14 required subject for all detectors is a non-goal; FR15 note parsing pinned and documented). Every entry accepted by both reviewers (`sol` = gpt-6.1-sol in the astra slot, `opus` = Opus in the flash slot); none open, no blocking entry deferred.

## Gate results

**Attested gate set on H = `47acbc0b`** (all supervised, queued 0 s):

| gate | exit | seconds | log |
|------|------|---------|-----|
| ci-steps-all | 0 | 307.6 | `.agent/runs/20261024-002419-754270/logs/ci-steps-all/20261024T022808-cfd4765a.log` |
| check-goldens-coverage | 0 | 0.0 | `.agent/runs/20261024-002419-754270/logs/check-goldens-coverage/20261024T023315-d80ad7ca.log` |
| check-goldens | 0 | 0.1 (tree-stamp trust of the green `ci-steps test` on this exact tree) | `.agent/runs/20261024-002419-754270/logs/check-goldens/20261024T023316-0a58948a.log` |
| cdo-gate | 0 | 200.1 | `.agent/runs/20261024-002419-754270/logs/cdo-gate/20261024T023316-c2443fa4.log` |

`git status --porcelain -- . ':!.agent'` empty after the gates. Task gates: 27.8 s and 32.1 s, both 0.

**CDO measurement** (supervisor `bench`, `logs/bench/20261024T014449-288da19d.log`, 94.8 s): branch `d0d84084`, release-fast, the 16 detectors, pinned CDO workspace, vs probe P3's master run (`/work/bench/252/p3/before-*.json`, same `bc3ccb18`, 0 dirty). **`fingerprint` multiset identical at both scopes**; finding counts unchanged (primary 485, all 5266). Distinct loose keys before → after (spec prediction in brackets):

| detector | primary | all |
|---|---|---|
| d12 | 23 → 23 [23] | 23 → 23 |
| d16 | 40 → 40 [40] | 40 → 40 |
| d18 | 10 → 10 [10] | 60 → 59 [59] |
| d22 | — | 58 → 52 [52] |
| d39 | 1 → 1 [1] | 1 → 1 |
| d40 | 57 → **93** [87] | 57 → 93 |
| d42 | 1 → 1 [1] | 1 → 1 |
| d43 | 5 → 5 [5] | 4573 → 4573 [4573] |
| d45 | 147 → 150 [150] | 147 → 150 |
| d48 | 28 → **24** [~4–5] | 29 → **25** [~5] |
| d59 | — | 59 → 72 [72] |

Re-measured after the d18 fix `bf952e08` (`logs/bench/20261024T015123-0276b60d.log`): fingerprint multiset still identical; d18 unchanged (10 → 10 primary, 60 → 59 all), so CDO has no quoted-field d18 pair. The two departures are explained, not defects.

**Default detector selection** (spec measurement items 1 and 3; supervisor `bench`, `logs/bench/20261024T015647-963b2ab3.log`; master `before/alsem` vs branch `after/alsem`, pinned CDO, outputs `/work/bench/252/default/`): findings unchanged (primary 2323, all 30800); `fingerprint` multiset identical at both scopes. Detectors outside the 16: every `(detector, fingerprint, looseFingerprint)` triple identical — 2209 findings at primary, 25906 at all. The 16 listed: fingerprints identical (114 / 4894 findings). (The bench script's own jq filter had a scope bug and printed 0; the comparison was redone on its saved outputs with a corrected filter.) **d40** 93 vs 87: the prediction predates rev 2's caller-table part, which separates more findings. **d48** 24/25 vs ~4–5: the spec recorded that its prediction could not see the loop object (not in the JSON); the subject includes it, so more keys stay apart. d4, d7, d41, d57, d60: 0 findings on CDO.

## Discoveries

```json
[
  {
    "subsystem": "l5 / d43 (IsHandled skip: dispatch edges walked)",
    "locator": "src/engine/l5/detectors/d43.rs (the dispatch-site walk follows direct-call, object-run-resolved and dependency-export edges, not variable-typed-call)",
    "symptom": "A codeunit that raises another object's IsHandled-guarded integration event through a codeunit VARIABLE (e.g. Mgr.RaiseOnBeforePost(...) where Mgr: Codeunit \"Posting Mgr\") gives no d43 finding: the call is a variable-typed-call edge, which d43 does not walk. Only bare calls (same object, or a table and its tableextension) reach it.",
    "kind": "bug",
    "origin_issue": 252,
    "reproducer": "Found while adding d43_caller_objects_distinct for #252: an in-workspace codeunit calling another codeunit's guarded raiser through a codeunit variable, and the same with a dependency event, gave 0 or 1 d43 findings, never 2; the table + tableextension shape (bare calls) gives 2.",
    "pre_existing": true,
    "capability": "d43 sees guarded event dispatch reached through every resolved call kind, including variable-typed calls",
    "acceptance": "A codeunit raising a guarded event via a codeunit variable gives the same d43 finding as the bare-call shape; a test pins both; CDO d43 counts are re-measured and each new finding triaged (triage-findings) before shipping as default."
  },
  {
    "subsystem": "model / call bindings (implicit Rec arguments)",
    "locator": "src/program/body/ir_walk.rs (bound_rec_var = None when is_implicit_rec); src/program/model/calls.rs:252-258 initial_binding_state ('non-record-arg'), ~305 upgrade_bindings (never upgrades it); d39.rs:115, d40.rs:101, d41.rs:82, d42.rs:152 require 'resolved'",
    "symptom": "A routine that forwards its implicit Rec/xRec to a helper (Helper(Rec)) never produces a d39-d42 finding, because the binding stays 'non-record-arg'. Forwarding a local record of the same table does. The model already knows the implicit Rec's table (record_types.rs pass 3 fills it from the object's own table or SourceTable).",
    "kind": "enhancement",
    "origin_issue": 252,
    "reproducer": "tests/cli/cli_loose_fingerprint.rs d41_implicit_rec_binding_is_not_reported: a table procedure passing Rec to a Reset helper gives no d41 finding, while passing a local Cust: Record Customer gives one.",
    "pre_existing": true,
    "capability": "an implicit Rec/xRec argument binds to its known table like a declared record variable, so the chain detectors (d39-d42) see it",
    "acceptance": "Helper(Rec) on a table routine and on a page with SourceTable gives the same d39-d42 findings as a local record of that table; d41_implicit_rec_binding_is_not_reported is deliberately updated; CDO d39-d42 counts re-measured and new findings triaged (> 30% FP ships the change behind opt-in per CLAUDE.md)."
  }
]
```

## Not delivered

- Nothing from the issue's acceptance. Stated limits (README and spec): an unresolved caller table is `?`; same-named d59 event overloads with the same guard-parameter name share a key; a renamed record variable keeps the key only when its table is known (d4, d18, d22, d60); renaming an event (d12, d45, d59) or an obsolete callee (d16) is a new key by design; the other 38 detectors keep the #247 path.
