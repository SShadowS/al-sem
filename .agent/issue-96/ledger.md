# Issue #96 ledger

## Claim

```json
{"run_id":"20261016-231908-77819d","issue":96,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/96-agent-discovery-l4-capability-c-a1","worktree":"U:\\Git\\al-sem-issue-96-a1","body_hash":"8d92a0de0c9f19f59456839f0df7c64bd533e386abb503f8bcb7746f5f80c5ce"}
```

Issue body hash: `8d92a0de0c9f19f59456839f0df7c64bd533e386abb503f8bcb7746f5f80c5ce`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**architectural.** The fix crosses several layers, so the spec file is the design: `docs/superpowers/specs/2026-10-16-issue-96-implicit-trigger-typed-edges-design.md`.
- **The typed-graph builder** gets a new edge kind.
- **The snapshot model** gets a new edge variant, so every exhaustive match changes.
- **Digest** gets a witness hop.
- **The L4 and cone PD temp-state binding**: the trigger `Rec` becomes receiver-bound. Without it, temporary records become physical writes.
- **Every cone-based detector** moves on real workspaces.

## Assumption probes

All probes ran on master `6b47f58d`. Details and commands are in `.agent/issue-96/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the issue reproduces | confirmed: `digest` gives `ValidateIt` `effects: []` and visits only 1 routine; `prove ValidateIt "writes-table:Log Row"` = **no** (unsound); `fingerprint` shows no `writes:` for ValidateIt. `query effects` DOES show `[Insert] on Log Row via implicit-trigger` | investigation §1 |
| removing the skip alone fixes it | **falsified**: trigger edges carry `callsite_id = op id` (`"{routine}/opN"`), never a `PCallSite` id, so the `call_site_by_id` lookup right after the skip drops them too | `program_calls.rs:1781-1792`, `combined_graph.rs:470-477, 527-529` |
| the skip guards against double counting | **falsified**: it is a byte-parity port of al-sem's `buildTypedEdges` (`da15e3e5`), with no stated reason; no other mechanism inlines trigger effects into the cone; platform trigger events have no publisher routine | investigation §4 |
| consumers tolerate a new kind | confirmed with gaps: the cone follows any edge with a `to` and already maps the via; snapshot `derive_typed_edges` silently drops unknown kinds (`_ => continue`); the digest `WitnessHop`/`HumanHop` have no trigger variant; several pieces already carry trigger support that is never used | investigation §2 |
| trigger `Rec` temp state is caller-bound | **falsified**: `Known(false)` by design (`ir_walk.rs:2083-2094`); measured `TempT.Insert(true)` → `OnInsert Rec.Modify()` gives an L4 physical `[Modify] … temp known(false)`. Joining the cone would spread this false positive | investigation §6 R2, probes `i96-ws3`/`ws4` |
| an existing fixture has Validate → OnValidate → write | no: `ws-implicit-trigger` reads (FindSet), `ws-member-call-codeunit` only `TestField`, `ws-triggers` empty. So A1-A3 use an inline tempdir workspace | investigation §6 |
| the argument-less `Modify()`/`Insert()` fire triggers (OUTSTANDING:126-136) | **falsified**: on master, `Modify()`, `Modify(false)`, `Insert()` and `Insert(false)` give NO trigger effect; `Modify(true)` and a non-literal `Insert(B)` do. The OUTSTANDING note is stale | probes `i96-ws5`/`ws6` |

## Base/Head SHAs

- B = `6b47f58daa91fdd20cbc0e25e14f1e520946ace0` (origin/master at claim)
- H = `b4c28b390992e072d3c6ab005812c64e7c6e7d67` (approved code head; the evidence commit follows it)

## Timeline

- Worktree created at master `6b47f58d`.
- Investigation (opus, read-only, subagents 1): `.agent/issue-96/investigation.md`.
- Classified as architectural; spec written.
- Spec panel (sol + opus, the operator roster). Register hash = the first 16 hex of sha256(spec file) at the round:
  - r1 (spec_rounds 1; pi_calls 2): both REJECTED revision 1. The critical findings were: digest returns first-hop-not-found for trigger facts (S1); the `PdEdgeKey` collides (S2); `TypedOutEdge` lacks the op (S3). Plus `Maybe` semantics, the golden prediction, A3 not pinning L4, no dependency case, and the cost plan. Revision 2 folded all 15 in.
  - r2 (pi_calls 4): both accepted S1-S15. Both raised N1 (A4's per-site paths cannot exist under the representative witness) and N2 (bulk-op `fires`); opus also raised N3-N5. Revision 2b fixed them.
  - r3 (pi_calls 6): opus accepted all 20 and raised R3-1 (the tie-break wording) and R3-2 (leftovers); sol re-raised N1 for the same tie-break. Fixed in revision 3.
  - r4 (spec_rounds 4; pi_calls 8): both accepted all 22; no new findings. CONVERGED.
- Plan: `docs/superpowers/plans/2026-10-16-issue-96-implicit-trigger-typed-edges.md`, tasks T0-T5 (plan_tasks 6).
- T0 (opus; task_attempts T0 = 1): `tests/cli/cli_implicit_trigger_cone.rs`, 8 tests. 6 are RED for the stated reasons (no typed trigger edge; prove = no; the L4 temp not bound). 2 pass by design: the fixture self-check, and the L4 precondition proving the red is the typed graph. A6 is delivered with a real source-bearing dependency (`symbol_app::write_source_app`). Report: `.agent/issue-96/t0.md`. Three findings went into the spec (revision 3a):
  - (1) the adapter's `can_fire_trigger` omits `modifyall`/`deleteall`; widened in scope (a model-layer gap, not a program-engine change);
  - (2) prove's `writes-table` is temp-blind, so A3's prove clause asserts the temp state of what prove reports; recorded as a discovery;
  - (3) A4's "one viaPath" vs digest's per-link PD proofs, which T3 decides, the spec changing first if needed.
- T1 (opus; task_attempts T1 = 1): PASS. Done:
  - `can_fire_trigger` gains `modifyall`/`deleteall`;
  - D1 typed edge (`operation_id`, `triggerKind`, `fires` always/maybe; serialized kebab-case);
  - Insert RunTrigger capture;
  - D3.1 table/tableext trigger `Rec` → PD(receiver);
  - D3.2 L4 op-receiver binding.

  Results: lib 2045 and clippy green; A2 green; the A3/A5 L4 asserts green; the cone and digest asserts are red until T2/T3.
  - **Interim:** the snapshot PD arena exempts `implicit-trigger` from its `debug_assert` (the miss is counted in `PD_EDGE_UNMAPPED`), so `r3_temp_class_parity_arena::pd_arena_links_are_snapshot_edges` is red until T3.
  - **Golden inventory** (in `t1.md`, not regenerated): r3a1 (3 files + manifest + the hand vector), r3a2 (2), r3a3 (4 + manifest), r1a (7 files, 42 lines), the ir-l2 snapshot (43 hashes: 12 D3.1 + 31 Insert-capture via `Debug`), and the l4-summary-baseline synthetic collision fixture (rewrite, not regen).
  - **Spec corrected (revision 3b):** N3 is wrong for the `Debug` snapshot; call-site bindings move; the synthetic fixture needs a rewrite.
- T2 (opus; task_attempts T2 = 1): PASS. Done:
  - `TypedOutEdge.operation_id` (sort key extended only for trigger edges, so non-trigger keys are byte-identical);
  - `PdEdgeKey` is a 6-tuple with the op;
  - `pd_solve` `BoundSite::Op`, reusing T1's L4 helper `trigger_receiver_temp_state` (DRY), with guards dropped explicitly;
  - `CapabilityFact.witness_first_hop_operation_id`, set in `retag` for every edge and serialized `skip_serializing_if` None.

  Results: lib 2048 and clippy green; A3 fully green; A5's cone part green; every `temp-state-incoherent` item gone. The remaining reds are snapshot/digest work (T3). Proofs: (c) breaks A3/A5 plus 2 unit tests; (d) breaks the new unit test until T3. Report: `.agent/issue-96/t2.md`.
- T3 (opus; task_attempts T3 = 1): PASS. All 8 acceptance tests green.
  - **Code:** the snapshot `ImplicitTrigger` edge; digest `WitnessHop`/`HumanHop`, op-keyed first hop (`is_first_hop`, which serves both Case C and `enumerate_proofs`), the explicit conditionality arm, and the memo identity. The plain PD-arena `debug_assert` is restored, byte-identical to master. The parity oracle twins by op, with A3/A4 probe shapes. lib 2048 and clippy green.
  - **Proofs:** (a), (e), (f), (d), (b) each fail and pass after revert; see the proof table.
  - **A4:** the code yields exactly one `viaPath` (at 76:8). A counterfactual where the first hop matches ANY trigger edge gives two (76:8 and 78:8), so the op selection is what gives the single representative, and break (f) protects it.
  - **Goldens:** T3 moved no new golden (no snapshot, digest, cli-b, r4 or r4f fixture has trigger edges). Report: `.agent/issue-96/t3.md`.
- T4a (opus): the synthetic L4 fixture `pd_substituted_via_collision{,_reversed}` was rewritten to the real trigger shape. A probe shows both edges produce the same `Insert|t2|b_op1|p0`, and the merge picks `implicit-trigger` over `inherited`. The r3a1 hand vector `edge_kind_enumeration` was updated (copied from the actual output). Report: `t4a.md`.
- T4 regen (receipt `.agent/golden-triage.md`, 21 MOVED; `agentflow run --name check-goldens-regen`, exit 101 because the two manifest matrices are hand oracles):
  - 19 files regenerated.
  - The r3a1/r3a3 `manifest.json` matrices were updated by hand to the Rust values (sonnet). Corpus: typed 207→212; r3a3 inherited 246→252, routinesWithInherited 139→143, via call 192→195 + implicit-trigger 3, read 210→216, static 759→765, `factsWithMoreThan1HopWitness` 246→252. Per-fixture blocks were recomputed from the goldens. The old ws-implicit-trigger block (direct 2, modify 2) was already stale: the HEAD golden held one direct read fact, and the block is unasserted and from the al-sem era. No fact was lost.
  - The line-by-line triage (golden-diff-triager, `golden-triage.md`) found **0 UNEXPLAINED** lines. ir-l2 has 44 changed routines (12 D3.1 + 32 Insert capture); the printed 43 undercounts because the drift report keys routines by name (a discovery).
  - `check-goldens` (no regen): green, 8/8.
- Commits: `43a3bf9f` (T0-T4 + spec + plan + CHANGELOG); `9ee65d1a` (the test helper takes a struct instead of `#[allow(clippy::too_many_arguments)]`, which `check-diff` forbids).
- T5 (opus): CDO measurement, the `release-fast` profile on master `6b47f58d` vs branch `9ee65d1a`, 2 runs each; report `t5.md`. DO's day-to-day checkout is at the same commit `bc3ccb18` as the CDO baseline, so it was not run separately.
  - `--program-call-graph-stats`: byte-identical (sha256 `c032c8db…94ef` both sides).
  - **Findings, cross-app:** `--scope all` +3800 new / −41 vanished; `--scope primary` +2 / −18. Every new or vanished finding was triaged: (a) a real trigger path or (b) a D3 temp change. **0 are (c).**
    - d43 at scope all goes 841 → 4573. That comes from d43's design: its "guarded tables" set is the caller's whole transitive write set, and one Base App site yields 2664 of them. At primary scope d43 goes 23 → 5, because CDO's setter now provably writes the tables.
  - **One (c) gap outside the new/vanished set:** d1's `path_temp_resolve::step_one_frame` does not bind the trigger receiver, so 3 d1 findings say "temp state uncertain" for a physical record. Fixed in T6.
  - **Cost, stated plainly (spec: report, not hide):**
    - `analyze` wall time **+98 %** (24.4 → 48.3 s, scope all) and **+113 %** (primary);
    - `digest` **+155 %** (32 → 82 s; it composes the cone twice);
    - `fingerprint` 0 %;
    - peak working set flat at about 2.83 GB.
    - Cause: the cone walk goes 4.5 → 28 s, max SCC 126 → 479, cone entries 5.5×, PD states 4.85×, PD links 5.05×, PD solve bytes 125 → 552 MB, `PD_EDGE_UNMAPPED` 0. A Base Application trigger hub drives it: Gen. Journal Line `OnInsert` → approvals → workflow `ExecuteResponse` → posting.
    - Every path is real may-execute behaviour. Making it cheaper is a separate optimisation (discovery 4), not a reason to drop sound edges.
