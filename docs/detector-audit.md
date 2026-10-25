# Detector audit ledger (all 41 detectors, AL-semantics correctness)

Read-only audit of every detector (d1..d51) against AL semantics, hunting BOTH false positives
AND false negatives (the latter invisible to finding-triage). 10 agents. ~30 gaps across ~30
detectors. CLEAN: d17, d19, d46 (and largely d10, d13, d38). Grouped by shared root (fix once →
many detectors), then singletons, then priority.

## Shared-root class A — temp records not gated (FP) [HIGH volume]
The temp epoch gated d1/d3/d10/d33/d36/d37/d39/d40, but these still fire on `temporary` records:
- ~~**d2** (event-fanout-in-loop): no temp guard; subscriber touching only a temp table → `any_db_subscriber=true` → fires.~~ **FIXED** (commit `fix(engine-audit-d2)`, engine branch): `is_known_temp` filter added to `D2Policy::terminals_at`; `any_db_subscriber` now keys off a Complete walk to a surviving op. Test: `tests/gap_audit_d2_guards.rs::subscriber_temp_record_ops_are_suppressed`.
- ~~**d4** (repeated-lookup-in-loop): `d4.rs:57-82` no temp_state check → repeated lookups on temp vars fire.~~ **FIXED** (commit `fix(engine-audit-d4)`, engine branch): `is_known_temp` gate added to the candidate loop (`tempRecord` skip stat); physical control still fires. Test: `tests/gap_audit_d4.rs::temp_record_repeated_lookup_is_suppressed` (+ control).
- ~~**d8** (commit-in-transaction): `writes_tables_of` counts TEMP-table writes toward the 3-table manager gate (FP inflation).~~ **FIXED** (commit `fix(engine-audit-a-cone)` 40c5300): d8 gate + write_count use new `writes_physical_tables_of`.
- ~~**d29** (modify-in-subscriber): by-value/temp records flagged.~~ **temp FIXED** (commit `fix(engine-audit-d29)`): `is_known_temp` gate added (in-memory record fires no DB triggers). by-value NOT exempted — a Modify on a by-value PHYSICAL record copy still writes the row and re-fires triggers (only filters/load are copy-local), so suppressing it would be a false negative.
- ~~**d43/d44/d45** (event capability cone): `capability_query::writes_tables_of` / `find_capabilities` carry NO `is_temp` annotation → temp writes produce spurious conflict/exposure findings.~~ **FIXED** (commit `fix(engine-audit-a-cone)` 40c5300): `CapabilityExtra::Table` already carries `temp_state`; added `fact_is_known_temp` + `writes_physical_tables_of`; d43/d45 write-sets + d44 write/read predicates filter known-temp. Tests in `capability_query`.
ROOT: capability cone + these detectors lack the `op.temp_state Known(true)` / temp-table check. FIX: add the temp gate (and a temp annotation to the capability cone facts for d43-45).

## Shared-root class B — trigger-Rec suppression incomplete for TABLE-LEVEL triggers (FP) — **FIXED (commit `fix(engine-audit-b)`, engine branch)**
`is_platform_loaded_trigger_rec` (mod.rs ~:143) suppresses page triggers + table FIELD `OnValidate`,
but NOT table-level `OnInsert`/`OnModify`/`OnDelete`/`OnRename` — where the platform ALSO loads `Rec`
and AUTO-PERSISTS it after the trigger. Affects:
- **d21** (read-without-load): `Rec.TestField` in `OnModify` → FP.
- **d37** (validate-without-persist): `Rec.Validate` in OnInsert/OnModify → platform persists → FP.
- **d39** (record-left-dirty): no auto-persist-trigger gate → FP on normal trigger patterns.
FIX: add OnInsert/OnModify/OnDelete/OnRename (table-level, receiver Rec) to the platform-loaded/auto-persist gate; d37/d39 treat those triggers as persisting.
FIXED: `TABLE_TRIGGERS_REC_AUTO_PERSIST` added to the `is_platform_loaded_trigger_rec` table arm
(covers d11/d21/d37, which consume the gate directly). d39's caller-side check was
`is_auto_persist_trigger_rec` (`autoPersistTriggerRec` skip stat); since #272 it is the owner rule
`implicit_rec_persisted_by_owner` (`implicitRecOwnerPersists` skip stat), which covers these triggers
too. Tests: `tests/gap_audit_b_table_triggers.rs`
(suppression + non-trigger / non-Rec controls); g9/g14 suites unchanged-green.

