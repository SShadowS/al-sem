# Issue #222: Incremental L4 host behind --l4-host (deferred #160 step 13b)

Claim: run `20261025-084210-508661`, attempt 1, branch `issue/222-incremental-l4-host-behind-l4-h-a1`, worktree `/work/al-sem-issue-222-a1`, base `0fff74b1d926be032e66cc84a5214873eb648b97`, body_hash `b13b56a1076f05349e9d255e37ed54e481b94ca286eccea9f68b19cb2c3a07e5`.

Reviewers: gpt-6.1-sol (slot astra) and Opus (slot flash). Attest with `--substitute astra=sol --substitute flash=opus`. heavy_gates=2. Merges pre-approved.

## Timeline
- 2026-10-25 08:47 UTC: claimed (#160 closed by the operator, which cleared `depends-on-open:#160`); worktree made on 0fff74b1.

## Classification
Architectural: a new LSP-side incremental L4 host with generation-checked cancellable work, plus invalidation over reverse reachability, SCCs, demand and config files, with incremental == fresh oracle tests. It spans `src/lsp/updater.rs`, the session/View from #160 steps 6/12, and `tests/lsp`. Pre-draft: `/work/bench/160-13/draft-step13-design.md` (host and worker sections), `/work/bench/160-13/open-questions.md`, `/work/bench/160-13/probes-todo.md`.

## Probes

Step 3, 2026-10-25, at base 0fff74b1. Read-only: code reading, greps and one lib unit run. No CDO run, no release build (non-speed step). P2, P9, P10 and P11 of `probes-todo.md` were 13a's (#221, merged as 70fc2e9b / #232); they are not re-run. P12 and P13 need CDO and are dropped (see the spec's open question OQ7). P15-P24 are new.