- T6 (sonnet; task_attempts T6 = 1): `path_temp_resolve::step_one_frame` binds the trigger `Rec` through `trigger_receiver_temp_state`, with 2 path tests and a break. Commit `0a5b8b7e`.
- Reviews of `0a5b8b7e`, a Sonnet review and the `code-review` skill at high (the Claude Code built-in, run as given):
  - CR1/RS: production d1 (`d1_liveness`, `d1_temp::cross_hop`) is still unbound, and T6 pinned only the test-only resolver (CR2);
  - CR3: the caller's guard on the firing op is not conjoined;
  - CR4: `snapshot_full` lacks the field;
  - CR5: a linear op lookup;
  - CR6: `fires` has two sources;
  - CR7: extra passes;
  - CR8: the spec cites an untracked note;
  - RS1: OUTSTANDING is stale;
  - RS2: refuted.
- T7 (opus; task_attempts T7 = 1): PASS, all fixed; commit `c4b5da61`.
  - Production d1 crosses trigger hops through `d1_liveness::classify_edge_param`; `cross_hop` only seeds in-loop call sites. A d1 DETECTOR-level test pins it: breaking the liveness arm turns it red.
  - `TypedOutEdge::site()` (callsite, else op) serves the guard lookup in BOTH `pd_solve` and the non-PD cone path, with a proof each.
  - `fires` is computed once in the adapter; a short walk-back op lookup (not O(1); see CR5); one pass in `build_typed_edges`.
  - Gates: lib 2051, the acceptance tests 10, clippy clean, no golden moved, CDO stats unchanged.
