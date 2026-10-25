# Issue #246: digest: opt-in table reads (--include-reads)

Claim: run `20261025-130917-08aebb`, attempt 1, branch `issue/246-digest-opt-in-table-reads-inclu-a1`, worktree `/work/al-sem-issue-246-a1`, base `d2d846690116d040a5cf41a3a0c365436943625b`, body_hash `2b705f187e4ba88d242101bacad67d20a8cd967831e1e15da24582b960c24995`.

Reviewers: gpt-6.1-sol (slot astra) and Opus (slot flash). Attest with `--substitute astra=sol --substitute flash=opus`. heavy_gates=2. Merges pre-approved. Operator goal: merge before the evening of 2026-10-26, so it can go out in v2.1.1 for the CentralGauge demo on 2026-10-27.

## Timeline
- 2026-10-25 13:10 UTC: claimed on operator order, relayed by alsem-main. #160 is done, so the hold on digest is lifted. Acceptance 1 (default output byte-identical) is the guard.

## Classification
Bounded. One opt-in flag on one command, `digest`. It adds a new effect kind, `DB_READ`, which reuses the existing coverage, witness and dependency-boundary machinery, so no new engine layer is needed. The risk is concentrated in two places:
- acceptance 1, byte-identical default output;
- acceptance 4, a CDO cost measurement.

The design goes in this ledger under `## Design`. It still gets the spec panel.

## Probes

All read-only, on master `d2d84669`. No CDO run. "Fact" below means a capability fact: one row saying "routine R does op X on resource Y".