| probe | result | evidence |
|---|---|---|
| P1 the LSP has no L4 consumer and is LIGHT only | **holds** | `grep -rn "build_l4\|View::new\|BuildProfile::FULL\|AnalysisTarget" src/lsp src/server.rs src/watcher.rs` prints nothing; `src/lsp/snapshot.rs:601`, `:662` build `BuildProfile::LIGHT` |
| P2 salsa | **n/a, done** | removed by #221 (70fc2e9b); `docs/OUTSTANDING.md:713` marks it done |
| P3 (crux) a solve with clean SCCs seeded from the previous revision projects equal to a fresh solve | **not run; plausible by reading** | needs the seeded entry, which is the feature itself. For it: `SummaryBundle::db_effects` is a key_rank merge, so its order is content-derived, not EffectId-derived (`effect_store.rs:794-847`); every member of an effective SCC records the SCC's ONE shared terminal set (`effect_store.rs:528-547`), so a per-member seeded set holds the same ids and `finish` hash-conses equal sets to one `EffectSetId` (`:607-668`); a leaf-style seeder already exists (`db_effect_solver.rs:1651-1692`). Against: interning order differs (`SolveIx` does not, `EffectId` does), so any reader of raw `EffectId`/`EffectSetId` numbers would leak. Gate: plan task 1 runs T2 (spec §5) before any other code; fallback F1 |
| P4 can rows be digested without new derives | **holds** | `ModelRoutine` derives `Debug, Clone` only (`workspace.rs:435-436`), and no field in its tree is a `HashMap`/`HashSet` (grep of `workspace.rs:300-560`, `body/features.rs`, `attributes.rs`, `site_key.rs` struct fields). The `l2_features.snapshot` golden already hashes the `Debug` text of the same feature types (`tests/l2_ir/ir_l2_snapshot.rs:60-73`), so that text is deterministic. Digest = sha256 of the `Debug` text; no derive, no golden moves |
| P5 every read the v2 solve makes | **FALSIFIES the pre-draft's rule 3** (candidate discovery) | Per id: all model rows of the id (`summary_runner.rs:1380-1446`), its `edges_by_from` list (`:1404`), its uncertainty edges (`:1471-1477`), the upgraded bindings of its call sites (keyed by `cs.id`, `cfg_walker.rs:1776`), its leaf summary and `fixed_roles` flag (`:1427`, `:1484`). Global: `FieldIndex` (`analysis.rs:74-78`), `shared_event_ids` (`:1395`), `graph.collided_site_callee` (`projection.rs:351`). **And the CALLEE's model row**: `event_param_temp_state` binds a subscriber parameter to a publisher parameter BY NAME (`summary_runner.rs:1152-1165`; unit run `row13_event_publisher_param_binds_by_name_quotes_ignored` passed at this base), `dispatch_bindings`/`opaque_subscriber` read the subscriber's `parameters` (`event_dispatch.rs:73-90`, `:98-140`), `substitute_pd_temp_state` takes the callee row (`summary_runner.rs:1045-1074`; called with `routines_by_id.get(e.to)` at `db_effect_solver.rs:314-319`, `:345-350`), and the roles fixpoint reads `body_avail_by_id` of the callee (`summary_runner.rs:835`, `:994`). A routine id leaves parameter NAMES out (`body/scope.rs:126-156`). So a subscriber parameter rename keeps every id, may leave the subscriber's own summary equal, and still changes the publisher. The pre-draft's "re-solve iff a successor's SUMMARY changed" would keep the publisher stale. Spec rule: a routine is dirty when it or any one-hop successor has a changed row (§4.3 R2) |
| P5b `base_intraprocedural_summary` reads only its own row | **holds** | the `routines_by_id` parameter is unused (`summary_runner.rs:509-515`, `_routines_by_id`) |
| P6 does a cross-app demand change move a global input | **partly falsified** | `FieldIndex`: no. A dependency file keeps every object; only its routines are filtered by demand (`workspace.rs:1698-1727`), and the index is built from `ws.tables` (`analysis.rs:157-167`). `shared_event_ids`: **yes**, it is a function of every `event-dispatch` edge (`event_dispatch.rs:58-70`), so a newly demanded publisher can flip it. The spec moves it into the row (one bit per `event-dispatch` edge of the routine). `collided_site_callee`: may move; kept global (a change means a Full refresh) |
| P7 do config files reach the solve | **holds** | `roots.config.json` reaches only `root_classifications`/`infra_diagnostics` (`workspace.rs:1913-1921`); the solve's inputs are `analysis.rs:79-87`. `al-sem.coverage.yaml` is read only for fingerprint input rows (`inputs.rs:517+`, `snapshot_full.rs:1192-1194`), which the host never builds. `.al-sem.json` is the LSP diagnostics config (`config.rs`), not an engine input |
| P8 what `WorkspaceInputs::read` reads, after step 12's split | **listed** | `app.json`; the `app.json` path walk; the `.al` walk and texts; `roots.config.json`; `al-sem.coverage.yaml`; the closure's PATHS (`discover_app_files`; the program build reads the bytes) (`inputs.rs:289-374`). Policy and baseline moved to `RequestInputs` (`inputs.rs:174-245`). The watcher drops a data write to any non-`.al` file other than `app.json` and `.al-sem.json` (`watcher.rs:165-171`), so a `roots.config.json` edit does not trigger the updater today |
| P14 server tests needing `on_swap`'s synchronous publish | **moot** | the spec leaves the diagnostics recompute on the updater thread (OQ5); `on_swap` only gains an O(1) request |
| P15 is `LspSnapshot.generation` a usable request counter | **falsified** | a `ConfigChanged` batch republishes on the SAME generation (`updater.rs:1503-1507`, `:1555-1558`). The host keeps its own counter, bumped by every `on_swap` call |
| P16 do file adds and removes keep routine ids | **falsified** (candidate discovery) | the model-instance id hashes the app id, the version and EVERY `.al` path (`model_instance_id.rs:89-107`, called from `inputs.rs:500-515`), and every internal id, op id and call-site id embeds it (`ir_walk.rs:1024`, `:1346`). So adding, removing or renaming a `.al` file, or bumping `app.json`'s version, renames every id: no reuse is possible. The spec makes the instance id a global input (a Full refresh, stated) |
| P17 can a host panic be contained | **no** | the shipped binaries build with `panic = "abort"` (`Cargo.toml:247-249`); `src/lsp` and `src/server.rs` have no `catch_unwind`. A panic in the host thread ends the LSP server. Stated as failure shape S6; the flag is hidden and off by default |
| P18 does the host's build path write to stdout (which would corrupt the LSP's JSON-RPC) | **holds by grep** | every `println!` in `exec_relation.rs`, `capability_cone.rs`, `pack/mod.rs` sits in a `#[cfg(test)]` module; the solve's warning goes through `batch::output::job_line`, which `eprintln!`s outside a batch (`output.rs:188-199`). Limit: a grep, not a run |
| P19 cross-app leaf and fixed-role membership | **row facts** | decided per routine from its own out-edges (`capability_cone.rs:3966-3981`); the leaf summary is `base_intraprocedural_summary` of the row plus `FieldIndex` |
| P20 line numbers the epic and the pre-draft cite | **moved** | epic row 13 cites `updater.rs:1206-1229`; at this base `spawn_updater` is `updater.rs:1465-1566`, `on_swap` is called at `:1506`, `:1527`, `:1545`, `:1553`, `:1557`; the server's closure is `server.rs:629-658`; `build_l4` is `analysis.rs:56-124`, called at `session.rs:986` and `detector_context.rs:1648`; the solve loop is `summary_runner.rs:1567-1654` |
| P21 a #221 (13a) spec file | **none in docs/** | 13a merged as 70fc2e9b (#232); no `issue-221` spec under `docs/superpowers/specs/`. What 13b inherits: `ChangeEvent::{PathChanged, ConfigChanged}`, `Updater::reconcile`, the `ConfigHandle` reload, watched ancestor caches |
| P22 a cross-app fixture with an embedded-source dependency for the oracle | **exists** | `tests/common/dep_dispatch_fixture.rs`: `write_own`, `write_dependency(path, revision)` (a revision changes bytes only, `:120-147`); `tests/lsp/main.rs` does not include it yet (needs `#[path]` lines, as `tests/cli/main.rs:12-22`). `tests/fixtures/lsp-diff-deps`'s embedded app has one routine and no record op, too thin for a demand edit |
| P23 seeding a solve needs the old bundle's rows by id | **available** | `SummaryBundle::routine_ix` + `db_effects` give each old routine's effects with `via` (`effect_store.rs:739-805`); the growing universe has `identity`/`intern` (`effect_universe.rs:142`, `:234`) |
| P24 the closure digest is a second read | **yes** | `semantic_golden::dependency_closure_digest` re-reads every `.app` (`semantic_golden.rs:351-375`). Read it BEFORE the build: a change that slips in between makes the next digest differ (a Full refresh), never a false reuse. Limit: an A-B-A swap of a `.app` between the digest and the build (S5) |

## Spec
- 2026-10-25: step 4 spec written: `docs/superpowers/specs/2026-10-25-issue-222-incremental-l4-host-design.md` (not committed; the conductor commits it).
- 2026-10-25: spec panel r1 (sol S1-S13, Opus O1-O19): all 32 source-verified and fixed; register `.agent/issue-222/findings.json`; spec §0.1 maps changes to ids; OQ1-OQ9 recorded as decided (§11).

## Task 1
2026-10-25: the seeded solve and the P3 gate. **P3 holds: T2 and T2b pass as designed on all 216 r0 corpus dirs and C1-C4. F1 did not fire.**