- T5b re-measure on `c4b5da61` (sonnet, `t5b.md`):
  - findings vs master are identical to T5-after;
  - only 10 d1 findings moved since T5: the 3 "(temp state uncertain)" lose the suffix and rejoin master's cohort;
  - the guard moved no finding (the cone shrank 0.16 %); 0 defects.
  - Cost vs master (mean of 2): analyze all 24.5 → 49.8 s (+104 %), primary 23.0 → 47.0 s (+104 %), digest 32.3 → 80.2 s (+149 %), fingerprint noise, peak flat at about 2.84 GB. The CHANGELOG now carries these numbers.
- Final panel r1 (final_rounds 1; pi_calls 10):
  - sol accepted all entries and raised F1 (important): the cross-app digest snapshot is built without the injected dependency-origin edges, so a dependency-internal trigger reaches the cone but not the digest. Also F2 (minor): a stale test comment.
  - opus re-raised S8 (minor: no test sends a trigger edge through the `dep_calls` injection) and raised:
    - N-F1 (important): the ordering engine erases trigger hops, so a false happens-before is possible;
    - N-F2 (minor): d1 misses a trigger fired directly in a loop (pre-existing);
    - N-F3/N-F4: the ledger and CHANGELOG are stale, now updated.
  - Opus: the cost is acceptable to merge with discovery 4, given flat memory, the unsound `prove` "no" fixed, and real paths only.