## Shared-root class C — loop-detector guards (Next-terminator + virtual table) missing in d2/d18 (FP)
- ~~**d2**: `D2Policy.terminals_at` has NO `is_terminator_next` filter (d1 has it at d1.rs:616) AND no `op_targets_virtual_system_table` filter → `repeat..until Rec.Next()` + virtual-table reads in a subscriber fire falsely.~~ **FIXED** (commit `fix(engine-audit-d2)`, engine branch): both `is_terminator_next` and `op_targets_virtual_system_table` filters added to `D2Policy::terminals_at` (mirroring d1). Tests: `tests/gap_audit_d2_guards.rs::{subscriber_terminator_next_only_is_suppressed,subscriber_virtual_system_table_reads_are_suppressed}`.
- ~~**d18**: receives `_ctx` unused → no virtual-table gate; `SetRange` on `Date`/`Integer` virtual table in a loop fires with wrong advice.~~ **DROPPED — not a true FP.** d18 measures loop-INVARIANT recomputation (a constant filter re-applied every iteration), which is genuine redundant work regardless of table kind — unlike the SQL-cost detectors (d1/d2) where a virtual table has literally zero cost. Suppressing a constant `SetRange` on `Integer`/`Date` in a loop would hide a (tiny) real inefficiency = a false negative, which the suppression-direction discipline ranks worse than this marginal FP. Keep firing.
FIX: apply the same d1 guards (terminator-Next, virtual-table) to d2 (DONE). d18 left as-is by design (see above).

## Shared-root class D — RecordRef ops invisible as record_operations (FN, + one FP)
RecordRef `Insert`/`Modify`/`Find`/`FindSet` surface as CALL SITES, not `L3RecordOperation` entries:
- **d1** FN: a loop doing bulk DML via RecordRef is silent (terminals_at only scans record_operations).
- **d10** FN, **d36** FN, **d43/d44** FN: RecordRef writes/loads invisible.
- **d11** FP: `RecordRef.GetTable(Rec)` load is unrecognized → Modify after it fires.
ROOT: the op model does not capture RecordRef DML as record ops (deep — touches L2 capture). LARGER; scope carefully.

## Shared-root class E — first-match vs net-effect filter/load bugs (FP) — **d41 + d42 FIXED (commit `fix(engine-audit-e)`)**
- ~~**d41** (transitive-filter-loss) Gap-T [HIGH]: `d41.rs:111-120` uses `.find()` (first `SetRange`/`SetFilter`), ignoring an intervening `Reset` → fires when no filter was active.~~ **FIXED**: step (1) now computes the NET filter state at the callsite (SetRange/SetFilter → filtered, Reset → cleared, last-wins in source order; mirrors d33 `was_filtered_before`); a filter the caller cleared with its OWN Reset before the call no longer fires. Witness = last active filter op. Test: `tests/gap_audit_e_filter_load.rs::d41_*`.
- **d33** FN-1: `was_filtered_before` scans slice order not source order; a `Reset`-before-`SetRange` in source but later in slice wrongly suppresses. (Still open — separate from d41; low risk, deferred.)
- **d42** ~~Gap-W [HIGH]: `AddLoadFields` with no prior `SetLoadFields` treated as restricted load — but in BC `AddLoadFields` alone = FULL load → fires wrongly.~~ **Gap-W DROPPED — misdiagnosis.** MS docs (devenv-partial-records) confirm `AddLoadFields` DOES activate partial load and narrows even without a prior `SetLoadFields` ("select fields using SetLoadFields **or subsequent AddLoadFields calls**"; the by-value JIT remedy "Call AddLoadFields before passing by value" only works if it narrows). Current code (`Pending::None + AddLoadFields → Known`) is CORRECT; the proposed fix would inject false negatives. ~~Gap-Y: FlowField names in SetLoadFields flagged (BC ignores them).~~ **Gap-Y FIXED**: `apply_field_read` (cfg_walker, faithful port) adds ANY read field to the callee required-load set incl. FlowFields; a FlowField is never a physical column (CalcFields, not SetLoadFields), so it can't be a "missing load" round-trip. New `flow_field_names_lc` (mod.rs) excludes FlowField/FlowFilter from d42's required set alongside the existing PK exclusion. Test: `tests/gap_audit_e_filter_load.rs::d42_*`.
FIX: net-effect (last SetRange/SetFilter/Reset by source position) for d41 (DONE); d42 skip FlowFields (DONE). AddLoadFields-only=full-load NOT applied (docs-verified misdiagnosis).