Discrimination proofs (`cargo test -p al-sem --lib summary_runner_seeded`; each break made with Edit, its count checked = 1, reverted with Edit):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T0a | the re-solved path's `diagnostics.extend(diag)` wrapped in `if seeding.is_none()` (seeded entry skips a re-solved SCC's diagnostic) | T0a `C1: projection differs` (only C1; T2/T2b also failed on C1) | yes (3/3, run with proof 2's revert) |
| T2 | B12: `seed_row`'s PD arm `=> {}` (terminal effects only) | T2 `C2: projection differs` (25 cases incl. C1, C2); T0a green | yes (3/3) |
| T2b | seeded rows collected in `late_rows` and `seed_row`'d after the loop | T2b `C2 k=["C"]: projection differs` (101 cases, C3 k=["G"] too); **T2 and T0a stayed green**: the pair is the proof | yes (`l4::` 164/164) |

Plan corrections:
- `compute_summaries_v2_bundle_seeded(inputs: &V2Inputs, control: &L4Control)`: the plan's 8 positional params trip `clippy::too_many_arguments` and `#[allow]` is barred, so the seven solve inputs ride in a `Copy` struct `V2Inputs`. `compute_summaries_v2_bundle_with_leaves` keeps its signature and calls the shared body `solve_v2(&inputs, None)`.
- `build_l4_controlled` returns `Option<SolveOutcome>` (`None` iff `req.control` is `None`), and `build_l4` calls it (asserting no control). With the plan's signature `build_l4_controlled` is dead code until Task 2, and clippy `-D warnings` fails. Task 2 `expect`s the outcome.
- `SolveBaseline` and `SolveOutcome` are `pub` (fields `pub`): their fields are read only by tests until Task 3, so `pub(crate)` trips dead-code.
- New `SummaryBundleBuilder::row_effects` (R3's builder-side comparison read).
- Row digests also cover leaf ids with no model row (their summaries are read by callers). Over-invalidation only.

Gates: `scripts/ci-steps task-affected` exit 0 (whole suite, 4320 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved.

## Task 2
2026-10-25: `View::prepare_l4_seeded` and `View::take_l4` (`src/engine/session.rs`). `prepare`'s rel fill moved into one helper `relation_of(&AnalysisTarget)` that both use (byte-neutral). Also the Task 1 review minor R1-2: T2 asserts `seeded_total > 0` across `all_cases()`.

Red: `cargo test -p al-sem --lib session::` failed to compile (no `prepare_l4_seeded`, `take_l4`; `L4Control` etc. not imported). Green: 26/26 `session::`.

Discrimination proofs (each break made with Edit, its count checked = 1, reverted with Edit):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| `prepare_l4_seeded_without_reuse_equals_query_plan` | `build_l4_controlled(.., None)` instead of the target's cross-app inputs | `dependency cross-app: l4 differs from the query plan's` (core summaries differ; the single-app cases stay green) | yes |
| same | plan's break: `rel` = `ExecRelation::single_app(target.resolved())` on every target | **green, redundant**: the cross-app base keeps only workspace callers' calls in `model.calls.edges` (`capability_cone.rs:3909-3925`), so the Primary payload and SCC, all `build_l4` reads, are the same. Stated limit in the test doc | (n/a) |
| `take_l4_releases_the_cell` | `take_l4` without `note_release(.., Cell::L4)` | `a second prepare_l4_seeded: expected a panic, none came` | yes |
| T2 `seeded_total > 0` (R1-2) | `seed_from` returns `clean.then_some(reuse).filter(\|_\| false)` (the seeded branch seeds nothing) | `T2 seeded no SCC across the cases` (fires before the per-case re-solved check, which this break also trips) | yes |
| same, isolated | `record`'s `None => self.outcome.seeded += 0` (seeding works, the count does not) | `T2 seeded no SCC across the cases` (the only failing check) | yes (`l4::`+`session::`+`summary_runner_seeded` 190/190) |

Plan corrections:
- `prepare_l4_seeded` and `take_l4` are `pub`, and so are `L4Control` and `L4Reuse` (`summary_runner.rs`): a `pub(crate)` method used only by tests until Task 3 trips dead-code under `-D warnings`, and a `pub` method cannot take a `pub(crate)` type (`private_interfaces`).
- The parity test compares core summaries too: the plan's projection (bundle, diagnostics, SCC) with an empty core map cannot see any cross-app difference. The query plan's core is rebuilt with `build_l4` off the same view (model, rel, cross-app inputs), since `prepare` drops it. The dependency fixture is `test_support::write_fixture_step4` (a dependency publisher with a workspace subscriber), run in both universes, plus r0 `ws-d1-multi-caller` single-app.
- The plan's SCC-source break is redundant (above); the real cross-app break is dropping the cross-app inputs.
- `summary_runner_seeded` (the Task 1 test module) is now `pub(crate)`, and `Projection`/`l4_projection` `pub(crate)`, so `session.rs` tests reuse `l4_projection`. `Projection`'s fields stay private; Task 3 lifts it.

Gates: `scripts/ci-steps task-affected` exit 0 (4322 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved.

## Task 3
2026-10-25: `IncrementalHost`, `Refresher`, `refresh`/`commit`/`refresh_committed`, `L4Projection` and the signature labels (`src/engine/incremental.rs`); the oracle harness `tests/lsp/l4_host_oracle.rs` with T0b (single-app half), T1a, T1b, `build_error_keeps_baseline`, `empty_workspace_is_build_error`. Each case runs in both universes. Reuses `WorkspaceInputs::read`, `AnalysisTarget::build`, `View::prepare_l4_seeded`/`take_l4`; no second engine.

Red: `cargo test --test lsp l4_host_oracle::` failed to compile (`unresolved import al_sem::engine::incremental`). Green: 5/5, first run after the implementation.

Discrimination proofs (`cargo test --test lsp l4_host_oracle::`; each break made with Edit, its count checked = 1, reverted with Edit):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T1a (R1-1) | `came_out_different` returns `(..) && false` (never different) | `t1a CrossApp: oracle` (only T1a) | yes |
| T1a (B1) | `seed_from` drops `\|\| self.different.contains(t)` (R3's successor clause) | `t1a CrossApp: oracle` (only T1a) | yes |
| T1b (B2) | `came_out_different` returns `.. \|\| true` (always different) | `t1b CrossApp: re-solved SCCs`, left `{A.A()},{B.B()},{L.L()}` right `{B.B()},{L.L()}` (only T1b) | yes |
| `build_error_keeps_baseline` | the target build's `map_err` also sets `self.last = None` (the hook reflowed it; the failing run proves it applied) | `CrossApp: the baseline is kept` (left `None`) | yes |
| `empty_workspace_is_build_error` | `AnalysisTarget::build(..).expect("BREAK")` | panics `the cross-app model could not be built`; `build_error_keeps_baseline` also panics (`could not compute modelInstanceId`), so both errors come from the target build, not the closure digest | yes |
| T0b | the host builds its target in the other universe | **green, redundant** on T1a (no dependency: both universes give the single-app answer). **Pending Task 5.** A reverted trial (Task 5's `#[path]` includes in `tests/lsp/main.rs`, T0b over `dep_dispatch_fixture::write_own(dir, "")` and over `write_own` plus a workspace routine calling `Lib.Inner()` directly) was green unbroken and RED under this break on the first one: `CrossApp: fresh host vs the query plan`. So T0b's bundle-part comparison catches the break; Task 5 needs only `write_own` | n/a (trial reverted) |

Plan corrections:
- `View::prepare_l4_seeded` gains `if (control.cancel)() { return Err(Cancelled) }` after the `rel` fill: §4.2's third phase check ("after the relation"). `PHASE_CHECKS = 3` counts inputs, target, relation. It is not pinned yet; T3 (Task 6) pins the call count `PHASE_CHECKS + total`.
- T0b compares `L4Projection::without_core()` (ids with a bundle row, their db effects, the terminal partition, diagnostics, warnings, SCC) of the fresh host with `L4Projection::of(view.l4(), &empty).without_core()` of a `query_plan` view: `prepare` drops the core summaries and `build_l4` is `pub(crate)`, so an integration test cannot rebuild them. `L4Projection::of(&L4Analysis, &HashMap<String, RoutineSummary>)` and `without_core` are `pub`.
- `L4Projection` lives in `incremental.rs`; the Task 1 test module re-exports it as `Projection` with `l4_projection` (`pub(crate)`, no warnings), and `per_id` is `pub(crate)` for its C2/C3 preconditions. `summary_runner::sha256_of_debug` is `pub(crate)` (host half and `baseline_fingerprint`).
- Labels: `Object.Member.Routine(var T1; T2): R`, the parameter `type_text` as the model has it; an id with no model row is labelled by its id. T1a/T1b labels are `A.A()`, `B.B()`, `L.L()`.
- For Task 5: the T0b other-universe break needs only `write_own` (see the trial above), not a fixture with an effect-carrying dependency call.

Gates: `scripts/ci-steps task-affected` exit 0 (4327 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved.

## Task 4
2026-10-25: oracle cases T1c-T1h, T1k-T1o, T1q, T1r in `tests/lsp/l4_host_oracle.rs` (each in both universes), the `case_pre` harness (preconditions asserted on fresh r0/r1 hosts plus a `query` plan view, BEFORE the oracle), and `L4Projection::routine(id)` (`pub`, the per-routine accessor). Folded in the Tasks 2+3 review minors: R2-1 test `second_prepare_l4_seeded_without_take_panics` (session.rs), R2-2 doc line on `prepare_l4_seeded`, R2-3 test `closure_error_keeps_baseline`.

Red/green: every case passed on Tasks 1-3's code at first run except T1g, whose PRECONDITION failed (below); no design bug (§9 F2/F4 not triggered). Red evidence for each case is its break.

Precondition that did not hold, and the reshape:
- T1g: with `S` doing `Rec.Modify()`, `S`'s projection differs between r0 and r1 only in `record_variable_id` (`.../rv/rec` vs `.../rv/recx`): the db effect carries the parameter's name. Reshaped `S`'s body to `Rec.Reset();` (no db effect; `RecordRoleSummary` has no name). `S` now projects equal, `P` differs (the dispatch binding), and the case still pins D-1. Precondition kept whole.

Discrimination proofs (`cargo test --test lsp l4_host_oracle::`, 19 tests; each break made with Edit, checked applied by `git diff`, reverted with Edit, then 19/19 green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T1c | combined: R2 without its two Δ conditions (key kept) AND `came_out_different` = `core.get(id).is_some() && (..)` | `t1c CrossApp: oracle` (also t1a, t1b, t1d, t1g, t1n, t1o, t1r) | yes |
| T1c single rules | R3-no-baseline-equal alone; B3 alone; B6 alone | green, redundant (spec §5.2) | n/a |
| T1d | B3: drop "no member in Δ" | `t1d CrossApp: oracle` | yes |
| T1d single rule | `edges` out of the row digest | green, redundant (spec §5.2): `A`'s `call_sites` change too | n/a |
| T1e | B5: drop the key condition; `diag_of[..]` made `.get(..).cloned().flatten()` so the seed reaches the oracle instead of the index panic | `t1e CrossApp: oracle` (only T1e) | yes |
| T1f | combined: R2 always true (`clean = !successors().any(different)`), same `diag_of` lookup | `t1f CrossApp: oracle` | yes |
| T1f single rule | B5 (key) alone | green, redundant (spec §5.2), seen in the B5 run | n/a |
| T1g | B6: drop the one-hop `delta.contains(t)` | `t1g CrossApp: oracle` (and t1b minimality) | yes |
| T1h | B7: instance id out of `host_global` | `t1h CrossApp: kind` (oracle held: redundancy as stated) | yes |
| T1k | B10: `FieldIndex` out of `solve_global` | `t1k CrossApp: oracle` (only T1k) | yes |
| T1l | B11: `roots.config.json` bytes in `host_global` | `t1l CrossApp: kind` (only T1l) | yes |
| T1m | B12: `seed_row`'s `ParameterDependent` arm pushes nothing | `t1m CrossApp: oracle` (also t1n) | yes |
| T1n | B13: shared bits out of the row (`shared.len() * 0`) | `t1n CrossApp: oracle` (only T1n) | yes |
| T1o | B14: `rows_of.insert(id, vec![r])` (last row only) | `t1o CrossApp: oracle` (only T1o) | yes |
| T1q | B15: `app.json` bytes out of `host_global` | `t1q CrossApp: kind` (oracle held) | yes |
| T1r | B16: `came_out_different` = `new != old` (db effects only) | `t1r CrossApp: oracle` (only T1r) | yes |
| R2-1 | delete the "L4 filled" assert | `a second prepare_l4_seeded: expected a panic, none came` (`cargo test -p al-sem --lib session::`) | yes (27/27) |
| R2-3 | `dependency_closure_digest(..).expect("BREAK")` | panic `BREAK: "read-failed <package folder>/Bad.app: not a file"` | yes |

Plan corrections:
- T1e and T1f breaks as the spec states them hit `diag_of[&members]`'s index panic (a seeded SCC with a new key has no diagnostic slot) before the oracle. The break also makes that lookup tolerant, so the failure is the named oracle assertion.
- R2-3's digest error is made deterministic on Linux with a DIRECTORY named `Bad.app` in the package folder (`read` fails: "not a file"); removing it gives the empty closure again, so the next refresh is Incremental with nothing re-solved.
- Full-kind label sets: T1h re-solves all five SCCs (`N.N()` included), T1k `{A.A(var Record T)},{U.U()}`, T1q the four T1a SCCs.

Gates: `scripts/ci-steps task-affected` exit 0 (4342 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `check-diff` no reasons. Commit `1f87e453`.

## Task 5
2026-10-25: cross-app cases in `tests/lsp/l4_host_oracle.rs`: `t1i_demand` (r1 and r2), `t1j_a_dependency_bytes_only`, `t1j_b_dependency_semantic`, `t1j_c_undeclared_package`, T0b's cross-app half (fixtures `write_own` and T1i's r0 next to T1A), and `t1s_interface_rebind` (review R4-1). `tests/lsp/main.rs` includes `symbol_app.rs` and `dep_dispatch_fixture.rs`. The harness is split into `host_r0` + `step` (one edit on a committed host) so T1i can chain two edits; `case_pre` calls them. R4-2: `query_view(dir, universe)` is the one copy of read-inputs / build target / `prepare(&query_plan())`, used by `Fresh::of` and T0b.

Red/green: every new case passed its preconditions and the oracle at first run; the only first-run failure was T1j's hand-stated Full label set (written empty before the run, then filled with the eight SCCs). No design bug. Red evidence for each case is its break.

Preconditions: none needed reshaping. T1i: r0 has an `Inner` row, r1 none, r2 one; `CallChain`'s projection differs r0→r1. T1j: (a) and (c) leave the projection equal, (b) changes it. T1s: every `A` and `Go` model row equal (Debug) r0→r1; `A`'s out-edges differ; `A`'s projection differs.

R4-1 (edges-only guard): such a case exists. Interface dispatch reads `implements` from the OBJECT header, and no `ModelRoutine` carries it (`src/program/model/workspace.rs:436-494`: the row has `call_sites` with the syntactic `PCallee`, `features.rs:141-159`, never a resolved target). T1s moves `implements IGo` from `X` (inserts `Cust`) to `Y` (inserts `Item`); no file is added, so the instance id holds; Δ is empty, and `A` is re-solved only because its `edges_by_from` digest part changed. Spec §5.2 gains a T1s row and one clause on T1d's row ("T1s pins the edges part alone"); the plan's coverage map lists T1s under Task 5.

Discrimination proofs (`cargo test --test lsp l4_host_oracle::`, 24 tests; each break made with Edit, `git diff --stat src/` showed the one-line change, reverted with Edit, `git diff --stat src/` empty, then 24/24 green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T1s (R4-1) | `row_digests`: `edges,` → `(),` in the row tuple (`summary_runner.rs:2030`) | `t1s CrossApp: oracle` (only T1s) | yes |
| T1i | R2 without "no member in Δ" (`seed_from`) | `t1i r1 CrossApp: oracle` (also t1a, t1b, t1d, t1g, t1n, t1o, t1r, t1s) | yes |
| T1i redundancy | demand adds/removes reach R2 only through new keys and the caller's own row | recorded as redundant (spec §5.2): T1c, T1d pin those rules; T1n the demand input | n/a |
| T1j (a), (b), (c) | B9: `&closure` out of `host_global` (`incremental.rs:138`) | `t1j (a) CrossApp: kind`, `t1j (b) CrossApp: kind`, `t1j (c) CrossApp: kind` (left `Incremental`); oracle held in all three: redundancy as stated for (b), no change for (a)/(c) | yes |
| T0b cross-app half | `single_app = self.universe != SingleApp` (host builds in the other universe) | `write_own CrossApp: fresh host vs the query plan` (T1A passed first: redundant there, as Task 3 found) | yes |

Plan corrections:
- `write_dependency_lib` is NOT made `pub`: T1j(b) uses `write_own_modify(dir, "")` (the same `Insert`→`Modify` text; it rewrites the workspace with identical bytes), so `dep_dispatch_fixture.rs` is untouched. Making it `pub` would need a second copy of `DEP_LIB_AL` in the test.
- Including `symbol_app.rs` warns `write_symbol_app is never used` in the `lsp` crate (other umbrellas use it). No `#[allow]`: T1j gains part (c), an undeclared symbol-only package written with `write_symbol_app` into `.alpackages`, which the closure digest covers (Full(HostGlobal); B9 makes it Incremental). It is the "guard for changes no row shows" of T1j's redundancy note, with no row or projection change at all.
- T1j is three `#[test]`s over one `t1j` helper, so B9's failure is seen in each part (a loop stops at the first).
- T1j's Full set: `Dep Lib.Inner()`, `Dep Lib.OnDepEvent()`, `Dep Lib.Outer()`, `Dep Sub.HandleDep()`, `Ws Sub.HandleInWs()`, `Ws Work.CallChain()`, `Ws Work.RaiseDep()`, `Ws Work.TouchCust()`. T1i: r1 `{Ws Chain.CallChain()}`, r2 `{Dep Lib.Inner()},{Ws Chain.CallChain()}`, as the spec hand-stated.
- §9 F6 (a demand change moving `collided_site_callee`) did not trigger: T1i is Incremental at r1 and r2.

Gates: `scripts/ci-steps task-affected` exit 0 (4347 passed, 8 skipped; first run failed clippy `type_complexity` on T0b's fixture array, fixed with a local `type Write = fn(&Path)`); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `cargo test --test cli dep --no-run` compiles; `check-diff` no reasons. Commit `3be3539d`.

## Task 6
2026-10-25: `t3_cancel_keeps_baseline` in `tests/lsp/l4_host_oracle.rs` (spec §5.4), in both universes. Fixture T1a; r0 with `never`; record `projection()` and `baseline_fingerprint()`; T1a's edit; a `refresh_committed` whose `cancel` counts its calls and returns true on call `PHASE_CHECKS + r0.total` (the top of the last SCC turn). Asserts: projection kept, fingerprint kept, `Err(Cancelled)`, calls == that number. Then a `never` refresh: Incremental, {L.L()},{B.B()},{A.A()}, and the oracle holds.

Red/green: green at first run (Tasks 1 and 3 already cancel, as the plan said). Red evidence is the two breaks below.

Per-phase cancellation: §5.4 asks only for the last-SCC-turn cancel, so no per-phase test was added. The call-count pin covers all three phase checks: removing any one shifts the count, so the cancel never fires (proof 2).

Discrimination proofs (`cargo test --test lsp l4_host_oracle::`, 25 tests; breaks made with Edit, `git diff --stat src/` showed them, reverted with Edit, `git diff --stat src/` empty, then green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T3 (O14) | `let taken = self.last.take();` at the start of `refresh`; `base`/`kind` read `taken`; `self.last = taken` only on success (4 Edits in `incremental.rs`) | `CrossApp: projection kept` (left `None`); `build_error_keeps_baseline` and `closure_error_keeps_baseline` fail too | yes |
| T3 (third phase check) | delete `if (control.cancel)() { return Err(Cancelled) }` after the relation in `View::prepare_l4_seeded` (`session.rs:1221`) | `CrossApp: projection kept` (only T3: the cancel never fires, so r1 commits) | yes |

Plan corrections: none.

Gates: `scripts/ci-steps task-affected` exit 0 (4348 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `check-diff` no reasons. Commit `dc52d061`.

## Task 7
2026-10-25: `src/lsp/l4_host.rs` (`pub mod l4_host;` in `src/lsp/mod.rs`): `Shared { requested, status }` behind ONE `Mutex` (S3); `HostHandle` (`Clone`; `request()` = lock, bump, unlock, send; `status()` = lock, clone); `HostStatus { generation, report }` (`Clone, Debug`); `run_worker(shared, wake, host: impl Refresher, after_drain: impl Fn())`: `recv`, drain (`try_recv`), `after_drain()`, read `g`, `refresh(&|| requested != g)`; on `Ok` `commit()` unconditionally (OQ4), then publish under the lock only if `requested == g`, with the `debug!` line `l4-host: published generation <g>: <report>`; `Build` is a `debug!`, `Cancelled` nothing. `spawn_l4_host(root, universe)` runs it on a thread named `l4-host` with an `IncrementalHost` and a no-op hook, then `request()`s once. A private `channel()` builds the handle and the worker's ends (shared by `spawn_l4_host` and the tests).

Tests (`cargo test -p al-sem --lib lsp::l4_host::`, 4): `t4a_stale_complete_unpublished_retained` (a real `IncrementalHost` on a T1a-shaped tempdir; the wrapper calls `host.refresh` with the WORKER's cancel, sends the report, then waits for a release, so the pause is after the host's last check), `t4b_no_stranded_request` (the hook requests once, via a `Mutex<Option<HostHandle>>` it takes, so no clone keeps the worker alive), `t4c_request_never_waits` (`request()` on a helper thread while refresh #1 is held; `done.recv_timeout`), `worker_exits_when_handles_drop` (on `spawn_l4_host` itself: first revision published, every handle dropped, `JoinHandle::is_finished` within the timeout). Every wait is a channel `recv_timeout` or a deadline loop with `yield_now` (10 s); no fixed sleep.

Red: the tests failed to compile with no implementation (`cannot find function channel/run_worker/spawn_l4_host`). First green run: T4c failed in the TEST (refresh #2 of the `Held` refresher waited for a second release that never came); fixed by dropping the release sender right after the first release. Then 4/4.

Discrimination proofs (each break made with Edit, checked applied, reverted with Edit, then 4/4 green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T4a (i) | publish guard `if s.requested == g` → `if true` | `the stale #1 is not published: Some(HostStatus { generation: 1, .. Full(NoBaseline) ..})` | yes |
| T4a (ii) | `commit()` only when `lock(&shared).requested == g` | `#1 was committed`: left `Full(NoBaseline)`, right `Incremental` | yes |
| T4b | read `g`, then hook, then drain (swap the drain and the read) | `status: None` after the 10 s wait | yes |
| T4c | `let _hold = lock(&shared);` across `host.refresh` | `request() returns while a refresh runs` (10 s) | yes |
| worker exits | the spawned closure keeps a `handle.clone()` | `the worker thread ends` (10 s) | yes |

Review fixes folded in (Tasks 5+6 review):
- R5-1: T3 is two-sided. After the cancel, the recovery refresh runs with a counting closure that never cancels and asserts `PHASE_CHECKS + report.total` calls (and `report.total == r0.total`). Proof: a duplicated `if cancel() { return Err(Cancelled) }` after the inputs (a fourth phase check) → `CrossApp: cancel calls of a full run` left 8, right 7; every earlier T3 assertion held (the cancel just lands one SCC turn earlier). Reverted, 25/25 green.
- R5-2: T1s asserts `X` and `Y` have one `Go` routine row each (the interface `IGo` has NO routine row: the first attempt asserted 3 and failed, 2 is the truth) and `A` one. Object rows: `ModelObject.implements_interfaces` (`workspace.rs:57`) moves, so `X`'s and `Y`'s OBJECT rows DIFFER; T1s now asserts that, and spec §5.2's T1s row says "routine rows" and why the digest is blind to it (it reads routine rows only). T1s's edges break re-run with the new preconditions: still `t1s CrossApp: oracle`.
- R5-3: T1s's `go` takes the object name as an argument.
- R5-4: left, as allowed.

Plan corrections: none to the design. `channel()` is a private helper not in the plan's interface list.

Gates: `scripts/ci-steps task-affected` exit 0 (4352 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `check-diff` no reasons. Commit `eca4419d`.

## Task 8
2026-10-25: `--l4-host` wiring.
- `src/main.rs`: `#[arg(long, hide = true)] l4_host` (doc: experimental, no editor output, a crash ends the server), passed to both `run_server` calls; test `t5d_flag_parses_and_is_hidden` in a new `#[cfg(test)] mod tests` at the end of the file.
- `src/server.rs`: `run_server(.., l4_host)`; `RootBuild.l4_host`; `ServerState.l4_host: Option<(HostHandle, JoinHandle<()>)>`; `build_server_state` spawns `spawn_l4_host(root, CrossApp)` (OQ1) after the snapshot build succeeds (a root that fails to build gets no host), and the `on_swap` closure's first statement is `handle.request()` (O(1), never waits: T4c). The diagnostics recompute is unchanged (OQ5). The host gets no sink, so it can send nothing to the editor (Q1). At shutdown `st.l4_host` is dropped with `st.tx`, not joined (like the updater).
- R7-2 (Task 7 review): the publish `debug!` logs `kind`, `seeded`, `total` only (`l4-host: published generation <g>: <kind>, seeded <s> of <t>`); the `l4-host: published generation <g>` prefix that T5e waits for is unchanged.
- R7-1: no code. A dead worker thread only matters in dev builds: the shipped profile (`release-ship`) aborts on panic, so a host panic ends the server (S6).

Tests (`cargo test -p al-sem --bin al-call-hierarchy`, 47 passed): `t5a_no_host_without_flag`, `t5b_didsave_requests_a_refresh` (a real `didSave` through `handle_notification` after a body edit of `Alpha.al`), `t5c_config_only_requests_a_refresh` (`ConfigChanged` on `state.tx`; status reaches 2, snapshot generation unchanged), `t5d_flag_parses_and_is_hidden`. Waits are deadline loops with `yield_now` (30 s); no fixed sleep. Red: compile errors (no `l4_host` field, old 7-argument `build_server_state`). Green at first run once it compiled.

Discrimination proofs (each break made with Edit, checked applied by `git diff | grep -c`, reverted with Edit, then green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T5a | `build.l4_host.then(..)` → `true.then(..)` | `no host without --l4-host` | yes |
| T5b | `on_swap`'s `handle.request()` → `let _ = handle` | `t5b`: `status: Some(HostStatus { generation: 1, .. })` after 30 s (t5c too) | yes |
| T5c | request only when `new.generation` moved (an `AtomicU64` of the last seen generation) | `t5c`: `status: Some(.. generation: 1 ..)` after 30 s; T5b stays green | yes |
| T5d | drop `hide = true` | `the flag is hidden: ..` | yes |

Plan corrections:
- `build_server_state` now takes `(workspace_root, config, channel, build: &RootBuild)` instead of seven loose arguments: adding `l4_host` as an 8th would trip `clippy::too_many_arguments` (no `#[allow]`). The two existing direct test callers use a new `test_build(sink, l4_host)` helper, which also replaced one `RootBuild` literal.
- `build_workspace` keeps its 7 arguments (same lint); its literal sets `l4_host: false` and `run_server` sets `root.build.l4_host = l4_host` on every root right after. The pass-through T5e pins is `main.rs` → `run_server` → `RootBuild`, which this keeps.
- The coordinator's T5 naming (T5b config-only, T5c parse, T5d flag reaches state) differs from the spec's; the spec's names are used, and every item is covered: config-only = T5c, parse/hidden = T5d, the flag reaching `ServerState` = T5a (off) + T5b/T5c (on).
- The first gate run failed clippy `items_after_test_module` (the main.rs test module sat before other items); moved to the end of the file.

Gates: `scripts/ci-steps task-affected` exit 0 (4356 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `check-diff` no reasons. Commit `00d9181a`.

## Task 9
2026-10-25: T5e, `tests/lsp/l4_host_binary.rs` (new member of `tests/lsp/main.rs`; `T1A` and `write_ws` in `l4_host_oracle.rs` are now `pub(crate)`). `t5e_flag_on_sends_nothing_new` spawns `env!("CARGO_BIN_EXE_al-call-hierarchy")` (the LSP binary keeps that name, `Cargo.toml` `[[bin]]`) twice on one canonicalized T1a tempdir, `--lsp --no-watcher --no-telemetry --verbose`, first with `--l4-host`, then without; HOME/USERPROFILE/XDG_CONFIG_HOME point at an empty tempdir and `RUST_LOG` is removed. Stdout frames are parsed on one thread (each frame asserted well-formed: `Content-Length` header, JSON body, `"jsonrpc": "2.0"`), stderr lines on another; a `Guard` kills and reaps the child on drop; every wait is a `recv_timeout` against one 60 s deadline. Session: `initialize` → response; `initialized`; `prepareCallHierarchy` on `A` (src/A.al 2:14) → read until its response, counting `publishDiagnostics` before it (O26 sync point; precondition: the response is a non-empty item array, so the root built); flag-on only: wait for the stderr line `l4-host: published generation 1`; `shutdown` → response; `exit`; then every message to stdout EOF; the child must exit. Assert: the sorted multisets of canonical messages (keys sorted recursively) are equal, THEN the `publishDiagnostics`-before-response counts are equal (so an extra message reports as a multiset difference, a timing difference as a count difference). Each run: 5 messages (initialize result, 2 `publishDiagnostics`, the prepare result, the shutdown result).

Red/green: green at first run (Task 8 had wired the flag). Red evidence is the two breaks.

Determinism: 5 consecutive runs of `cargo test --test lsp l4_host_binary`, all green (about 0.03 s each).

Discrimination proofs (each break made with Edit, checked applied by `git diff | grep -c`, reverted with Edit, then green):

| test | break | failed assertion | reverted green |
|---|---|---|---|
| T5e (i) | `main.rs`'s `--lsp` branch passes `false` instead of `args.l4_host` to `run_server` | `waiting for "l4-host: published generation 1" on stderr: timed out` (60 s) | yes |
| T5e (ii) | `build_server_state`, when the host is spawned, sends one extra `textDocument/publishDiagnostics` (`file:///extra.al`, empty) through the server sink | `the same server-to-client messages, with and without --l4-host` | yes |

Break (ii) sends synchronously in `build_server_state` (deterministic) rather than from the worker after its publish (which would race `shutdown`); it reaches the client through the same sink.

Review fixes folded in (Task 8 review):
- R8-1: `host_reaches` tests `generation >= N`.
- R8-2: comment reworded (the smaller correct change): `ServerState.l4_host`'s doc says it is never joined and the worker usually lives until the process exits, because the updater's `on_swap` holds a handle clone; the shutdown comment says the worker ends only if the updater also ends. The type stays `Option<(HostHandle, JoinHandle<()>)>` (plan §4.2).
- R8-3, R8-4: no change, as asked.

Gates: `scripts/ci-steps task-affected` exit 0 (4357 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `check-diff` no reasons. Commit `bc079c9a`.

## Task 10
2026-10-25: CHANGELOG `Unreleased/Added` entry for the hidden `--l4-host` host: what it re-solves, no editor output (only the status and the `debug!` line), cross-app only, what forces a Full refresh (`.al` add/remove/rename, any `app.json` change such as a version bump, any dependency-closure change, table fields or collided sites), cancellation and stale results, the crash note (S6, the shipped profile aborts on panic), and D-1/D-2. `docs/OUTSTANDING.md`: "A future incremental L4 path" closed, pointing at the spec. Spec: appended §13 "As built" (deviations from the ledger; no accepted anchor range touched). As-built check: every coverage-map test exists by name (grep over `src` and `tests`).

Gates: `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved; `scripts/ci-steps task` exit 0 (whole suite, 4357 passed, 8 skipped); `check-diff` no reasons. Commit `bc4b94ca`.

## Final round 1 fixes
2026-10-25, on `bc4b94ca`, from `.agent/runs/20261025-084210-508661/final-r1-merged.md`. Every break was made with Edit, checked to apply once (`grep -c` = 1), run, and reverted with Edit.

| FX | verdict | change | proof (break → result; after revert → green) |
|---|---|---|---|
| FX-1 | fixed | `run_worker` publishes under the lock, logs after it | no test (stated limit: a blocking logger would replace the process-global one) |
| FX-2 | fixed | T4c `Held::refresh` panics on a release timeout | worker holds the lock across `refresh` → t4c FAILED in 10 s ("request() returns while a refresh runs"; Held: "refresh #1 was never released") |
| FX-3 | fixed | T5e: `protocol::path_to_uri`; no canonicalize on Windows | no break: Linux cannot run the Windows path (stated limit) |
| FX-4 | fixed | `prepare_l4_seeded` asserts `Base`, `Rel`, `L4` unreleased | loop over `[Cell::L4]` only → `prepare_l4_seeded_refuses_a_released_rel_or_base` FAILED ("expected a panic, none came") |
| FX-5 | fixed | `host.refresh`/`host.full`/`host.sccs_resolved`/`host.sccs_seeded` emitted per completed solve | `host.sccs_seeded` emission deleted → t5e FAILED (`(Some(1), Some(1), Some(4), None)`) |
| FX-6 | fixed + recorded | T1t (uncertainty edges), T1u (`collided_site_callee`); bindings, leaf content, fixed flag recorded redundant in §13 | uncertainty out of the row (`uncertainty.len() * 0`) → t1t FAILED (oracle, CrossApp: `member-not-found` vs `external-target`), t1u ok; `collided.len() * 0` in `solve_global_digest` → t1u FAILED (oracle), t1t ok |
| FX-7 | fixed | one `run_server` call in `main.rs` | `false` for `args.l4_host` in that call → t5e FAILED (stderr wait timed out) |
| FX-8 | fixed | CHANGELOG lists the fixed-role flag | doc |
| FX-9 | fixed | `sha256_of_debug` streams through a `fmt::Write` hasher | `update(s.trim_end())` → `sha256_of_debug_streams_the_old_bytes` FAILED (C1); precondition: a text > 64 KiB |
| FX-10 | fixed | `prepare_l4_seeded` drops the cross-app `dependency_calls` | drop deleted → `prepare_l4_seeded_drops_the_dependency_calls` FAILED (`Some(2)`) |
| FX-11 | fixed | `labels` takes the re-solved ids only | covered by every oracle case's label set (no new test) |
| FX-12 | fixed | `run_worker` skips `g == done` | skip off (`if false && ...`) → `t4d_leftover_wake_refreshes_nothing` FAILED (3 refreshes, want 2); T4b green with the fix |
| FX-13 | fixed | `spawn_l4_host` runs the worker under `guarded(.., end_the_server)`: eprintln + `process::abort()`; debug-only seam `AL_SEM_L4_HOST_TEST_PANIC` | `abort()` deleted → `host_panic_ends_the_server` FAILED ("the server ends after the panic", 60 s) |
| FX-14..18 | recorded | spec §13 "Final round 1"; FX-14 also at `sha256_of_debug` | — |

FX-6 per guard: bindings redundant (caller row; route callee = edge `to` or collided pick, callee `var` in its row one hop; dependency var-ness from closure `.app`); uncertainty edges pinned by T1t; leaf content redundant (`base_intraprocedural_summary(row, _, field_index)`, field index in R0); fixed-role flag redundant (app_guid in row vs `app.json`, `body_available` in row, out-edges in row); `collided_site_callee` pinned by T1u (Full(SolveGlobal)).

Gates: `scripts/ci-steps task-affected` exit 0 (4364 passed, 8 skipped); `scripts/check-goldens` exit 0 (1251 passed, 4 skipped), no golden moved.

## Final panel
- Round 1 on bc4b94ca. sol: fix-first. Opus: ship. code-review high: no correctness bug. Merged into FX-1..FX-18; all fixed or recorded in 1c42ba9e.
- Rebased onto 3f5030f9 (#288). The only conflict was CHANGELOG, resolved by keeping both entries; every subject survived. New head: b63fe860.
- Round 2: Opus said ship, with every FX accepted; its combined break of the five digest parts now fails T1t and T1u. sol accepted 23 and re-raised FX-13 (F1: the panic test was not cfg-gated; F2: eprintln! could panic before abort). Both were fixed in d2e7baf6 as FY-1 and FY-2.
- Round 3: sol and Opus both said ship. All 63 register entries are accepted by both. Converged after 3 of 5 rounds.
- Not changed: spec §13's FX-6 argument cites `capability_cone.rs:3968-3981`. After the #288 rebase that code is at `:4058-4071`. Opus confirmed the argument still holds. The citation is left as is: a spec edit after H would cost post-merge its identity path.

## Gate results
B = 3f5030f9 (origin/master, #288). H = d2e7baf6b2898de72f849ec7fd2cc796b46fcfc9. Each gate ran through `agentflow run` on H and exited 0: check-goldens-coverage, ci-steps-all, check-goldens and cdo-gate. Only the evidence commit follows H. There is no bench: the issue is non-speed.

## Design notes (not filed)
- D-1 (from P5): the v2 solve reads callee MODEL ROWS, not only callee summaries; early cutoff on summaries alone is unsound for event dispatch (a subscriber parameter rename). Shapes the spec (§4.3 R2). Not a bug in today's code.
- D-2 (from P16): any `.al` file add, remove or rename renames every routine id (the model-instance id hashes the file list). Incremental reuse across such an edit is impossible without rewriting ids; the spec takes Full. Candidate follow-up only if a consumer appears.