- T8 (opus; task_attempts T8 = 1): PASS; commit `a71dba9c`.
  - **N-F1:** the ordering engine emits a call link for trigger hops, keyed by the op; an ordering test with a break pins it.
  - **F1:** root-caused as GENERAL and pre-existing. A plain dependency-internal call loses its write in the cross-app digest exactly like the trigger shape, so it is discovery 7, not fixed here.
  - **S8:** A6 extended with the dependency-origin trigger shape, asserted on the cross-app cone through `run_policy_check`. Breaking the trigger-edge injection fails it.
  - **F2:** comment fixed.
  - Gates: lib 2053, the trigger tests 12, clippy, `check-goldens` all green; no golden moved; CDO stats unchanged.
  - **Process note:** the new A6 test body was appended with a bash heredoc instead of Edit (reported by the agent). Its content was reviewed in the diff and is covered by the green run and the break.
- T5c (sonnet, `t5c.md`), on `a71dba9c`:
  - **analyze:** 0 findings moved; byte-identical to `c4b5da61` at both scopes.
  - **digest:** 18 ordering items moved, 0 defects.
    - 16 are factId-only (the trigger-first occurrence ids now carry the op).
    - 2 gained `EXTERNAL_IO_IN_EVENT_SUBSCRIBER_TXN`, through `CreateLogEntries` `DeleteAll` → `Insert(true)` → `OnInsert` → `IsolatedStorage.Contains`. **Correction (final r3, opus):** that label means a write and an IO joined by an EVENT hop, and this path has none; it is trigger-only. T9 stopped trigger links from setting the event reason, and T10 confirmed both items are GONE on `6332e578`.
  - **Wall time:** unchanged vs `c4b5da61` (analyze 46.4/45.8 s, digest 78.3/77.1 s).