## Singleton high-value
- **d29 FP-1 [HIGH]**: `Modify(false)` / `Insert(false)` (RunTrigger=false — the canonical pattern to AVOID recursive trigger re-fire) is NOT exempted (d29.rs:117 no arg inspection). Also `ModifyAll` in `OnAfterModifyEvent` (raises OnAfterModifyAllEvent, no recursion).
- ~~**d37 FN-1 [MED]**: `ModifyAll` in PERSIST_OPS (d37.rs:31) — but ModifyAll does NOT persist the CURRENT record's Validate result → Validate→ModifyAll passes the persist check while the change is discarded (silent FN).~~ **FIXED** (commit `fix(engine-audit-d37)`): `ModifyAll` removed from `PERSIST_OPS` (only `Modify`/`Insert` write R's Validate'd state); rootCause/doc updated. ws-d37 r4 golden rebaselined (wording-only, count unchanged, fingerprints identical). Tests `gap_audit_d37_modifyall.rs`.
- ~~**d20 FN [MED]**: `unconditional_exit_kind` (body_walk.rs:202) misses `break_statement` → unreachable-after-`break` not flagged.~~ **FIXED** (commit `fix(engine-audit-d20)`): `break_statement` → `Some("break")`; d20 words it "control leaves the enclosing loop". Block-scoped scan ⇒ no over-mark / no FP. (NOT TS-parity — al-sem indexed only the 3 routine-exit kinds; Rust-owned correctness add.) Tests `gap_audit_d20_break.rs`.
- ~~**d22 FN [MED]**: implicit-trigger `Rec` FlowField reads invisible → `Rec."Balance (LCY)"` in OnAfterGetRecord generates no PFieldAccess → d22 silent.~~ **FIXED** (commits `engine-d22`, plan `docs/superpowers/plans/2026-06-13-d22-implicit-rec-flowfield.md`): register the implicit trigger/page `Rec` as a record variable (L2, table_name=None) so `Rec.<field>` reads are captured; L3 `record_types` pass-3 fills its `table_id` from the effective-own-table (Table/Page/TableExt/PageExt, reusing the existing implicit-Rec OP resolution). d3 gains an `is_platform_loaded_trigger_rec` skip; argument-binding classification keeps `rec`/`xrec` as `implicit-rec` so forward-detectors + the call graph are unchanged. 150 projection goldens rebaselined; no detector/call-graph golden moved. Tests `gap_audit_d22_implicit_rec.rs` + l3rt vectors.
- **d34 FN / d35 FN [MED, by-design]**: `Unknown` effect skipped (avoid-FP G6 choice) → transitive commit-in-loop / in-subscriber missed in partial-coverage. Contentious (open-world conservative) — likely DEFER.

