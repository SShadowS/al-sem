# B3 Phase A: CDO triage verdicts

**Result: 0 regressions, 0 unexplained.** Every difference in [cdo.md](cdo.md) has a verdict
below. 21 opt-in d40 rows remain as stated limits or by-design reports (section 4). They are
not regressions.

## 1. What was compared

- **Harness.** `aldump --b3 U:/Git/DO-cdo-baseline/Cloud --b3-triage docs/b3-triage/cdo.md`
  (`release-fast`, `ALSEM_NO_PREFLIGHT_CACHE=1`). Every detector, opt-in ones included, runs
  twice over one L3 model. The "old" run uses L3's call resolution. The "new" run uses the
  program engine's resolution, turned into L3's call-edge shape by the adapter
  (`src/engine/l3/program_calls.rs`).
- **Corpus.** CDO, the pinned DO worktree at commit `bc3ccb18` (clean tree).
- **Engine.** `feat/b3-phase-a` at `68e24183` (fixes 5a-5e, plus Task 6: the adapter's
  stage 2, dependency-callee bindings; see section 2c).
- **DO.** DO (`U:/Git/DO/Cloud`) is at the same commit as CDO. Its harness output is identical
  apart from the title (see [do.md](do.md); re-checked 2026-10-05 at `68e24183`). These
  verdicts cover DO too.
- **How verdicts were reached.** The first wave (triage files A-E) judged every call-site
  row by category and a findings-level sample per detector. Fixes 5a-5e then closed every
  regression that wave found. This pass re-ran the harness, compared the new table with the
  one committed at Task 4 row by row, and re-triaged every row that is new or whose text
  changed. Rows that did not change keep their first-wave verdict.
- **The category rule.** Detectors are unchanged code. So a finding difference is a fix when
  the call-site changes behind it are fixes and the detector's reaction follows from them.
  The findings-level checks below test that the reaction follows.
- Triage files live in `.superpowers/sdd/2026-10-04-b3-phase-a/` (gitignored, local only):
  `triage-A.md` .. `triage-E.md`, and `task-5a-report.md` .. `task-5e-report.md`.

### Movement since the Task 4 table

| measure | Task 4 | now |
|---|---:|---:|
| findings, old side | 2361 | 2237 |
| findings, new side | 2561 | 2402 |
| differences removed / added / changed | 72 / 272 / 416 | 89 / 254 / 515 |
| differing call sites | 763 | 847 |

The old side moved too, because fixes 5c-5e changed detectors and L4 facts for both runs.
Task 6 moved only the site count (791 -> 847: 56 binding-only sites, section 2c); the
findings and the difference rows are the same as after fix 5e.

Difference rows by detector (Task 4 -> now):

| detector | kind | Task 4 | now | why it moved |
|---|---|---:|---:|---|
| d1-db-op-in-loop | added | 118 | 135 | +17 from 5a's new overload picks |
| d1-db-op-in-loop | changed | 372 | 465 | +91 from 5a picks; +2 were "removed" (InitNewEntry) |
| d1-db-op-in-loop | removed | 2 | 0 | 5a: InitNewEntry picks the Integer overload, as L3 did |
| d14-dead-routine | removed | 68 | 68 | - |
| d16-obsolete-routine-call | added | 4 | 6 | +2 from 5a picks |
| d21-read-without-load | removed | 1 | 1 | - |
| d3-missing-setloadfields | changed | 4 | 4 | - |
| d34-commit-in-loop | added | 19 | 22 | +3 from 5a picks |
| d35-commit-in-event-subscriber | added | 2 | 2 | - |
| d39-record-left-dirty-across-chain | added | 6 | 0 | 5d, 5e |
| d40-transitive-load-missing (opt-in) | added | 55 | 21 | 5e (40 gone, 6 new) |
| d40-transitive-load-missing (opt-in) | removed | 0 | 19 | 5e owner rule |
| d45-event-transitive-table-exposure | added | 3 | 0 | 5c |
| d46-commit-in-lifecycle | changed | 1 | 1 | - |
| d47-io-unsafe-txn | added | 0 | 2 | 5a picks |
| d48-io-in-loop | added | 58 | 58 | - |
| d48-io-in-loop | changed | 9 | 16 | +7 from 5a picks |
| d8-commit-in-transaction | added | 6 | 5 | 5c removed the eSeal :25 row |
| d8-commit-in-transaction | changed | 11 | 13 | +2 from 5a picks |
| d9-transaction-span-summary | added | 1 | 3 | 5c removed eSeal :18; 3 rows moved from "changed" |
| d9-transaction-span-summary | changed | 19 | 16 | 3 rows moved to "added" |
| d9-transaction-span-summary | removed | 1 | 1 | - |