- **Cost decision:** both final-panel reviewers, round 1, say the regression may be waived with discovery 4 filed. Reasons: memory is flat, it closes the unsound `prove` "no", and every new path is real. Sol: "reported regression can be waived; final verification cannot."
- Final panel r2 (final_rounds 2; pi_calls 12):
  - opus accepted all 38 and raised N-F5 (minor: a trigger-only ordering path got `event-subscriber-may`);
  - sol re-raised CR5 (the doc claimed an O(1) lookup that is really a short walk-back) and N-F3 (empty ledger sections).
- T9 (sonnet; task_attempts T9 = 1): `inter_hb` separates trigger from event broadcast. Both are May, but `implicit-trigger-may` is distinct from `event-subscriber-may`. Twin tests and a break. The CR5 doc was corrected. Commit `6332e578`.
- Final panel r3 (final_rounds 3; pi_calls 14): both accepted all entries except N-F3, which both re-raised (the freeze sections are filled now). Opus also asked to re-verify that the 2 trigger-only ordering items vanish after T9 (minor).
- Final panel r4 (final_rounds 4; pi_calls 16): both accepted all 40; sol found nothing new. Opus raised N-F7 (minor: the ledger summary said 39 entries, and the T7 bullet still said O(1)). Fixed.
- Final panel r5 (final_rounds 5; pi_calls 18): both accepted all 41; no new findings. CONVERGED. `origin/master` = `6b47f58d` = the branch base, so no rebase is needed.
- Gates on `6332e578`: ci-steps-all 0, coverage 0, check-goldens 0, **cdo-gate 1**. Two frozen CDO L4 digests diverged: `cdo_whole_program_v2_matches_frozen_digest` and `cdo_reverse_index_matches_slow_oracle`.
- T10 (opus, `t10.md`): the canonical texts behind both digests, master vs branch, triaged line by line.
  - Whole-program: 82 D3.1 trigger-`Rec` temp lines (3 of them key splits), 28 D3.2 caller bindings, 11 bulk-trigger effects, 0 guard, **0 OTHER**.
  - Reverse index: 4 added rows, all bulk-trigger, **0 OTHER**.
  - The 2 ordering items are gone; no other digest fact moved since T5c.
  - Process trap found and handled: a shared target dir silently reused the master-built test binary once. That run was discarded and redone from branch sources.