## Singleton bugs (correctness)
- ~~**d4 BUG-5**: finding id `d4/{routine}/{loop}/{varLc}` omits the literal key → two distinct keys in the same (routine,loop,var) → DUPLICATE ids (no merge_by_terminal).~~ **FIXED** (commit `fix(engine-audit-d4)`, engine branch): the literal key is appended to the id ONLY when a variable has 2+ qualifying key groups — single-key ids stay byte-identical (r4 golden verified unmoved). Test: `tests/gap_audit_d4.rs::two_distinct_keys_in_same_loop_get_distinct_ids`.
- ~~**d7 BUG-1**: rootCause doubles the anchor routine name (`chain.join` already includes it, appended again, d7.rs:218-223).~~ **DROPPED — misdiagnosis.** The trailing `→ first.name` is INTENTIONAL cycle-closing notation (start = end shows the loop closes); the `ws-d7-event-cycle` golden freezes `SubB → … → SubB`. Faithful to al-sem; not a bug.
- **d7 FN-1**: SCC anchor = first SORTED member; if that is external/dep → `else continue` drops the whole cycle (should pick first WORKSPACE member). **DEFERRED** — only manifests with cross-app dependency cycles, and changing the anchor moves the rootCause closing name (golden churn). Contentious; low value.
- ~~**d12 BUG-1**: missing `dep_routine_ids` primary gate (d12.rs:56-57 commented, not coded) → cross-app dep integration events fire d12.~~ **FIXED** (commit `fix(engine-audit-bugs)`): `ctx.dep_routine_ids.contains(pub_routine_id)` skip added (a dependency app's dead event is not the user's to fix). Source-only unaffected (empty dep set). `dependency` skip stat.
- **d14 FP**: `internal_reachable_externally` hardcoded false (detector_context.rs:273) → `internalsVisibleTo` friend-only routines flagged dead. (Documented TODO — wiring app.json.) DEFER (schema).
- ~~**d50 BUG**: `CommitBehavior::Suppress` not in the effective-commit set (d50.rs:139 has ignore/error only) → false fire.~~ **DROPPED — misdiagnosis.** BC `CommitBehavior` has only `Ignore` / `Error` (+ implicit `Default`); there is NO `Suppress` member (MS Learn devenv-commitbehavior-attribute). d50's `ignore`/`error` check is complete.
- ~~**d51 BUG**: HTTP GET before Error fires (io_direction not filtered, unlike the commit check, ordering_facts.rs:435-453); io_label fallback hardcodes "write-direction".~~ **FIXED** (commit `fix(engine-audit-bugs)`): the `IO_BEFORE_ESCAPING_ERROR` arm now suppresses a PROVEN read-direction request (HTTP GET/HEAD → idempotent on retry, no duplication), mirroring the sibling commit arm; `unknown` (Send) keeps firing (suppression-direction safe). io_label fallback left — only reaches FILE-write / unknown, never proven-read. Tests in `ordering_facts`.
- **d47/d48/d51 FN**: IsolatedStorage.Set / TaskScheduler.CreateTask not modeled as IO (is_io_type narrows to HTTP|FILE, ordering_facts.rs:37).

## Priority fix order (clearest, highest-value first; group by shared root)
1. ~~**B** — table-level trigger-Rec gate (d21/d37/d39) — extend the existing mod.rs gate, high-volume, easy.~~ DONE.
2. **A** — temp-gate d2/d4/d8/d29/d43/d44/d45 — extend temp check / cone temp annotation, high-volume. (d2, d4, d8, d43/44/45 DONE; only d29 remains, folded into #5.)
3. **C** — d2 + d18 loop guards (terminator-Next, virtual table) — mirror d1, easy.
4. ~~**E** — d41 net-effect filter + d42 AddLoadFields-full-load + FlowField — clear FP bugs.~~ DONE (d41 net-effect + d42 FlowField; Gap-W dropped as docs-verified misdiagnosis).
5. ~~**d29 Modify(false)** exemption — clear, canonical pattern.~~ DONE (RunTrigger=false + temp).
6. **bugs**: ~~d4 id~~ (DONE), ~~d7 rootCause~~ (dropped-misdiagnosis), d7 anchor (deferred cross-app), ~~d12 dep gate~~ (DONE), ~~d50 Suppress~~ (dropped-no-such-enum), ~~d51 io_direction~~ (DONE) — small correctness fixes.
7. **FN**: ~~d20 break~~ (DONE), ~~d22 implicit-Rec FlowField~~ (DONE — own session, full design), ~~d37 ModifyAll-not-persist~~ (DONE) — premise-narrowing fixes.
8. **D** (RecordRef-as-op) — larger (L2 capture); **d34/d35 Unknown-skip**, **d14 internalsVisibleTo**, **d47/d48/d51 IsolatedStorage/TaskScheduler** — investigate/defer (open-world or schema).