Row-level: 539 difference rows are byte-identical to Task 4. 51 are gone, 149 are new, and
170 kept their key but changed kind or text. Unattributed rows: 3 -> 1.

## 2. Call sites, by category (847 rows)

Fixed means the new edge is right. Neutral means only details changed, with no effect on
what a detector can conclude.

| category | count | verdict | evidence | source |
|---|---:|---|---|---|
| Method/Unknown(RecordTableProcedure) -> Method/Resolved | 251 | 251 fixed | Every target is a `tableextension` procedure on the receiver's table; argument counts match; checked by script, all rows. | triage-A |
| Method/MemberNotFound -> Method/Resolved | 23 | 23 fixed | Target exists and is visible from the caller. All rows. | triage-A |
| Method/Unknown(UntrackedReceiver) -> Method/Resolved | 3 | 3 fixed | Receiver type read from source. All rows. | triage-A |
| Method/ExternalTarget -> Method/Resolved | 7 | 7 fixed | Target is workspace source. All rows. | triage-A |
| Direct/Ambiguous -> Direct/Resolved | 83 | 83 fixed | 72 first wave (probe printed each picked overload; each argument type looked up). 11 new from 5a: `InsertMergeFields` x10, `GetValue` x1; see below. | triage-B, this pass |
| Method/Ambiguous -> Method/Resolved | 61 | 61 fixed | 41 first wave. 20 new from 5a; see below. | triage-B, this pass |
| Method/Resolved -> Method/Ambiguous | 0 (was 3) | gone | The 3 `EMailLog.InitNewEntry` regressions. 5a now picks the `TemplateLineNo: Integer` overload (CDOEMailLog.Table.al:357), the same as L3, so the sites no longer differ. | task-5a |
| workspace-run-no-entry (new) | 55 | 55 fixed | See below. | triage-C, task-5b, this pass |
| Builtin/Builtin -> PageRun/Opaque | 10 (was 31) | 10 fixed | Literal runs of dependency pages (Sales Statistics, Error Messages, ...). The 21 workspace-page rows moved to workspace-run-no-entry. | triage-C |
| Builtin/Builtin -> ReportRun/Opaque | 0 (was 3) | moved | All 3 are workspace reports; now workspace-run-no-entry. | triage-C |
| external-object-receiver | 0 (was 31) | moved | All 31 are workspace pages; now workspace-run-no-entry. | triage-C, task-5b |
| Builtin/Builtin -> PageRun/Resolved | 12 | 12 fixed | Each target page has the `OnOpenPage` the edge lands on. All rows. | triage-C |
| Builtin/Builtin -> Dynamic/Unknown(DynamicObjectRunTarget) | 15 | 15 fixed | Variable object ids; really dynamic. All rows. | triage-C |
| PageRun/Resolved, details | 49 | 49 fixed | Old target was a wrong trigger (e.g. the first `OnAction`); new is `OnOpenPage`. All rows. | triage-C |
| PageRun/Opaque, details | 25 | 25 neutral | Same shape both sides; only the external type text differs. | triage-C |
| CodeunitRun/Opaque, details | 8 | 8 neutral | Same shape both sides; only details differ. | triage-C |
| Method/Unknown(UntrackedReceiver) -> Builtin/Builtin | 25 | 25 fixed | Receiver is a platform type; method is in the catalog. All rows. | triage-D |
| Method/MemberNotFound -> Builtin/Builtin | 22 | 22 fixed | Platform method, not a workspace member. All rows. | triage-D |
| Method/Unknown(CompoundReceiver) -> Builtin/Builtin | 10 | 10 fixed | Chained platform calls. All rows. | triage-D |
| Method/Unknown(EnumStatic) -> Builtin/Builtin | 1 | 1 fixed | Enum static method. | triage-D |
| Method/ExternalTarget -> Builtin/Builtin | 5 | 5 fixed | Platform method, not a dependency member. All rows. | triage-D |
| external-record-receiver | 67 | 67 fixed | Record of a dependency table; method checked in the `.app` symbols. All rows. 5 of them now also carry `dep-bindings-source` (section 2c). | triage-D |
| external-other | 56 | 56 fixed | Dependency or platform object. All rows. 5 of them now also carry `dep-bindings-source` (section 2c). | triage-D |
| trigger-routes-filtered, trigger-beyond-l3 | 3 | 3 fixed | `tableextension` triggers L3 never modelled. All rows. | triage-D |
| external-object-receiver + dep-bindings-source (binding-only, new) | 56 | 56 fixed | Same edge both sides (`Method/ExternalTarget`, same external type); only the bindings differ: L3 `unresolved-callee`, adapter `resolved` with the dependency routine's `var`-ness. See 2c. | task-6 |
| **total** | **847** | **814 fixed, 33 neutral, 0 regression, 0 unexplained** | | |