- Re-minted both digests with `CDO_WS` + `ENFORCE_CDO_WS=1` + `REGEN_TEMP_GOLDENS=1` (receipt `.agent/golden-triage.md`). New values `eb68cefc…` and `ca9a0a94…` match T10's expectation exactly. Commit `b4c28b39`.

## Acceptance matrix

See the spec's `## Acceptance matrix`: A1 (digest effect + `viaPath` through the trigger, both modes), A2 (cone physical write set + prove), A3 (a temporary receiver gives no physical write), plus golden/CDO/detector measurement.

## Discrimination proofs

Every break was made with Edit, run (it failed on the stated line), reverted with Edit, the file `touch`ed, and re-run green. The details and logs are in the named task report.

| # | test(s) | break | decisive failure | report |
|---|---|---|---|---|
| (a) | A1, A2, A4, A6 | reinstate the `build_typed_edges` skip | `combined graph: MixedValidate implicit-trigger typed edges by op {}, want 3 ops`; `prove ValidateIt writes-table:Log Row = "no", want yes` | t3.md |
| (b) | A3, A5 | remove the L4 op-receiver binding | `L4 V: … want temp parameter-dependent(0), got … unknown` | t3.md |
| (c) | A3, A5 + 2 unit | remove the cone `bound_site` arm for `implicit-trigger` | A3/A5 cone physical writes for temporary receivers | t2.md |
| (d) | A4 + unit | drop `operation_id` from `TypedOutEdge`'s key | the trigger edges collapse; A4 fails | t2.md, t3.md |
| (e) | A1-A5 | drop the op from `PdEdgeKey` on both sides | `pd arena: snapshot edges Some(6) and 7 share the key ("implicit-trigger", …)` | t3.md |
| (f) | A1, A4, A5, A6 | `is_first_hop` ignores `witness_first_hop_operation_id` | `ValidateIt: DB_INSERT … has no viaPath whose first hop is implicit-trigger … paths=[]` | t3.md |
| D1 | D1-style path tests | T6: path resolver trigger arm | `left: Unknown / right: Known(false)` | t6.md |
| CR1 | d1 detector test | T7: remove the `d1_liveness` trigger arm | the d1 finding reports "(temp state uncertain)" | t7.md |
| CR3 | guard test (×2 paths) | T7: guard lookup by callsite only, in `pd_solve` and in the non-PD cone path | the Skip guard is absent; the caller passing `Skip=true` inherits the write | t7.md |
| N-F1 | ordering unit + e2e | T8: no trigger link in `reconstruct_call_chains` | a `write ≺ http` edge appears | t8.md |
| S8 | A6 dependency-origin | T8: the `dep_calls` injection skips trigger edges | `cross-app: dep-log-insert findings at lines [2, 15], want [2, 9, 15]` | t8.md |
| N-F5 | ordering twin tests | T9: a trigger link sets the event flag | the trigger test fails (16 passed, 1 failed) | t9.md |
| fires | `Insert(true)` vs `Insert(B)`/`ModifyAll` | (covered by A1's conditionality literals) | — | t0.md, t1.md |

## Findings register summary

41 entries (`findings.json`):
- spec panel: S1-S15, N1-N5, R3-1, R3-2;
- code review: CR1-CR8;
- Sonnet review: RS1, RS2;
- final panel: F1-sol, F2-sol, N-F1 … N-F7.

Dispositions:
- **fixed:** all others;
- **refuted:**
  - RS2: the sort key is deterministic without the field;
  - F1-sol: not a #96 defect, because the cross-app digest misses EVERY dependency-internal edge (pre-existing, discovery 7);
- **deferred (minor):** N-F2 (d1 in-loop trigger, pre-existing, discovery 6).

No blocking entry is deferred.

## Gate results

All 4 gates ran under the supervisor on head `b4c28b390992e072d3c6ab005812c64e7c6e7d67` and returned exit 0. The worktree's `git status --porcelain -- . ':!.agent'` was empty afterwards. Logs are under `.agent/runs/20261016-231908-77819d/logs/`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 421.1 | `ci-steps-all/20261017T045234-b6bfcf7a.log` |
| check-goldens-coverage | 0 | 3.0 | `check-goldens-coverage/20261017T045935-4e6fad8d.log` |
| check-goldens | 0 | 114.3 | `check-goldens/20261017T045938-383a8c4e.log` |
| cdo-gate | 0 | 105.3 | `cdo-gate/20261017T050133-bfbdfeb5.log` |

Earlier gate runs were stopped when review fixes changed the code, or failed only on the frozen CDO digests before their triaged re-mint (`6332e578`, cdo-gate 1). Those results are not used.

## Discoveries

Filed (the 5-discovery cap): items 1, 4, 5, 6 and 7. Noted, NOT filed because of the cap, with full detail kept below for a later sweep: item 2 (R3a-4 artifacts) and item 3 (the ir-l2 drift report count).

6. **d1 misses an implicit trigger fired directly inside a loop.**
   - Subsystem `l5 / d1`; locator `src/engine/l5/d1_graph.rs` (the `loop_depth` lookup through `call_site_by_id`).
   - Symptom: a loop that does `T.Validate(...)`, or `T.Insert(true)`, whose trigger performs a DB operation is not reported. The firing record op is not a d1 seed, and an implicit-trigger edge's `loop_depth` reads 0 because the edge carries an op id, not a call-site id.
   - Capability: d1 treats a trigger-firing record op inside a loop as an in-loop call into the trigger.
   - Acceptance: a fixture with `repeat T.Validate(Amount, 1) until …` whose `OnValidate` does `L.Insert()` yields a d1 finding through the trigger.
   - Pre-existing; #96 made it visible (opus final r1, N-F2). Kind: bug.
7. **The cross-app digest snapshot drops every dependency-internal edge.**
   - Subsystem `l5 / digest (cross-app snapshot)`; locator `src/engine/l4/capability_cone.rs:3427-3474` (`build_cross_app_base` keeps only workspace-origin calls in `resolved.calls`; dependency-origin calls go to injected typed edges), `src/engine/l5/digest_cli.rs:1572-1580`, `src/engine/l5/snapshot.rs:1141-1147` (`compose_snapshot` rebuilds from `resolved.calls` only).
   - Symptom: workspace `W` calls dependency `P`, which calls dependency `Q` or fires a dependency trigger, and `Q` writes table T. The cross-app cone of `W` (prove, fingerprint, detectors) has the write; the cross-app DIGEST of `W` shows no effect. Probe from #96 T8: `InsertViaDep → Dep Ops.DoInsert → InsertLog` gives an empty effect list.
   - Capability: the cross-app digest snapshot is composed from the same edge set as the cross-app cone, including the injected dependency-origin edges, with coherent L4 temp evidence.
   - Acceptance: that shape's cross-app digest reports the `DB_INSERT` with a `viaPath` through `P`/`Q`, agreeing with the cone.
   - Pre-existing (sol final r1 F1). Kind: bug.

1. **prove's `writes-table` predicate ignores temp state.**
   - Subsystem `l5 / prove`; locator `src/engine/l5/prove.rs:388-393`.
   - Symptom: `prove <routine> "writes-table:X"` answers yes for a routine whose only write to X is on a temporary record, because the predicate matches any `DB_*` write effect by resource name. After #96, a `TempT.Validate` whose `OnValidate` does `Rec.Modify()` makes prove say yes, while the cone's physical write set does not contain X.
   - Capability: `writes-table` (or a physical variant of it) answers about physical writes, consistent with `ConeDerivedStore::writes_physical_tables_of`.
   - Acceptance: a test where the only write is temporary gives prove = no for the physical predicate.
   - Pre-existing (master). Kind: bug.
3. **The ir-l2 snapshot drift report undercounts changed routines when overloads share a name.**
   - Subsystem `tests / l2_ir snapshot`; locator `tests/l2_ir/ir_l2_snapshot.rs` `drift_report`.
   - Symptom: the report keys routines by name in a map, so overloads collapse to one entry. In #96, 44 routines changed, but the report said 43; it missed the changed first overload of `ws-overload-callexpr-discriminator` `Target.P`. The test still fails correctly; only the printed count and list are wrong.
   - Capability: the drift report keys routines by their full identity (name + overload index or signature).
   - Acceptance: a snapshot drift on one overload of a same-named pair is listed by itself.
   - Pre-existing. Kind: bug.
4. **The capability cone and digest pay about 2× for implicit-trigger hubs on cross-app CDO.**
   - Subsystem `l4 capability cone / pd_solve`; locator `src/engine/l4/capability_cone.rs` (cone walk), `src/engine/l4/pd_solve.rs`.
   - Symptom: after #96, `alsem analyze` on CDO takes 24 → 48 s, and a digest 32 → 82 s; peak memory is flat. Measured: max SCC 126 → 479, cone entries 5.5×, PD states 4.85×, PD links 5.05×. A Base Application hub (Gen. Journal Line `OnInsert` → approvals → workflow `ExecuteResponse` → posting) joins large SCCs.
   - Capability: the cone composes trigger-heavy SCCs without a linear blow-up, e.g. by sharing SCC summaries or bounding PD state fan-out by an equivalence class. Digest composes the cone once per run, not twice.
   - Acceptance: CDO `analyze` back within 10 % of the pre-#96 time with byte-identical findings, and digest at most 1.1× analyze's cone cost.
   - Reproducer: `tools/census-probe`-style timing or `ALSEM_CONES_CENSUS=1 alsem analyze <CDO baseline> --scope all`, on master after #96.
   - Kind: enhancement.
5. **d43 reports the caller's whole transitive write set as "guarded tables".**
   - Subsystem `l5 / d43`; locator `src/engine/l5/detectors/d43.rs:180-187`.
   - Symptom: on CDO, d43 (`IsHandled` skip) at `--scope all` gives 4573 findings after #96 (841 before). Each caller reaching a trigger hub gets about 300 per-table findings per setter, and 2664 come from ONE Base Application site. They are real paths but not actionable: the set is the caller's transitive writes, not the writes the `IsHandled` guard actually skips.
   - Capability: d43's table set is the writes inside the guarded (skipped) region of the publisher, not the caller's whole cone.
   - Acceptance: on CDO, d43 findings per setter are bounded by the publisher's guarded writes. A fixture where the publisher's guarded body writes A while the caller also writes B elsewhere reports A only.
   - Pre-existing (the design predates #96; #96 made it visible). Kind: bug.
2. **R3a-4 dependency symbol artifacts drop implicit-trigger edges** (`src/engine/deps/dep_artifact_l4.rs:473-477` admits only Direct/Method/Interface). A sibling of #96 on the artifact path; spec non-goal. Kind: enhancement.

## Not delivered

- **Cross-app DIGEST of a dependency-INTERNAL trigger path.** When a dependency routine itself fires a dependency trigger, the cross-app digest does not show the write. The cone, prove, fingerprint and the detectors do, and A6 pins that. This is not a #96 gap: the cross-app digest snapshot drops every dependency-internal edge, of any kind (discovery 7). The issue's own acceptance, a workspace routine calling `Validate` and digest/cone/test, is fully met, including a workspace caller firing a dependency table's trigger (A6, original shape).
- **Cost is not recovered.** CDO `analyze` takes about 2× as long and digest about 2.5×, with peak memory flat (discovery 4). This is reported, as the spec requires, not hidden.
