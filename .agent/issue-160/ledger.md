# Issue #160 ledger: attempt 7 (epic step 4: `ExecRelation`)

Earlier attempts' ledgers are in git history: step 0 at `5f096af9` (PR #165), step 1 at `576dd296` (PR #170), step 1b at `6c6bce17` (PR #172), step 2 at `d4d8b71f` (PR #174), step 3 at `c554b704` (PR #178). This file replaces them for attempt 7.

Part of #160. This PR delivers epic step 4 only (spec
`docs/superpowers/specs/2026-10-18-issue-160-analysis-session-design.md` §5.5 and §9, row **4**).
#160 stays open until its last step (owner decision, 2026-10-18): the PR says "Part of
#160" and carries no closing keyword.

## Claim

```json
{"run_id":"20261020-034103-38ab8c","issue":160,"attempt":7,"session":"local:71f7a720-41fd-4208-b8ee-a38bb3975305","branch":"issue/160-alsem-batch-build-the-model-onc-a7","worktree":"U:\\Git\\al-sem-issue-160-a7","body_hash":"2ec746e00deded290965d462f57e8ade73a96d606db1b7f89361f532a6d32883"}
```

Base `c554b704d86d954fece216ab148f15b536c2854c` (make-worktree, origin/master). Build
output: `H:\al-sem-targets\al-sem-issue-160-a7` (`target_root`).

Reviewers: the operator's standing choice (2026-10-14, used on steps 1b, 2 and 3):
gpt-6.1-sol via pi (slot astra) and Opus 5.5 via the Agent tool (slot flash). Register
marks are keyed `sol` / `opus`; attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**Architectural.** Step 4 introduces `ExecRelation` (`src/engine/l4/exec_relation.rs`) with
`Transfer::{Cone, Primary}` and makes the cross-app base, the detector context, r3a5 and the
L4 analysis read one relation instead of each building its own graph. It touches several
subsystems (l4 graph building, detector context, r3a5 projection, counters) and carries a
byte-identity contract across every golden family plus the CDO detector baseline. A child
spec is written: `docs/superpowers/specs/2026-10-20-issue-160-step4-exec-relation-design.md`.

## Assumption probes

All read-only against `c554b704` in `U:/Git/al-sem-issue-160-a7` (2026-10-20). No build
or test was run: every probe is a source read or a grep. "Falsified" rows reshaped the
child spec as stated.

