# Issue #272 — an implicit Rec/xRec argument binds to its known table (d39–d42 see Helper(Rec))

## Claim

```json
{"run_id":"20261024-210819-3b21e9","issue":272,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/272-agent-discovery-272-a1","worktree":"/work/al-sem-issue-272-a1","body_hash":"5b73669aca90adb30bb93b9982b070402d72f547ea959d6e4c0ada27e32bc8c7","reconciled":[]}
```

Issue body hash (claim `body_hash`): `5b73669aca90adb30bb93b9982b070402d72f547ea959d6e4c0ada27e32bc8c7`.
Base (`make-worktree`): `a6d94a914c94a5babb84c65abaee1ffd2cc7986c` (master after #289).
Reviewers (operator instruction, confirmed for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator (2026-10-24): queue #264 → #265 → #271 → (#289 by operator decision) → #272. Claimed with a one-off `--loop-max 6` (5 used since the reset).

## Classification

**Bounded.** Three small edits in existing code paths (the binding's initial state, the implicit-Rec label, a d39 skip), no new structure.

## Assumption probes

Run by the design author (opus); evidence under `/work/bench/272/` (fixture `ws-shapes`, Base App workspace `ws-baseapp`, prototype `proto-v2.diff`, binaries `bin/{before,v1,v2}`, CDO `measure/{before,v1,v2}`).

| probe | result | evidence |
|---|---|---|
| why Rec/xRec stays non-record-arg | `ir_walk.rs` ~:1857 `is_implicit_rec` sets `bound_rec_var = None` → `source_record_variable_id` None; `initial_binding_state` (calls.rs:290) makes a binding upgradable only with that id; `upgrade_bindings` skips it | source |
| where the implicit Rec's table is known | `ir_walk` builds a synthetic `Rec` record variable (no table) for table / tableext / pageext and a page with SourceTable; no synthetic `xRec`; `record_types.rs` pass 3 fills `table_id` for `rec`/`xrec` from the object's table (page SourceTable, tableext base, pageext base page's SourceTable). Codeunit `TableNo` and report dataitems already bind `Rec` as `"local"` (works today) | ir_walk.rs:2094-2160, record_types.rs:204-258 |
| unknown-table case | a page whose SourceTable is missing still gets the synthetic Rec; the binding upgrades; loose subject `?`; d39–d42 need no table except d42's PK exclusion (then excludes nothing) | source |
| mislabel | any undeclared `Rec`/`xRec` is labelled `"implicit-rec"` (`unwrap_or(true)`), incl. a codeunit without TableNo — harmless today, wrong once implicit-rec bindings upgrade; `r0-corpus/ws-argument-bindings` pins exactly `HelperImplicitRec(Rec)` in a codeunit | source |
| other readers of bindings | d37 `post_validate_helper_verdict`, `record_load_points`, `callee_applies_op_to_by_var_arg` (detectors/mod.rs), L4 cfg_walker / summary_runner, a program_calls counter; L4 matches by parameter id / name / `source_parameter_index` (an implicit Rec has none) → parameter roles do not move | Base App d39 candidate counts unchanged in v2 |
| what d39–d42 do with a resolved implicit Rec | d39 admits `implicit-rec` with one guard (table-level OnInsert/OnModify/OnDelete/OnRename); d40 skips `implicit-rec` before the binding (C2(a)); d41, d42 do not check the kind | d39.rs:127, d40.rs:92 |
| tests pinning it | `cli_loose_fingerprint::d41_implicit_rec_binding_is_not_reported`; `gap_audit_b_table_triggers::table_level_trigger_rec_ops_are_suppressed` (`MakeDirty(Rec)` in OnModify, vacuous today); `r0-corpus/ws-d40/src/Trigger.al` (`TouchHelper(Rec)`); l2 snapshot + r1a differential for `ws-argument-bindings`; r4 goldens ws-d39..d42 use locals only | grep |
| fixture probe (`ws-shapes`) | before: only local-record routines fire. v1 (calls.rs line only): d41 fires on table Rec, bare `SetRange` Rec, tableext, page OnAction, pageext, xRec; d42 on table and page Rec; d39 also fires on table procedures, field OnValidate, page OnAction, tableext OnBefore/OnAfterModify; d40 skipped. Table-Rec d41 has the same loose fingerprint as the local (`a8fc…`) | `shapes-{before,v1,v2}.json` |
| goldens on prototype v2 | `check-goldens` 1167 passed, 3 failed: the d41 test (expected), `ir_l2_features_snapshot_over_r0_corpus`, `differential_l2_features_match_goldens` — both `ws-argument-bindings` moving `sourceKind "implicit-rec"` → `"unknown"`, `sourceVariableName` dropped (the mislabel fix). No r4 or cli-a golden moved | `goldens-v2.log` |
| CDO before → v1 → v2 (bc3ccb18, dirty 0, cross-app) | primary: d39 3→3→3, d40 143→143→143 (same fingerprints), d41 0→0→0, d42 2→2→2, d37 12→12→12; all: d39 3, d40 143, d41 0, d42 2 unchanged; **d37 345 → 163 → 163**; every other detector unchanged (0 fingerprints added). ~387 implicit-rec bindings at resolved edges; none reaches a filter-resetting callee, a dirty-leaving callee or a narrowed load | `measure/` |
| Base App triage (single-app) | v1 d39: 351 new, all forwarding a table/page `Rec` (299 Page, 49 Table, 3 TableExtension; 232 OnAction, 50 OnValidate, 50 OnLookup, …); ~12 read in source (e.g. `AssemblyAvailabilityMgt.ShowItemAvailabilityFromAsmLine(Rec, …)` in a page OnAction; table procedures handing Rec back via `OnCleanProdBOMNo(Rec)`) — all FP → **v1 d39 FP ≈ 100%**, root cause fixed by the skip (v2: 0 new d39). d42: 1 new, `VATEntry.SetGLAccountNoWithResponse` (`SetLoadFields("G/L Acc. No.")`, `FindSet(true)`, then `AdjustGLAccountNoOnRec(Rec)` reads 9 other fields) — **real** (n=1). d41: 0 new. d40: 0 by design. d37: CDO −182 (all Base App dependency, `--scope all`: 175 Table + 7 TableExtension Validates on Rec in table procedures, now "helperPersistsUnknown" because Rec is handed by `var` to an `OnAfter…(Rec, …)` event or helper; 12/12 sampled have that shape; removed findings were FPs — the caller owns the table-procedure Rec); Base App single-app d37 −1 (`SegmentLine.StartWizard2` → `PAGE.RunModal(Rec)`, FP). d11: −20 on Base App, 0 on CDO (16 `ItemTrackingLines.Scan*` with `InitRecForBarcodeScanner(Rec)` running `Init()` first; 2 `ErrorMessage.LogLastError` where `GetLastContext(Rec)` loads Rec; 1 `GenJnlLine.UpdateLineBalance`) — all FPs now correctly suppressed | `ba-*.json(l)` |
| page Rec auto-persist (G-9) | **superseded** by the BC 28.4 probes (`rev2/bcprobe-run1.txt`, `rev3/bcprobe-results.txt`; see the BC table in `## Design`) | — |

## Design

Revision 3 (after spec round 2; same opus author; evidence root `/work/bench/272/rev3/`: prototype `proto-rev3-with-breaks.diff` (env-gated `P272_BREAK_*` breaks, `P272_DEBUG` print in d41/d37), binary `bin/rev3`, fixtures `ws-shapes` (extended), `ws-readers`, new `ws-gshadow` (snapshots in `rev3/`), verdicts `rev3/delta/VERDICTS.tsv`, BC probe `rev3/bcprobe-src/`, `rev3/bcprobe.sha256`, `rev3/bcprobe-results.txt`).

**Classification.** Bounded: four edits in existing code, no new structure — the binding's initial state (calls.rs); the implicit-Rec label and the synthetic-Rec shadowing rule (ir_walk.rs); d39's owner rule replacing the class-B helper (d39.rs, detectors/mod.rs); docs and CHANGELOG.

**Goal.** A table, tableext, pageext or page-with-SourceTable implicit `Rec`/`xRec`, passed as a bare argument, binds like a declared record variable. d41 and d42 then report it the way they report a local record. d39 reports it except where the caller or the platform persists `Rec` (measured on BC 28.4, scope below).

**Non-goals.** d40 keeps its C2(a) skip (see Not delivered). No L4 parameter role for the receiver. No loose-subject table for `xRec` (stays `?`). The codeunit-`TableNo` `Rec` keeps its typed `local` binding (D1).

**Changes.**
1. `calls.rs` `initial_binding_state`: `source_kind == "implicit-rec"` starts as `"unresolved-callee"`, so it can be upgraded. Side effect (CHANGELOG): d37's helper-persistence walk now sees these sites; a site handing `Rec` to an unresolved, ambiguous or arity-mismatched callee becomes `suppress-unknown`, exactly as for a local. In cross-app mode this also suppresses d37 for dependency callers: the PrimaryGraph keeps edges for workspace callers only, so a dependency caller's call has no callee ("no-callee"; D2).
2. `ir_walk.rs`: two separate facts. (a) The object exposes an implicit record: the existing `has_implicit_rec(o, source_table_name)` minus its codeunit-TableNo case — Table, TableExtension, PageExtension, or a Page with a resolved SourceTable. (b) The name is not declared: no parameter, local or object global of any type is named `rec`/`xrec`. `implicit-rec` needs (a) and (b); otherwise the existing chain `parameter` → `local` → `unknown`. In `ir_record_variables`, a declared `Rec` of ANY type — parameter, local or object global, scalar or record — suppresses the synthetic `Rec` row; the object-global part applies only to Table, TableExtension, Page and PageExtension (O15: AL0155 was checked only there, so a codeunit-TableNo or report-dataitem `Rec` keeps today's typed path even if a global `Rec` is declared; Base App declares none). Effect: a global record `Rec` is then promoted at L3 (workspace.rs:1128-1139 no longer finds a phantom row first) and binds `global` with its own table; a scalar `Rec` stays `unknown`. alc rejects a page/table global named `Rec` (AL0155, `rev3/global-rec-compile.txt`), so the global case is robustness only; a local `Rec: Integer` in a table procedure does compile.
3. `d39.rs` + `detectors/mod.rs`: ONE guard. Delete `is_auto_persist_trigger_rec` (d39 was its only caller). d39 keeps `implicit-rec` among its admitted kinds. The skip applies only when all three hold: the binding is `implicit-rec`; the name is `rec` (`xRec` always reports); the caller has no load point after the call (`record_load_points`, before the call by source order, not `branch_exclusive` — d39's own "dirt discarded by a later load" rule for parameters). A comment states why there is no `is_variable` call (an implicit binding has no record-variable id, so the `rec` name bucket is the match). The rule `implicit_rec_persisted_by_owner` is true for:
   - **Table / TableExtension:** any procedure and any field trigger (the caller owns `Rec`); any trigger except `TABLEEXT_TRIGGERS_AFTER_WRITE` = {OnAfterInsert, OnAfterModify, OnAfterRename} (all three measured lost). Table OnInsert/OnModify/OnRename measured persisted; tableext OnBeforeInsert/OnBeforeModify/OnBeforeRename measured persisted; OnDelete/OnAfterDelete stay exempt, unmeasured, as moot (the row is gone: no write to lose, no fix to suggest).
   - **Page / PageExtension:** any procedure, plus `PAGE_TRIGGERS_REC_PERSISTED`, on a page that is neither `Editable = false` nor `ModifyAllowed = false`; for a pageext the base page is looked up by name, and a base page outside the model counts as modifiable. `PAGE_TRIGGERS_REC_PERSISTED` = OnAction, OnValidate, OnLookup, OnAssistEdit, OnDrillDown, OnNewRecord, OnInsertRecord, OnModifyRecord, OnBeforeAction, OnAfterAction, OnAfterValidate (all measured persisted), plus OnBeforeValidate (by analogy with OnAfterValidate; not probed).
   - **Everything else is false → d39 reports:** OnAfterGetRecord (measured lost), OnOpenPage, any other trigger, a read-only page (measured lost), other object types.
   - **Known false negative, runtime-only (as today):** a modifiable page opened in view mode loses the write (measured), but the property-only guard exempts it — the open mode is a runtime fact static analysis cannot see. Same for a page procedure called from a non-persisting trigger.
   - One stat, `implicitRecOwnerPersists`, replaces `autoPersistTriggerRec` (nothing pins the old name).
4. Docs and CHANGELOG: `detectors/mod.rs:336` (doc pointer to the deleted function) and `:366-373`, `docs/detector-audit.md:26-27`, `calls.rs:280`, `tests/cli/cli_loose_fingerprint.rs:904-907`, the d39 module doc. CHANGELOG Fixed: the binding, the label, scalar and global shadowing. Changed: the d39 rule; d37 more conservative at unresolved/ambiguous/arity-mismatched `Rec` sites; d11, d21, d33 now see helper loads and filters on `Rec`.

**Acceptance matrix** (every break run on the rev3 prototype; fixture counts against the extended `ws-shapes`).

| # | requirement | test | break that must fail it |
|---|---|---|---|
| A1 | table procedure `ResetIt(Rec)` gives d41 with the same loose fingerprint as the local | rename `d41_implicit_rec_binding_is_not_reported` → `…_is_reported`: 2 findings, equal `looseFingerprint` | revert change 1 |
| A2 | d41 on page OnAction, pageext OnAction, tableext procedure, bare `SetRange` + `Rec`, field-OnValidate `xRec`, codeunit global `Rec` | cli test over `ws-shapes` | revert change 1: every implicit-rec d41/d42/d39 finding disappears (18 lines, `rev3/shapes-brk-CH1.txt`) |
| A2b | labels: page `Rec` = `implicit-rec`; local scalar `Rec` = `unknown`; declared local `xRec` = `local`; declared `Rec` + `xRec` → `xRec` = `implicit-rec`; page and table global scalar `Rec` = `unknown`; page and table global record `Rec` = `unknown` at L2, then `global` with the global's table (G Other, 50301) at L3 | L2 test asserting `sourceKind` on `ws-shapes` + `ws-gshadow`, plus a model test asserting the final binding kind and table on `ws-gshadow` | drop the local shadow check → scalar `Rec` = `local` (1 line); drop the global shadow check → all 4 gshadow sites `local`, final binding `local` on the object's own table (50304/50300) instead of `global` 50301; ignore (b) → `implicit-rec` (2/4 lines). Outputs `rev3/a2b-*.kinds`, `rev3/gshadow-final-binding.txt`. No finding moves under the local-shadow break, so only `sourceKind` catches it |
| A3 | d42 on table `Rec` and page `Rec`, same loose fingerprint as the local | new test | revert change 1 |
| A4 | shared readers over `ws-readers` with controls: d37 suppressed when the helper persists / callee unresolved / arity mismatch, reported for read-only helper, by-value helper, wrong variable, helper before the Validate; d11 suppressed when a helper loads `Rec` by var, reported for by-value and load-after-Modify; d21 suppressed when a helper loads, reported for wrong variable; d33 suppressed when a helper filters by var, reported for by-value and no filter; plus the local-record reference shapes | cli test | revert change 1 → exactly the before set (run) |
| A5 | d39 does NOT report: table procedure, table field OnValidate, table OnModify, page OnAction / field OnValidate / OnModifyRecord, card page procedure → `Dirty(Rec)`, pageext OnAfterAction, tableext OnBeforeModify / OnBeforeRename / OnAfterDelete. DOES report: tableext OnAfterModify / OnAfterInsert / OnAfterRename, `xRec`, `Editable = false` list OnAction, `ModifyAllowed = false` card OnAction, pageext action on a read-only list, OnAfterGetRecord, a procedure reloading `Rec` after the call, a local (unchanged) | new test over `ws-shapes` + `gap_audit_b_table_triggers::table_level_trigger_rec_ops_are_suppressed` | owner rule returns false → +11 d39 and the gap test FAILS (single guard: this one break now fails it); drop the reload condition → `ExtReloadAfter` disappears; `P272_BREAK_PAGEPROC` → `PageProcDirty` appears; `P272_BREAK_MODALLOWED` → `S NoMod Card` OnAction finding disappears; `P272_BREAK_PAGEEXTBASE` (read the pageext's own properties) → `S RO List Ext` OnAction disappears (all run) |
| A6 | d40 still skips the implicit `Rec` | r4 `ws-d40` golden | delete C2(a) → `differential_r4_findings_match_goldens` FAILS (run); fixture +21 d40 |
| A7 | undeclared `Rec` in a codeunit without TableNo is not implicit-rec | regen the l2 snapshot + r1a goldens for `ws-argument-bindings`; only `CallVariations/cs3` changes (`implicit-rec` → `unknown`, `sourceVariableName` dropped) | restore `unwrap_or(true)` |
| A8 | L4 parameter roles unchanged | `aldump --r3a2-summary-core` over `ws-shapes` + `ws-readers` byte-identical before vs after (rev3 rerun) | — (pin) |

**False-positive and false-negative shapes.** FP: d39 on a temporary `Rec` behind an `IsTemporary` guard (`WorkflowStepInstance.BuildTempWorkflowTree`). FP: d39 in an OnOpenPage that hands `Rec` on to be inserted later (`Certificate`). FN: d39 on a modifiable page opened in view mode (runtime-only, as today). d42 with `SetLoadFields` and no reload (same as a local). d41/d42 do not check temporary state.

**BC results** (BC 28.4 Linux sidecar, TestPage; app `/work/bench/272/bcprobe`, codeunit 71020; published, run, uninstalled with data deleted, unpublished). "Default" = `TestPage.Trap(); PAGE.Run(...)`, the mode a page really opens in; `Editable()` read right after opening; the helper does `Validate` only, never `Modify`.

| shape | opened | Editable() | result |
|---|---|---|---|
| Editable=false list, OnAction | default | No | LOST |
| same | OpenEdit | No (property forces view) | LOST |
| pageext action on that list | default | No | LOST |
| editable list, OnAction (control) | default | Yes | PERSISTED |
| same editable list | OpenView | No | LOST — mode effect alone |
| ModifyAllowed=false list | default / OpenEdit | Yes | LOST — property effect alone |
| Editable=false card | OpenEdit | No | LOST |
| ModifyAllowed=false card | OpenEdit | Yes | LOST — property effect alone |
| editable card via `PAGE.Run(rec)` | default | Yes | PERSISTED |
| pageext OnAfterAction / OnBeforeAction | edit | | PERSISTED |
| pageext field OnAfterValidate | edit | | PERSISTED (at close) |
| table OnInsert / OnModify / OnRename | | | PERSISTED |
| tableext OnBeforeRename / OnAfterRename | | | PERSISTED / LOST |

Revision-2 rows still stand (`rev2/bcprobe-run1.txt`): card triggers and procedure persisted; OnAfterGetRecord lost; tableext OnBeforeInsert/OnBeforeModify persisted, OnAfterInsert/OnAfterModify lost. Both properties lose the write on their own, independent of the mode. The 10 REAL d39 sites have exactly the measured list shape.

**Measurement (done, on rev3).** CDO (the pinned CDO baseline @ bc3ccb18, dirty 0, `rev3/cdo-comp.txt`): primary 2,323 → 2,323, 0 fingerprints moved; `--scope all` 30,800 → 30,618, only d37 345 → 163 (182 removed, 0 added); opt-in d40 143 → 143, same fingerprints; `alsem fingerprint` byte-identical; rev2b vs rev3: 0 fingerprints differ. Base App (`--single-app --scope all`): d39 351 → 363, d42 78 → 79, d11 2,341 → 2,321, d37 1,401 → 1,400; d21 2,569, d33 185, d41 14 unchanged; rev3 = rev2b (0 fingerprints differ); the exempted d39 set is the same 340. Triage (`rev3/delta/VERDICTS.tsv`): d39 +12 = 10 REAL (one shape, now BC-measured: `Assembly Quotes`/`Assembly Orders` read-only list OnAction → `ShowItemAvailabilityFromAsmHeader(Rec, …)`) + 2 FP; d42 +1 REAL (`VATEntry.SetGLAccountNoWithResponse`); all 21 Base App removals are FP removals (d11 −20 = 17 `Scan*` + 2 `LogLastError` + 1 `UpdateLineBalance`; d37 −1 `SegmentLine.StartWizard2`). CDO d37 −182 is conservative suppression, not demonstrated FP elimination: in VERDICTS.tsv 22 read rows are `FP-by-source; removal-is-conservative-suppression` and 160 are `conservative-suppression (class only, not read)`. Goldens (`rev3/goldens.log`): 1,167 passed, 3 failed — the expected three (the d41 test and the two `ws-argument-bindings` l2 goldens), one changed routine.

**Pre-stated fallbacks.** (a) More than 30% FP among the triaged new d39/d41/d42 findings: find the root cause; if not fixable in scope, gate that detector's implicit-rec binding behind opt-in. Measured: d39 2/12 = 16.7% per finding, **conditional on the 10 REAL verdicts**, and per distinct shape 2 FP shapes against 1 real shape; d42 0/1; d41 no new findings → no gate. (b) A platform-behaviour row is contradicted: move that trigger or property to the reporting side and re-triage. It already applies to one measured contradiction, view mode, which cannot be moved because the mode is not in the source; it is recorded as a runtime-only false negative (as today), not as an exemption justified by persistence. If a Windows Cronus run disagrees with any row, apply (b) to that row — in particular, if read-only lists persist there, the 10 REAL become FP, d39 goes to 12/12 FP and (a) forces the opt-in gate. (c) Any golden other than the two `ws-argument-bindings` l2 goldens moves: triage line by line before any regen (measured: none moved).

**Not delivered** (reconciled with the issue's "the same d39–d42 findings as a local"). d41, d42: delivered. d39: delivered with owner exemptions, by design — a local belongs to the routine; an implicit `Rec` belongs to the caller, or the platform writes it (measured for the cases in change 3). Without the exemption Base App would gain 340 more d39 findings; 6 read at random, all correctly exempt (`rev3/delta/ba-d39-exempted.tsv`). A root-cause rule, not an opt-in demotion; it knowingly misses the view-mode runtime case. d40: not delivered — C2(a) stays; deleting it would add 3,199 Base App and 52 CDO findings, dominated by platform-loaded triggers and table procedures where the caller loads `Rec`; a per-trigger d40 rule is a separate design (D3).

**Population limits.** Base App and CDO contain no pageext or tableext code that hands `Rec` to a d39 candidate; CDO has 0 implicit-rec d39 candidates even with the exemption off (dependency callers are outside the PrimaryGraph). The pageext and tableext rows rest on the BC probes only.

**Owed / not checked.** A Windows Cronus confirmation of the BC rows (CLAUDE.md; cannot be done in this container). Not probed: pageext OnBeforeValidate (by analogy), table OnDelete and tableext Delete triggers (moot), a table field OnLookup behind a read-only page (the Table arm exempts it regardless of page editability — a stated limit), a page procedure called from a non-persisting trigger, the real web client's default modes (TestPage only). The Base App L4 summary dump was OOM-killed in rev 2 and not retried; the L4 pin rests on the two fixtures. 160 of the 182 CDO d37 removals have a class verdict only.

Discoveries (pre-existing, for the freeze): **D1 (+O8)** d37 and d11 still fire on `Rec` in table procedures where the caller owns the record (Base App single-app d37 1,400, d11 2,321; CDO `--scope all` d37 163); a codeunit-TableNo `Rec` is bound `local`, but `Codeunit.Run(X, Rec)` hands it back. **D2** the cross-app PrimaryGraph has no edges for dependency callers, so d37's `site_pick` finds no callee → `suppress-unknown` (221 no-callee + 1 unresolved on CDO): the whole CDO d37 −182, of which 181 are still reported on Base App single-app. **D3** d40's C2(a) also skips `Rec` in page OnOpenPage, where the platform has not loaded it yet (135 Base App candidates, not triaged).

## Design revision 2 (superseded by revision 3 above)

Revision 2 (after spec round 1 and the operator's session restart; fresh opus author; evidence root `/work/bench/272/rev2/`: prototype `proto-rev2b.diff` (+ env-gated breaks `proto-rev2b-with-breaks.diff`), binaries `bin/{before,rev2b,breaks}`, fixtures `ws-shapes` (extended) + `ws-readers`, per-fingerprint verdicts `rev2/delta/VERDICTS.tsv`, BC probe `rev2/bcprobe-*`).

**Goal.** A table, tableext, pageext or page-with-SourceTable implicit `Rec`/`xRec`, passed as a bare argument, binds like a declared record variable. d41 and d42 then report it the way they report a local record. d39 reports it except where someone else persists `Rec` — the caller, or the platform, as measured on BC 28.4.

**Non-goals.** d40 keeps its C2(a) implicit-rec skip (see Not delivered). No L4 parameter role for the receiver (a table procedure dirtying the caller's `Cust.Foo()` record stays a known gap). No loose-subject table for `xRec` (stays `?`). The codeunit-`TableNo` `Rec` keeps its existing typed `local` binding (Discovery D1).

**Changes.**
1. `calls.rs` `initial_binding_state`: `source_kind == "implicit-rec"` starts as `"unresolved-callee"`, so `upgrade_bindings` can resolve it. Side effect (O7, stated in the CHANGELOG): d37's helper-persistence walk now also sees these sites; a `Rec`-taking site with an unresolved, ambiguous or arity-mismatched callee becomes `suppress-unknown` (more conservative), as it already is for a local.
2. `ir_walk.rs`: the label from two separate facts. (a) object-level availability — the object exposes an implicit record: the existing `has_implicit_rec(o, source_table_name)` (ir_walk.rs:143) minus its codeunit-TableNo case, i.e. Table / TableExtension / PageExtension / Page with a *resolved* SourceTable; (b) argument resolution — the name `rec`/`xrec` is not declared by any parameter, local or object global, of any type. `implicit-rec` requires (a) and (b); otherwise the existing chain: `parameter`, then a record variable → `local`, else `unknown` (lets the G-8 global backfill rebind a codeunit global `Rec`). Same file, `ir_record_variables`: a declared `Rec` of ANY type (parameter or local, scalar included) suppresses the synthetic `Rec` row (today only record declarations do, so a scalar `Rec` was bound to a phantom record).
3. `d39.rs` + `detectors/mod.rs`: ONE guard. `is_auto_persist_trigger_rec` is replaced by `implicit_rec_persisted_by_owner(routine, ctx)` (d39 was its only user; mod.rs has a doc reference). d39 keeps `implicit-rec` in its admitted kinds; the skip applies only when the binding is `implicit-rec` AND the name is `rec` (`xRec` is persisted by nobody → always reports) AND the caller has no load point after the call (a `record_load_points` entry before the next statement, not `branch_exclusive` — the same "dirt discarded by a later load" test d39 uses for parameters, d39.rs:174-224). `implicit_rec_persisted_by_owner` is true for: Table / TableExtension — any procedure, any field trigger, table-level OnInsert/OnModify/OnDelete/OnRename (former class B), any tableext trigger except `OnAfterInsert`/`OnAfterModify`/`OnAfterRename`; Page / PageExtension — any procedure and the triggers OnAction, OnValidate, OnLookup, OnAssistEdit, OnDrillDown, OnNewRecord, OnInsertRecord, OnModifyRecord, in both cases only when the page (for a pageext, the base page found by name) is not `Editable = false` and not `ModifyAllowed = false` (a base page outside the model counts as modifiable). Everything else → false → d39 reports. The trigger lists are named constants citing the BC measurement. New stat `implicitRecOwnerPersists` replaces `autoPersistTriggerRec` (no test pins the old stat).
4. Docs: `detectors/mod.rs:336` and `:366-373`, `docs/detector-audit.md:26-27`, `calls.rs:280`, `tests/cli/cli_loose_fingerprint.rs:904-907` (FR2 doc), the d39 module doc. CHANGELOG under Fixed (binding, label, scalar shadow) and Changed (the d39 rule; d37 more conservative at unresolved/ambiguous/arity-mismatched `Rec` sites; d11/d33 now see helper loads and filters on `Rec`).

**Acceptance matrix** (every break was run on the prototype; result in parentheses).

| # | requirement | test | break that must fail it |
|---|---|---|---|
| A1 | table procedure `ResetIt(Rec)` gives d41 with the same loose fingerprint as the local | rename `d41_implicit_rec_binding_is_not_reported` → `…_is_reported`; 2 findings (ViaRec, ViaLocal), equal `looseFingerprint` | revert change 1 (ViaRec d41 gone) |
| A2 | d41 on page OnAction, pageext OnAction, tableext procedure, bare `SetRange` + `Rec`, field-OnValidate `xRec`, codeunit global `Rec` (G-8) | new cli test over a repository copy of `ws-shapes` | revert change 1 (all gone except the global-Rec one, which hangs on change 2) |
| A2b | label: page `Rec` = `implicit-rec`; scalar `Rec: Integer` = `unknown`; declared local `xRec` = `local`; declared `Rec` + forwarded `xRec` → `xRec` = `implicit-rec`; codeunit global `Rec` = `unknown` | new L2 test asserting `sourceKind` per site | ignore (b) → scalar Rec and declared xRec turn `implicit-rec` (run); drop the any-type shadow in `ir_record_variables` → scalar Rec turns `local` (run; no finding moves, so only this `sourceKind` test catches it) |
| A3 | d42 on table Rec and page Rec, same loose fingerprint as the local | new test | revert change 1 |
| A4 | shared readers over `ws-readers`, with controls: d37 suppressed for helper-persists / unresolved / arity-mismatch, reported for read-only helper, by-value, wrong variable, helper before Validate; d11 suppressed when a helper loads Rec by var, reported for by-value and load-after-Modify; d21 suppressed when a helper loads, reported for wrong variable; d33 suppressed when a helper filters by var, reported for by-value and no filter; plus the local-record versions | new cli test | revert change 1 → exactly the before set (run: identical to the before binary) |
| A5 | d39 does not report the persisted implicit Rec (table procedure, table field OnValidate, table OnModify, page OnAction / field OnValidate / OnModifyRecord, tableext OnBeforeModify); DOES report tableext OnAfterModify / OnAfterInsert, `xRec`, `Editable = false` page OnAction, OnAfterGetRecord, a table procedure reloading `Rec` after the call; a local still reports | new test over `ws-shapes` + `gap_audit_b_table_triggers::table_level_trigger_rec_ops_are_suppressed` | (i) `implicit_rec_persisted_by_owner` returns false → 6 new d39 on the fixture and the gap test FAILS (run); (ii) drop the reload condition → `ExtReloadAfter` disappears (run) |
| A6 | d40 still skips the implicit Rec | r4 `ws-d40` golden (`TouchHelper(Rec)`) | delete C2(a) → `differential_r4_findings_match_goldens` FAILS on ws-d40 (run); fixture +15 d40 |
| A7 | undeclared `Rec` in a codeunit without TableNo is not implicit-rec | regen the l2 snapshot + r1a goldens for `ws-argument-bindings` (only `CallVariations/cs3`: `implicit-rec` → `unknown`, `sourceVariableName` dropped) | restore `unwrap_or(true)` |
| A8 | L4 parameter roles unchanged | `aldump --r3a2-summary-core` over `ws-shapes` + `ws-readers` byte-identical before vs after | — (pin) |

**False-positive shapes.** d39 on a page opened in view mode (the edit-mode write is a runtime fact → a missed real case, not an FP). d39 on a temporary `Rec` behind an `IsTemporary` guard (`WorkflowStepInstance.BuildTempWorkflowTree`, FP). d39 in an OnOpenPage that hands `Rec` on to be inserted later (`Certificate`, FP). d42 when `SetLoadFields` has no reload (same as a local). d41/d42 do not check temporary state.

**Measurement (done).** `cdo-measure` before/after at both scopes + explicit opt-in d40 runs; Base App as its own workspace (`--single-app --scope all`); per-fingerprint delta with verdicts for every added/removed finding (`rev2/delta/VERDICTS.tsv`, 216 rows). CDO (cross-app): primary 2,323 → 2,323 (0 fingerprints moved); `--scope all` 30,800 → 30,618, only d37 345 → 163 (182 removed, 0 added); d39 3, d41 0, d42 2 unchanged; opt-in d40 143 → 143 identical at both scopes; `alsem fingerprint` byte-identical. Base App: 74,035 → 74,027 — d39 351 → 363, d42 78 → 79, d11 2,341 → 2,321, d37 1,401 → 1,400; d21 2,569, d33 185, d41 14 unchanged; opt-in d40 4,618 identical. Triage: d39 +12, all read — 10 REAL (`Assembly Quotes`/`Assembly Orders` OnAction → `ShowItemAvailabilityFromAsmHeader(Rec, …)` on `Editable = false` lists: picked values are discarded, as measured), 2 FP (`WorkflowStepInstance.BuildTempWorkflowTree`, `Certificate.OnOpenPage`) → **17% FP, below 30% → ships default**; d42 +1 REAL (`VATEntry.SetGLAccountNoWithResponse`); removed: d11 −20 (17 `Scan*`, 2 `LogLastError`, 1 `UpdateLineBalance`) and d37 −1 (`SegmentLine.StartWizard2`), all verified FPs; 340 Base App d39 exempted by the rule (222 page OnAction, 40 page OnLookup, 33 table field OnValidate, 14 page OnValidate, …), 6 read at random, all correctly exempt. CDO d37 −182: 22 read, the rest classed mechanically (182/182 Base App Table/TableExt procedures validating `Rec`, none a trigger); the cross-app vs single-app gap is measured — the d37 debug print shows 221 `no-callee` + 1 `binding-not-resolved` suppressions and 181/182 are still reported single-app: conservatism from a missing dependency-caller edge (D2), not proof of FP; still D1-class.

**BC probe** (BC 28.4 Linux sidecar, app `/work/bench/272/bcprobe`, TestPage runs; sources `rev2/bcprobe-Probe*.al`, readings `rev2/bcprobe-run1.txt`): a helper doing only `R.Validate(...)` on a `var` record, no Modify, re-read with a fresh `Get`. **Persisted:** editable card in edit mode — OnAction, a page procedure run from an action, field OnValidate / OnLookup / OnAssistEdit / OnDrillDown (at close or record leave), OnNewRecord, OnInsertRecord, OnModifyRecord; tableext OnBeforeInsert / OnBeforeModify. **Lost:** `Editable = false` page OnAction, `ModifyAllowed = false` page OnAction, an editable page opened in view mode, OnAfterGetRecord, tableext OnAfterInsert / OnAfterModify. Not measured: pageext, other list pages, the real client's default mode, Rename triggers, a page procedure called from elsewhere; a Windows Cronus confirmation is owed (CLAUDE.md).

**Pre-stated fallbacks.** (a) d39, d41 or d42 > 30% FP on the triaged new findings: find the root cause; if not fixable in scope, gate that detector's implicit-rec binding behind opt-in (measured: d39 2/12 = 17%, d42 0/1, d41 0 new → no gate). (b) A later BC measurement contradicts a row of the trigger table: move that trigger to the reporting side and re-triage (the lists are named constants citing the measurement). (c) Any golden other than the two `ws-argument-bindings` l2 goldens moving: triage line by line before any regen (measured: none moved).

**Not delivered (reconciled with the issue's "the same d39–d42 findings as a local").** d41, d42: delivered (same findings and loose fingerprint; `xRec` subject `?`). d39: delivered with exemptions, by design — a local belongs to the routine; an implicit `Rec` belongs to the caller (table procedures, field triggers) or is written by the platform (table triggers, tableext OnBefore*, editable-page triggers and procedures, measured); reporting it "like a local" would add 340 Base App findings (6/6 sampled are persisted) — a root-cause rule backed by a BC measurement, not an opt-in demotion. d40: not delivered — C2(a) stays ("not loaded before the call" is false for a platform- or caller-loaded `Rec`); deleting C2(a) would add 3,199 Base App d40 findings (4,618 → 7,817) and 52 on CDO (143 → 195), dominated by OnAction (866), table procedures (605), table field OnValidate (448), page procedures (399); a per-trigger d40 rule is a separate design (D3).

Discoveries (pre-existing, for the freeze): D1 (+O8) d37 and d11 still fire on `Rec` in table procedures where the caller owns the record (Base App single-app d37 1,400, d11 2,321; CDO `--scope all` d37 163); a codeunit-`TableNo` `Rec` is bound `local`, so d39 still treats it as codeunit-owned, but `Codeunit.Run(X, Rec)` hands it back. D2 in cross-app mode the `PrimaryGraph` holds workspace callers only, so for a dependency caller's call d37's `site_pick` finds no callee → `suppress-unknown` (the CDO −182 vs single-app −1). D3 d40's C2(a) also skips `Rec` in page OnOpenPage, where the platform has NOT yet loaded it (135 Base App candidates, not triaged).

Not checked (rev 2): the Base App L4 role pin (`aldump --r3a2-summary-core ws-baseapp` OOM-killed after 5.5 min; rests on byte-identical fixture summaries + fully explained deltas); 160/182 CDO d37 removals have a class verdict only; the delete-and-rename final shape of the guard (the fold was tested by breaking class B, which moved no finding); pageext base-page lookup for a symbol-only read-only base page; D3's 135 candidates; BC coverage gaps above.

Probe-table corrections (rev 2): the "unknown-table case" row is superseded — a page WITHOUT SourceTable gets no synthetic `Rec` and its `Rec` argument is `unknown` (ir_walk.rs:2124-2146); a SourceTable that differs between `#if` branches also gives `None` (workspace.rs:819-824); d42's PK/FlowField exclusion uses the callee parameter's table (d42.rs:208-225). Added: a scalar local named `Rec` did not block the synthetic `Rec` row (fixed by change 2). The v1 "d39 ≈100% FP" claim is withdrawn (see Measurement). CDO d39 is 3 even with the exemption disabled: CDO has no implicit-rec d39 candidates, so Base App is the population for the d39 rule.

## Design revision 1 (superseded by revision 2 above)

**Goal.** A table, tableext, page (with SourceTable) or pageext implicit `Rec`/`xRec` passed as a bare argument binds like a declared record variable, so d41 and d42 report it the way they report a local record.

**Non-goals.** d39 does not report an implicit Rec (triage: never the owner). d40 keeps its implicit-rec skip (the platform or the caller loads Rec). No L4 role for the receiver (a table procedure dirtying the caller's `Cust.Foo()` record stays a known gap). No loose-subject table for `xRec` (stays `?`).

**Changes.**
1. `calls.rs` `initial_binding_state`: `source_kind == "implicit-rec"` starts as `"unresolved-callee"` so it can be upgraded.
2. `ir_walk.rs`: an exact label — `rec` is implicit only when it resolves to the synthetic Rec (no table, not a parameter); `xrec` only when undeclared and that synthetic Rec exists; everything else `"unknown"` (lets the G-8 global backfill rebind a codeunit global named `Rec`).
3. `d39.rs`: a named skip `implicitRec` before the source-kind filter; drop `"implicit-rec"` from the admitted kinds; delete `is_auto_persist_trigger_rec` (this skip covers all it covered; d39 was its only user). Reason: a table/tableext Rec belongs to the caller (procedures, field triggers) or is persisted by the platform (table-level triggers); a page/pageext Rec is persisted by the page (G-9, as d37 already assumes).
4. CHANGELOG entry; d39/d41 docs where they describe implicit Rec.

**Acceptance matrix.**

| # | requirement | test | break that must fail it |
|---|---|---|---|
| A1 | table procedure `ResetIt(Rec)` gives d41, same loose fingerprint as the local | rename `d41_implicit_rec_binding_is_not_reported` → `…_is_reported`; 2 findings (ViaRec, ViaLocal) | revert change 1 |
| A2 | d41 on page OnAction (SourceTable), pageext, tableext, bare `SetRange` | new cli test over `ws-shapes` | revert change 1; `implicit_rec_var` always None |
| A3 | d42 on table Rec and page Rec | new test | revert change 1 |
| A4 | xRec forwarded in a field OnValidate gives d41 | new test | `"xrec"` arm → `false` |
| A5 | d39 never fires on an implicit Rec (table proc, field OnValidate, table-level trigger, page triggers, tableext OnBefore/OnAfter); a local still fires | new test + `table_level_trigger_rec_ops_are_suppressed` (now really guards) | delete the d39 skip (v1: 6 findings on the fixture) |
| A6 | d40 still skips implicit Rec | r4 ws-d40 golden (`TouchHelper(Rec)`) | delete the C2(a) skip (to run at implementation) |
| A7 | undeclared `Rec` in a codeunit without TableNo is not implicit-rec | regen the l2 snapshot + r1a goldens for `ws-argument-bindings` (triaged: exactly the mislabel fix) | restore `unwrap_or(true)` |

**False-positive shapes.** d39 on any implicit Rec (the skip). d42 when `Rec.SetLoadFields` is not followed by a reload (same imprecision as locals; none in the population). A non-editable page's Rec (a d39 finding could be real → FN risk). tableext `OnAfterModify`/`OnAfterInsert` really lose the dirt (the skip hides a rare real case, FN). d41/d42 do not check temporary state.

**Measurement plan.** `cdo-measure` before/after at both scopes + a d40 run with `--detector`; a second population: Base App source as its own workspace (`/work/bench/272/ws-baseapp`, `--single-app`); triage every added or removed d39–d42 finding plus the side effects on d37 and d11.

**Pre-stated fallbacks.** (a) If d41, d42 or d39 shows > 30% FP among new findings: find the root cause (as done for d39); if not fixable in scope, gate the implicit-rec binding for that detector behind opt-in. (b) If the page-persists assumption is disproved on BC: narrow the d39 skip to table objects and re-triage page findings. (c) Any golden other than the two `ws-argument-bindings` l2 goldens moving: triage line by line before any regen.

Discoveries (pre-existing, for the freeze): D1 d37 (and very likely d11) still fire on `Rec` in table procedures where the caller owns the record — 145 such d37 findings remain on CDO `--scope all`. D2 d37 drops more on CDO cross-app (−182) than on Base App single-app (−1); guessed cause (CDO subscribers making publishers' persistence unknown) unverified.

## Timeline

- 21:08 claim #272 attempt 1, worktree at `a6d94a91`. Probes + design dispatched (opus).
- ~22:00 classified bounded; design revision 1 in `## Design`.
- Spec panel r1 (spec_rounds 1 of 5): sol **rework** (S1–S4 important, S5 minor; `spec-r1-sol.md`); opus approve with fixes (O1–O3 important, O4–O8 minor; `spec-r1-opus.md`). Both agree the binding repair is right; the dispute is the d39 policy. Register written (13 entries, all `open`).
- PAUSED for the operator's 3-session restart (relayed by alsem-main). The revision-2 design author was stopped before writing anything (it had started the BC sidecar). Resume point and its full brief: `/work/alsem/.agent/runs/20261024-210819-3b21e9/resume-272.md`. Next step: design revision 2 (fresh opus author), then spec round 2 with FULL briefings (pi thread gone).
- Resumed after the restart (alsem-main verified session + run id). Design revision 2 (opus): BC 28.4 probe, evidence-backed d39 exemptions, label from availability + per-name declaration, ws-readers controls, VERDICTS.tsv, reconciled Not delivered. Register: all 13 `fixed` (rev 2 anchor).
- Spec panel r2 (spec_rounds 2 of 5, full briefings): sol **rework** — accepted S3, S5, O1, O2, O3, O5–O8; re-raised S1, S2, S4, O4; new N1 (important: object-global `Rec` still leaves a phantom synthetic record) and N2 (important: the probe's Editable/ModifyAllowed runs used OpenView, confounded with view mode; the 10 REAL d39 are read-only LIST actions, unmeasured; Windows confirmation owed). Opus approve with fixes — all 13 accepted; minors N1–N6 (`spec-r2-opus.md`). Design revision 3 dispatched to the same author.
- Design revision 3 pasted (rev 2 kept below as superseded); register 21 entries (S6, S7, O9–O14 new), marks reset. Spec panel r3 (spec_rounds 3 of 5, diff-only): sol approve, all 21 accepted; Opus approve, all 21 accepted, new minor O15 (global-shadow check also reaches codeunit-TableNo / report dataitems). Change 2 amended for O15. Spec panel r4 (spec_rounds 4 of 5, diff-only): both approve, all 22 accepted → **converged**.
- Acceptance tests dispatched (opus); cargo builds held while alsem-second's #277 CDO bench runs (coordination). Tests + fixtures written (`tests/fixtures/issue-272/`, not r0-corpus, so no golden family moves; A1 renamed; new `cli_implicit_rec_bindings`, `l2_implicit_rec_source_kind`, `r3a2_implicit_rec_pin`), not yet compiled; failing output pending the build hold.
- Build hold lifted ("walls done" from alsem-second). Red on base (logs `/work/bench/272/tests/{cli-new,cli-a1,l2,r3pin}.log`): A1 FAIL (only ViaLocal d41); A2 FAIL (8 implicit-Rec d41 rows missing, incl. ViaGlobalRec, mislabelled implicit-rec on master); A3 FAIL (only D42ViaLocal); A5 FAIL (only D39ViaLocal); A4 FAIL (extra d11 D11Loaded, d21 D21Loaded, d33 D33Filtered, d37 D37Arity/D37Persists/D37Unresolved = the prototype's before set); A2b L2 ws-shapes FAIL (ShadowScalar, ViaGlobalRec `implicit-rec`), ws-gshadow FAIL (4 × `implicit-rec`), model FAIL (`implicit-rec` not `global`); A8 ×2 and the d40 test PASS (pins).
- Plan: `docs/superpowers/plans/2026-10-24-issue-272-implicit-rec-bindings.md`, 2 tasks (plan_tasks charged 2). Bounded with ≤2 tasks → no per-task reviewer.
- Implementation (opus): 811a85c2, both tasks in one commit (Task 1 alone leaves A5 red under the hook); FIXTURE_ROOTS tripwire 39 → 42. A7 regen moved only the two `ws-argument-bindings` l2 goldens (cs3). Builds paused once for alsem-second's #277 wall bench (alsem-main load report), resumed throttled. Task gate `ci-steps task` exit 0. Pushed; draft PR #298.
- Final panel r1 (final_rounds 1 of 5): sol approve with fixes (FS1–FS4 minor), Opus approve with fixes (FO1–FO2 important, FO3–FO7 minor), code-review skill at high (FC1–FC9). FC3, FC6 refuted. Fixes 85e60d10 (FIXTURE_ROOTS 42 → 43, new ws-owner fixture, proofs Q1–Q6).
- Final panel r2 (2 of 5): Opus approve, all accepted; sol accepted all but re-raised FC4 (name-only delete exemption). Fix 91f62e0c (proof Q7). Final r3 (3 of 5): both accept FC4.
- Final-head re-measure (91f62e0c): 0 fingerprint differences vs prototype; CHANGELOG wording 4d040f8a (FO5). Final r4 (4 of 5): Opus accepts FO5 (raiser-only rule; sol accepted-by-rule). Register converged.
- Branch gates on 4d040f8a, all exit 0 and supervised: ci-steps-all (`logs/ci-steps-all/20261025T035551-56a08312.log`), check-goldens-coverage (`…/20261025T040118-c0b6de1d.log`), check-goldens (skipped on the tree stamp: this exact tree passed `ci-steps test`; `…/20261025T040119-22e53e10.log`), cdo-gate PASS (`…/20261025T040119-4f7f4a41.log`). origin/master unmoved (a6d94a91): no rebase, no range-diff. B = a6d94a914c94a5babb84c65abaee1ffd2cc7986c, H = 4d040f8af6f2c40330e793afa19fcb14cfa48257. check-diff B..H: no reasons.
- Freeze: `sanitize` flagged two local CDO paths (Design Measurement paragraph, discovery 2 reproducer); both redacted to `the pinned CDO baseline` / `<CDO_WS>`. Wording only — the Design's reviewed anchor hash (632e217f…) predates this redaction; no mark reset.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| A1 `d41_implicit_rec_binding_is_reported`, A2 `shapes_d41_reports_the_implicit_rec`, A3 `shapes_d42_reports_the_implicit_rec`, A4 `readers_see_the_implicit_rec` | P1 revert change 1 (`calls.rs` drop `\|\| source_kind == "implicit-rec"`) | all four FAIL (A2: 7 implicit-rec d41 rows gone, only ShadowDeclaredXRec/D41ViaLocal/ViaGlobalRec left; A3: only D42ViaLocal; A4: back to the before set +D11Loaded/D21Loaded/D33Filtered/D37Arity/Persists/Unresolved); A5 also FAILS (only D39ViaLocal) — `proofs/P1-revert-change1.log` | 17/17 ok — `proofs/restored-final.log` | 811a85c2 |
| A2b `shapes_rec_argument_source_kinds` | P2 local shadow back to record-only (`routine_declares(routine,"Rec")` → `out.iter().any(..Rec)`) | FAIL: `S Card ShadowScalar H.Num` = `local rec` (want `unknown -`); no other test moves — `proofs/P2-drop-local-shadow.log` | as above | 811a85c2 |
| A2b `gshadow_rec_argument_source_kinds`, `gshadow_global_record_rec_binds_its_own_table` | P3 drop the object-global part of `rec_declared` | both FAIL: all 4 gshadow sites `local rec`; model test: Table 50304 binding `source_kind: "local"` — `proofs/P3-drop-global-shadow.log` | as above | 811a85c2 |
| A2b (all three) | P4 ignore fact (b) (drop `&& !declared(&lc)`) | all three FAIL: 4 gshadow sites `implicit-rec`; ws-shapes ShadowScalar and ShadowDeclaredXRec `implicit-rec` — `proofs/P4-ignore-b.log` | as above | 811a85c2 |
| A7 `ir_l2_features_snapshot_over_r0_corpus`, `differential_l2_features_match_goldens` | P5 restore old label `rec_var.map(..table_name.is_none()).unwrap_or(true)` | both FAIL on `ws-argument-bindings` CallVariations cs3 (golden `unknown`, rust `implicit-rec` + `sourceVariableName` "rec"); A2b also FAILS — `proofs/P5-restore-unwrap-or-true.log` | as above | 811a85c2 |
| A5 `shapes_d39_owner_rule`, `gap_audit_b_table_triggers::table_level_trigger_rec_ops_are_suppressed` | P6 `implicit_rec_persisted_by_owner` returns false | both FAIL: A5 +11 d39 (S Card OnAction/OnModifyRecord/OnValidate/PageProcDirty, S Card Ext2 OnAfterAction, S Cust D39ViaRec/OnModify/OnValidate, S Cust Ext OnBeforeModify, S Cust Ext2 OnAfterDelete/OnBeforeRename) — `proofs/P6-owner-rule-false.log` | as above | 811a85c2 |
| A5 `shapes_d39_owner_rule` | P7 drop `&& !reset_after_call()` from the implicit-rec skip | FAIL: `S Cust Ext2 ExtReloadAfter` missing — `proofs/P7-drop-reload.log` | as above | 811a85c2 |
| A5 `shapes_d39_owner_rule` | P8 page arm `(!is_trigger \|\| named(..))` → `named(..)` (page procedure not exempt) | FAIL: extra `S Card PageProcDirty` — `proofs/P8-page-proc-not-exempt.log` | as above | 811a85c2 |
| A5 `shapes_d39_owner_rule` | P9 `page_may_modify` ignores `modify_allowed` | FAIL: `S NoMod Card OnAction` missing — `proofs/P9-ignore-modifyallowed.log` | as above | 811a85c2 |
| A5 `shapes_d39_owner_rule` | P10 pageext reads its own properties (`"PageExtension"` arm made unreachable) | FAIL: `S RO List Ext OnAction` missing — `proofs/P10-pageext-own-props.log` | as above | 811a85c2 |
| A6 r4 `differential_r4_findings_match_goldens` (ws-d40), `shapes_d40_still_skips_the_implicit_rec` | P11 d40 C2(a) skip made unreachable | both FAIL: ws-d40 findingCount 2 → 3 (golden byte mismatch); ws-shapes 1 → 22 d40 rows (+21) — `proofs/P11-delete-c2a.log` | as above | 811a85c2 |
| A8 `r3a2_implicit_rec_pin` (×2) | none (a pin: L4 summary core byte-identical before/after; passed on the base too) | — | ok in `proofs/restored-final.log` | 811a85c2 |
| F1 `owner_d39_temp_table_and_delete_record` | Q1 TableType override does not mark implicit-rec bindings (`is_temporary && false`) | FAIL: extra `O Temp Card OnAfterGetRecord L76` — `proofs/Q1-F1-no-temp-binding.log` | 18/18 ok — `proofs/r1-restored-final.log` | 85e60d10 |
| F2 `owner_d39_temp_table_and_delete_record` | Q2 `PAGE_TRIGGERS_REC_DELETED` arm disabled | FAIL: extra `O Card OnDeleteRecord L46` — `proofs/Q2-F2-no-delete-exempt.log` | as above | 85e60d10 |
| F3 `gshadow_rec_argument_source_kinds` (O15 rows) | Q3 drop the `own_record_rec &&` gate in `shadows_implicit_rec` | FAIL: `G TableNo OnRun` and `G Report OnAfterGetRecord` `local rec` → `unknown -` — `proofs/Q3-F3-drop-o15-gate.log` | as above | 85e60d10 |
| F6 `shapes_d41_reports_the_implicit_rec` (line identity) | Q4 fixture: swap the P1/P2 OnAction bodies in `ws-shapes/src/Page.al` (same line count; `(object, routine)` rows unchanged) | FAIL: `S Card OnAction` L32 → L41 (the old row key would have passed); the A8 pin fails too (fixture changed) — `proofs/Q4-F6-swap-onaction-bodies.log` | as above | 85e60d10 |
| F5 `shapes_d39_owner_rule` | Q5 base-page lookup misses (`object_by_type_name("Page-BREAK-Q5", ..)`) | FAIL: `S RO List Ext OnAction` missing — `proofs/Q5-F5-base-page-not-found.log` | as above | 85e60d10 |
| F4 `shapes_d39_owner_rule` | Q6 (P7 re-run after the reorder) drop `&& !reset_after_call()` | FAIL: `S Cust Ext2 ExtReloadAfter L33` missing — `proofs/Q6-F4-drop-reload-after-reorder.log` | as above | 85e60d10 |
| FC4 `owner_d39_temp_table_and_delete_record` | Q7 drop `is_trigger &&` from the `PAGE_TRIGGERS_REC_DELETED` arm | FAIL: `O RO Card OnDeleteRecord L91` missing — `proofs/Q7-FC4-drop-is-trigger.log` | 18/18 ok — `proofs/r2-fixed.log` | 91f62e0c |

## Findings register summary

42 entries. Spec panel (22: S1–S7, O1–O15): all `fixed`, accepted by sol and Opus (converged at spec round 4 of 5). Final panel (20: FS1–FS4 sol, FO1–FO7 Opus, FC1–FC9 code-review skill at high): 18 `fixed`, FC3 and FC6 `refuted` (Design non-goal / disclosed, measured conservative suppression; both accepted by both reviewers). FO5 (CHANGELOG numbers) closed by raiser-only acceptance: Opus `accepted`, sol `accepted-by-rule`, fix_commit 4d040f8a (CHANGELOG only). Final rounds used: 4 of 5. Fix commits: 85e60d10 (r1 fixes), 91f62e0c (FC4), 4d040f8a (FO5).

## Gate results

- `ci-steps task` on 811a85c2: exit 0 (4220 tests). `task-affected` on 85e60d10 and 91f62e0c: whole suite 4221/4221, no golden moved.
- Final-head re-measure (91f62e0c, release-fast; `/work/bench/272/final/SUMMARY.md`): CDO (bc3ccb18, clean) primary 2,323 → 2,323; `--scope all` 30,800 → 30,618 (only d37 345 → 163); opt-in d40 143 → 143; Base App single-app 74,035 → 74,027 (d39 351 → 363, d42 78 → 79, d11 2,341 → 2,321, d37 1,401 → 1,400); opt-in d40 4,618 → 4,618. 0 fingerprint differences vs the rev3 prototype on every run; `alsem fingerprint` byte-identical before vs final. One stat moved (Base App d39 `implicitRecOwnerPersists` 341 → 340, `tempRecord` 256 → 257, FC1), no finding.

## Discoveries

```json
[
  {
    "subsystem": "l5 detectors d37/d11 (caller-owned Rec)",
    "locator": "src/engine/l5/detectors/d37.rs, src/engine/l5/detectors/d11.rs; src/program/body/ir_walk.rs (codeunit-TableNo Rec bound local)",
    "symptom": "d37 and d11 still fire on Rec in table procedures, where the caller owns the record (Base App single-app: d37 1,400, d11 2,321; CDO --scope all: d37 163). A codeunit-TableNo Rec is bound 'local', but Codeunit.Run(X, Rec) hands it back to the caller.",
    "kind": "bug",
    "origin_issue": 272,
    "reproducer": "alsem analyze <Base App ws> --single-app --scope all: d37/d11 findings whose routine is a Table/TableExtension procedure acting on Rec.",
    "pre_existing": true,
    "capability": "An ownership rule for d37/d11 like d39's implicit_rec_persisted_by_owner, triaged on Base App.",
    "acceptance": "d37/d11 findings on caller-owned Rec in table procedures are triaged; the false-positive class is removed or justified; CDO primary unchanged unless triaged."
  },
  {
    "subsystem": "cross-app PrimaryGraph (dependency callers)",
    "locator": "src/engine/l5/detectors/d37.rs (site_pick); the cross-app PrimaryGraph build",
    "symptom": "In cross-app mode the PrimaryGraph keeps edges for workspace callers only, so a dependency caller's call has no callee and d37 returns suppress-unknown (221 no-callee + 1 unresolved on CDO). This is the whole CDO d37 −182 under #272; 181 of those are still reported on Base App single-app.",
    "kind": "bug",
    "origin_issue": 272,
    "reproducer": "Compare d37 at alsem analyze <CDO_WS> --scope all with the same Base App findings in --single-app mode; debug d37's site_pick for 'no-callee'.",
    "pre_existing": true,
    "capability": "Dependency-internal call edges available to detectors that judge dependency code in cross-app mode, or an explicit 'out of graph' verdict instead of suppress-unknown.",
    "acceptance": "d37 on dependency code gives the same verdict cross-app as single-app for the same source; CDO primary unchanged."
  },
  {
    "subsystem": "l5 detector d40 (C2(a) implicit-rec skip)",
    "locator": "src/engine/l5/detectors/d40.rs (C2(a))",
    "symptom": "d40's C2(a) skips every implicit Rec, including Rec in a page OnOpenPage, where the platform has not loaded it yet (135 Base App candidates, not triaged). Deleting C2(a) outright adds 3,199 Base App and 52 CDO findings, dominated by platform- or caller-loaded Rec.",
    "kind": "enhancement",
    "origin_issue": 272,
    "reproducer": "Delete C2(a) in d40 and run alsem analyze --detector d40-transitive-load-missing on Base App --single-app --scope all; filter the added findings to OnOpenPage.",
    "pre_existing": true,
    "capability": "A per-trigger d40 rule for the implicit Rec (loaded vs not yet loaded by the platform), triaged on Base App.",
    "acceptance": "d40 reports implicit-Rec sites where Rec is provably not loaded (e.g. OnOpenPage), with a triaged FP rate under 30%; other implicit-Rec sites stay skipped."
  },
  {
    "subsystem": "ABI ingestion (page Editable / ModifyAllowed)",
    "locator": "src/program/model/abi_rows.rs:56-63 (editable/modify_allowed None); src/engine/l5/detectors/mod.rs page_may_modify",
    "symptom": "Dependency (symbol-only) pages carry no Editable/ModifyAllowed, so d39's read-only-page rule never fires for a pageextension of a Base App page; the base page counts as modifiable. The Base App 28.4 SymbolReference.json does carry them (Editable on 977 of 2,616 pages, ModifyAllowed on 301; e.g. Assembly Quotes / Assembly Orders Editable '0').",
    "kind": "enhancement",
    "origin_issue": 272,
    "reproducer": "A pageextension of 'Assembly Orders' whose OnAction calls a helper that Validates Rec without Modify: d39 skips it (implicitRecOwnerPersists), although the base page is Editable = false.",
    "pre_existing": true,
    "capability": "ABI ingestion reads page Editable and ModifyAllowed from SymbolReference Properties into the model's page rows.",
    "acceptance": "A pageextension over a read-only dependency page reports d39 like one over a read-only workspace page (fixture with a symbol-only base page); ABI vectors and goldens updated only for the new fields."
  },
  {
    "subsystem": "L4 roles (table-procedure receiver)",
    "locator": "src/engine/l4/summary_runner.rs (roles built only for routine.parameters); src/engine/l5/detectors/mod.rs implicit_rec_persisted_by_owner",
    "symptom": "d39 exempts a table procedure's Rec because 'the caller persists Rec', but nothing judges that caller: the receiver Rec has no parameter role, so a codeunit calling Cust.Foo() where Foo dirties Rec and never calling Cust.Modify() is reported nowhere.",
    "kind": "enhancement",
    "origin_issue": 272,
    "reproducer": "Codeunit: Cust.Get(..); Cust.Foo(); (no Modify) with table procedure Foo() { Helper(Rec); } and Helper validating its var record: no d39 finding at either level.",
    "pre_existing": true,
    "capability": "An L4 receiver role (dirtReturnedToCaller for the implicit Rec of a table procedure), so d39 can judge the caller of a member call.",
    "acceptance": "The reproducer reports d39 on the codeunit caller; Base App triage of the new findings under 30% FP; CDO primary unchanged unless triaged."
  },
  {
    "subsystem": "d39 owner rule — platform confirmation",
    "locator": "src/engine/l5/detectors/mod.rs PAGE_TRIGGERS_REC_PERSISTED, TABLEEXT_TRIGGERS_AFTER_WRITE; /work/bench/272/rev3/bcprobe-src/",
    "symptom": "Every BC persistence row behind d39's implicit-Rec owner rule was measured only on the BC 28.4 Linux sidecar via TestPage; CLAUDE.md requires a Windows Cronus confirmation. If read-only list pages persist there, the 10 real Base App d39 findings become false positives (fallback (a) then gates the binding opt-in).",
    "kind": "enhancement",
    "origin_issue": 272,
    "reproducer": "Publish the #272 probe app (rev3/bcprobe-src, codeunit 71020) to a Windows Cronus BC 28 and compare with rev3/bcprobe-results.txt.",
    "pre_existing": false,
    "capability": "The probe rows confirmed (or corrected) on Windows Cronus, with the trigger constants and CHANGELOG updated accordingly.",
    "acceptance": "Each row of the #272 BC table has a Windows Cronus result; any disagreement moves that trigger/property to the reporting side and re-triages Base App."
  }
]
```

## Not delivered

- d40: C2(a) kept; the issue's "same d40 findings as a local" is not delivered (Design "Not delivered"; discovery 3).
- d39: delivered with owner exemptions by design; known runtime-only false negative for a modifiable page opened in view mode; a pageextension of a dependency page always counts as modifiable (discovery 4).
- Windows Cronus confirmation of the BC rows: owed (discovery 6).