| probe | result | evidence |
|---|---|---|
| P1. Table reads are produced as facts at the source | TRUE. `Get`/`Find*`/`IsEmpty`/`Count*`/`Next`/`CalcFields`/`CalcSums` map to op `read`, kind `table`, one direct fact per record operation, witness = the operation id. `LockTable` is op `lock`, not `read`. | `src/program/body/capability/table_commit.rs:24-50` (`map_op`), `:53-85` (`extract_table`); the L4 cone uses the same map (`capability_cone.rs:609`) |
| P2. The cone propagates reads to callers (inherited facts) | TRUE. The cone has no op filter. Its inherited-fact dedup key includes `op`, and its own doc measures reads as the larger class on CDO (1,463 read keys vs 292 write keys at the #33 collision). | `capability_cone.rs:1282-1300`; `fingerprint` goldens show inherited read witnesses, e.g. `tests/cli-b-goldens/fingerprint/ws-txn-d46-pos.human.txt` ("read table … DoSetup (via DoSetup …) → direct Setup.Get") |
| P3. **Crux**: reads reach the tree digest walks | TRUE. Digest's tree is built by the same `compose_tree` as `fingerprint`'s. `derive_capability_facts` copies every direct and raw fact with no op filter. `digest_one_root` reads `idx.facts_by_routine`, which holds reads today. The ONLY place reads drop is `digest_one_root`'s filter, `effect_type_of(f).is_some()` → `map_op("read") == None`. | `snapshot.rs:1709-1725` (`snapshot_fact`), `:1744-1790`; `session.rs:916-937` (one `compose_tree`, then `TreeRead::Effects` → `digest_effects`); `digest.rs:3773-3782`, `:3845-3848` |
| P4. The digest tree already pays for reads today | TRUE. Reads are composed, indexed and stored in the tree whether or not digest uses them. Flag-on adds only the per-root witness and effect work for read facts. | P3; `compose_capabilities` (`detector_context.rs:1350`) has no op filter |
| P5. Reads follow the same coverage rules as writes | TRUE. Entry coverage and the unresolved cone are per root and op-blind. The effect loop (dedup identity, `dedupe_key`, rule-B grouping, the `kt` key class for `DB_*`) never branches on op beyond `effect_type_of`. A `DB_READ` name gets the `DB_*` temp-class key for free (`starts_with("DB_")`). | `digest.rs:3997-4004`, `:4143-4149`; `digest_cli.rs:841-881` |
| P6. A dependency read stops at the boundary (rule B, #160 step 6 option A) | TRUE by construction. The boundary marking is built from edges. Carriers match by `fact_equivalent`, which compares op only for equality. `boundary_terminal` and the group key `B|{type}|{kt/nt}|{boundary}` are op-blind. A read behind a dependency call becomes a `DB_READ` group with `detail.boundary`, `resourceCount`, `resources`, exactly like `t1`'s `DB_MODIFY`. Demand (`program::resolve::demand`) is call-reachability only, so it does not prune read-only dependency code. | `digest.rs:834-866`, `:1535`, `:3997-4004`; `tests/cli/cli_digest_query_dependencies.rs:611-662` (`t1`); `tests/common/dep_dispatch_fixture.rs:153-161` (`write_own_modify`, the template for a `Cust.Get()` variant) |
| P7. Cost drivers grow with the read count | TRUE. Per changed root, each distinct fact identity runs `reconstruct_witness_paths` (BFS, plus `enumerate_proofs` for parameter-dependent temp facts). The ordering engine then runs over every effect of the root. Reads are about 5x writes in cone keys (P2). On DO, 34.6% of read memberships are known-temp (`effect_query.rs:20`). Cost is bounded: digest computes only its changed roots (#66), and witness depth and path caps still apply. | `digest.rs:3887-3907`, `:4230-4290`; `effect_query.rs:18-24` |
| P8. `detail.tempState` can be joined for reads | TRUE. The L4 db-effect store carries read rows (the same doc table counts 77,009 temp reads), and `DbTempStates::for_effect` is op-blind. Today the CLI join runs only for `DB_INSERT\|DB_MODIFY\|DB_DELETE`. **Corrected after Task 2 (CDO):** this holds for the store; the store HAS read rows (`query effects` on `IssueFinanceChargeMemo`: 307 ops, mostly reads). But it does not mean every read joins. On `d`, 1,059 of 1,133 reads get `no-l4-row`, because the cone reaches ops the root's L4 summary does not hold (529 ops, 524 tables vs L4's 52). Writes fail the same way in the DEFAULT output (772 on `d`). It is a pre-existing cone/L4 reach gap, not a read-specific one. See Task 2. | `digest_cli.rs:1080-1098`; `effect_query.rs:20`; Task 2 |
| P9. Ordering engine reaction to a new effect type | **FALSE as first written** (spec r1, S2/O3). `DB_READ` is in none of the classes (`is_external_io`, `is_db_write`, `is_ui_window_sink`, `COMMIT`), but generic labels attach to ANY occurrence. `ORDERING_UNKNOWN` attaches when the occurrence has no order index or owner, and `COMMIT_REACHABLE` attaches when its routine group has a commit. So a graded read WOULD carry labels. The other effects' labels filter by class, so they do not depend on a read being present. `scoped` is indexed by position (`scoped.get(i)`). | `ordering_engine.rs:77-87`, `:489-525`; `digest.rs:4238-4290` |
| P10. `map_op` is shared beyond digest | TRUE. `ordering_kept_roots` (`digest.rs:4686`) and the ordering-only digest path use it. Changing `map_op` itself would move ordering, prove and the r4f goldens. The flag must act in `digest_one_root` only, not in `map_op`. | `digest.rs:4686`, `:4575`, `:4598`, `:4735` |
| P11. Plumbing precedent exists | TRUE. `--tests` (#254) runs exactly the path a new flag needs: `DigestCli.tests` → `DigestRequest.tests` → `digest_plan(.., tests)` → `TreeDemand.tests` → `digest_effects(.., demand.tests, ..)`. `batch` re-parses `DigestCli`, so a clap flag reaches batch too. `prove_plan` calls `digest_plan(.., false)`. | `src/bin/alsem.rs:719-721`, `:1713-1722`; `session.rs:100-102`, `:336-365`, `:932-937`; `digest_cli.rs:1755-1767`, `:1868-1872` |
| P12. Goldens covering digest; default byte-identical | Digest goldens: `tests/cli-b-goldens/digest/` (20 fixtures × json+human + 1 diff), via `--test cli cli_b_digest_differential::`. Also touched by the same code: `tests/cli-b-goldens/prove/`, `tests/r4f-goldens/` (`project_r4f_digest_effects`, `compute_digest_effects`), the rule-B tests (`digest_rule_b_tests.rs`, `cli_digest_query_dependencies.rs`) and `without_dependencies_cross_app_equals_single_app`. With the flag off, the filter is unchanged, so none can move. `schemaVersion` stays `1.6.0` (the `--tests` precedent: opt-in and additive, no bump). | `tests/cli/cli_b_digest_differential.rs:1-40`; `digest_cli.rs:1545-1553` |
| P13. "One `DB_READ` per table" holds per call site, not per table | NUANCE (not false). `dedupe_key` includes the terminal anchor (operation id). `Customer.Get` then `Customer.FindSet` in one routine gives TWO `DB_READ` effects on Customer, the same as two `Modify` sites give two `DB_MODIFY` today. Acceptance 2 holds as worded (one `Get` gives one effect; two tables give two), but a consumer that wants "tables read" must dedupe by `detail.resourceId`. | `digest.rs:3010-3045` |

No acceptance assumption came out false. P9's "no label" claim was false (spec r1). The ordering exclusion (Design, item 4) still stands, and test T4 now pins it. P13 is recorded as a candidate discovery.

## Design

**Shape.** `--include-reads` is a digest-only clap flag. When set, `digest_one_root` treats a fact with `op == "read"` and `resource_kind == "table"` as a `DB_READ` effect. The rest of the effect loop runs unchanged. Reads are already in the tree digest walks (P3, P4). So nothing upstream changes: no cone change, no tree change, no new data. `map_op` stays as it is, so ordering, prove and r4f cannot move (P10).

**Plumbing** (the `--tests` path, P11):
1. `DigestCli.include_reads` (`#[arg(long = "include-reads")]`, default false) → `DigestRequest.include_reads` → `digest_plan(.., tests, include_reads)` → `TreeDemand.include_reads` → `digest_effects(.., include_reads)` → `compute_digest_effects_cli(.., include_reads)` → `digest_query` / `digest_query_with` → `digest_one_root`. `prove_plan` and every other caller pass `false`. Existing `TreeDemand` literals in the tests get `include_reads: false`.
2. In `digest_one_root`, one local, `effect_type = |f| effect_type_of(f).or_else(|| (include_reads && f.op == "read" && f.resource_kind == "table").then_some("DB_READ"))`, used at both sites that call `effect_type_of` today (the filter and the loop).
3. `detail` comes from `effect_detail_of` unchanged: `resourceId`, plus `resourceDisplay` when the table resolves to a name. The CLI temp-state join (`digest_cli.rs:1080`) adds `"DB_READ"` to its match, so reads carry `detail.tempState` like writes do. This arm is unreachable with the flag off.
4. **Ordering:** with the flag on, `DB_READ` effects are left out of `ordering_inputs`, and each remaining effect's `scoped` result is mapped back to its own position. With the flag off, the existing code runs unchanged, with no filter and no remap vector. `DB_READ` is therefore **not ordering-graded**: its `guarantees` and `scopedGuarantees` are always `[]`. Here `[]` means "not graded". It does NOT mean "graded, and no label applies". This is unlike the other unclassified types (`TELEMETRY`, `EVENT_PUBLISH`, `UI_ERROR`, …), which stay in ordering and can carry `ORDERING_UNKNOWN` or `COMMIT_REACHABLE` (P9). The `--include-reads` help text and the CHANGELOG say this. Why: no existing guarantee needs a read as a participant (the label sets are writes, commits, IO and UI), and the per-effect chain work in ordering would grow about 5x (P7).
5. **Witness and boundary:** no new rule. A direct read is witnessed at its operation. An inherited read uses the same BFS/DAG witness. A read behind a dependency call becomes a rule-B group (`DB_READ`, `detail.boundary` = the first dependency routine, `resourceCount`, `resources`, `conditionality: unknown`, via = the primary path), exactly as writes do (P6).
6. **Flag off = byte-identical:** the new branch sits behind `include_reads &&`, which is evaluated first. Flag-off adds one bool test per fact and nothing else (no remap, no extra key). `schemaVersion` stays `1.6.0`. The CHANGELOG notes "Still 1.6.0: `DB_READ` and `payload.includeReads` are opt-in".
7. **Echo (OQ2, decided):** with the flag set, the document gets `payload.includeReads: true`. The key is absent when the flag is off. A stored document can then tell "no reads" from "not asked". This follows the `--tests` precedent, where the block's presence is the marker.

**Documented limits** (help text + CHANGELOG; this is not an exhaustive "tables read" inventory):
- One `DB_READ` per read SITE, not per table (P13). Physical and known-temp reads of the same table are separate effects, because the `kt`/`nt` class is part of the key.
- A read on an unresolved table has no `detail.resourceId` (`resource_display_of`, `digest.rs:85-104`).
- A rule-B boundary group has no `resourceId` and no `tempState`. It carries `resourceCount` (complete) and `resources`, which lists at most 10 names (`digest.rs:4120-4140`; the group's `origin_operation_ids` is empty, so the temp join renders nothing, `digest_cli.rs:693-703`).
- `LockTable` is op `lock`, not a read (`table_commit.rs:38`), although L4 counts it as a read verb.
- `CalcFields`/`CalcSums` read the record's own table only. FlowField source tables are not modeled.
- `DB_READ` is not ordering-graded (item 4).

**`readTables[]` vs full effects (decided):** full `DB_READ` effects only. They reuse all the machinery and carry the witness acceptance 3 requires. A consumer derives "tables read" from `resourceId` plus the group `resources`, within the limits above.

**Acceptance → proof** (every test drives the production path, the `alsem` binary or `run_digest_pipeline`; each break and revert is done with Edit and recorded):
- **A1, T1 (default byte-identical):** the existing `cli_b_digest_differential`, prove and r4f goldens stay green with NO regen. This is the default-output oracle.
- **A1, T2 `include_reads_adds_only_db_read`:** over `DIGEST_CORPUS`, run both modes and parse both documents as JSON. Normalize the flag-on document in exactly these steps:
  1. Remove every effect whose `type == "DB_READ"`.
  2. Remove every `entryDiagnostics` item whose `effectType == "DB_READ"`. Those are `evidence-unavailable` and `temp-state-incoherent`, the only two diagnostic sources, and both are tagged with `effectType` (`digest_cli.rs:1045-1053`, `:1092-1097`).
  3. Subtract the number of removed effects from `payload.summary.totalEffects`. This is the only count derived from effects: `rootsRequested`, `rootsResolved` and `totalUnresolved` do not read effects (`digest_cli.rs:1473-1483`).
  4. Remove `payload.includeReads`.

  Then assert the result equals the flag-off document. **Precondition asserts:** the corpus yields more than 0 `DB_READ` in total, and at least 1 each in `ws-d1-multi-caller`, `ws-txn-d46-pos` and `ws-d8-commit-in-tx` (all three have `Get`/`FindSet` in source; e.g. `ws-d8-commit-in-tx/src/posting.al:8` `Header.Get`). The label-level pin of item 4 is T4, not T2.

  Break: default the flag to true in `digest_one_root`, so default output carries reads → T1 fails. Break: the remap uses `scoped.get(i)` over all effects (an off-by-position remap) → T2 fails only if a corpus fixture has a read that sorts before a labelled effect. The implementer runs this break and records the fixture it failed on. If no fixture fails, the proof records that T4 alone pins the remap (O10).
- **A2, T3:** a temp-workspace CLI test. It declares tables `Customer` and `Vendor` in the workspace, so both resolve (O6), and has these routines:
  - `OneRead`: `Customer.Get`.
  - `TwoReads`: `Customer.Get` + `Vendor.FindFirst`.
  - `TempRead`: `TempCust: Record Customer temporary; TempCust.FindFirst()`.
  - `Caller` → `Helper`, where `Helper` does `Customer.Get`.

  With the flag, assert:
  - `OneRead` has exactly 1 `DB_READ`, and `TwoReads` has exactly 2, each with the expected `resourceId` and `resourceDisplay`.
  - The physical read has `detail.tempState == "known(false)"` and `TempRead` has `"known(true)"`. This pins the tempState arm (S3).
  - `Caller`'s `DB_READ` has `provenance: transitive`, a via path `Caller>Helper`, and evidence at the `Get` line in `Helper`. This pins the primary inherited witness (S3).
  - Without the flag, there are zero `DB_READ` and no `includeReads` key.

  Breaks, one at a time: drop the read arm → the counts fail. Drop `"DB_READ"` from the temp-join match → the tempState asserts fail. Make the inherited witness projection empty for reads, e.g. `paths.clear()` when `effect_type == "DB_READ"` and the fact is inherited → the `Caller` asserts fail.
- **A2/ordering, T4 (pins item 4):** in T3's workspace, `ReadCommit` does `Customer.Get; Customer.Modify; Message('x'); Commit()`. With the flag, assert:
  - the `DB_READ` has `guarantees == []` and `scopedGuarantees == []`;
  - the `DB_MODIFY`, `COMMIT` and `UI_MESSAGE` effects carry the same `guarantees` and `scopedGuarantees` as in the flag-off run.

  `DB_READ` sorts before `UI_MESSAGE` (sort is by type first, `digest.rs:4203-4212`), so a shifted remap moves a label. Two separate breaks: (a) put `DB_READ` back into `ordering_inputs` → the read gets `COMMIT_REACHABLE` and T4 fails; (b) the off-by-position remap → `UI_MESSAGE`'s labels move and T4 fails.
- **A3, T5:** a cross-app CLI test on `dep_dispatch_fixture` with `Inner` doing `Cust.Get()` (a `write_own_modify`-style variant). `digest --include-reads --routine CallChain` must give exactly one `DB_READ` group with boundary `XBase.Outer`, `resourceCount` 1, resources `Customer`, an empty via path (the root's own call), and no hop inside the dependency. Breaks:
  - read-specific (O5): restrict the read arm to `provenance == "direct"` → the inherited dependency read vanishes and T5 fails;
  - rule B: as `t1` does, compose over primary callers only → the group is lost.
- **A4, M1 (task 2):** see the measurement plan below.
- **A5:** T2–T5 all spawn `CARGO_BIN_EXE_alsem` or call `run_digest_pipeline`, and each has its recorded discrimination proof.

**Measurement plan (A4, OQ3 decided).** This is #277's method, so the numbers compare directly with the admission rows (`docs/measurements/2026-10-25-issue-277-160-step12.md:3-12`, `:76-92`).
- **Binary and workspace:** one `release-fast` binary of the branch head; the pinned CDO baseline (git `bc3ccb18`, clean); peaks are kernel `VmHWM` via `/work/bench/160-10-11a/maxrss.py`.
- **Argv, verbatim:**
  - `d` = `digest <CDO_WS> --file $F --format json`, with `$F` = `src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al`;
  - `h` = `digest <CDO_WS> --file $F88 --format json`, with `$F88` = `src/Features/HandledState/CDOUnhandledSalesPagesMgt.Codeunit.al` (88 roots);
  - `ds` = `d` with `--single-app`;
  - `Up-r` = #277's `Up` batch manifest `[a, d, h, ev, qt]`, with `--include-reads` on `d` and `h`. Up was the shape that set the `("digest", X)` row; this is S4's batch shape.
- **Runs:** each of `d`, `h`, `ds` with and without `--include-reads`, and `Up` and `Up-r`. 3 runs each, with the modes alternating.
- **Recorded:** median and range of wall time and peak; output bytes; the `DB_READ` count; the count of `temp-state-incoherent` diagnostics with `effectType == "DB_READ"` (O8: if above 0, record a discovery, with no fix in this issue).
- **Where it goes:** a new section in a measurement doc, `docs/measurements/2026-10-2x-issue-246-include-reads.md`, plus a summary in the CHANGELOG.

**Admission (S4/O4, decided before measuring).** The rows are keyed by (command, universe), not by flag (`admission.rs:32-72`).
- If any reads median (`d`, `h`, or `Up-r`) is above the `("digest", X)` row of 3,932,160 KiB, raise that row **in this issue, in its own commit**, to the measured maximum rounded up to 256 MiB, with the numbers in its comment. Do the same for `("digest", S)` against `ds`.
- Raising is cheap. `W = min(4, ⌊29,360,128 / P⌋)` stays 4 for any row up to 7,340,032 KiB. If the row goes above that, W falls for every digest batch, and the commit says so.
- No new job kind for the flag. That would be a schema change to admission; it is not needed unless the peak goes past 7.34 GB, and that is the stop condition below.

**Fallbacks, stated up front:**
- **Reads missing from the tree** for some root shape (P3 wrong): stop and record a discovery. Do not add a second read source.
- **T2 fails after the normalization above:** a non-read effect really changed (shared dedup, key or remap). Find the cause. Never widen the normalization beyond its 4 steps.
- **T4's break (a) does not fail** (no fixture makes a graded read carry a label): record item 4 as a cost-only choice with that stated limit, not as pinned.
- **High opt-in cost** (more than 2x wall time, or more than 1.5x peak): ship, state the numbers, and open a follow-up for a known-temp filter or `readTables[]`. The default path does not change.
- **Admission:** apply the rule above. If a reads peak goes past 7,340,032 KiB, stop and take it to the operator before merging, because W would drop for every digest batch.

**Change table (spec r1):**

| change | findings |
|---|---|
| T2's exact 4-step normalization, plus the >0 / named-fixture preconditions | S1, O1 |
| P9 corrected; item 4 rewritten ("not graded"); T4 pins exclusion and remap with separate breaks; T4-(a) fallback | S2, O2, O3 |
| T3 adds tempState (physical/temp) and the inherited-witness asserts, each with its own break | S3 |
| Measurement uses #277's `d`/`h`/`ds`/`Up` argv and maxrss.py, plus bytes and `DB_READ` counts; admission row raised in its own commit by a rule fixed in advance; 7.34 GB stop | S4, O4 |
| Documented-limits list (per site, kt/nt split, unresolved, groups without resourceId/tempState, 10-name cap, LockTable, CalcFields, not graded) | S5, O3, O6, O7 |
| T5 gets a read-specific break | O5 |
| T3 declares `Customer`/`Vendor` and asserts `resourceId` | O6 |
| M1 counts `DB_READ` incoherence diagnostics; discovery if above 0 | O8 |
| `payload.includeReads: true` when set (item 7) and normalization step 4 | O9 |

**Open questions, decided (spec r1):**
- OQ1: include known-temp reads, with `detail.tempState` and the `kt`/`nt` split. Boundary groups carry no tempState (limits list). Both reviewers agreed.
- OQ2: echo `payload.includeReads: true` when set (item 7). sol said no, Opus said yes. Decided yes: it costs 2 lines, it is additive, and flag-off bytes do not change.
- OQ3: #277's `d`/`h`/`ds`/`Up` argv (measurement plan above).
- OQ4: exclude reads from ordering, documented as "not graded" and pinned by T4. Both reviewers agreed.
- OQ5: no reads flag for other commands. `map_op` is unchanged. Every other caller passes `false` explicitly, including `prove_plan`, `digest_rule_b_tests.rs:115,325` and the `digest_query` call sites at `digest.rs:4575, 4598, 4735, 7690, 7753, 8288`.

**Plan size:** 2 tasks.
- **Task 1:** items 1–7, the help text, and tests T2–T5 with their discrimination proofs.
- **Task 2:** M1, the admission row (its own commit, only if the rule fires), the measurement doc, and the CHANGELOG with the limits.

Task 1 is larger than first planned (two more tests), but it is still one flag.

## Task 1

**Implemented.**
- Flag path: `DigestCli.include_reads` (`--include-reads`, with help that states the limits and "not graded") → `DigestRequest.include_reads` → `session::digest_reads_plan`, which is `digest_plan` plus the per-job `TreeDemand.include_reads` (the prove_plan pattern, because `Plan.trees` is private) → `digest_effects` → `compute_digest_effects_cli` → `digest_query_reads` → `digest_query_with` → `digest_one_root`.
- Every other caller passes `false`: `digest_query`, the ordering path, `run_digest_pipeline`, prove, the fingerprint trees, and the session and rule-B tests.
- `DB_READ`: a local `effect_type_of` closure in `digest_one_root`, gated by `include_reads && op == "read" && kind == "table"`. `map_op` is unchanged.
- Ordering: `graded` filters `DB_READ` out of `ordering_inputs`, and the remap zips the graded effects with `scoped`. Flag off, `graded` is always true: no vector, same order.
- The tempState join gains a `DB_READ` arm.
- `payload.includeReads: true` is written only when the flag is on.
- Fixture helper: `write_own_inner` in `tests/common/dep_dispatch_fixture.rs`, now also the body of `write_own_modify`, so the `lsp` umbrella sees no dead code.
- Tests: `tests/cli/cli_digest_include_reads.rs`. All four spawn `CARGO_BIN_EXE_alsem`.

**TDD.** Red first: all 4 new tests failed with `unexpected argument '--include-reads'`. One correction during green: T3's evidence line is 0-based, as `t1` reads it (`line_of` had a +1). After that, green: 4/4.

**Discrimination proofs.** Every break was made with Edit and confirmed applied exactly once (Edit refuses a missing or ambiguous match; `grep -c` of the new text = 1). Each was reverted with Edit to the exact original text.

| # | break (Edit) | target | broken run | reverted |
|---|---|---|---|---|
| B1 | read arm `include_reads &&` → `true &&` (default on) | T1 | FAIL: `cli_b_digest_differential` json, human and diff (ws-d8-commit-in-tx gains `[DB_READ]`); T2 "flag-off has DB_READ"; T3; T5 | 4/4 + goldens pass |
| B2 | remap `effects.iter_mut().filter(graded).zip(&scoped)` → `effects.iter_mut().zip(&scoped)` (off-by-position) | T2 (O10), T4(b) | FAIL: **T2 on `ws-d8-commit-in-tx`** ("flag-on minus its reads differs from flag-off"); T4: the read got `[COMMIT_REACHABLE, WRITE_PENDING_AT_UI]` (UI_MESSAGE's labels) | pass |
| B3 | read arm `f.op == "read"` → `f.op == "B3-no-read"` | T3 counts | FAIL: T3 (OneRead 0 ≠ 1), T4, T5, T2 (the >0 precondition) | pass |
| B4 | tempState match drops `\| "DB_READ"` | T3 tempState | FAIL: T3 only (`tempState` "" ≠ "known(false)") | pass |
| B5 | per-part `paths.clear()` when `DB_READ` and not direct | T3 inherited witness | FAIL: T3 (via [] ≠ ["Caller>Helper"]); T5 (via [] ≠ [""]) | pass |
| B6 | `graded` compares `"B6-graded"` (reads back in the ordering input) | T4(a) | FAIL: T4 only (read guarantees `[COMMIT_REACHABLE]` ≠ []). **T2 stayed green**, which confirms O2: the strip test cannot see this | pass |
| B8 | read arm adds `&& f.provenance == "direct"` | T5 (read-specific, O5) | FAIL: T5 (0 ≠ 1 `DB_READ`); T3 (Caller 0 ≠ 1) | pass |
| B9 | `FingerprintIndexes.boundary` → `None` (rule B off) | T5 rule B | FAIL: T5 (`boundary` "" ≠ "XBase.Outer"; the read becomes a plain effect on table 18) | pass |

- B2 / O10: T2's remap break fails on **`ws-d8-commit-in-tx`**, the first corpus fixture, so T2 alone also pins the remap. T4(b) pins it too.
- B1–B9 ran with the dependency fixture as `write_own_read`. I then folded that helper into `write_own_inner` with the same `Inner` text (`Cust.Get('A');`). After the fold, T5 passes again in the gate run.

**Gate** (logs in my scratchpad; output redirected and grepped, never piped through tail):
- `bash scripts/ci-steps task-affected`: exit 0. It ran the whole suite (a change outside the map): 4410 passed, 8 skipped.
- `bash scripts/check-goldens`: exit 0, 1255 passed, 4 skipped. No golden file changed (`git status` shows no `tests/*goldens*` path).

## Task 2

**Measurement:** by the coordinator, in `docs/measurements/2026-10-25-issue-246-digest-include-reads.md`; raw data and `run.sh` in `/work/bench/246/`. The admission rule fired for `("digest", X)` only: the `Up` flag-on median is 4,062,196 KiB, above 3,932,160. No stop (max 4,075,056 < 7,340,032).

**Point 1: the `ev`/`qt` checksums in the Up batch. Verdict: volatile fields, no leak.**
- None of the bench argv passed `--deterministic`. `events fanout` writes a live `generated_at`; `query touches` writes a live `generatedAt` and `deterministic: false` (`pinned_or_now_iso8601`, `gate/format_json.rs:459`). `analyze --format sarif` has no timestamp, so `a` was equal.
- Probe (cdo-profile build of 953a8e0e, on `<CDO_WS>`, single runs, not timed):
  - `batch` `[digest --file $F --include-reads, events fanout, query touches]` and standalone `events fanout` / `query touches`, all `--deterministic`. `cmp`: `ev` and `qt` are byte-equal. The batch digest carried 1,133 `DB_READ`, so the flag was live in the same view.
  - A non-deterministic standalone run against the deterministic one, after `jq -S`, differs only in `generated_at` (ev) and `generatedAt`/`deterministic` (qt).
- No code change. No test was added, because nothing leaks. The Task 1 tests already pin that the flag is per job: `TreeDemand.include_reads` is per tree, and T2 compares flag-off output byte for byte.

**Point 2: `tempState` for reads. Verdict: not a join bug. It is a pre-existing cone/L4 reach gap, so per the fallback: discovery, no code change. P8 corrected.**
- On `d`, all 1,059 incoherent reads have reason `no-l4-row`.
  - 1,047 share ONE evidence site, `RecRef.FindFirst` (`CDOEMailHandler.Codeunit.al:243`), each for a different table.
  - The other 12 have `evidence-unavailable`.
- **Same in the default output for writes:** 476 `DB_INSERT` + 290 `DB_MODIFY` + 6 `DB_DELETE` carry `no-l4-row`, at one `RecRef.Insert` and one `FilterRecord.Modify`. Writes fail at 95%, reads at 93%.
- **The key and the store are right:** the join filters the root's L4 rows by operation id, from the view's `l4`. `query effects --routine IssueFinanceChargeMemo` (L4) lists 307 ops, many of them reads, so P8's "the store has read rows" holds. But 0 of the 529 ops behind that root's unjoined reads are among them, and the reads span 524 tables against L4's 52. The capability cone reaches code that the root's L4 summary does not hold.
- **The evidence is misplaced:** all those reads land on one resource-less RecordRef carrier. `fact_equivalent` compares `resourceId` only when both facts have one (`digest.rs:837-866`, witness.ts parity), so the BFS takes the RecordRef read as the carrier for a read of any table.
- **About 22k diagnostics on `h`:** acceptable for an opt-in flag. They are true statements ("no L4 row, so no tempState"), the default output already emits the same diagnostic for writes, and suppressing them for `DB_READ` alone would make reads and writes disagree.
- **Flagged as a design question, not acted on:** whether `no-l4-row` should be one diagnostic per site, or a per-entry count instead of one per effect, for reads and writes alike. That changes default output, so it is out of scope here.

**Commits:**
- `359d5cb8`: admission row `("digest", X)` = 16 × STEP, with the numbers in the message and the comment. New lib test `digest_row_covers_include_reads`: the row is 16 steps, the budget fits 7, and `Up` keeps W = 4. Proof: with the row back at 15 × STEP the test FAILS (`grep -c` of the break = 1); restored, 8/8 pass. No existing T17 expectation changed.
- `6f681432`: the numbers file, with Notes for both root causes, and the CHANGELOG `Added` entry (cost in CPU-seconds and peaks, walls "under host load 5–24", the limits, the schema note, the tempState gap).

**Gate on `6f681432`:**
- `bash scripts/ci-steps task-affected`: exit 0, 4411 passed, 8 skipped.
- `bash scripts/check-goldens`: exit 0, 1255 passed, 4 skipped.
- `agentflow check-diff --base d2d84669… --head HEAD`: `{"reasons": []}`.
- The worktree is clean apart from `.agent/`.

## Final round 1 fixes

Findings: `.agent/runs/20261025-130917-08aebb/final-r1-merged.md`, on `6f681432`. One commit, "#246 final r1: G-1..G-8".

**Fixed**
- G-1 (cr2), confirmed by probe: `Root` → `Mid` (a resource-less `RecRef.FindFirst`, then calls to a Customer and a Vendor reader) gave both reads factId `d67d65ec86f62d43`, same evidence line, same via path. Fix in `digest_one_root`: a `DB_READ` (non-group) appends its `detail.resourceId` to the key effect type. Writes keep their key; the same shape still gives Customer's and Vendor's `DB_MODIFY` one factId. Test `reads_of_two_tables_at_one_operation_keep_distinct_fact_ids` (red before the fix).
- G-2 (cr4): `job_kind` returns `("digest reads", U)` for a digest with `--include-reads`. Rows: `("digest", X)` back to 15 × STEP; `("digest reads", X)` = 16 × STEP (`Up` on); `("digest reads", S)` = 3 × STEP (`ds` on, 732,292 KiB). Tests: `digest_row_covers_include_reads` (adapted: new kind, flag-on `Up` = `arreq` keeps W 4), `plain_digest_keeps_its_row` (15 steps, `adheq` W 4), bin test `digest_job_kind_follows_include_reads`. Every T17 test unchanged and green.
- G-3 (cr5): `schemaVersion` comment notes `DB_READ` and `payload.includeReads` under the flag.
- G-4 (cr6): closure renamed `effect_type_with_reads`; `graded` is `t != "DB_READ"` (no `DB_READ` exists without the flag).
- G-5 (cr8): `map_op` doc names `digest_one_root`'s `DB_READ` mapping.
- G-6 (sol F1): numbers doc says "equal byte sizes", not byte-identical, and that the runs lacked `--deterministic`. Admission note for G-2 added.
- G-7 (opus F1): `include_reads_is_per_job_in_a_batch` runs `alsem batch` `[digest --include-reads, digest]` (`--deterministic`), asserts one shared view, each job byte-equal to its standalone run, `DB_READ` only in job 0. Module doc adjusted.
- G-8 (opus F2): CHANGELOG says "where L4 holds the operation (about 7% of reads on CDO)", adds the G-1 key note and the new admission kind.

**Recorded only**
- G-9 (cr1): accepted limit. `DB_READ` evidence at a RecordRef carrier is the pre-existing witness rule writes share; fixing `fact_equivalent` moves default output. Filed discovery; CHANGELOG states it.
- G-10 (cr3): accepted. 93% `no-l4-row` is the pre-existing cone/L4 reach gap (writes 95%). Filed discovery.
- G-11 (cr7): accepted. `digest_query_reads` / `digest_reads_plan` keep every flag-off call site unchanged.
- G-12 (cr9): accepted for now; an options struct for `digest_effects`'s bools is a possible follow-up.

**Discrimination proofs** (each break by Edit, unique match enforced by Edit; reverted by Edit):
| test | break | broken | restored |
|---|---|---|---|
| `reads_of_two_tables_..._fact_ids` | DB_READ key condition → `"DB_READ_BROKEN"` | FAIL (assert_ne, line 455) | PASS |
| same, write half | key condition → `starts_with("DB_")` | FAIL ("a write key moved") | PASS |
| `include_reads_is_per_job_in_a_batch` | session.rs: `demand.include_reads` → `plan.trees.iter().any(\|(_, t)\| t.include_reads)` | FAIL (job 1 stdout) | PASS |
| `digest_job_kind_follows_include_reads` | guard → `d.include_reads && false` | FAIL | PASS |
| `plain_digest_keeps_its_row` | `("digest", X)` → 16 × STEP | FAIL | PASS |
| `digest_row_covers_include_reads` | `("digest reads", X)` → 15 × STEP | FAIL | PASS |
| `reads_are_not_ordering_graded_and_move_no_label` (G-4 `graded` change) | `graded` → `t != "DB_READ_BROKEN"` | FAIL | PASS |

**Gate on the commit:** `ci-steps task-affected` exit 0 (4415 passed, 8 skipped); `check-goldens` exit 0 (1257 passed, 4 skipped); no golden moved.

## Final panel
- Round 1 on 6f681432: sol said ship (F1 minor), Opus said ship (F1 and F2 minor), and code-review (high) raised 9 items. These were merged into G-1..G-12; G-1..G-8 were fixed in be0ae929.
- Round 2: sol and Opus both said ship. All 27 entries are accepted by both, so the panel converged after 2 of 5 rounds.
- The four gates passed on be0ae929. Master then moved to fffef2d6 (#319, agentflow only). The rebase was clean, every commit subject is intact, and the range-diff shows every branch commit "=", so no extra panel round was needed. The new head is 86bb4639, and the gates were re-run on it.

## Gate results
- B = fffef2d6 (origin/master, #319). H = 86bb46396af22b18bca7216e242113ff1d05da7b.
- Each gate ran through `agentflow run` on H and exited 0: check-goldens-coverage, ci-steps-all, check-goldens and cdo-gate.
- After H there is only the evidence commit.

## Discoveries
- Discovery (Task 2): **the cone/L4 reach gap behind `no-l4-row`.** On CDO cross-app, the capability cone of `IssueFinanceChargeMemo` reaches 529 ops / 524 tables that its L4 summary does not hold (L4: 307 ops / 52 tables). So `detail.tempState` is missing for about 95% of that root's transitive DB effects, writes included in the default digest. Which edge kinds the cone follows and L4 does not is still to be found (suspect: open-world object runs from the `RunReport` path).
- Discovery (Task 2): **resource-less RecordRef facts are witness carriers for every table.** `fact_equivalent` ignores `resourceId` when one side has none, so one `RecRef.FindFirst` becomes the evidence for 1,047 reads of 524 different tables (and one `RecRef.Insert` for 472 inserts) on `d`. The evidence line is then not where the table is touched. This is a witness-quality issue in the default output for writes as well.
- Candidate (P13): digest effects are per call site, so `--include-reads` gives one `DB_READ` per read site, not per table. Fine for this issue. If CentralGauge wants "tables read" directly, that is a `readTables[]` follow-up.
- Candidate (limit, not a bug): `CalcFields`/`CalcSums` are modeled as a read of the record's own table only. The FlowField's source tables (the CalcFormula's tables) are not modeled as reads, so `--include-reads` will not list them.