| probe | result | evidence |
|---|---|---|
| P1 The cross-app base cuts the model's calls to primary callers | held | `capability_cone.rs:3571-3582` (filter `!dep_routine_ids.contains(&e.from)`), `:3665` (`resolved.calls = Arc::new(calls)`) |
| P2 The base's primary graph gets the FULL event graph (dispatch from any publisher, dependency publishers included) | held | `:3583`, `:3585`; event loop `combined_graph.rs:367-400` has no caller filter; `R3a5CrossAppBase` doc `:3455-3465` (dependency publishers with an out-edge = dispatch) |
| P3 Epic: step 4 "replaces the context's rebuild (`detector_context.rs:935-945`)" | **falsified (stale)**: step 2 already removed the cross-app context graph; only the single-app arm of `context_graph` builds a graph | `detector_context.rs:1110-1124` (cross-app returns `Arc::clone(&b.graph)`); child 0-2 §4.4.8 ("the context's graph is gone"). Reshaped §3/§4.5: step 4 deletes `context_graph` (single-app arm) |
| P4 The dependency half is a second `build_combined_graph` with no events, only `typed_edges` kept, appended after the primary ones | held | `:3598-3617` |
| P5 The context's `resolved_call_edge_by_callsite` comes from `model.calls`, i.e. the primary half cross-app | held | `detector_context.rs:1169`, `:1462-1463`; P1 |
| P6 `edges_from_order` = first appearance of `from` in emission order (calls, then dispatch) | held | `combined_graph.rs:405-415`; d17 iterates it `d17.rs:104-107` |
| P7 d43 reads `ctx.graph.typed_edges` (Cone payload) | held | `d43.rs:85` |
| P8 Step 4 saves no graph-building work: two internal half-builds remain cross-app | held (stated as non-goal) | P4: both builds are needed for byte identity of `typed_edges`; only the counter drops |
| P9 The dependency half's uncertainty edges are built and dropped today | held | `:3617` keeps only `.typed_edges`; `build_combined_graph` emits uncertainty for every to-less/ambiguous edge `combined_graph.rs:270-361` |
| P10 `build_combined_graph` reads only `resolved.edges` (not diagnostics, upgraded bindings, external targets) | held | grep over `combined_graph.rs:259-1000`: `resolved.edges` only (`:273`, `:561`, `:579`) |
| P11 An empty half contributes no typed edge (skipping it is byte-neutral) | held | `build_typed_edges` iterates `resolved.edges` only for edges, plus event dispatch, which the dependency half never gets (`no_events`, `:3609-3612`) |
| P12 The dependency half clones `upgraded_bindings` (15.5 MB on CDO) and `external_targets` for nothing | held | `:3604-3607`; step 2 census `base.upgraded_bindings` 15 519 744 B (`docs/measurements/2026-10-19-issue-160-step2.md:191`) |
| P13 The cone reads only `graph.typed_edges` | held | `compose_cone_over_graph` `:2822-2832` uses `graph` once, in `build_typed_edge_graph`, which reads `graph.typed_edges` only (`:1229-1257`); 13 call sites (`:3847`, `:4281`, `detector_context.rs:1230`, 10 in `cone_derived.rs`) |
| P14 Keeping `CombinedGraph` as the carrier is the low-risk path | held; count corrected in round 1 (S1-18): the first "49" counted any occurrence of a field name (`rg -l 'edges_by_from\|edges_from_order\|typed_edges\|uncertainty_edges' src`), not reads of the carrier | 32 `src` files read a field through a `graph.` value (`rg -l 'graph\.(edges_by_from\|typed_edges\|edges_from_order\|uncertainty_edges\|nodes)' src`, opus round 1); 13 detectors read `graph.edges_by_from`; 12 files build `CombinedGraph` literals; `DetectorArtifacts.graph: Arc<CombinedGraph>` `detector_context.rs:704` |
| P15 The L4 solve reads `edges_by_from` and `uncertainty_edges`, never `typed_edges` | held | `summary_runner.rs:1427`; `db_effect_solver.rs:1097`, `:3004-3006`; grep for `typed_edges` in `summary_runner.rs`, `db_effect_solver.rs`: none outside test literals |
| P16 Tarjan over the same graph is deterministic; `SccResult: PartialEq` | held | `analysis.rs:141-152`; `scc.rs:69` (walks `nodes` in order); `scc.rs:39` derives `PartialEq` |
| P17 Single-app SCC is computed inside `build_l4` only when none is handed in | held | `analysis.rs:61-66`; `session.rs:295-298` passes `None` single-app (citation corrected in round 1, S1-14; `:302-305` is the det-fill span) |
| P18 `base.event_graph` is a clone of `resolved.events.graph`; readers are r3a5 and the census only | held | `:3583`, `:3675`; grep: `:3826`, `:3744`; nothing mutates `resolved.events` between `:3583` and `:3665` |
| P19 `base.nodes` = `ws.routines` ids in row order; readers are r3a5's cone and the census only; the cone sorts nodes | held | `:3587`, `:3849`, `:3777`; sort `:1256-1257` |
| P20 The snapshot (legacy cut) builds its own graph over `resolved.calls` | held | `build_r3a3_source_only_base` `:4216-4224`; callers `snapshot.rs:1153`, `snapshot_full.rs:84`, `:4346` |
| P21 `diff` without `--strict` builds a `View` and fills nothing, so an eager relation in `View::new` would add a build | held | `diff/cli.rs:122-133`; counter row `diff ws ws` bcg 1/side = the snapshot (`cli_build_counters.rs:418-436`) |
| P22 `into_query_parts` drops the base; `cross_app_query_drops_the_base` asserts `(1, 1)` and stays true when the relation is dropped too | held | `session.rs:451-460`; `capability_cone.rs:5049-5059`; `l4.scc` becomes the one remaining SCC owner |
| P23 No existing fixture has a WORKSPACE subscriber on a source-bearing DEPENDENCY publisher | **falsified** (round 1, S1-8): `write_fixture_107(.., true)` has workspace subscriber `HandleWs` on dependency publisher `OnDepRec2`. Reshaped §5.2: it builds on that writer's pattern; what is new is a dependency caller of the publisher, a `Commit` in the subscriber and a dependency `local` helper | `capability_cone.rs:5144-5181` (`WS_AL_107`), `:5189-5226` (`write_fixture_107`) |
| P24 The temp proof cannot see a dependency caller or an event-dispatch edge | **falsified as stated** (round 1, S1-6): the proof takes every model routine, dependency routines included, so a dependency `local` helper called by a dependency sibling would be proven if dependency call edges entered Primary. What holds is narrower: removing `event-dispatch` edges cannot move it. Reshaped §5.2: the limit is scoped to the "Primary callers only" break, and a dependency `local` helper makes (3) fail under the "every caller" break | `closed_world_temp.rs:105-141` (every routine; `resolved_callsites` from Primary `edges_by_from`, `:113-120`), `:206` (`kind != "procedure"`), `:244-246` (dead local refused), `:251` (`direct`/`method` only); event edges only for `event-subscriber` routines `src/program/model/events.rs:380-381`, kind from the attribute `src/program/body/ir_walk.rs:2762-2767` |
| P25 `ws-d17-drift` has ONE caller; a new `tests/r0-corpus` dir moves three golden families | held | `tests/r0-corpus/ws-d17-drift/src/caller.al`; golden `findingCount` 1; CLAUDE.md "A NEW `tests/r0-corpus/` fixture moves THREE golden families" |
| P26 The `build_detector_context[_with]` shims have 42 call sites in 13 files | held | grep `build_detector_context\(_with\)\?(` |
| P27 No test or script reads the spans this step renames or deletes | held | grep over `tests`, `scripts`, `src` for `base_graph`, `base_artifacts_facts`, `event_combined_graph`, `core_scc_tarjan`: only `docs/measurements` (historical) and the defining lines |
| P28 Every single-app counter row that constructs a relation fills `l4` (round 1, S1-9: `diff` without `--strict` and `graph` construct none), so moving the single-app Tarjan into `new` moves no `scc` cell | held | `cli_build_counters.rs`: every single-app row has `v2` = 1 (`analyze_single_app`, `digest_single_app`, `prove`, fingerprint, `diff --strict`, `events_single_app`, `policy_check_single_app`, `query_single_app`) |
| P29 A CDO-gated `--lib` test runs under `scripts/cdo-gate` | held | `scripts/cdo-gate:101-102` runs `-p al-sem --lib`; step 3's CDO half `capability_cone.rs:5857-5862` (`crate::cdo::cdo_ws_or_enforce()`) |
| P30 Epic sketch: `adj: [Adjacency; 2], scc: [SccResult; 2]` eager in `new` | **reshaped (amendment M3/M4)**; round 1 (S1-5) corrected the reason: policy check already builds the Cone pair twice per view (det cone + RAW cone), so M3 is a retention-versus-rebuild tradeoff, not "one reader" (see P35). Holding it would keep the typed graph alive through the runs and add a Tarjan to query and d47-only views; step 4 holds the Primary SCC only, so query counts `scc` 1, not 2 | `compose_cone_over_graph` builds and drops its typed graph and SCC `:2832-2850`; query row `cli_build_counters.rs:592-601` |
| P31 The epic's step-4 line numbers (`:3459-3469`, `:3496-3515`, `:4019-4021`, `detector_context.rs:935-945`) predate steps 1-3 | **falsified (stale)**; every one re-located in the child spec §3 | `:3571-3582`, `:3598-3617`, `:4221`, `detector_context.rs:1110-1124` |
| P32 The two half-builds cannot interact: every dedupe set in the build is per call site, and a call site has one `from` | held (round 1) | `combined_graph.rs:270` (`interface_open_world_emitted`), `:576` (`interface_typed_emitted`) |
| P33 `build_detector_artifacts` runs the `l4` solve itself, after its cone, from its own `graph`/`L4Request` inputs, so deleting `L4Request.scc` requires it to take the relation | held (round 1, S1-1) | `detector_context.rs:1130-1136` (signature), `:1230` (cone), `:1493` (`build_l4`); `analysis.rs:42-45` |
| P34 The single-app SCC is built after the det cone and owned by `l4` alone today; after step 4 it is built before the cone and co-owned by the relation | held (round 1, S1-4); 794 455 B on CDO | `analysis.rs:61-66`; `events.rs:381` (`release(L4)`) then `:391`; `pipeline.rs:242` keeps the graph cell through the RAW build, `:309` releases it; `docs/measurements/2026-10-19-issue-160-step2.md:85` |
| P35 Policy check builds two cones over one view graph (det + RAW), each building the typed graph and a Tarjan | held (round 1, S1-5) | `pipeline.rs:265-271`; `capability_cone.rs:2832-2850`; counter row policy `cone` 2 `scc` 3 (`cli_build_counters.rs:533-561`) |
| P36 Single-app models never cut, even with dependency ABI rows, so the oracle's single-app `dep_ids` is empty | held (round 1, S1-10) | `detector_context.rs:1118` (`build_combined_graph` over `resolved.calls`, no filter) |
| P37 `build_typed_edge_graph` has two legacy callers besides the cone: `project_r3a3` and `compute_r3a3_real_matrix` | held (round 1, S1-11) | `capability_cone.rs:4033`, `:4404` |
| P38 `cross_app_commit_span_sees_dependency_writes` pins the cone's view of dependency typed edges; its recorded seam names `context_graph`/`base.graph` | held (round 1, S1-15) | `tests/cli/cli_digest_query_dependencies.rs:466-474` |