### 2a. The 31 new overload picks (fix 5a)

5a decides an overload only on the positions where the candidates differ. These 31 sites
were Ambiguous before and now resolve. Every pair of overloads here differs only in one
record parameter's table, so the argument's declared table decides. Checked in source:

- `InsertMergeFields` x13: overloads at CDOEMailTemplateLine.Table.al:1136 (`var CIDAttachment:
  Record "CDO E-Mail Template Attachment"`, obsolete) and :1155 (`"CDO Template Line Attachment"`).
  E.g. :795 passes `EMailTemplateAttachmentTemp: Record "CDO Template Line Attachment"` (:791)
  -> :1155. CDOEMailTemplLineReport.Table.al:297/300 pass `MergeFieldCIDAttachment: Record
  "CDO E-Mail Template Attachment"` (:291) -> :1136. CDODocumentFolderManagement.Codeunit.al:29
  passes a Template Line Attachment (:7) -> :1155.
- `GetValue` x6: overloads at CDOEMailTemplateMergeField.Table.al:217/228/239/244. Every
  caller passes a `"CDO Template Line Attachment"` with 5 arguments -> :244. E.g.
  CDOMergeTableTopBottom.Table.al:204 (`CIDAttachment` declared :174); the forwarder at :241
  passes its own `CIDAttachment` parameter (:239).
- `GetHtmlTable` x2: CDOEmailTemplateMergeTable.Table.al:176 vs :188; both callers pass a
  Template Line Attachment parameter -> :188.
- `CreateEDocLogEntries` x3, `CreateDOMailLogEntries` x4: CDOLogManagement.Codeunit.al
  overloads take `Record "CDO E-Mail Template Line"` (:33-:83) or `Record "CDO Document
  Template Line"` (:136-:214). Every caller passes a Document Template Line (e.g.
  CDOSendMailManagement.Codeunit.al:181, parameter declared :178); the argument count picks
  among the 10/11/12-argument forms.
- `FindEMailTemplateLine` x3: CDOEMailHandler.Codeunit.al:133 vs :148. CDOUnhandledSalesPagesMgt
  passes `DocumentTemplateLine: Record "CDO Document Template Line"` (:473, :659, :994) -> :148.

The 5a implementer also hand-checked all 44 new picks against source (`task-5a-report.md`).

### 2b. workspace-run-no-entry (fix 5b)

A run of a workspace page or report that has no entry trigger (`OnOpenPage`, `OnPreReport`).
The new edge is `PageRun/Opaque` or `ReportRun/Opaque` with no external type: the object is
ours, and there is no routine to land on. Old edges were `Builtin` (24) or `MemberNotFound`
(31). triage-C read every target file and found no entry trigger; its one caveat was that 31
of them were labelled ExternalTarget. 5b fixed that label. This pass re-checked 6, spread over
files: CDOCustomerCard.Page.al:251 -> "CDO Customer Calendar"; CDOVendorList.Page.al:58 ->
report "CDO Update Vendor Setup"; CDOEmailEditor.Page.al:744 -> "CDO Email Attachments Manage";
CDOUnhandledSalesOrder.Page.al:459 -> "CDO Handled Filter Warning"; CDOMergeTableField.Table.al:487
-> "CDO  Merge ML Texts"; CDOSetupWizard.Page.al:442 -> "CDO E-MailTemplateImportWorksh". None of
the six target files has an entry trigger.

