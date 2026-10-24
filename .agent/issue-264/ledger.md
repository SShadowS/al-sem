# Issue #264 — interface dispatch over a non-extensible enum is still open-world

## Claim

```json
{"run_id":"20261024-074709-5c2b5f","issue":264,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/264-agent-discovery-resolver-interf-a1","worktree":"/work/al-sem-issue-264-a1","body_hash":"1e15086c4edb6ebbae2579bb2d68cb216320417b3ac9f62a65a26e5a88884777","reconciled":[]}
```

Issue body hash (claim `body_hash`): `1e15086c4edb6ebbae2579bb2d68cb216320417b3ac9f62a65a26e5a88884777`.
Base (`make-worktree`): `9bd2996fa2d293b48ad6d3997e52544c264745a0`.
Reviewers (operator instruction, 2026-10-23, confirmed again 2026-10-24 for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator (2026-10-24, in this window: "i confirm"): queue #264 → #265 → #271 → #272, relayed from alsem-main. The shared loop budget had been reset (2 used); claimed with a one-off `--loop-max 3`.

## Classification

**Architectural.** The change lives in the resolver: interface-dispatch classification (`combined_graph.rs`'s `interface-open-world` uncertainty, the snapshot's `polymorphic`/`openWorld`, the call edge's `resolution`/`candidates`), driven by where the receiver value comes from (an enum field, variable or parameter of a non-extensible enum). It moves the CDO unknown/ambiguous ratchets wherever such dispatch exists, and the cones that d35/d1/digest read. A spec document goes to the spec panel.

## Assumption probes

Run by the design author (opus). Scratch `/work/bench/264/` (probes, `cdo/` master outputs).

| probe | result | evidence |
|---|---|---|
| P1a resolver | the interface route types the receiver by its declared type only: `resolver.rs:2930-2992` fans out over `implementers_of` (every implementer in the program, `index.rs:879`) as `Polymorphic`; `full.rs:438-443` makes `Polymorphic` always `Partial{ReverseDependentImplementers}` (applied `full.rs:736`); `full.rs:670-672` records `SiteFacts.interface` | read; repro switch dump `Maybe`, `to: Demo Impl.NextKm`, `total_impls 1`, `enum_implementers [Enum 50102]` |
| P1b north-star bucket | an interface site with a real route already counts `resolvedSource`/`resolvedAbiExternal` (`edge.rs:640-660`); `conditionalResolved` here means manual-binding event subscribers; completeness matters only for an empty fan-out (`edge.rs:667-677`) | repro stats `resolvedSource 4`, `conditionalResolved 0`; CDO whole-program `conditionalResolved 266` all events, primary `0` |
| P1c adapter | `program_calls.rs:2023-2150`: one `Maybe` edge per model implementer, one to-less `ExternalTarget` per outside implementer; `enum_implementers` metadata only (`:2119`) | read |
| P1d uncertainty | `combined_graph.rs:358-381` adds `interface-open-world` on every targeted interface edge; `:384-404` on to-less ones | read |
| P1e snapshot | `l5/snapshot.rs:2201` always `open_world: Some(true)`; `test_coverage.rs:502-515` has its own closed rule | read |
| P1f d35 path | call-site-local kind (`db_effect_solver.rs:842`); `capability_cone.rs:3568-3603, 4327-4345` make coverage partial; `capability_query.rs:152-166` answers `Unknown`; `d35.rs:62-70, 149-170` reports medium; `confidence.rs:112` adds the `dynamic-dispatch` cap | master `/work/bench/264/repro-closed.master.json`: medium d35, `interface-open-world` (same for direct-closed and ext) |
| P2a Extensible default | absent = not extensible; an enumextension fails AL0504 for absent and `false`, `true` compiles; symbol files hold `"1"`, `"0"` or absent | alc 18.0.41.62505, `/work/bench/264/probes/ext-{absent,false,true}`; Base App symbols 47 × `"0"`, 498 × `"1"`, 21 absent |
| P2b extension from another app | not compiled cross-app; the same-app AL0504 rejection is the load-bearing evidence | as P2a |
| P2c implementations | every value maps to an implementation or the enum declares `DefaultImplementation` (else AL0596); `UnknownValueImplementation` for an unmatched ordinal; all are codeunits the enum's app can see, so the program's implementer set is a sound superset | `probes/impl-missing` (AL0596), `probes/impl-default` compiles; MS Learn DefaultImplementation property |
| P2d data flow | the receiver must be a routine local; every write from a closed enum; never passed as an argument or a `foreach` variable (parameters, globals, return values can be set from outside) | AL semantics (compile-time question; sidecar not needed) |
| P3 existing data flow | exists: `infer_receiver_type_for_expr` (`receiver.rs:1443`) types field/local/parameter/call-result RHS as `EnumType{name}`; gaps: `X::Value` drops the enum name (`receiver.rs:1612-1616`), only a first-assignment walker (`ir_walk.rs:2402`), no extensibility fact on `ObjectNode`/`AbiObject` (properties already lowered/parsed), enum values not lowered/ingested | read, grep |
| P4 CDO population | workspace model: 19 interface sites (30 edges); all 5 source interface enums are `Extensible = true`; exactly 2 closed sites: `CDOeDocPrePostValid.Codeunit.al:28-29, 54-55`, enum CDN 6086237 (`Extensible "0"`, 2 implementers = its 2 mapped codeunits); dependencies: 12 non-extensible interface enums, ~55 enum-sourced assignments (regex); d35: primary 52 findings, 41 mention open-world, 6 sole-reason; all 162/81/19; every sole-reason one traces to extensible or list-held interfaces → predicted 0 d35 findings disappear on CDO | `/work/bench/264/cdo/` (release-fast master: switch-dump, stats, analyze primary/all) |
| P5 goldens/ratchets | no histogram bucket moves → no ratchet re-pin (incl. `CDO_AMBIGUOUS_RESOLVED`, `program_resolve_harness.rs:2898`); semantic-edges goldens do not serialize completeness; the r0 enum fixture dispatches through a global (stays open); `test-selection-graph/Tax.al` becomes closed but its asserts (`cli_graph_export.rs:242-250`, `cli_digest_test_coverage.rs:288`) do not change; `--switch-dump` text and graphify `open_world_reason` change for closed edges | grep/read |
| P6 grammar bump 960ed312 → c89c2ecb (measured by alsem-second) | nothing moves: tests, goldens and CDO stats identical; `recoveredFiles` 0 on both sides (whole program), routine `parseIncomplete` 0/5,479 (workspace), `aldump --l2` byte-identical. #264's clean-parse rules see the same input on either grammar | `/work/bench/grammar-c89/REPORT.md` |
| P7 fixture compile (sonnet, alc 18.0.41 via `/opt/al-tool/al compile`, symbols from `/work/bench/v201/ws-a/.alpackages`) | T1, T2, T3 (all 7 RHS shapes + A4 shapes), T4, T22, T27 compile clean. T21 (dependency and workspace both declare enum "Demo Kind") is REJECTED: `error AL0197: An application object of type 'Enum' with name 'Demo Kind' is already declared by the extension 'Dep264 …'` (different ids, name alone). T9b (field `Kind` + procedure `Kind()`) compiles on the table and on a tableextension, each with `AL0486 … This warning will become an error in a future release` | `/work/bench/264/probes/fx-*/` |

Consequence of P7: S1's same-name cross-app confusion cannot arise in a compiling closure, so the identity typer is defence in depth (it also guards input al-sem parses but alc rejects). The design keeps it. T21 becomes a stated-precondition defensive test that cites AL0197. T9b keeps its shape, with AL0486 noted.

Spec: `docs/superpowers/specs/2026-10-24-issue-264-closed-enum-interface-dispatch-design.md`.

## Timeline

- 07:47 claim #264 attempt 1 (`--loop-max 3`), worktree at `9bd2996f`.
- Classified architectural; design author dispatched (opus).
- Spec panel r1: sol rework (9 findings), opus approve-with-fixes (12). Rev 2 accepted all; sol 6 premise refuted by alc probe P2e (AL0130) but the allowlist adopted.
- Spec panel r2: opus approve; sol approve-with-fixes (S1 binding guards, S8 test breaks re-raised). Both recommended deferring ABI `Values` ingestion → conductor decision C1 (symbol-only enums stay open; Discovery D1). Rev 3.
- Spec panel r3: opus approve (+N3 → O15); sol approve-with-fixes (T9b types reversed; D1 reproducer). Rev 4.
- Spec panel r4: both approve, no new findings. Register 25 entries, all `fixed`, all accepted by sol and opus. spec_rounds used 4 of 5. Spec committed `303868e2`.
- 08:40 acceptance tests red (opus): 6 red, 3 green controls, uncommitted. 08:50 fixture compile probe P7 (sonnet).
- 08:53 plan (opus) committed `cbc20247`: 6 tasks, plan_tasks charged 6 (remaining 6). Task 1 implementer dispatched (opus).
- 09:2x Task 1 committed `090629b3` (1 attempt). Gate `task-affected` (whole suite, `NEXTEST_PROFILE=ci` so it runs past the expected reds): 3995 run, 3989 pass, 6 fail = exactly the six expected i264 reds; fmt/clippy-dev/gen-syntax green. F5: `l2_ir` 73/73, no golden moved. Lowerer-reviewer (opus): I1 important — an `#if` between `Implementation` list elements stays in one entry's raw text → conductor decision: keep the plan's interface; Task 2's parser fails OPEN on any entry containing `#`, pinned by a test. M1–M4 minor, not fixed (single-quoted value names, escapes, comments, nested values: all forms alc rejects or that match `ident_text` elsewhere).
- Task 1 review (sonnet): approve; minors (a) early-return comment, (b) lowerer pin of the I1 shape → sent with Task 2.
- Task 1 minors `877a0737` (gate 3996/3990, six expected reds). Task 2 `bf5a32c1` (1 attempt; gate 4013 run / 4007 pass / six expected reds; no golden moved): `ObjectNode.closed_enum` + `enum_impls`, `enum_closure`, `parse_impl_pairs` (fails open on `#`/unreadable text), abi_ingest open, `dedup_objects` (AND + clear), PACK_SCHEMA 7, 37 hand-built literals updated. T9 written in the cli file (uncommitted).
- Task 2 review (sonnet): approve. Carried into Task 3: (important) namespace stripping in `read_impl_pairs`/`fold_ref_name` could let a same-named codeunit from another namespace satisfy `enum_covered_by_routes` → qualified refs must match namespace or count as not covered; (minor) `enum_closure` returns open when `implements` is empty. Minors 3–4 (quoted dotted interface names false-open; odd `ref_text` shapes resolve to nothing) are safe-direction, noted only. Task 3 dispatched.
- Task 3 `172edacc` (1 attempt; gate 4037 run / 4031 pass / six expected reds, no golden moved; F1 smoke: repro-closed stats identical). The cli file stays uncommitted: the six reds are model-level and turn green only in Task 5. Resolver tests live in `closed_enum.rs`. Namespace-qualified implementation refs are refused in the fact (the graph holds no namespace data). Stated limits (break stayed green because another guard covers it): T11 real-route vs §2 coverage (added `unknown_route_on_the_edge`); enum_not_implementing needs both guards broken; unassigned_local's ≥1-write rule redundant with empty sources. Conductor decision on plan issue 4 (qualified DECLARED types): Task 4 requires a Unique name hit (ambiguous → open) with a two-namespaces test; the residual case "right enum not loaded, a same-named one is" is a stated limit for the final panel.
- Task 3 review (sonnet): approve with one required fix. IMPORTANT soundness hole: an `#if` group used as an OPERAND lowers only its first arm and records a `SyntaxIssue` without setting `parse_incomplete`/Recovered, so a hidden write to V escapes `Occurrences` → fix: any syntax issue inside the routine span leaves the site open (+ Reassign-in-non-first-arm test). Minors: module-doc reword (routes are a superset), stated limit that interface identity is the folded last segment, optional ABI-key helper. Sent with Task 4.
- `f71faea6` Task 3 review fix (any `SyntaxIssue` overlapping the routine span → open; test uses the assignment-target operand shape, because argument-list `#if` arms are already all read; red before the fix = hole was real). Stated limit: T26 now also covered by the issue check; the Unknown arm stays as an independent guard.
- `76cfe6c8` Task 4 (1 attempt; gate 4047 run / 4041 pass / six expected reds; no golden moved): identity typer arms (variable, `Rec.Field` via owner, `Enum::"X"::V`, `"X"::V`, `FromInteger`, bare call), binding guards (T9b/T9c/T9d), `resolve_object_ref_strict` (own-app duplicates → Ambiguous) for enums. Process deviation: implementation written before tests; red evidence is per-arm breaks, each isolating its own test (proof table). Conductor decision: the strict lookup also goes into `enum_covered_by_routes` (two same-named codeunits in different namespaces both implementing I satisfy first-wins coverage) → follow-up commit.
- `b412812d` strict implementer lookup (the first-wins case closed wrongly; proof row in table).
- Task 4 review (sonnet, over f71faea6..b412812d): approve with one required fix. IMPORTANT I1: the own-app shortcut picks an own closed `Ns1.Kind` when the code means a dependency's extensible `Ns2.Kind` (qualified, or unqualified via `using`) → enum identity requires whole-index uniqueness. Minors: M1 Record receiver table first-wins (same uniqueness check), M3 overlap boundary test, M2 DRY helper (`name_shadows_bare_call`; adding the named return to the existing guard must be measured on histogram/goldens). Sent with Task 5.
- `a0e2f7a0` Task 4 review fixes (whole-program name uniqueness for enum identity and implementer lookup; table uniqueness in `field_enum`; overlap edges; `receiver::name_shadows_bare_call` shared, the existing guard now also checks the named return — repro stats identical, no golden moved; CDO in Task 6). All three new soundness tests red before the fix.
- `200d679c` Task 5 (1 attempt): `DispatchMeta.closed_over`, combined_graph `closed_sites`, snapshot `openWorld: Some(!closed)`, unresolved_cone skip, CHANGELOG Fixed entry; the cli acceptance file committed (six reds green + T9, T12, T13, T14, T29). Gate `task-affected` without ci profile: exit 0, 4057/4057; `check-goldens` 1099/1099. Process deviation (Tasks 4, 5): implementation before some tests; red evidence is per-mutation breaks. Stated limits: T14 pins that an unmapped committing implementer still yields d35 YES (routes keep every implementer); T9's site half only via the fact assert.
- Task 5 review (sonnet, a0e2f7a0 + 200d679c): approve once IMPORTANT 1 is settled — `name_shadows_bare_call` adds the named return to the SHARED bare-call guard, against the spec's "no change to shared receiver typing"; only repro-closed stats + goldens measured. Resolution: the CDO M1 histogram run (already dispatched) decides; identical → keep and record; moved → revert to closed_enum only. Minors: to-less-branch `!closed_sites` guard (unreachable today, add for defence), snapshot `group_edges[0]` coupling comment/test, CHANGELOG mention of graphify `open_world_reason` / `--switch-dump` text, T14 limit in the holes doc or non-goals.
- `523a601c` Task 6 code half (1 attempt): CDO census `cdo_i264_closed_interface_site_census` in program_resolve_harness (12/318, runs under cdo-gate; skips without CDO_WS), Task 5 review minors (to-less-branch guard, snapshot coupling comment, CHANGELOG notes incl. the T14 limit). Gate `task-affected` 4058/4058. All 6 plan tasks done.
- Rebase onto origin/master `1134bed1` (#239, #241, #160 step 6, #278, #237) before the final panel; overlap: combined_graph.rs, snapshot.rs, unresolved_cone.rs (#160 step 6), CHANGELOG. Pre-rebase head `523a601c`. Delegated to the implementer (opus). Done: head `db03371b`; one conflict (CHANGELOG Fixed, kept #237 then #264); the three code files auto-merged with every #264 hunk intact beside #160's code. Gate 4132/4132, check-goldens 1125/1125, check-diff clean, census still 12/318. #160 step 6 adds no new reader of open-world signals; its `unfetched-dependency` status does not reach polymorphic groups. Panel question: a closed site whose candidate is a body-less dependency row counts as closed, and that row's missing body still makes the cone partial.
- Merge order (alsem-main, 2026-10-24): #160 step 6 → #278 (grammar bump) → #264. Do not merge #264 until alsem-main says so; rebase and re-gate on the master of that moment. Update: #160 step 6 merged `b028ec32`; hold lifted, but #278 merges before #264. Shared grammar clone pulled to c89c2ecb (fast-forward, node-types unchanged; P6 measured no move). Task 3's gate may straddle the pull; the step-11 gates on the rebased head run on c89c2ecb regardless.

## Acceptance tests (red, uncommitted)

Written by an opus agent in `tests/cli/cli_d35_unknown_commit.rs` (`#264` section). `cargo test --test cli cli_d35_unknown_commit::`: 12 pass (old + 3 new controls), 6 new fail for the stated reason. Log: scratchpad `i264.log`.

| test | pins | red output |
|---|---|---|
| `i264_t1_closed_enum_repro_has_no_d35_and_a_closed_site` | T1, A1, A2 | `expected no d35 finding, got ["Commit may be reachable … (interface-open-world; …)"]`; site `openWorld Some(true), want Some(false)`; meta lacks `closed_over`; `interface-open-world` emitted |
| `i264_t2_closed_enum_direct_call_is_complete_with_no_d35` | T2, A1, A2 | `the NextKm edge must be Complete, is Partial { reason: ReverseDependentImplementers }` + as T1 |
| `i264_t4_absent_extensible_is_closed` | T4, A5 | as T1 |
| `i264_t3_rhs_shapes_are_closed` | T3, A8 | all 7 shapes (FromLocal, FromParam, FromField, FromQuotedValue, FromEnumValue, FromInteger, FromOwnCall) open, no `closed_over`, uncertainty emitted |
| `i264_t22_prove_and_digest_see_no_blocker_for_a_closed_site` | T22 | `prove obligations (unresolvedCallsites, openWorldRows) = (0, 1), want (0, 0)`; blocker `open-world-dispatch "Strategy.NextKm in IsDue"`; digest lists the NextKm row `openWorld: true` |
| `i264_t27_dependency_implementer_outside_the_model_stays_open` | T27 | single-app half green (open, to-less edge precondition holds); cross-app half red: site not closed |

Green controls (assert "unchanged"; discrimination belongs to the implementation tasks): `i264_a3_extensible_true_stays_open` (A3), `i264_a4_parameter_codeunit_global_and_argument_receivers_stay_open` (A4: T5, T6, T6b, T7, T8), `i264_t21_same_name_enum_resolves_in_the_declaring_app` (T21).
Test contract the implementation must keep: `DispatchMeta` gets a field named `closed_over` that is part of its derived `Debug` (the tests read the Debug text because `DispatchMeta` is not `Serialize`).
Left to the plan: T6 (discrimination), T8b, T9, T9b–T9d, T10–T20, T23–T26, T28–T34.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| `enum_values_are_lowered_with_implementations` | filter `p.name == "implementations"` | FAILED `left: []` vs `right: ["\"I\" = \"Impl A\", \"J\" = \"Impl J\""]` | ok (4 passed) | `090629b3` |
| `enum_value_implementation_if_arms_give_one_entry_per_arm` | `.take(1)` after the implementation map | FAILED `left: ["\"I\" = \"P\""]` vs `right: [.."P", .."Q"]` | ok (4 passed) | `090629b3` |
| `enum_value_inside_if_wrapper_is_lowered` | preproc arm `_ if false && is_preproc_wrapper(node)` | FAILED `left: ["A"]` vs `right: ["A", "X"]` | ok (4 passed) | `090629b3` |
| `non_enum_objects_have_no_enum_values` | `if kind == ObjectKind::Enum` → `if true` | FAILED at `all(enum_values.is_empty())` (enumextension value lowered) | ok (4 passed) | `090629b3` |
| `enum_value_implementation_element_if_stays_one_raw_entry` | `.filter(\|v\| !v.contains('#'))` on implementations (unique anchor; an earlier refused Edit's green run discarded) | FAILED `left == right failed: []` | ok (5 passed) | `877a0737` |
| T15 `enum_closure_if_arms_disagree_is_open` | `.take(1)` on the Extensible entries | `left: (true, [("demo strategy","demo impl")]) right: (false, [])` | ok (17 passed) | `bf5a32c1` |
| T17 `enum_closure_recovered_file_is_open` | `(parse_incomplete && false)` | `left: (true, [..]) right: (false, [])` | ok | `bf5a32c1` |
| `enum_closure_non_enum_is_open` | drop the `kind != Enum` check | `left: (true, []) right: (false, [])` | ok | `bf5a32c1` |
| RF4 `enum_closure_maps_each_implemented_interface` (+ `value_without_mapping_or_default_is_open`) | `!all(covered) && false` | `left: (true, [("i","a i"),("j","a j"),("i","b i")])`; value_without FAILED too | ok | `bf5a32c1` |
| T32 `enum_closure_default_and_unknown_are_mapped` | drop the `unknownvalueimplementation` arm | `demo unknown missing from [..]` | ok | `bf5a32c1` |
| T32 (same test) | `(defaults.contains(iface) && false)` | `assertion failed: closed` | ok | `bf5a32c1` |
| I1 `enum_closure_element_if_in_implementation_list_is_open` | `read_impl_pairs(raw).unwrap_or_default()` (skip unreadable) | `left: (true, [("i","def i"),("j","def j")])` | ok | `bf5a32c1` |
| `parse_impl_pairs_reads_pairs_and_rejects_anything_else` (+ I1) | `ref_text` accepts any char (`\|\| true`) | `left: Some([("\"I\"", "\"P\"\n#if X"), ..]) right: None` | ok | `bf5a32c1` |
| RF5 `enum_closure_includes_conditional_values` (+ RF4, value_without) | `enum_values.iter().take(1)` | FAILED (3 tests) | ok | `bf5a32c1` |
| `enum_closure_absent_extensible_is_closed` | absent counted as open | FAILED (+3 closed tests) | ok | `bf5a32c1` |
| `enum_closure_unknown_value_is_open` | only `== "true"` counts as extensible | FAILED (only this) | ok | `bf5a32c1` |
| `enum_closure_true_is_open` (+ unknown, if_arms) | Extensible ignored (`&& false`) | FAILED (3) | ok | `bf5a32c1` |
| `enum_closure_false_is_closed` | any Extensible entry counts as open (`\|\| true`) | FAILED (only this) | ok | `bf5a32c1` |
| T16 `dedup_objects_ands_closed_enum_and_clears_impls` (+ unions) | plain id dedup | `assertion failed: !objects[0].closed_enum`; unions `left == right failed` | ok | `bf5a32c1` |
| T33 (same test) | drop only `kept.enum_impls.clear()` | panicked at `enum_impls.is_empty()` (build.rs:2889) | ok | `bf5a32c1` |
| T31 `pack_round_trips_closed_enum` | `#[serde(skip)]` on `closed_enum` | `assertion failed: back.files[0].objects[0].closed_enum` | ok | `bf5a32c1` |
| Task 3 red phase: base, T28, RF2 closed, RF3, T32 present, T19, T34 precondition | `closed_over` stub returns empty | 7 FAILED, e.g. `left: Partial { reason: ReverseDependentImplementers } right: Complete` | ok (24 passed) | `172edacc` |
| T8b | walk skips Parenthesized children | FAILED (1 of 38) | ok | `172edacc` |
| T23 | foreach `var` not walked | FAILED | ok | `172edacc` |
| T24 | `With { body }` walked instead of declined | FAILED | ok | `172edacc` |
| T26 | `StmtKind::Unknown => Some(())` | FAILED | ok | `172edacc` |
| T18 | `(parse_status != Clean && false)` | FAILED | ok | `172edacc` |
| `i264_unknown_route_on_the_edge_stays_open` | real-route check `&& false` | FAILED | ok | `172edacc` |
| T20 + T32 missing default | coverage `\|\| true` | both FAILED | ok | `172edacc` |
| T19 | `target_object` AbiSymbol `.filter(\|_\| false)` | FAILED | ok | `172edacc` |
| T25 + RF2 open half | quoted identifiers not counted as V | both FAILED | ok | `172edacc` |
| RF2 closed half | `name_of` unfolded | FAILED `left: Partial.. right: Complete` (+5) | ok | `172edacc` |
| T34 | per-interface entry check `\|\| true` | FAILED | ok | `172edacc` |
| T6b `i264_t6b_assigned_parameter_stays_open` | parameters accepted as the local | FAILED | ok | `172edacc` |
| `i264_global_of_the_same_name_stays_open` | globals check `&& false` | FAILED | ok | `172edacc` |
| T30 (+T25, RF2) | `filter_map` (any closed write suffices) | 3 FAILED | ok | `172edacc` |
| T28 | `writes.len() == 1` | FAILED | ok | `172edacc` |
| `i264_enum_not_implementing_the_interface_is_not_a_source` | implements-I check AND coverage dropped | FAILED (+T20, T32) | ok | `172edacc` |
| namespace (fact + resolver tests) | refusal `\|\| true` | both FAILED; the resolver closed on Ns1's codeunit (risk was real) | ok | `172edacc` |
| `enum_closure_without_interfaces_is_open` | `implements.is_empty()` check removed | FAILED | ok | `172edacc` |
| `i264_operand_if_group_in_the_routine_stays_open` | before the fix; then issue check `&& false` | `assertion failed: is_open(completeness(..))` both times | ok (40 passed) | `f71faea6` |
| T3 `i264_t3_rhs_shapes_complete_at_resolver` | `Enum::"K"::V` arm → None | `shapes left open: ["FromEnumValue"]` | ok | `76cfe6c8` |
| T3 (+T29) | field arm `.filter(\|_\| false)` | `["FromField"]`; t29 FAILED | ok | `76cfe6c8` |
| T3 | `"X"::V` arm off | `["FromQuotedValue"]` | ok | `76cfe6c8` |
| T3 | FromInteger arm off | `["FromInteger"]` | ok | `76cfe6c8` |
| T3 | bare-call arm off | `["FromOwnCall"]` | ok | `76cfe6c8` |
| T3 (+7 closed tests) | variable arm off | `["FromLocal", "FromParam"]` | ok | `76cfe6c8` |
| T9b | `table_scope_has_routine` guard `&& false` | FAILED | ok | `76cfe6c8` |
| T9c + T9d | call shadow guard `&& false` | both FAILED | ok | `76cfe6c8` |
| T9d | only the return-name part of `in_scope` dropped | FAILED | ok | `76cfe6c8` |
| T29 `i264_t29_two_closed_enums_are_complete` | `enums.truncate(1)` | FAILED | ok | `76cfe6c8` |
| RF1 number half | raw text used as the enum name | `assertion failed: by number`, `left: Partial.. right: Complete` | ok | `76cfe6c8` |
| RF1 namespace half | dotted type text declined | `assertion failed: namespace-qualified` | ok | `76cfe6c8` |
| `i264_same_named_enums_in_two_namespaces_stay_open` | plain first-wins `resolve_object_ref` | FAILED (closed on the Ns1 enum) | ok | `76cfe6c8` |
| `i264_quoted_value_shadowed_by_a_variable_stays_open` | `in_scope` guard `\|\| true` | FAILED | ok | `76cfe6c8` |
| T21 `i264_t21_field_enum_resolves_in_the_declaring_app` (defensive, AL0197) | field enum resolved from the caller | FAILED | ok | `76cfe6c8` |
| `i264_same_named_implementers_in_two_namespaces_stay_open` | plain `resolve_object_ref` in `enum_covered_by_routes` | FAILED `is_open(completeness(&ws))` (site closed) | ok (50 passed) | `b412812d` |
| I1 `i264_qualified_name_of_a_dependency_enum_stays_open` + `i264_unqualified_name_shared_with_a_dependency_enum_stays_open` | own-app-only count instead of the whole-program count | both FAILED (red before the fix too) | ok | `a0e2f7a0` |
| M1 `i264_table_name_shared_with_a_dependency_stays_open` | table uniqueness check `&& false` (re-run alone after a double-break run was discarded) | FAILED | ok | `a0e2f7a0` |
| M3 `issue_overlap_is_half_open_on_both_edges` | `a.start <= b.end` / `b.start <= a.end` | panicked "starts where the routine ends" / "ends where the routine starts" | ok | `a0e2f7a0` |
| `i264_issue_in_another_routine_keeps_the_site_closed` | any issue in the file opens the site | FAILED | ok | `a0e2f7a0` |
| T9d through `name_shadows_bare_call` | return-name line `&& false` (redone after a non-applying break) | FAILED | ok | `a0e2f7a0` |
| T1, T2, T3, T4, T27, T12, T29 | combined-graph closed-site skip removed | 7 FAILED | ok (23) | `200d679c` |
| T12 (+T29, T27) | skip only on the meta-carrying edge | 3 FAILED | ok | `200d679c` |
| T13 + T14 | candidate edges removed from the model at closed sites (the plan's combined-graph break stays green: d35 reads model edges) | title `… (incomplete call coverage)` instead of YES | ok | `200d679c` |
| T2 | `completeness_for_shape` restored, `closed_over` kept | "the NextKm edge must be Complete, is Partial" | ok | `200d679c` |
| T22 | `unresolved_cone` skip `&& false` | FAILED | ok | `200d679c` |
| T27 | `dependency.is_empty()` gate dropped | "T27 single-app: openWorld … Some(false)" | ok | `200d679c` |
| T29 | `closed_over.truncate(1)` | FAILED | ok | `200d679c` |
| A4 / T6 (deferred) | parameters accepted in rule 2 | T6b ParamAssigned FAILED; T6 ParamOnly (control) green | ok | `200d679c` |
| `cdo_i264_closed_interface_site_census` (program_resolve_harness, CDO-gated; runs under cdo-gate) | `closed_over.clear()` after the sort in the adapter | FAILED `left: (0, 318)`, `right: (12, 318)` | ok (14.8 s); skips without CDO_WS | `523a601c` |
| `i264_fo1_if_alternative_routines_close_only_when_every_arm_is_closed` | `all` → `any` over rows in `closed_interface_sites` (also red before the fix) | "FO1 open arm first: the open arm must keep the unknown Commit, got none" | ok | `f781026e` |
| same (snapshot half) | snapshot reads the representative's meta again | subscriber row `open_world: Some(false)` | ok | `f781026e` |
| `i264_fo2a_if_alternative_return_types_stay_open` | `return_disagrees … && false` | "FO2a closed first" | ok | `061a2d1c` |
| `i264_fo2b_if_alternative_table_field_types_stay_open` | field union `&& false` | "FO2b closed first" | ok | `061a2d1c` |
| `i264_fc4_types_from_a_recovered_file_stay_open` | owner clean check / callee clean check `&& false` | "field from a recovered file" / "return type from a recovered file" | ok | `061a2d1c` |
| `i264_fc3_quoted_value_named_like_an_implicit_rec_field_stays_open` | guard `&& false` / guard limited to Table objects | "table" / "page" | ok | `c47c5af7` |
| `i264_fc5_conflicting_if_declarations_of_the_value_stay_open` | MalformedDuplicate takes the first local | "FC5 closed first" | ok | `c47c5af7` |
| `i264_fo3_if_alternative_enum_declarations_stay_open` / `…_dependency_enum_declarations_stay_open` | build.rs:371 / :308 back to `dedup_by` | "FO3 closed first" / "FO3 dependency closed first" | ok | `d3da6897` |
| `i264_new1_quoted_value_in_a_report_data_item_stays_open` (first version in the parent trigger did not discriminate; replaced by the child-trigger shape) | the Report/XmlPort decline `&& false` | `assertion failed: is_open(site_completeness(&report, "onaftergetrecord"))` | ok (63 passed) | `66fd6e14` |
| T9 `i264_t9_symbol_only_enum_stays_open` (committed in `200d679c`; site half is a stated limit: N3 keeps it open even with `closed_enum: true`) | `closed_enum: true` in abi_ingest | panicked at :1310 dumping `closed_enum: true` | ok | (lands with the cli file) |

## Findings register summary

`findings.json`: 40 entries. Spec panel (4 rounds): S1–S9 (sol), O1–O15 (opus), C1 (conductor scope decision) — all `fixed`. Final panel (5 rounds; sol joined at round 3 after a Codex usage limit): FO1–FO6 (opus), FC2, FC3, FC5, FC6 (code-review skill; FC1/FC4/FC8/FC9 folded into FO1/FO2), N21–N25 (sol). All `fixed` except FC6 (minor, cost only) `deferred`; FO4 closes_by the cdo-gate record on H. Every entry accepted by sol and opus.

## Measurements (M1–M4, CDO, sonnet; report `/work/bench/264/cdo-after/REPORT.md`)

Branch `200d679c` vs master `9bd2996f` baseline, release-fast, the pinned CDO workspace @ bc3ccb18 clean, grammar c89c2ecb (P6: no move).

| M | prediction | measured |
|---|---|---|
| M1 histogram | unchanged | byte-identical `stats.json` (whole 46328, unknown 0, ambiguousResolved 23, conditionalResolved 266; primary 20916 / 0). The shared bare-call guard change (named return) moves nothing → kept (Task 5 review important 1 settled) |
| M2 closed sites | single-app 0; cross-app 2 workspace + some dependency | single-app 0; cross-app 12 of 318 (2 CDO IPrePostValidator, 3 CDN INetworkProfileSelector, 3 CTS-SYS IAzureBlobAuthorization, 1 MS System App AI Service Name, 3 MS Base App IC Data Exchange); all source-checked correct; missed closes not checked |
| M3 analyze primary / all | 0 d35 disappear | findings 2323 / 30800 identical, d35 52 / 162, bySeverity + byDetector + (fingerprint, id, severity, title, confidence) identical; only text: two d35 findings `2726dd730c6a872e`, `a10124f0a17d5512` "+10 more" → "+9 more" partly analysed (correct: RunPrePostValidation leaves the list) |
| M4 ratchets | no move | none move (M1 identical); no test pins the "+10 more" text |
| re-measure after the final r1 fixes (head `0b4519b4`, `/work/bench/264/cdo-r1/`, sonnet) | no move from FO2a/FO2b | `stats.json` byte-identical to master AND to 200d679c; analyze primary/all byte-identical to 200d679c (vs master only the same "+10 → +9 more" text on the two d35 findings). Verdict: identical |
| re-measure after the final r3 fixes (head `05f82b1c`, `/work/bench/264/cdo-r3/`, sonnet) | no move from N2-1 | stats.json, an-primary.json, an-all.json byte-identical to cdo-r1. Verdict: identical. (alsem-second then asked to hold CDO runs for #277 Task 1 timing; held.) |

## Gate results

| gate | head | exit | log |
|---|---|---|---|
| ci-steps-task (pre-panel) | `db03371b` | 0 (30 s, supervised; tree clean) | `.agent/runs/20261024-074709-5c2b5f/logs/ci-steps-task/20261024T104655-9233be79.log` |
| ci-steps-task (after final r1 fixes + NEW-1) | `66fd6e14` | 0 (29 s, supervised; tree clean); pushed to PR #283 | `.agent/runs/20261024-074709-5c2b5f/logs/ci-steps-task/20261024T112837-729dfd9a.log` |
| ci-steps-task (after r3 fixes) | `05f82b1c` | 0 (33 s, supervised) | `.agent/runs/20261024-074709-5c2b5f/logs/ci-steps-task/20261024T121407-d22eac5d.log` |
| ci-steps-task (after r4 fix) | `ae4955ed` | 0 (43 s, supervised) | `.agent/runs/20261024-074709-5c2b5f/logs/ci-steps-task/20261024T122529-bbf8eb43.log` |
| **ci-steps-all** (H) | `ae4955ed` | **0** (380 s, supervised) | `.agent/runs/20261024-074709-5c2b5f/logs/ci-steps-all/20261024T131303-f0122d0a.log` |
| **check-goldens-coverage** (H) | `ae4955ed` | **0** (supervised) | `.agent/runs/20261024-074709-5c2b5f/logs/check-goldens-coverage/20261024T131923-224763e4.log` |
| **check-goldens** (H) | `ae4955ed` | **0** (0.1 s, tree stamp trusted; supervised) | `.agent/runs/20261024-074709-5c2b5f/logs/check-goldens/20261024T131924-4e31040c.log` |
| **cdo-gate** (H) | `ae4955ed` | **0** (444 s, supervised; incl. `cdo_i264_closed_interface_site_census` ok, harness 204 passed) | `.agent/runs/20261024-074709-5c2b5f/logs/cdo-gate/20261024T131924-0841e7f4.log` |

Tree clean after the full set (`git status --porcelain -- . ':!.agent'` empty).

Draft PR #283 opened at `db03371b` (CI runs during the final panel).

Final panel round 1 (final_rounds 1 of 5): Opus reviewer + built-in `code-review` skill at high launched. sol (gpt-6.1-sol) failed: "Codex error: The usage limit has been reached" → per the operator's earlier choice for this outage ("Wait for the limit", during #250), retry every 30 min.
- Opus r1 (`final-r1-opus.md`): approve with fixes; two BLOCKING wrong closes reproduced end to end, both from `#if` alternatives: FO1 whole-procedure arms (closed_sites / snapshot read one row), FO2 return type (a) and field type (b) collapsed by dedup. Minors FO3 (T16/T33 test the helper, not the use), FO4 (CDO measured at 200d679c; cdo-gate on the attested head), FO5 (REPORT M2 wording: sites 3, 6–8 close via the VALUE arm).
- code-review skill at high (`final-r1-codereview.md`, 9 unverified candidates): FC1 = FO2a; FC2 resolver Complete vs model withheld for symbol-only implementers (comment wrong); FC3 `"X"::V` vs implicit-Rec field; FC4 callee/table file parse status; FC5 DRY `declared_type`; FC6/FC7 cost; FC8 snapshot `group_edges[0]`; FC9 the Task 5 to-less guard is harmful (to-less edge = implementer outside the model, open-world correct). Conductor dispositions: fix FO1–FO3, FC1–FC5, FC8, remove FC9's guard; defer FC6/FC7 (only interface sites pay; 318 on CDO). Fixes dispatched to the implementer.
- Round-1 fixes: `f781026e` (FO1/FC8/FC9), `061a2d1c` (FO2a=FC1/FO2b/FC4; build-level: disagreeing #if return types → none, table fields unioned), `c47c5af7` (FC3/FC5), `d3da6897` (FO3/FC2), `0b4519b4` (CHANGELOG). Suite 4140/4140, check-goldens 1126/1126, census 12/318, check-diff clean. Each new test red before its fix (proof rows in the implementer report).
- Final round 2 (final_rounds 2 of 5): Opus approve; re-ran every repro at 0b4519b4 (v4openfirst 0 → 2 d35 = control; v1, v2 closed_over → []; base still closed). Marked FO1–FO5, FC2, FC3, FC5, FC6 accepted. CDO `stats.json` at 0b4519b4 (cdo-r1) byte-identical to master. New minor NEW-1 (Report/XmlPort data-item fields reachable by bare name) → fix dispatched. sol still at its Codex limit.
- NEW-1 fixed `66fd6e14` (register FO6). Operator reset the Codex limit ("Try again, just reset"). Final round 3 (final_rounds 3 of 5): sol full review of `66fd6e14` (marks FO1–FO6, FC2, FC3, FC5, FC6 + new findings); Opus marks FO6.
- Round 3 results: Opus approve (FO6 accepted; asked for the NEW-1 proof row, recorded). sol rework (`final-r3-sol.md`, source-only review): accepted FO1, FO2, FO3, FO5, FO6, FC2, FC5, FC6; re-raised FO4 (no cdo-gate record yet — by design it closes_by the cdo-gate gate on the attested head) and FC3 (via N2-2/N2-3). New: N2-1 important (dedup keeps the first copy's source_table/table_no/extends_target → implicit Rec from the wrong #if page), N2-2 important (field guard treats a type-conflicted field as absent), N2-3 important (inherited protected variables missing from in_scope), N2-4 minor (census counts any-row closed, not every-row). Sent to the implementer to source-verify and fix.
- Round-3 fixes (all four confirmed): `d84a11ea` N2-1 (build-level: dedup clears conflicting source_table/table_no/extends_target), `b0d74381` N2-2, `4045d6ab` N2-3, `05f82b1c` N2-4 (census every-row; CDO 12/318, snapshot agrees). Implementer slip: a clippy-red commit went in through a chained shell, then amended (own unpushed commit) before the gate. ci-steps-task at `05f82b1c` exit 0 (`logs/ci-steps-task/20261024T121407-d22eac5d.log`), pushed.
- Final round 4 (final_rounds 4 of 5): Opus approve (all six accepted; new repros pctl/pif/ectl/eif/ppos/pifpos). sol accepted N22, N23, N24, FC3, FO4 (the closes_by rule) and RE-RAISED N21 with a real counterexample: a cleared `extends_target` removes a conflicted tableextension from every table's field lookup, so a page on A no longer sees its `"Demo Kind"` field and `"X"::V` closes on the closed enum object (a regression introduced by d84a11ea). Fix dispatched; round 5 is the last allowed.
- `ae4955ed` N25 (= sol r4 N21 counterexample): conflicted tableextensions kept in `untargeted_table_extensions`, counted by `table_scope_may_have_field` and field_enum's procedure check; caller-outside test, both orders, red before the fix. ci-steps-task exit 0 (`logs/ci-steps-task/20261024T122529-bbf8eb43.log`), pushed.
- Final round 5 (final_rounds 5 of 5): sol approve (N21, N25 accepted; stated limit accepted), Opus approve (re-ran sol's outside-caller shape: fa/fb/ra/rb open, fpos/rpos close). **Final panel converged**: 40 entries, all `fixed`/`deferred` (FC6 deferred, minor), all accepted by sol and opus; FO4 closes_by the cdo-gate record on H. Discoveries D1 (ABI enum closure) and D2 (conflicted-extension member resolution) recorded.
- Hold (alsem-second, #277 Task 1 binding timing, ~1–2 h): no CDO analyze/digest/cdo-gate and no `ci-steps all` until it signals done. Unit tests and small builds allowed. Lifted ~12:55 ("the box is free").
- PR #283 CI at `ae4955ed`: test, release, GitGuardian green.
- Step 11: origin/master unmoved since the pre-panel rebase → no rebase (range-diff not needed). **B = `1134bed19706b3e66f7676ad98510b1dad99d327`, H = `ae4955ed304e1d6de18549aac28b47f0f080bd1b`.** Full set started (ci-steps-all, check-goldens-coverage, check-goldens, cdo-gate).

## Discoveries

```json
[
  {
    "subsystem": "program::abi_ingest / engine::deps::symbol_reference (dependency enum facts)",
    "locator": "src/engine/deps/symbol_reference.rs:494-513 (RawObject has no Values), :1453-1467 (EnumTypes reader), :520-532 (raw_object_property first match, None for a valueless entry); src/program/abi_ingest.rs:515-540",
    "symptom": "An interface call whose value comes from a non-extensible enum declared in a dependency that ships no source (symbols only) stays open-world (interface-open-world; d35 unknown), because #264 closes only source enums.",
    "kind": "capability gap (precision)",
    "origin_issue": 264,
    "reproducer": "The CentralGauge #256 repro split into three apps so that ONLY the enum is symbol-only. App S ships source: interface \"Demo Strategy\" and codeunit \"Demo Impl\". App E depends on S and ships its .app without source: enum \"Demo Kind\" (Extensible = false, value Standard -> \"Demo Impl\"). The workspace depends on E and S and keeps the table, \"Demo Publisher\" (IsDue) and \"Demo Subscriber\". In the default cross-app analyze, \"Demo Impl\"'s body comes from S's source, so the edge has a target. Today alsem analyze still reports the d35 unknown finding with reason interface-open-world.",
    "pre_existing": true,
    "capability": "A symbol-only enum is closed when its symbol Extensible property is absent or \"0\"/\"false\" (every entry, no conflicts, no valueless entry) and every value's Implementation (comma list positional over ImplementedInterfaces), DefaultImplementation and UnknownValueImplementation resolve to loaded codeunits; the same loaded-set check as #264 then applies.",
    "acceptance": "On the three-app reproducer the NextKm edge is Complete with closed_over [Enum \"Demo Kind\"] and d35 gives no finding; \"1\", valueless, unknown text and conflicting duplicates stay open; a mapped implementation absent from the symbols stays open; a count mismatch between Implementation and ImplementedInterfaces stays open."
  },
  {
    "subsystem": "program::resolve::index (table extension lookup)",
    "locator": "src/program/build.rs dedup_objects (clears a conflicting extends_target, #264 d84a11ea); src/program/resolve/index.rs table_extensions_of / untargeted_table_extensions",
    "symptom": "A tableextension whose #if arms extend different tables loses its extends_target in the merge, so a call from code OUTSIDE the extension into one of its procedures (Rec.ExtProc() on either table) no longer resolves. Before #264 it resolved against the first arm's table only, which was wrong for the other build. Lost precision, never a wrong close.",
    "kind": "capability gap (precision)",
    "origin_issue": 264,
    "reproducer": "tableextension 50120 \"Kind Ext\" written as two #if/#else arms, one `extends \"Demo Item\"`, the other `extends \"Other Item\"`, each adding `procedure ExtProc()`. A codeunit with `DemoItem: Record \"Demo Item\"` calls `DemoItem.ExtProc();`. aldump --program-call-graph-stats counts that edge as unresolved instead of resolving it to Kind Ext.ExtProc.",
    "pre_existing": false,
    "capability": "Ordinary member resolution treats an extension whose #if arms extend different targets as extending each of them (the union of the arms' tables), recording the arm condition, instead of dropping it.",
    "acceptance": "On the reproducer the call resolves to Kind Ext.ExtProc from either table; a single-target extension is unchanged; the CDO histogram does not move; #264's closed-enum proofs stay unchanged (they already read untargeted_table_extensions)."
  }
]
```

## Not delivered

Every acceptance item A1–A8 is delivered as revised by the converged spec. Out of scope by decision (spec Non-goals, Discovery D1): enums known only from symbol files stay open. Stated limits: an unmapped implementer that commits still yields d35 YES (T14); a call from outside into a conflicted extension's procedure no longer resolves (Discovery D2); FC6 (per-site re-walk cost) deferred.