## Design

See the child spec.

## Base/Head SHAs

| round | B | H |
|---|---|---|
| final gates (step 11) | `c554b704d86d954fece216ab148f15b536c2854c` | `0dbd986ae9d16142c040079ac5e1cc6cce20fe7d` |

## Acceptance matrix

Child spec §6, as delivered:

| # | Epic row 4 item | Evidence |
|---|---|---|
| A4.1 | `Transfer::{Cone, Primary}`, two payloads | `ExecRelation` (`src/engine/l4/exec_relation.rs`) with private fields and `cone()` / `primary()` / `scc()`; the cone takes `&[TypedEdge]` (compile-time, `cb418d6f`); per-field parity oracle (502 cases + CDO, both models) |
| A4.2 | Eager adjacency/SCC | Primary SCC computed in `new`; `build_l4` runs no Tarjan (counter proof (b)); the Cone pair stays inside the cone (deferred by amendment M3) |
| A4.3 | Counters `build_combined_graph` 1, `graph_scc` per M4 | `cli_build_counters`: cross-app `bcg` 2->1 in six rows; new rows `analyze_cross_app_with_a_dependency_half` (bcg 1, break (a) fails `got 2, want 1`) and `analyze_single_app_d47` (scc 2) |
| A4.4 | Base, context, r3a5, L4, query, policy RAW read the relation | consumer wiring `9ac21fdd` + `0c814ec2`; goldens unchanged; §5.2/§5.3/§5.4 fixtures with discriminations against `new` |
| A4.5 | Snapshot on the legacy cut | snapshot path unchanged in substance (mechanical `&[TypedEdge]` adaptation only); cli-b and r3a3 goldens unchanged |
| A4.6 | Dependency uncertainty edges kept | `rel.dependency_uncertainty()`; §5.4 test; census `rel.dependency_uncertainty` 77 904 B on CDO cross-app |
| A4.7 | Byte-identical everywhere | every golden target green with none regenerated; CDO histogram SHA-256 `c032c8db…` both sides; SARIF byte-identical both universes |
| A4.8 | CDO detector baseline identical | cdo-measure: 2277 -> 2277 primary, 30712 -> 30712 all, fingerprints 0 added / 0 removed |
| A4.9 | Dependency-publisher fixture, "Primary callers only" discrimination | `step4_dependency_publisher_primary_rule` (proof rows: "Primary only" fails (1) and (2), (3) green as the stated limit; "every caller" fails (2) and (3)) |
| A4.10 | Two-caller `from_order` fixture, reverse it, d17 flips | `step4_from_order_picks_the_first_caller` (reversing flips the sample to DoB, precondition held) |
| A4.11 | E14 CDO ratchets | `scripts/cdo-gate` exit 0 on H (Gate results); parity line `2 cases, 1 with a dependency half and event dispatch, 1 with dependency uncertainty` |
| A4.12 | E17 Transfer rules reproduce today's adjacency per field | §4.3 field table + per-field parity + §5.2 |
| (I6) | Three single-app peak bounds | analyze / policy check / events fanout `--single-app`: every head median ≤ parent + 3 % on process and post-attach peak |