### 2c. Dependency-callee bindings (Task 6, adapter stage 2)

The adapter now upgrades the argument bindings of an exact call into a dependency routine
with that routine's parameter `var`-ness (from its declaration for a source dependency;
from `AbiParams::Complete` for a symbol-only one). L3 never does: it has no dependency
routines, so its bindings stay `unresolved-callee`. On CDO this changes 66 sites, all into
source dependencies: 56 that differed from L3 only by bindings (the new row above), and 10
that already differed (5 `external-record-receiver`, 5 `external-other`).

Verdict: **fixed**. Evidence:
- The strings come from the callee's own declaration. Spot check:
  `CDOIssueDocument.Codeunit.al:18` `ReminderIssue.Set(ReminderHeader, ..)` becomes
  `0:var resolved`; Base Application's `Reminder-Issue.Set` declares
  `var NewReminderHeader: Record "Reminder Header"`.
- [cdo-deps.md](cdo-deps.md) runs the adapter with and without this stage alone: 66
  differing sites and **0 finding differences** for every detector. So no row in section 3
  moved because of it; only the attribution text of rows whose routines contain these
  sites now names `dep-bindings-source`.
- Sites the adapter leaves alone are counted, not shown as differences: 6 runs into a
  dependency page with no `OnOpenPage` (no routine stands behind them), and 0 with
  untrusted parameters (`Missing` / `CollapsedUntrusted`).

## 3. Findings, by detector (858 rows)

| detector | kind | rows | verdict | evidence |
|---|---|---:|---|---|
| d1-db-op-in-loop | added | 135 | fixed | 118 first wave (6 checked across 5 categories, all true). 17 new, see 3a. |
| d1-db-op-in-loop | changed | 465 | fixed | 372 first wave (6 shapes checked, 85 count-only rows scanned in full). Since then 93 rows are new (91 + the 2 InitNewEntry rows) and 145 kept their key but changed text; see 3b. |
| d14-dead-routine | removed | 68 | fixed | Each routine is now reached through a newly resolved call. All 68. (triage-E) |
| d16-obsolete-routine-call | added | 6 | fixed | 4 first wave. 2 new: CDOEMailTemplLineReport.Table.al:297/:300 now call the `[Obsolete]` `InsertMergeFields` overload (CDOEMailTemplateLine.Table.al:1135-1136). True. |
| d21-read-without-load | removed | 1 | fixed | triage-E. |
| d3-missing-setloadfields | changed | 4 | fixed | triage-E, all. |
| d34-commit-in-loop | added | 22 | fixed | 19 first wave (3 checked). 3 new: CDOElectronicDocumentMgt.Codeunit.al:120 and CDOLegacyeDocDispatcher.Codeunit.al:176 (call inside a `repeat`), CDOeDocumentsDispatcher.Codeunit.al:38 (`repeat DoDispatch` -> :143). Each reaches `CreateEDocLogEntries` -> `UsageMgt.LogEDocUsage` -> `LogUsage`, which runs `Commit()` (CDOUsageManagement.Codeunit.al:35). True. |
| d35-commit-in-event-subscriber | added | 2 | fixed | triage-E, all. |
| d40-transitive-load-missing (opt-in) | added | 21 | stated limits / by design | See section 4. Count re-verified in this table: 6 + 1 + 1 + 13. |
| d40-transitive-load-missing (opt-in) | removed | 19 | fixed | Old-side forwarders whose callers only the new resolution finds. The owner is now judged, so the requirement goes to the caller, which loads the record. E.g. CDOLogManagement.Codeunit.al:114 forwards `DocumentTemplateLine`; its callers (:29, :104) now resolve, and the routine at :29 loads the record with `GetV2Line` (:27) first. (task-5e) |
| d46-commit-in-lifecycle | changed | 1 | fixed | triage-E. |
| d47-io-unsafe-txn | added | 2 | fixed | New. CDOeDocumentsDispatcher.Codeunit.al:286 (`CopyStream`, already IO for d48 on both sides) now reaches a later `Commit()` through `CreateEDocLogEntries` -> `LogUsage` (CDOUsageManagement.Codeunit.al:35). The detector's IO rule is unchanged; the reachable commit is real. |
| d48-io-in-loop | added | 58 | fixed | triage-E (3 checked). |
| d48-io-in-loop | changed | 16 | fixed | 9 first wave (cappedBy only). 7 new, all cappedBy gains `dynamic-dispatch`; see 3b. |
| d8-commit-in-transaction | added | 5 | fixed | triage-E (3 carry the d8-design caveat, section 6). The 6th row (CDOeSealServiceMgt.Codeunit.al:25, a regression) is gone after 5c. |
| d8-commit-in-transaction | changed | 13 | fixed | 11 first wave. 2 new: CDOLegacyeDocDispatcher.Codeunit.al:229/:253, "writes 3 -> 4" tables: the transaction now includes the `CDO E-Mail Log` insert (CDOLogManagement.Codeunit.al:243, non-temporary record :216) through the resolved `CreateEDocLogEntries`. |
| d9-transaction-span-summary | added | 3 | fixed | CDOSendCustStatementMgt.Codeunit.al:17 (op7) and :77 (op6, op9). After 5c-r1 d9 needs 2 physical tables in the span. Old side: only `Customer.Modify` is visible. New side: at :17, `DOCustSetup.CreateAutomaticPeriodStatement` (tableext, CDOCustomer.TableExt.al:259) -> `CreateStatement` inserts `CDO Statement Journal Line` (declared :363, `Insert` :379) and sends mail; at :77 the resolved `CreateEDocLogEntries` adds the `CDO E-Mail Log` insert. The span really writes those tables before the commit. |
| d9-transaction-span-summary | changed | 16 | fixed | Same rows as the first wave. Both sides now count physical tables only (5c-r1), so the "writes N" numbers dropped on both sides. The new side still reaches more tables, for the reasons triage-E gave. The 1 unattributed row (CDODCPermAssignmentMgt.Codeunit.al:40) is the first-wave "fixed" row: the setup wizard's `Page.Run` now lands on `OnOpenPage`, which inserts. |
| d9-transaction-span-summary | removed | 1 | fixed | triage-E. |
| **total** | | **858** | **837 fixed, 21 d40 (section 4), 0 regression, 0 unexplained** | |

