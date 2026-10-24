# Issue #271 — d43 sees guarded event dispatch through every resolved call kind

## Claim

```json
{"run_id":"20261024-150827-745d83","issue":271,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/271-agent-discovery-l5-d43-ishandled-a1","worktree":"/work/al-sem-issue-271-a1","body_hash":"bde979ee5e053058f20db4808a4d9ca61fbae2c9b1eb323e42af6b5429c0dfc6","reconciled":[]}
```

Issue body hash (claim `body_hash`): `bde979ee5e053058f20db4808a4d9ca61fbae2c9b1eb323e42af6b5429c0dfc6`.
Base (`make-worktree`): `f40087d567d0768e8ff6c8f909d8ad2c7c6fccf7` (master after #265).
Reviewers (operator instruction, confirmed for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator (2026-10-24, "i confirm"): queue #264 → #265 → #271 → #272. Claimed with a one-off `--loop-max 4` (3 used since the reset).

## Classification

**Bounded.** A one-line edge-kind admission in one detector (`d43.rs` `enumerate_dispatch_sites`, :86-88), prototyped and measured: 0 CDO movement, 1 true-positive fixture gain, no golden movement predicted.

## Assumption probes

Run by the design author (opus). Evidence under `/work/bench/271/` (`ws/{A..I}`, `measure/`, `sites.tsv`, prototype in `proto/` + `target-proto/`, base binaries in `before/`).

| probe | result | evidence |
|---|---|---|
| what d43 detects | a caller guarding table writes on an IsHandled actual where a subscriber of that event sets IsHandled := true but does not write the guarded table | d43.rs module doc; `enumerate_dispatch_sites` :71-209 |
| edge kinds the walk follows | `direct-call`, `object-run-resolved`, `dependency-export`; `dependency-export` is dead (never built) | combined_graph.rs builds direct-call / variable-typed-call / interface-dispatch / object-run-resolved / object-run-unresolved / implicit-trigger / event-dispatch only; l5/snapshot.rs:1878 |
| what `Cu.Method()` / `Rec.Method()` / `Page.Method()` / `this.Method()` become | `PCallee::Member` → `DispatchKind::Method`; `Resolution::Resolved` → typed edge `variable-typed-call`; the program engine resolves all of them | program_calls.rs:1641; combined_graph.rs:734-752; `alsem graph` on ws/{B,C,E} |
| other kinds the walk misses | `interface-dispatch` (a `Maybe` candidate set; still the edge kind for #264's closed enums). Object runs: publishers are never OnRun. implicit-trigger / event-dispatch are not caller→publisher calls | code; #264 diff |
| should d43 follow variable-typed-call | yes: the same exact resolved call as a bare call; argument indices exclude the receiver, so IsHandled binding is unchanged; opt-in d61 already reads every resolved call edge (`resolved_call_edge_by_callsite`) | ir_walk.rs:1078; d61.rs:184; prototype `confirmed` on every shape |
| should d43 follow interface-dispatch | no (non-goal): a may-call set, which d43's ladder would call `confirmed` (overclaim); CDO population 0 | F-iface 0 → 1 confirmed with `D43_PROTO=if`; CDO `vtif` = `vt` |
| repro on master (release-fast) | A bare 1; B codeunit var 0; C record var (table event) 0; D table + tableext 2; E page var 0; F interface 0; G bare + codeunit var 1 (the issue's "never 2"); H dependency publisher via codeunit var 0; I `this.` 0 | `/work/bench/271/before/alsem analyze <ws> --detector d43-event-ishandled-skip` |
| repro with variable-typed-call added | A 1, B 1, C 1, D 2, E 1, F 0, G 2, H 1, I 1, all `confirmed` | prototype, `D43_PROTO=vt` |
| tests/goldens pinning d43 | d43 findings only in `tests/r4-goldens/ws-event-ishandled*.r4.golden.json` (6 files), none moved by a member-call raise; stats goldens cover no affected fixture | grep; before-vs-prototype diff over all 216 r0-corpus fixtures |
| corpus movement under the prototype | ws-d61 d43 0 → 1 (true positive); ws-d54, ws-d55 d43 `skipped.other` 0 → 2; their r4 goldens run other detectors only → no golden should move | per-fixture loop |
| text to update | `tests/cli/cli_loose_fingerprint.rs:1350-1351` comment ("d43 does not walk variable-typed-call"); d43 module doc Substrate section | — |
| CDO master (`bc3ccb18`, dirty 0, default cross-app) | primary 5 d43 (all `likely`); all 4573 (6 confirmed, 4567 likely); detectorStats candidatesConsidered 22, skipped.other 9319 | `/work/bench/271/measure/before/` |
| CDO with variable-typed-call | primary 5, all 4573, finding sets unchanged; skipped.other 9319 → 9629 (+310); with interface-dispatch too: unchanged; prototype gate off: identical to master | `/work/bench/271/measure/{after-vt,after-vtif,proto-off}/` |
| why +310 sites and 0 findings | per-site census of the 310 variable-typed-call sites: 0 with a setter subscriber (44 guard + writes, 53 guard no writes, 213 no guard); CDO's own events have no in-model subscriber that sets IsHandled | `/work/bench/271/sites.tsv` (temporary census in the prototype) |
| sibling workspaces | 0 before and after; they have no `.alpackages` (weak evidence) | loop log |

## Design

**Goal.** d43 finds a guarded IsHandled dispatch through every exact, resolved call kind: bare calls, codeunit / record / page variables, `this.`, and dependency publishers reached through a variable.

**Non-goals.** Following `interface-dispatch` (a may-call candidate set; d43's ladder would call it `confirmed`; CDO population 0 — a later issue could add it with a confidence cap). Changing how d43 reads the caller's writes (`writes_physical_tables_of(caller)`, an existing imprecision, the same for every call kind). Manual-binding subscribers (an existing FP axis, independent of call kind).

**Design.** In `enumerate_dispatch_sites`, accept `edge.kind == "variable-typed-call"` next to `direct-call` and `object-run-resolved`; drop d43's own dead `dependency-export` arm (d43 only — the other dead `dependency-export` arms in capability_cone.rs:1212, 1225, digest.rs:2845, 4342, ordering_inter.rs:134, 304 are left as they are; Discovery D1). Update the d43 module doc (Substrate: admitting member calls is a deliberate departure from the al-sem port, which `d43.rs:1-2` and `:69-70` call "faithful" — say so under the al-sem-retired policy), the comment at `cli_loose_fingerprint.rs:1350-1351`, and CHANGELOG (Fixed). Precedent: `pd_solve.rs:147` already treats `direct-call | variable-typed-call` as one call kind; d61 reads the first targeted edge per call site (`detector_context.rs:2047-2061`).

**Acceptance matrix.** Items 1–4 fail under one break: remove `"variable-typed-call"` from the allow-list. Item 5 uses the inverse break (also admit `interface-dispatch`). Record red and green for each.

| # | item | test | break that must fail it |
|---|---|---|---|
| 1 | codeunit-variable shape = bare-call finding | NEW `d43_codeunit_variable_matches_bare_call` (cli_loose_fingerprint.rs): A and B shapes each 1 finding with equal `confidence.level` (`confirmed`), `severity`, `affectedTables`, `primaryLocation.routineName` (the setter); their `evidencePath[0]` routines in different objects | B gives 0 |
| 2 | the issue's #252 shape | NEW `d43_caller_objects_distinct_codeunit_variable` (G: bare call in PostingMgr + codeunit-var call from PostingCaller): 2 findings, one anchor, different `looseFingerprint` | 1 finding |
| 3 | dependency event through a codeunit var | NEW r4 test in `tests/r4/r4_cross_app_program.rs` with its OWN helper (not `ishandled_workspace`, which already has a guarded bare `OnBeforeSend` dispatch inside the dependency, `:593-595`, and gives `["confirmed"]` without this change): the dependency publishes `OnBeforeSend` on `Dep Sender` with NO dependency-internal guarded dispatch; the only guarded write is in the workspace `Ws Caller`, which calls `Sender.OnBeforeSend(IsHandled)` through a codeunit variable. Asserts `d43_confidence` → `["confirmed"]` AND that the finding's evidence path starts at `Ws Caller` | `[]` (asserted under the break) |
| 4 | record-variable, page-variable and `this.` shapes | table-driven test over C (`D.OnBeforePost(IsHandled)` on a table publisher), E (a page publisher called through a page variable) and I (`this.OnBeforePost(..)`; its `app.json` declares runtime 14.0, which `this` needs): 1 `confirmed` each | 0 |
| 5 | interface dispatch stays out (deliberate) | NEW `d43_interface_dispatch_not_followed` (cli_loose_fingerprint.rs, next to item 1): F gives 0 | inverse break: also admitting `interface-dispatch` → 1 |
| 6 | no golden moves | `scripts/check-goldens` (no regen) | — (a moved golden is a discovery to explain, never a regen) |
| 7 | CDO re-measured, new findings triaged | Measurement plan | — |

**False-positive shapes (for triage).** A different receiver instance with a manual-binding subscriber (existing axis); the caller writes the guarded table outside the guard (existing write imprecision); the IsHandled actual passed from a field or other expression (the binding falls back to the argument text; a bare PARAMETER instead gets `source_kind = "parameter"` and its own name, which d43 prefers, ir_walk.rs:1855-1884, d43.rs:134-161 — neither shape alone makes a false positive). None is specific to variable calls.

**Measurement plan.** Before = base `f40087d5` release-fast (`/work/bench/271/before/`); after = branch HEAD release-fast in its own folder. `python /work/alsem/.claude/skills/cdo-measure/scripts/measure.py run --bin <dir> --label before|after --out <dir>` on the pinned CDO workspace, then `measure.py comp`; both sides `bc3ccb18`, dirty 0. Record d43 counts at `--scope primary` and `--scope all` (default cross-app) and d43 `detectorStats` (expected candidatesConsidered 22 → 22, skipped.other 9319 → 9629). Diff the d43 fingerprint sets; EVERY new d43 finding is triaged against the AL source with triage-findings. Predicted new set: empty (5 → 5, 4573 → 4573); a non-empty set contradicts the prototype and is explained first.

**Pre-stated fallbacks.** (a) CDO triage >30% FP among NEW findings → do not ship and do not demote d43 (already default with 4573 findings); find the shared FP shape and fix its root cause (e.g. exclude that receiver or binding shape); if no in-scope root-cause fix fits, stop and send it back to design as architectural. (b) Any golden moves → triage it line by line before any regen. (c) After-side detectorStats DELTA ≠ +310 skipped → compare against the prototype census `/work/bench/271/sites.tsv`. The plan compares the delta, not the totals: `candidatesConsidered + skipped.other` (9341 / 9651) is 9 above the census line counts (9332 / 9642) on both sides, so the totals alone do not fire (c).

Discovery D1: the dead `dependency-export` string arms outside d43 (capability_cone.rs:1212, 1225; digest.rs:2845, 4342; ordering_inter.rs:134, 304) could be removed or asserted unreachable.

## Timeline

- 15:08 claim #271 attempt 1, worktree at `f40087d5`. Probes + design dispatched (opus).
- alsem-main asked to take #289 first; operator decision (AskUserQuestion): "Finish #271 first" → #271, then #289, then #272.
- 15:35 classified bounded; design in `## Design`. Spec panel r1: both approve-with-fixes (S1 important: item 3 could not discriminate; S2, S3, O1–O7 minor). Revision 2.
- Spec panel r2: both approve (10/10 accepted). **Converged**, spec_rounds 2 of 5. Opus N1 (bookkeeping): D1 goes into the `## Discoveries` JSON at the freeze.
- Plan (1 task) `742ee7b3`; plan_tasks 1. Bounded, one task → no per-task reviewer.
- Task 1 (opus) `30b3db52`, 1 attempt: `task-affected` exit 0 (4161 passed), `check-goldens` 1134 passed, no golden moved; check-diff clean. Deviations: items 1, 2, 4, 5 use the file's `run_252` helper (guarded writes to `Customer`/`Item` instead of the probes' `PostingEntry`; shapes otherwise A, B, C, E, F, G, I). The implementer's first gate run is void (shared-scratchpad log names collided with another agent); the reruns are clean.
- ci-steps-task at `30b3db52` exit 0 (31 s, supervised; `logs/ci-steps-task/20261024T155439-204b375c.log`). Draft PR #291.
- Final round 1 (final_rounds 1 of 5): sol approve (no findings); Opus approve with fixes (O1 doc wording "bare calls and object runs"; O2 item 4 stops at its first failing row); code-review skill (7 candidates, `final-r1-codereview.md`): CR1 item 5 has no stated precondition (could pass vacuously) → fix; CR2 item 4 does not check the caller → fix (with O2); CR3 shared exact-call predicate → deferred, discovery; CR4 dead dependency-export arms elsewhere → refuted (Discovery D1); CR5 item 3 builds twice → fix; CR6 = O1 → fix; CR7 the plan points at the ledger → refuted (the ledger is committed at the freeze). Fixes dispatched.
- `342d1ce5` (FC1, FC2/O2, FC5, FC6/O1): proofs re-run (removal break: all four tests red, item 4 all three rows in one run; inverse break: item 5 red with the precondition holding). ci-steps-task exit 0 (`logs/ci-steps-task/20261024T160408-937c5f22.log`), pushed.
- Final round 2 (final_rounds 2 of 5): both approve, every entry accepted (Opus on FC7: holds because the freeze commits the ledger). **Final panel converged**: 17 entries; FC3 deferred (Discovery D2), FC4 and FC7 refuted (accepted by both), the rest fixed. CDO before/after (item 7) dispatched (sonnet) on `342d1ce5`.
- CDO before/after done (see Measurements): 0 findings moved, only `skipped.other` +310.
- Step 11: origin/master unmoved → no rebase. **B = `f40087d567d0768e8ff6c8f909d8ad2c7c6fccf7`, H = `342d1ce533cb7828a4e78374c04d54396f75f77c`.** Full set started.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| item 1 `d43_codeunit_variable_matches_bare_call` | removal: `variable-typed-call` dropped from the allow-list (grep count 0) | `left: (1, 0) right: (1, 1)` (codeunit-var shape 0 findings) | 9/9 after revert | `30b3db52` |
| item 2 `d43_caller_objects_distinct_codeunit_variable` | same | `left: 1 right: 2` | ok | `30b3db52` |
| item 3 `d43_follows_a_codeunit_variable_call_to_a_dependency_publisher` (own helper `ishandled_var_workspace`; caller id asserted to start with `{WS_GUID}:Codeunit:50230#`) | same | `left: [] right: ["confirmed"]` | ok | `30b3db52` |
| item 4 `d43_record_page_and_this_calls` (C, E, I; `this.` row runtime 14.0) | same | `d43-271-c: []`, `left: 0 right: 1` | ok | `30b3db52` |
| item 5 `d43_interface_dispatch_not_followed` (control) | inverse: `interface-dispatch` also admitted (grep count 2) | fails at :1609 with 1 `confirmed` finding | ok | `30b3db52` |

Red phase on the base: items 1–4 red with the outputs above; item 5 green (control).

## Measurements (item 7, CDO before/after, sonnet; `/work/bench/271/final/`)

Before = base `f40087d5` release-fast; after = `342d1ce5` release-fast. The pinned CDO workspace @ `bc3ccb18`, dirty 0 on both sides (meta.json); both binaries al-sem 2.0.3. Commands: `measure.py run --bin <dir> --label before|after --out /work/bench/271/final`, then `measure.py comp`.

| | before | after |
|---|---|---|
| d43 findings, `--scope primary` | 5 | 5 |
| d43 findings, `--scope all` | 4573 | 4573 |
| d43 candidatesConsidered | 22 | 22 |
| d43 skipped.other | 9319 | 9629 (+310, as predicted) |
| all findings primary / all | 2323 / 30800 | 2323 / 30800 |

d43 fingerprint diff: 0 added, 0 removed at both scopes → nothing to triage (prediction held). No other detector moved; `fingerprint.json` identical; the only differing line in the full `analyze-all` JSON (ignoring `generatedAt`) is the d43 `skipped.other` counter. Fallbacks (a)–(c) did not fire.

## Findings register summary

`findings.json`: 17 entries. Spec panel (2 rounds): S1–S3 (sol), O1–O7 (opus) — all `fixed`. Final panel (2 rounds; code-review skill at high + sol + opus): FC1, FC2, FC5, FC6 `fixed`; FC3 `deferred` (minor; Discovery D2); FC4, FC7 `refuted` (both reviewers accepted the refutation). Every entry accepted by sol and opus.

## Gate results

| gate | head | exit | log |
|---|---|---|---|
| ci-steps-task | `30b3db52` | 0 (31 s) | `.agent/runs/20261024-150827-745d83/logs/ci-steps-task/20261024T155439-204b375c.log` |
| ci-steps-task | `342d1ce5` | 0 (43 s) | `.agent/runs/20261024-150827-745d83/logs/ci-steps-task/20261024T160408-937c5f22.log` |
| **ci-steps-all** (H) | `342d1ce5` | **0** (434 s, supervised) | `.agent/runs/20261024-150827-745d83/logs/ci-steps-all/20261024T161133-810266a1.log` |
| **check-goldens-coverage** (H) | `342d1ce5` | **0** (supervised) | `.agent/runs/20261024-150827-745d83/logs/check-goldens-coverage/20261024T161847-fce0035b.log` |
| **check-goldens** (H) | `342d1ce5` | **0** (tree stamp trusted; supervised) | `.agent/runs/20261024-150827-745d83/logs/check-goldens/20261024T161847-08012543.log` |
| **cdo-gate** (H) | `342d1ce5` | **0** (656 s, supervised) | `.agent/runs/20261024-150827-745d83/logs/cdo-gate/20261024T161848-bf5c5b30.log` |

Tree clean after the full set. Grammar c89c2ecb; the pinned CDO workspace @ bc3ccb18.

## Discoveries

```json
[
  {
    "subsystem": "l4/l5 edge-kind matching (dead dependency-export arms)",
    "locator": "src/engine/l4/capability_cone.rs:1212, :1225; src/engine/l5/digest.rs:2845, :4342; src/engine/l5/ordering_inter.rs:134, :304",
    "symptom": "Several modules still match on the edge kind \"dependency-export\", which no builder emits (combined_graph.rs builds none; l5/snapshot.rs:1878 says it is never built). The arms are dead code and suggest edges that do not exist.",
    "kind": "enhancement",
    "origin_issue": 271,
    "reproducer": "grep -rn 'dependency-export' src/ lists the match arms; grep for a TypedEdge built with that kind finds none.",
    "pre_existing": true,
    "capability": "Either remove the dead dependency-export arms or assert they are unreachable, so the edge-kind vocabulary matches what the combined graph builds.",
    "acceptance": "No module matches on a TypedEdge kind the combined graph never builds; suite and goldens unchanged; CDO histogram and finding set unchanged."
  },
  {
    "subsystem": "l4/l5 edge-kind matching (exact resolved call kinds)",
    "locator": "src/engine/l5/detectors/d43.rs:95-97; src/engine/l4/pd_solve.rs:147; src/engine/l5/ordering_inter.rs:137",
    "symptom": "The set of 'exact, resolved call' TypedEdge kinds (direct-call, variable-typed-call, …) is a hand-written string list in each consumer. d43 missed member calls (#271) because its list lacked variable-typed-call; the next new edge kind can be missed the same way.",
    "kind": "enhancement",
    "origin_issue": 271,
    "reproducer": "Compare the edge-kind string lists in d43.rs:95-97, pd_solve.rs:147 and ordering_inter.rs:137 with the kinds combined_graph.rs builds.",
    "pre_existing": true,
    "capability": "One predicate next to where TypedEdges are built (e.g. TypedEdge::is_exact_resolved_call) used by every consumer that means 'an exact, resolved call'.",
    "acceptance": "d43, pd_solve and ordering_inter use the shared predicate; their outputs, the suite, the goldens and the CDO finding set are unchanged."
  }
]
```

## Not delivered

All acceptance items 1–7 delivered. `interface-dispatch` is deliberately not followed (non-goal; item 5). FC3 (shared exact-call predicate) deferred as Discovery D2; the dead `dependency-export` arms outside d43 are Discovery D1.