## Timeline

- 2026-10-20 03:41 preflight ok; claim attempt 7; make-worktree base `c554b704`.
- Classification architectural; child spec drafting dispatched (opus spec author).
- Child spec drafted (521 lines), probes P1-P31 (3 falsified: P3 and P31 stale epic citations, P30 reshaped into amendments M3/M4). Process slip recorded: the spec author corrected two citation line numbers in this ledger with `sed -i` instead of the Edit tool (CLAUDE.md rule); the conductor checked the two rows and they are correct.
- Spec round 1 (spec_rounds charged, 4 left): sol 11 findings (6 important), opus 12 (3 important); merged S1-1..S1-18 (7 important); 18 fixed by the spec author (S1-12 was a briefing path error). Amendments to the epic now M1-M9.
- Spec round 2 (3 left): both accepted S1-1..S1-18; new S2-1..S2-4 (minor), all fixed.
- Spec round 3 (2 left): both accepted all 22 against spec hash `0bcdf34b92da722c`, no new findings. **Converged.**
- Plan `docs/superpowers/plans/2026-10-20-issue-160-step4-exec-relation.md` (spec author): 7 tasks; plan_tasks charged 7 (5 left). Step 6 (acceptance tests) is Task 1: the three fixtures pin today's behaviour on the base code first, then each later task turns its own tests red before the code. halt-check: not halted. Implementer dispatched (opus, teammate `impl4`).
- Tasks: T1 `b2369db3` (fixtures pinned on the base; every spec literal matched), T2 `43221921` (commit A: relation over a copied assembler + independent oracle, 502 cases + CDO; carries the spec and plan), T3 `484ca663` (commit B: one assembler after the recorded text-identity check), T4 `cb418d6f` (cone takes `&[TypedEdge]`; carries the oracle-doc fix), T5 `9ac21fdd` (every consumer reads the relation; counters; renames), T6 `d11d57b0` (epic amendments M1-M9, child 0-2 table, CHANGELOG), docs `98940afb` (two new counter rows, numbers-file skeleton). Each per-task gate green (task-affected, check-diff `[]`, no golden moved).
- Independent sonnet reviews: T1-T2 (`task-review-t12.md`: 1 important, 5 minor; fixed in `7b7da36e`), T3 (`task-review-t3.md`: 3 minor; doc fix in `cb418d6f`, ledger re-runnable diff command added by the conductor, #3 declined as the reviewer advised), T4-T5 (`task-review-t45.md`: 1 important — no counter test with a non-empty dependency half, so break (a) as specified stayed green — and 4 minor; fixed in `e8e5f0fa`, where break (a) as specified now fails `got 2, want 1`).
- Process note: twice the implementer amended an unpushed HEAD commit to fold in a requested change (T2 `b3946378` -> `43221921`, T4 `71d61521` -> `cb418d6f`); ledger rows name the final SHAs.
- Step 9 task gate on `98940afb`: exit 0, supervised, 113.4 s, log `logs/ci-steps-task/20261020T060252-f1a4d8d8.log`. Branch pushed; draft PR #180 opened so CI runs during the final panel.
- Final panel round 1 (final_rounds charged, 4 left): built-in `code-review` high, sol (pi) and opus (Agent) in parallel. The CDO bench (Task 7, `bench-plan.md`) runs after the panel converges. 19 merged findings F1-1..F1-19 (1 important: the bench plan's cdo-measure `--bin` path); 16 fixed (`0c814ec2` code, `b610a68e` docs, bench-plan.md, one ledger line), 3 refuted with evidence (F1-3 lazy SCC vs I3, F1-4 half-builder out of scope, F1-6 tripwire kept / CLAUDE.md protected). Task gate on `b610a68e`: exit 0, supervised, 112.8 s, `logs/ci-steps-task/20261020T062943-1bee0268.log`; pushed.
- Final panel round 2 (3 left): opus accepted all 41 (refutations F1-3, F1-4, F1-6 upheld; its first attempt died on an expired login and was rerun after the operator's `/login`); sol accepted 39 and re-raised F1-11 and F1-18, fixed in `0729f7f7`.
- Final panel round 3 (2 left): both accepted F1-11 and F1-18, no new findings. **Converged** (41 entries, every one accepted by both).
- Task gate on `0729f7f7`: exit 0, supervised, 111.5 s, `logs/ci-steps-task/20261020T072508-7901094d.log`.
- Task 7 bench (after convergence, through the supervisor `--name bench`), split in two runs to fit the 45-minute timeout: part 1 = release-fast binaries (parent `c554b704` in a scratch worktree, head in its own target dir), cdo-measure findings, histogram SHA, census, the CDO parity line (`--nocapture`); part 2 = `bench_ab.py` peaks.
- Bench part 1 (supervised, exit 0, 554.4 s, `logs/bench/20261020T072631-269bbbeb.log`, head `0729f7f7af8a87429309108907dec513e44d5b07`): A4.8 findings analyze-primary 2277 -> 2277 (fingerprints 0 added / 0 removed / 2270 kept), analyze-all 30712 -> 30712 (0 / 0 / 30684); both sides `bc3ccb18`, dirty 0. A4.7 histogram SHA-256 `c032c8db56f1fd070fc4b7e4f1971a73019a4c8569db81a6aaeb7e0c1a1594ef` both sides. SARIF byte-identical cross-app and single-app. Census (cross-app): gone `base.combined_scc` 4 590 120 B, `base.event_graph` 2 830 824 B, `base.graph` 26 294 162 B, `base.nodes` 3 324 090 B; new `rel.graph` 26 294 162 B, `rel.scc` 4 590 120 B, `rel.dependency_uncertainty` 77 904 B (net -6.08 MB retained). CDO parity line: `2 cases, 1 with a dependency half and event dispatch, 1 with dependency uncertainty, 24.6s`.
- Bench part 2a, bounded (supervised, exit 0, 126.2 s, `logs/bench/20261020T073625-fc885d96.log`, 5 rounds, release-fast, CDO single-app). Bands = 3 % of the parent median, per metric. `as` (analyze): process 578.0 -> 577.0 MB (band 17.3), post-attach 297.0 -> 296.0 (band 8.9); `ps` (policy check, fixture policy): process 576.0 -> 577.0 (band 17.3), post-attach 455.0 -> 458.0 (band 13.7); `es` (events fanout): process 575.0 -> 576.0 (band 17.3), post-attach 297.0 -> 294.0 (band 8.9). All within bounds; stdout SHA-256 equal on every command. The single-app SCC lifetime change (S1-4) costs nothing measurable here.
- Bench part 2b, reported (supervised, exit 0, 722.5 s, `logs/bench/20261020T073859-87695f67.log`, 5 rounds, CDO cross-app): `ax` (analyze) process 2835.0 -> 2834.0 MB, post-attach 2139.0 -> 2136.0, wall 34.6 -> 34.4 s; `dx` (digest, P5 diff) process 2837.0 -> 2835.0, post-attach 2160.0 -> 2118.0, wall 36.6 -> 36.7 s. Stdout SHA-256 equal on both. Report only (no bound).
- Numbers file `0dbd986a` (implementer; docs + CHANGELOG only, filled from the artifacts above, nothing re-run): `docs/measurements/2026-10-20-issue-160-step4.md`.
- Step 11: `git fetch`; `origin/master` = base `c554b704`, so no rebase and no range-diff. B = `c554b704d86d954fece216ab148f15b536c2854c`, H = `0dbd986a`. Full gate set started on H.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| `step4_dependency_publisher_primary_rule` | base-code stand-in for §5.2 "every caller" (Task 5 repeats it in `new`): `build_cross_app_base` builds `graph` over `all_calls` instead of the cut `calls` (applied: grep count 1) | FAILED: `(2) HandleWs's commit span roots: want [[OnDepPost]], got [["DoPost"]]` and `(3) Touch proven closed-world temp: [("r0/385b6430…", 0)]`; (1) green. Same break also failed `step4_dependency_uncertainty_stays_out_of_primary` (graph holds DepCaller `unresolved-call`) | reverted, 3 passed | b2369db3 |
| `step4_dependency_publisher_primary_rule` | base-code stand-in for §5.2 "Primary callers only": after the graph build, drop `event-dispatch` edges whose `from` is in `dep_routine_ids` (applied: grep count 1) | FAILED: `(1) HandleWs's reverse callers: want [("event-dispatch", "OnDepPost")], got []`, `(2) … want [[OnDepPost]], got [["HandleWs"]]`; (3) green (the spec's stated limit); other two tests ok | reverted, 3 passed | b2369db3 |
| `step4_from_order_picks_the_first_caller` | base-code stand-in for §5.3: `graph.edges_from_order.reverse()` after the build (applied: grep count 1) | precondition held; FAILED: sample routine `r0/b7390f9c…` (DoB), object `…d170/Codeunit/50172`, want `r0/b100f2fa…` (DoA), `…/Codeunit/50171`; other two ok | reverted, 3 passed | b2369db3 |
| `step4_dependency_uncertainty_stays_out_of_primary` | base-code stand-in for §5.1 (d): dependency half-build's `uncertainty_edges` appended to `graph.uncertainty_edges` (applied: grep count 1) | FAILED both: `graph uncertainty_edges from DepCaller: [… kind: "unresolved-call" …/cs0]` and `context uncertainty_edges_by_from[DepCaller]: [… "unresolved-call" …]`; other two ok | reverted, 3 passed | b2369db3 |
| `relation_equals_the_legacy_base_per_field` | red first: `new` returned an empty relation | FAILED: `ws-api-page: nodes: len new 0 legacy 2` | real `new`: 3 passed (502 cases) | 43221921 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (i), in the COPIED `assemble`: each `edges_by_from` list `sort_by(to)` instead of `sort_by_key(edge_sort_key)` (applied: grep count 1) | FAILED: 30 `edges_by_from[..]` failures, first `ws-argument-bindings: edges_by_from[r0/0b8b3060…]: len new 4 legacy 4; first difference at 0` (also `scc` on some dirs) | reverted, 3 passed | 43221921 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (ii), in the COPIED `assemble`: `DispatchMeta.isolated: false` (applied: grep count 1) | FAILED on `edges_by_from` only, in 4 cases: `ws-txn-d47-event-neg-isolated` and `ws-r2c-mixed`, each arm; diff `isolated: false` new vs `isolated: true` legacy | reverted, 3 passed | 43221921 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (a), in `new`: dependency typed edges placed BEFORE the primary ones (applied: grep count 1) | FAILED on `typed_edges` only: `r3a4-fixtures/ws (cross-app)`, `r3a5-fixtures/ws (cross-app)`, `step4 inline (cross-app)` | reverted, 3 passed | 484ca663 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (b), in `new`: the dependency half assembled over `events` instead of an empty event graph (applied: grep count 1) | FAILED on `typed_edges` only, `step4 inline (cross-app)`: `len new 6 legacy 5; first difference at 5: new Some(TypedEdge { kind: "event-dispatch", …` (the duplicate dispatch) | reverted, 3 passed | 484ca663 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (c), in `new`: Primary assembled over `[primary, dependency].concat()` (applied: grep count 1) | FAILED: `edges_by_from`, `edges_from_order`, `typed_edges` on r3a4/r3a5/step4-inline cross-app, `scc` on r3a4 and step4 inline, `uncertainty_edges` on the step4 dependency-uncertainty fixture | reverted, 3 passed | 484ca663 |
| `relation_equals_the_legacy_base_per_field` | §5.1 (d), in `new`: dependency uncertainty also appended to `graph.uncertainty_edges` (applied: grep count 1) | FAILED on `uncertainty_edges` only, `step4 dependency uncertainty (cross-app): len new 1 legacy 0` (the only case with dependency uncertainty: the §5.4 fixture added at Task 2 is what makes this break visible) | reverted, 3 passed | 484ca663 |
| `relation_equals_the_legacy_base_per_field` (review #1) | in `new`: drop the `dependency_uncertainty` assignment (`drop(d.uncertainty_edges)`; applied: grep count 1). Before the fix the precondition (counted from the relation) tripped first and hid the field diff | precondition held (now counted from the oracle's `lunc`); FAILED on the field: `step4 dependency uncertainty (cross-app): dependency_uncertainty: len new 0 legacy 1` | reverted, 3 passed | 7b7da36e |
| `relation_equals_the_legacy_base_per_field` (review #3) | the per-root loop skips each root's first dir (`dirs.iter().skip(1)`; applied: grep count 1) | FAILED: `assertion left == right failed: cases run; left: 492, right: 502` | reverted, 3 passed | 7b7da36e |
| `step4_dependency_publisher_primary_rule` | §5.2 "Primary callers only", in `new`: drop `event-dispatch` edges whose publisher's `app_guid` differs from the subscriber's (applied: grep count 1) | FAILED: `(1) HandleWs's reverse callers: want [("event-dispatch", "OnDepPost")], got []`, `(2) … want [[OnDepPost]], got [["HandleWs"]]`; (3) green (the stated limit); the other two step4 tests ok | reverted, 3 passed | 9ac21fdd |
| `step4_dependency_publisher_primary_rule` | §5.2 "every caller", in `new`: Primary assembled over `[primary, dependency].concat()` (applied: grep count 1) | FAILED: `(2) … want [[OnDepPost]], got [["DoPost"]]`, `(3) Touch proven closed-world temp: [("r0/385b6430…", 0)]`; (also `step4_dependency_uncertainty_stays_out_of_primary` (2) both halves) | reverted, 3 passed | 9ac21fdd |
| `step4_from_order_picks_the_first_caller` | §5.3, in `new`: `graph.edges_from_order.reverse()` after `assemble` (applied: grep count 1) | precondition held; FAILED: sample `r0/b7390f9c…` (DoB), object `…d170/Codeunit/50172`, want `r0/b100f2fa…` (DoA), `…/50171` | reverted, 3 passed | 9ac21fdd |
| `step4_dependency_uncertainty_stays_out_of_primary` | §5.4 break (d), in `new`: dependency uncertainty also appended to `graph.uncertainty_edges` (applied: grep count 1) | FAILED part (2) only: `(2) rel.primary().uncertainty_edges from DepCaller: [… "unresolved-call" …]` and `(2) context uncertainty_edges_by_from[DepCaller]: […]`; (1) green | reverted, 3 passed | 9ac21fdd |
| `step4_dependency_uncertainty_stays_out_of_primary` | §5.4, in `new`: drop the `dependency_uncertainty` assignment (`drop(d.uncertainty_edges)`; applied: grep count 1) | FAILED part (1) only: `(1) rel.dependency_uncertainty: want [(DepCaller, "unresolved-call", Some(…/cs0))], got []` | reverted, 3 passed | 9ac21fdd |
| `cli_build_counters::analyze_cross_app_default` | §5.5 (a), in `new`: the dependency half built through the counted `build_combined_graph`. As the spec words it (inside `if !dependency.is_empty()`) the break stays GREEN: `ws_dep`'s dependency makes no call, so its half is empty and skipped. Run instead with the guard also opened (`… \|\| !ws.routines.is_empty()`, the old base's unconditional second build; applied: grep count 1 each) | FAILED: `[analyze cross-app] build.combined_graph: got 2, want 1` (and every row: each relation counted 2) | reverted, 42 passed | 9ac21fdd |
| `cli_build_counters::analyze_single_app` | §5.5 (b), in `build_l4`: `let scc = Arc::new(graph_scc(graph))` instead of `rel.scc` (applied: grep count 1) | FAILED: `[analyze single-app] build.graph_scc: got 3, want 2` (17 rows fail, every one with a solve) | reverted, 42 passed | 9ac21fdd |
| `cli_digest_query_dependencies::cross_app_commit_span_sees_dependency_writes` | §5.5 seam, in `new`: skip the dependency typed-edge extend (`drop(d.typed_edges)`; applied: grep count 1) | FAILED: `left: ["span-has-no-known-writes"] right: ["span-has-known-writes"]`; doc re-stated with this seam and date | reverted, 1 passed | 9ac21fdd |
| `cli_build_counters::analyze_cross_app_with_a_dependency_half` (review t45 #1) | §5.5 (a) AS SPECIFIED, in `new`: the second `assemble` swapped for the counted `build_combined_graph` INSIDE the `if !dependency.is_empty()` guard (applied: grep count 1 each) | FAILED: `[analyze cross-app, dependency half] build.combined_graph: got 2, want 1`; the other 43 counter tests (the `ws_dep` rows) green | reverted, 44 passed | e8e5f0fa |
| `cli_build_counters::retained_census_counts_the_cross_app_graph_and_scc_on_the_relation` (final F1-9) | census now printed by the owner: drop `census_cross_app_base`'s `b.rel.emit_census()` (applied: grep count 0 after) | FAILED: `rel.graph lines: left 0, right 1`; the other 3 census tests ok | reverted, 4 passed | 0c814ec2 |
| `cli_build_counters::retained_census_counts_the_single_app_scc_once` (final F1-9) | drop `ExecRelation::single_app`'s `rel.emit_census()` (applied: grep count 0 after) | FAILED: `rel.scc lines: left 0, right 1`; the other 3 ok. Stated limit: the stderr ORDER cannot discriminate the placement (the `rel.*` lines sit right after `model` whether `new` or the owner prints them; `census_hook::mark` is a no-op outside its own test) | reverted, 4 passed | 0c814ec2 |

## Findings register summary

41 entries in `findings.json`. Spec panel: S1-1..S1-18 (7 important) and S2-1..S2-4, all
fixed. Final panel: F1-1..F1-19 (1 important, F1-10: the bench plan's cdo-measure path),
16 fixed and 3 refuted with evidence (F1-3 lazy SCC against invariant I3; F1-4 a faster
dependency half-build is out of step 4's scope; F1-6 the tripwire is kept and CLAUDE.md is a
protected path). Every entry accepted by both reviewers (sol = gpt-6.1-sol, slot astra;
opus = Opus 5.5, slot flash). The built-in `code-review` high and three independent sonnet
task reviews (`task-review-t12.md`, `task-review-t3.md`, `task-review-t45.md`, 2 important +
12 minor, all handled in `7b7da36e`, `cb418d6f`, `e8e5f0fa`) fed the work.

## Gate results

- Task 1 (b2369db3): §5.2/§5.3/§5.4 literals confirmed on base `c554b704`, none disagreed: (1) one `event-dispatch` from OnDepPost; (2) roots `[OnDepPost]`; (3) no `Touch` entry; §5.3 one d17 finding sampled at DoA / object 50171; §5.4 no DepCaller uncertainty in graph or context. §5.4's to-less edge kind (for Task 5 part 2): `unresolved-call`. `cargo test -p al-sem --lib step4_` 3 passed; `scripts/ci-steps task-affected` 3568 passed, 8 skipped; pre-commit goldens OK (907 passed); check-diff `reasons: []`.
- Task 2 (43221921): `cargo test -p al-sem --lib exec_relation` 3 passed; parity printed `502 cases, 1 with a dependency half and event dispatch, 1 with dependency uncertainty` (both from the inline fixtures; no corpus dir has dependency uncertainty). CDO half (`--profile cdo`, prefix CDO_WS): `2 cases, 1 with a dependency half and event dispatch, 1 with dependency uncertainty, 23.9s`, ok. `scripts/ci-steps task-affected` 3571 passed, 8 skipped; pre-commit goldens OK; check-diff `reasons: []`.
- Task 3 (484ca663) text-identity check, BEFORE deleting the copy: old = `combined_graph.rs` `uncertainty_sort_key`, `is_edge_kind_enum`, `is_object_run_kind`, `dispatch_kind_to_object_type`, and `build_combined_graph` through `build_typed_edges` (lines 185-196, 202-215, 236-249, 259-819); new = `exec_relation.rs` lines 109-708. `diff old new`: blank lines between helpers; `pub fn build_combined_graph(` vs the copy's doc + `fn assemble(`; `resolved: &ResolvedCalls` vs `call_edges: &[CallEdge]` (both functions); the deleted counter line; `for ce in &resolved.edges` vs `for ce in call_edges` (3 sites); `build_typed_edges(workspace, resolved, …)` vs `(workspace, call_edges, …)`; the old `// build_typed_edges — …` section comment. No other difference. Re-runnable from history (Task 3 review #1): `diff <(git show c554b704:src/engine/l4/combined_graph.rs | sed -n '185,196p;202,215p;236,249p;259,819p') <(git show 43221921:src/engine/l4/exec_relation.rs | sed -n '109,708p')`. (The parameter is named `call_edges`, not the plan's `edges`: the body already has a local `edges: Vec<CombinedEdge>`.) Tests: `exec_relation` 3 passed; `engine::l4` 137 passed; task-affected 3571 passed, 8 skipped; CDO half (cdo profile) `2 cases, 1 …dispatch, 1 …uncertainty, 22.6s` ok; pre-commit goldens OK (907); check-diff `reasons: []`.
- Tasks 1-2 review fixes (7b7da36e), `task-review-t12.md` #1-#5 (#6 no change; #5 as a doc line: `write_fixture_107` has its own guids, a second codeunit and a symbol-only variant, so a shared writer is not a clean change). `exec_relation` 3 passed, parity `502 cases, 1 …dispatch, 1 …uncertainty` (now counted from the oracle); task-affected 3571 passed, 8 skipped; pre-commit goldens OK; check-diff `reasons: []`.
- Task 4 (cb418d6f; amended from 71d61521 to fold in the Task 3 review's `exec_relation` tests doc fix, no commit hash or commit-A/B wording): `compose_cone_over_graph(typed: &[TypedEdge], ..)` and `build_typed_edge_graph(typed: &[TypedEdge], ..)`; 16 call sites pass `&graph.typed_edges` (capability_cone 4, detector_context 1, cone_derived 10, plus the internal one). No new test (compile-time guarantee, per plan). `cargo check --all-targets` clean; task-affected 3571 passed, 8 skipped; pre-commit goldens OK; check-diff `reasons: []`.
- Task 5 (9ac21fdd): one commit (not split). `cargo test -p al-sem --lib` 2170 passed, 4 ignored; `--test cli` `cli_build_counters::` + `cli_digest_query_dependencies::` 48 passed; task-affected 3571 passed, 8 skipped (after every break reverted); CDO half (cdo profile) `2 cases, 1 …dispatch, 1 …uncertainty, 22.1s` ok; pre-commit goldens OK, no golden moved; check-diff `reasons: []`. Counter rows edited in the same commit: `bcg` 2→1 analyze cross-app, 3→2 d47, 3→2 digest cross-app, 2→1 EVENTS_CROSS_ROW, 2→1 policy cross-app, 2→1 QUERY_CROSS_ROW. Not done: plan step 1's "both red on the Task 4 code" was not run as a separate pass; the counter proof (a) (the old two-build counting gives `bcg` 2) and the §5.4 part-(1) proof stand in for it. Found: `ws_dep`'s dependency half is EMPTY (its dependency makes no call), so the counter row cannot tell "dependency half counted" from "half skipped"; §5.5 (a) needed the guard opened to fail (row above).
- Task 6 (d11d57b0): docs only (epic §5.5 amendment block M1-M3/M5-M8, Decision 18 note, §9 row 4 M4/M9, §5.9 `rel` note; child 0-2 §4.4.8 "After step 4" table with moved cells bold and the rename line; CHANGELOG `[Unreleased]` step 4 entry with `<Task 7>` placeholders for the CDO baseline/histogram, census bytes and peak bounds). task-affected 3571 passed, 8 skipped; check-diff `reasons: []`.
- Tasks 4-5 review fixes (e8e5f0fa), `task-review-t45.md`: #1 new counter case on `dep_dispatch_fixture::write_own` (non-empty dependency half), bcg 1 / scc 2, proof row above; #2 `dependency_uncertainty` doc (no production reader); #3 `build_detector_artifacts(.., rel: &ExecRelation, ..)`; #4 new row `analyze_single_app_d47` = program 1, bcg 2, cone 1, v2 0, scc 2, pd 1, ctx 1, snap 1 (observed; matches the M4 rule: relation + legacy-cut snapshot graph, relation Tarjan + snapshot cone Tarjan; the review's guess of scc 3 was not the rule's value); #5 was already in Task 6's CHANGELOG, plus a correction there: `build.graph_scc` does move in the newly pinned single-app d47 row. `cli_build_counters::` 44 passed; task-affected 3573 passed, 8 skipped; pre-commit goldens OK; check-diff `reasons: []`. (Superseded: `98940afb` added both new cases to the child 0-2 §4.4.8 "After step 4" table; final panel F1-16.)
- Final panel r1 fixes: code 0c814ec2 (F1-1 private fields + `scc()`/`dependency_uncertainty()`, F1-2 `ExecRelation::single_app`, F1-5 split by move via `Arc::try_unwrap(mem::take(..))` with a clone fallback, F1-8 r3a5 uses `rel.primary().nodes` (r3a5 goldens unchanged: the cone sorts its graph nodes and reads the list only into a set and keyed maps), F1-9 `emit_census()` called by the owner, F1-14 code comments, F1-17 doc); docs b610a68e (F1-11..F1-15, F1-18, F1-19; F1-10 in `bench-plan.md`, outside the repo). task-affected 3573 passed, 8 skipped (both commits); CDO half (cdo profile) `2 cases, 1 …dispatch, 1 …uncertainty, 23.5s` ok; pre-commit goldens OK, no golden moved; check-diff `reasons: []`.

**Final gate set on H `0dbd986ae9d16142c040079ac5e1cc6cce20fe7d` (step 11; every run supervised):**

| gate | exit | seconds | log (under `.agent/runs/20261020-034103-38ab8c/logs/`) |
|---|--:|--:|---|
| ci-steps-all | 0 | 434.0 | `ci-steps-all/20261020T075653-83d3fa92.log` — 3573 passed, 8 skipped; perf-bounds green |
| check-goldens-coverage | 0 | 0.8 | `check-goldens-coverage/20261020T080409-87a08572.log` |
| check-goldens | 0 | 0.6 | `check-goldens/20261020T080412-66a48302.log` — skipped on the tree stamp: this exact tree passed `ci-steps test` inside ci-steps-all, which runs every golden target |
| cdo-gate | 0 | 295.9 | `cdo-gate/20261020T080413-f16e6209.log` — program_resolve_harness 201, lsp 2, l4_summary_differential 20, `--lib` 2167 passed; `cdo-gate: PASS` |

`git status --porcelain -- . ':!.agent'` empty after the gates. Toolchain rustc 1.96.0; grammar
tree-sitter-al `8682cb5f`; CDO baseline pinned at `bc3ccb18`, `CDO_WS` identity
`1e512deb11c28d51` (sha256 of `<path>@bc3ccb18`, first 16 hex).

## Discoveries

None filed.

## Not delivered

- The Cone pair (typed adjacency and its SCC) is not held in the relation: amendment M3 defers
  it to step 6/7a, its first second reader; policy builds it twice until step 6.
- `dependency_calls` and `role` are not on the relation (amendments M7, M8: steps 6 and 8).
- The epic §5.5 sketch's separate `typed` / `combined` / `from_order` vectors: amendment M1
  keeps `CombinedGraph` as the carrier until step 11.
- No test pins the census PHASE of the `rel.*` buckets (F1-9 stated limit).
- #160 itself stays open: this PR is step 4 of the epic ("Part of #160", no closing keyword).