Gone since Task 4 (all were first-wave regressions): d1 x2 at CDOEMailLog.Table.al:378/:379
(now "changed" only: both sides find them), d39 x6, d45 x3 at CDOEvents.Codeunit.al:753,
d8 at CDOeSealServiceMgt.Codeunit.al:25, d9 at CDOeSealServiceMgt.Codeunit.al:18, and the 40
d40 rows that 5e fixed.

### 3a. The 17 new d1 "added" rows

All 17 sit behind 5a picks and are true database operations inside loops:
- 15 are in the merge-field code. `InsertMergeFields` loops over merge fields
  (`repeat ... until EMailTemplateMergeField.Next(-1) = 0`, CDOEMailTemplateLine.Table.al:1170-1178)
  and calls the now-resolved `GetValue` / `GetHtmlTable`. `GetValue`'s case arms do
  `Cont.Get` (CDOEMailTemplateMergeField.Table.al:271), `UserSetup.Get` (:277, :284),
  `Salesperson.Get` (:278), `Employee.FindFirst` (:286), `RecRef.FindFirst` (:266), and so on;
  the merge-table rows (CDOEmailTemplateMergeTable.Table.al:259, :370, :618, :621, :693;
  CDOHtmlTableStyle.Table.al:185) are reached through `GetHtmlTable`.
- 1 is `EMailLog.Insert(true)` (CDOLogManagement.Codeunit.al:243), reached from the
  `repeat` at CDOElectronicDocumentMgt.Codeunit.al:118-121 through `CreateEDocLogEntries`.

### 3b. The re-worded d1 / d48 rows

Most of the 238 new or re-worded d1 rows (93 new, 145 re-worded) and the 7 new d48 rows change in one way: the new
side's `cappedBy` gains `dynamic-dispatch`. Single-variable proof: the harness tables taken
just before and just after 5a (`task5a-triage-before-t5a.md`, `task5a-triage-after.md`)
contain 169 and 410 such rows, and nothing else changed between the two binaries. The cause
is real code that the new picks make reachable:
- `GetValue` runs `Codeunit.Run("Codeunit ID", EMailCodeunitParameter)` with a field value
  (CDOEMailTemplateMergeField.Table.al:309): a dynamic codeunit run.
- `LogUsage` calls `ModuleManager.IsModuleActivated`, which dispatches through
  `Interface "CDO Module License"` (CDOModuleManager.Codeunit.al:13): open-world interface
  dispatch, which d1 reports as `dynamic-dispatch`.

The other changes follow from the same picks: the evidence path now starts at an outer loop
(for example the `repeat` at CDODocumentEMailManagement.Codeunit.al:116 that sends one mail
per shipment and reaches `InsertMergeFields`), "Also reached from N" grows, and confidence
drops from likely to possible where a new cap appears. These are the shapes the first wave
already judged fixed.

## 4. d40 (opt-in): the 21 new-side-only rows

d40 is opt-in, so none of these reach default output. Re-verified in this table: exactly
these 21 locations.

| class | count | rows | verdict |
|---|---:|---|---|
| global record set in another routine | 6 | CDOLegacyeDocDispatcher.Codeunit.al:176, :187; CDOeDocumentsDispatcher.Codeunit.al:143, :443 (global set in `SetEMailTemplateLine`); CDOEMail.Codeunit.al:476 (set in `SetLogInfo`); CDOEMailTemplateLine.Table.al:705 (`GlobalEMailTemplateHeader`, loaded by `GetEMailTemplateHeader()`) | stated limit: d40 only looks inside the routine |
| load through a method called on the record | 1 | CDOEMailPrintStatement.Page.al:680 | stated limit: L4 roles exist only for explicit parameters |
| true finding, low value | 1 | CDOEmailTemplateMergeTable.Table.al:208 (`CIDAttachment` never loaded before sample-data use) | fixed (true) |
| public obsolete API forwarder, no workspace caller | 13 | CDOElectronicDocumentMgt.Codeunit.al:17, :26, :35; CDOMailManagement.Codeunit.al:21, :31, :41; CDOCopyEMailTemplate.Report.al:154, :157; CDOEMailTemplateManagement.Codeunit.al:273, :283, :313, :323, :333 | by design: the owner is outside the workspace, so d40 reports at the forwarder (ruling in 5e) |

## 5. Fixes made during triage

| fix | what | commits |
|---|---|---|
| 5a | Overload pick: only the positions where candidates differ decide (`pick_candidate`, `src/program/resolve/arg_dispatch.rs`). CDO ambiguousResolved 67 -> 23. | `a4a25894` |
| 5b | Adapter label: a run of a workspace object with no entry trigger is a Page/Report/Codeunit run, Opaque, not ExternalTarget. Plus 5a doc fixes. | `51a9963c`, `69193969` |
| 5c | L4 cones follow temporary records through `var` parameters (d45/d8); d9 gates on physical span tables. | `ee1a06cb`, `8ba47822` |
| 5d | d39: a forwarded `var` parameter is not an owned local; walker any-path dirt; walker applies calls inside `exit(...)`; CDO L4 digest re-frozen. | `ffa39303`, `e89ba748`, `6fed7f74` |
| 5e | d40 counts helper loads and lets the record's owner decide (with the `owner_judged` gate); d39 judges at the call site and ignores a reload in the other branch arm; walker sees a by-value record loaded by a `var` helper. | `a437172c`, `c7de98d6`, `9964965f`, `874b816c`, `bfc9e473` |

## 6. Stated limits and follow-ups

From the ledger (`progress.md`). None of these is a regression in this table.

- **Bare record calls.** The program engine treats a bare record call (no receiver) as a plain
  call: 571 shape mismatches on CDO, 193 of them trigger-capable. The adapter keeps L3's own
  implicit-trigger edge for those ops. Follow-up: classify bare record calls in the program
  engine, then drop the fallback.
- **Implicit-trigger fan-out.** The program engine ignores trigger site rules (`Validate`
  fans out to every field's `OnValidate`; `RunTrigger = false` still fires). The adapter
  applies L3's rules and drops 1,005 routes on CDO. Follow-up in the program engine (it also
  affects the LSP call hierarchy).
- **`Rec.Rename`.** Neither engine models it as a record operation, so `OnRename` is never
  reached. The adapter's rename counter is a tripwire only.
- 6 Opaque dependency runs differ from L3 only in `external_type_ref`.
- **Harness attribution** does not follow event publisher -> subscriber edges.
- **d40 limits** (section 4). d40 ignores branch arms: a load in the other arm hides a finding
  (cost one true low-value row, CDOEmailTemplateMergeTable.Table.al:210).
- **d39** misses a reload before the call on a loop back-edge.
- **d8 design.** d8 counts the commit routine's own downstream writes as its caller's
  transaction. True text, but not caused by B3 (3 first-wave rows carry this caveat).
- Walker opaque / no-role branches (`cfg_walker.rs` ~:1263/:1286) still update `loaded` only
  for var-to-var (over-reports).
- 5a: the pick assumes the call compiles; the ratchet comment cites a gitignored pick list.
- Minor code notes: `summary_runner.rs:707` comment omits d39 as a reader; no CuVar/report-var
  test for the 5b arm and no `debug_assert!(is_run)` there; a 5c allowlist comment merges
  into the prior rustdoc paragraph; one `Cow` clone per site (Task 2).
- `e89ba748` alone fails the CDO gate (`6fed7f74` fixes it); resolve at merge (squash, or fold).
- The r0 fixture comment in `ws-member-call-resolution/src/Helper.Codeunit.al` is stale
  (Task 8 regen).

## 7. r0 corpus ([r0-corpus.md](r0-corpus.md))

Regenerated by `cargo test --test r4 b3_triage_r0::` against this engine; the committed table
is current (206 workspaces, findings old 390 / new 390, 127 differing sites).

Findings: 2 differences, both fixed (triage-E).
- d14 removed, `ws-overload-callresult-guards/src/Caller.Codeunit.al:23`: the call at :20 now
  resolves to this overload, so the routine is reachable.
- d14 added, `ws-member-call-resolution/src/Helper.Codeunit.al:11`: `LocalHelper` is `local`,
  so `Helper.LocalHelper` from another object (Caller.Codeunit.al:11) is not a valid call. L3's
  edge was wrong; the routine really is dead.

Call sites (127):
- 65 "details" rows: same edge kind on both sides; neutral.
- 29 rows where the new side knows more (old unresolved or builtin, new resolved, builtin,
  run, or interface): fixed. Example: `Page.RunModal(Page::"Audit Target Page")` now lands on
  that page's `OnOpenPage`.
- 4 external rows: fixed (dependency objects).
- 1 dynamic row (`ws-builtin-dispatch-audit` AuditCaller.Codeunit.al:44, variable page id):
  fixed.
- 1 `LocalHelper` row: fixed (see above).
- 16 rows unresolved on both sides with a different reason: neutral.
- **11 rows where the new side knows less than L3** (L3 had a builtin or a resolved target; the
  program engine says Unknown or Ambiguous): `StrLen` (IRPageE.Page.al:20,
  BuiltinPrecedenceCollision), `GetNameW` (IRPageG.Page.al:24, WithScopeGuard),
  `"Shadowed Field".CreateInStream` (RBFBase.Table.al:103), `XmlElement.Create().AsXmlNode`
  and `RecRef.KeyIndex(1, 2).FieldCount` (CTCaller.Codeunit.al:121, :131),
  `CurrPage.MyAddIn...` x2 (CustomerCard.Page.al:57, :61), `Rec."Dup Field".CreateInStream`
  (RFCCaller.Codeunit.al:79), `"RD Collide".GetDisplayName` (RDBase.Report.al:69), `T.P` with an
  enum value (ws-overload-enum-discriminator, Caller.Codeunit.al:7), `T.V` with an InStream
  (ws-overload-negatives, Caller.Codeunit.al:7). That is 11 sites in 8 fixtures.
  Verdict: **stated limit, not a regression**. No new edge is wrong: each is the program
  engine's deliberate fail-closed answer on an adversarial fixture, and these fixtures are
  pinned by `tests/program_resolve_harness.rs` (the r0 `unknownByReason` ratchets). None moves
  an r0 finding, and CDO has no site of this kind (its only "-> Unknown" category is the 15
  truly dynamic runs). Follow-up: when the switch reaches other workspaces, a precision loss
  of this kind could hide a finding L3 would have made; the enum-literal overload case
  (5a: "an untyped argument at the discriminating position still degrades") is the most
  likely to occur in real code.

## Bar

CDO: **regressions 0, unexplained 0.** r0: **regressions 0, unexplained 0.** The exit bar
(0 and 0) is met.
