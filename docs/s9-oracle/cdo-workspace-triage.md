# S9 oracle triage: compiler call graph vs program resolver (workspace CDO)

Input: `cmp-ws.json`, 221 non-Run disagreement rows = **240 target pairs** (Call 125, Trigger 110, Event 5).
Every pair was opened at its source line. Workspace paths below are relative to
`U:/Git/DO-cdo-baseline/Cloud/`. Compiler lines come from `whole/graph.jsonl` (edge `line`).
Program-code citations are in `U:/Git/al-call-hierarchy`.

## Tally

| Verdict | Call | Trigger | Event | Total |
|---|---:|---:|---:|---:|
| PROGRAM BUG | 13 | 24 | 0 | **37** |
| COMPILER LIMIT | 22 | 86 | 5 | **113** |
| MAPPING | 90 | 0 | 0 | **90** |
| UNVERIFIED | 0 | 0 | 0 | 0 |
| **Total** | 125 | 110 | 5 | **240** |

Per-class totals by shape: Call = S1 10 + S3 3 + S4 22 + S5 25 + S6 63 + S7 2 = 125;
Trigger = S8 4 + S9 38 + S10 20 + S11 8 + S12 40 = 110. (Rows counted per target pair.)

| Shape | Verdict | Class | Side | Pairs |
|---|---|---|---|---:|
| S1 parens-less call in an expression | PROGRAM BUG | Call | compiler-only | 10 |
| S3 call inside a ternary `c ? a : b` | PROGRAM BUG | Call | compiler-only | 3 |
| S8 `DeleteAll(true)` fires no OnDelete | PROGRAM BUG | Trigger | compiler-only | 4 |
| S10 `Insert()`/`Modify()` with no RunTrigger treated as firing | PROGRAM BUG | Trigger | program-only | 20 |
| S4 `[EventSubscriber]` attribute counted as a call | COMPILER LIMIT | Call | compiler-only | 22 |
| S9 trigger edge despite RunTrigger absent/false | COMPILER LIMIT | Trigger | compiler-only | 38 |
| S11 implicit-Rec `Insert(true)`/`Delete(true)` not modeled | COMPILER LIMIT | Trigger | program-only | 8 |
| S12 `Validate` → field `OnValidate` not modeled | COMPILER LIMIT | Trigger | program-only | 40 |
| S13 built-in trigger event `OnOpenPageEvent` not modeled | COMPILER LIMIT | Event | program-only | 5 |
| S5 interface member → implementer edges | MAPPING | Call | compiler-only | 25 |
| S6 instance `X.Run()`/`X.RunModal()` classed as Call | MAPPING | Call | program-only | 63 |
| S7 call inside an inactive `#if` arm | MAPPING | Call | program-only | 2 |

---

## PROGRAM BUG shapes

### S1 — parens-less zero-argument call used inside an expression (10 Call, compiler-only)

AL allows a zero-argument call without `()`. When such a call sits inside an expression
(an argument, an `if` condition, `not X.M`, `exit(M)`), the program makes no call site.

| Caller | Target | Evidence |
|---|---|---|
| codeunit 6175277 IsApplicationAreaSupported | 437d…/9179 GetApplicationAreaSetup | `src/Platform/ApplicationArea/CDOApplicationAreaMgmt.Codeunit.al:64` `CustomDimensions.Add('…', ApplicationAreaMgmtFacade.GetApplicationAreaSetup);` |
| 6175277 UpdateDOBasic | same | same file `:48` |
| 6175277 SetApplicationArea | same | same file `:99` |
| 6175277 UpdateeDocumentApplicationArea | same | same file `:185` |
| codeunit 6175301 PostAndHandle | 9179 IsFoundationEnabled; table 36 IsApprovedForPosting | `src/Features/SendOnPosting/CDOSalesPostandHandle.Codeunit.al:33` `if ApplicationAreaSetup.IsFoundationEnabled then`; `:36` `if not SalesHeader.IsApprovedForPosting then` |
| codeunit 6175350 PostAndHandle | 9179 IsFoundationEnabled; table 38 IsApprovedForPostingBatch | `src/Features/SendOnPosting/CDOPurchPostandHandle.Codeunit.al:30`, `:33` |
| page 6175291 OnClosePage | 6175277 SetApplicationArea | `src/Platform/Setup/CDOSetup.Page.al:744` `IF ApplicationAreaMgmt.SetApplicationArea THEN` |
| codeunit 6175373 DoXmlExportWasUsed | 6175373 eDocumentAppendedToPDFAndSentInDO (bare identifier, no receiver) | `src/Features/EDocuments/CDOeDocSetupStatusRetr.Codeunit.al:100` `exit(eDocumentAppendedToPDFAndSentInDO);` |

Cause in our code: `collect_calls_v2` emits a site only for `ExprKind::Call`. Its
`ExprKind::Member` arm only recurses into the receiver (`src/program/resolve/extract.rs:699-702`),
and a bare `Identifier` falls to `_ => {}` (`extract.rs:730-732`). So `X.M` and `M` used as a
value are never call sites. (The parens-optional handling in `receiver.rs` only types receivers
of calls that were already extracted.) The fix must stay field-shadow-safe: a bare `X.M` is a
call only when `M` resolves to a procedure and not a field/property of `X`'s type.

### S3 — call inside a ternary expression (3 Call, compiler-only)

| Caller | Target | Evidence |
|---|---|---|
| codeunit 6175371 GetPrePostValidationType | 6175368 eDocsFeatureEnabled | `src/Features/EDocuments/CDOeDocPrePostValid.Codeunit.al:62` `… := eDocumentsFeatureMgt.eDocsFeatureEnabled() ? Enum::…::Default : …;` |
| codeunit 6175307 SendElectronicDocument | 6175369 CreateeDocumentDispatcher, CreateLegacyeDocDispatcher (compiler kind `Interface`, call site → implementer) | `src/Features/EDocuments/CDOElectronicDocumentMgt.Codeunit.al:80-81` `? DispatcherFactory.CreateeDocumentDispatcher(…) : DispatcherFactory.CreateLegacyeDocDispatcher(…)` |

Cause: the lowerer turns a ternary into `ExprKind::Unknown`. It lowers the children into the
arena but does not link them (`crates/al-syntax/src/lower/mod.rs:1975-1982`, comment names
"ternary"), and `collect_calls_v2` does not descend into `Unknown`. The same comment lists
`in`/`is`/`as` expressions and list literals as unlinked containers, so calls inside those are
lost the same way (no instance in this diff). The fix is an IR `Ternary {cond, then, else}`
variant (or linking the children of `Unknown`).

### S8 — `DeleteAll(true)` does not fire OnDelete in the program (4 Trigger, compiler-only)

| Caller | Target | Evidence |
|---|---|---|
| codeunit 6175399 OnAfterDeleteCustomer | table 6175286 OnDelete | `src/Platform/Setup/CDODataDeleteHandler.Codeunit.al:152` `CDOCustomerSetup.DeleteAll(true);` |
| page 6175306 OnAction | table 6175335 OnDelete | `src/Features/Email/Templates/CDOEMailTemplateLines.Page.al:381` `VariantEntry.DeleteAll(true);` |
| table 6175283 OnDelete | table 6175334 OnDelete | `src/Features/Email/Templates/CDOEMailTemplateHeader.Table.al:544` `DocumentTemplateLine.DeleteAll(true);` |
| table 6175334 CopyCriteriaTo | table 6175335 OnDelete | `src/Features/Email/Templates/CDODocumentTemplateLine.Table.al:1547` `NewCriterion.DeleteAll(true);` |

Cause: `resolve_implicit_trigger` maps only `insert/modify/delete/validate/rename`
(`src/program/resolve/resolver.rs:2015-2021`); `deleteall` and `modifyall` fall to "unrecognised
op: honest empty". `DeleteAll(true)` runs OnDelete per record; `ModifyAll(F, V, true)` runs OnModify
per record. The workspace has 10 `DeleteAll(true)` sites (e.g. also
`src/Features/Printing/CDOPrintqueue.Page.al:123`, implicit Rec, hidden because the compiler also
misses implicit-Rec ops — see S11).

### S10 — `Insert()` / `Modify()` without RunTrigger treated as firing the trigger (20 Trigger, program-only)

AL's RunTrigger parameter defaults to **false**. `Modify();` and `INSERT;` do not run OnModify/OnInsert.

| Caller | Target | Evidence |
|---|---|---|
| table 6175334: SetSubject, SetBackgroundPDF, SetFPBackgroundPDF, SetLPBackgroundPDF, SetMergePDF, SetRequestPage, SetHTMLTemplate, SetPlainTextEmailBody, DeleteBackgroundPDF, DeleteFPBackgroundPDF, DeleteLPBackgroundPDF, DeleteMergePDF, DeleteRequestPage, DeleteHTMLTemplate, DeletePlainTextEmailBody, DeleteEmailTemplate (16) | 6175334 OnModify | `src/Features/Email/Templates/CDODocumentTemplateLine.Table.al` `Modify();` at :686, :490, :527, :566, :603, :654, :732, :763, :509, :548, :587, :622, :671, :630, :789, :1432 |
| table 6175330 UpdateFromXml | 6175330 OnModify | `src/Features/PaymentLinks/CDOPaymentLinkTemplate.Table.al:180` `Modify();` |
| page 6175298 CreateUpdateTempRecs | table 91 OnInsert | `src/Platform/Setup/CDOSetupWizardUserSetup.Page.al:60` `INSERT;` (SourceTable "User Setup", :11) |
| page 6175305 InitPage | table 23 OnInsert | `src/DocumentTypes/RemittanceAdvice/CDOVendorRemittanceAdvice.Page.al:247` `Insert();` (SourceTable Vendor, :9) |
| page 6175401 NewRec | table 9 OnInsert | `src/Features/Email/Templates/CDODownloadCountryTemplate.Page.al:155` `INSERT;` (SourceTable "Country/Region", temporary) |

Cause: `TriggerSiteRule::of` (`src/program/resolve/applicability.rs:172-178`) reads a literal
only for `modify`/`delete`, and maps "no argument" to `None` = may fire; `Insert`'s slot is never
read, so even `Insert(false)` fires. Correct rule: no argument ⇒ `false`; read slot 0 for
`insert`/`modify`/`delete`/`deleteall` and slot 2 for `modifyall`; a non-literal ⇒ may fire.
Note: the same bug also produces **hidden agreements** — for qualified `X.Insert()`,
`X.Insert(false)`, `X.Modify()`, `X.Delete()` the compiler over-approximates too (S9), so both
graphs carry the same wrong edge and the diff cannot see it. In the workspace the compiler has
~135 such qualified no-`true` trigger edges (64 `Insert()`, 10 `Insert(false)`, 28 `Modify()`, 43
`Delete()`-family); the program fix will turn those into new compiler-only pairs that are S9.

---

## COMPILER LIMIT shapes

### S4 — the event name in `[EventSubscriber(…)]` becomes a Direct "call" to the publisher (22 Call, compiler-only)

The subscriber does not call its publisher. The compiler records the identifier-form event name in
the attribute as a Direct edge at the attribute line (one line above the procedure).

| Caller (subscriber) | Target (publisher) | Attribute line |
|---|---|---|
| codeunit 6175310 OnAfterCopyPurchaseDocument | 6620 OnAfterCopyPurchaseDocument | `src/Features/SendOnPosting/CDOSubscribers.Codeunit.al:918` |
| 6175310 OnAfterCopySalesDocument | 6620 OnAfterCopySalesDocument | `:880` |
| 6175310 OnAfterCreateCustomerFromTemplate | 1381 … | `:997` |
| 6175310 OnAfterInsertServHeader | 5923 … | `:255` |
| 6175310 OnAfterShowEDocumentElements | CDN 6086228 … | `:1016` |
| 6175310 OnAfterUpdatePostedSalesDocument | 442 … | `:966` |
| 6175310 OnBeforePostWithLines | 5980 … | `:1088` |
| 6175310 OnBeforeSalesPost | 80 OnBeforePostSalesDoc | `:1072` |
| 6175310 OnUpdatePurchLinesByFieldNo… | table 38 … | `:190` |
| 6175310 OnUpdateSalesLinesByFieldNo… | table 36 … | `:108` |
| 6175310 OnUpdateServLinesByFieldNo… | table 5900 … | `:284` |
| codeunit 6175358 (6 subscribers) | CDN 6252181 / page 6252183 publishers | `src/Integration/DeliveryNetwork/CDOeCandidatesEventHandler.Codeunit.al:9, 20, 30, 50, 61, 75` |
| codeunit 6175360 LoadFilesBeforeAttachingToDocument | CDN 6225535 OnBeforeAttachPdfToeDocument | `src/Integration/DeliveryNetwork/CDOCDNEDocEmbedReqSubs.Codeunit.al:108` |
| codeunit 6175362 "Company Triggers_OnCompanyOpenCompleted" | System 2000000003 OnCompanyOpenCompleted | `src/Platform/Telemetry/CDOTelemetry.Codeunit.al:462` |
| codeunit 6175374 / 6175402 ShowNotificationOnRoleCenterOpen | Core 6192817 OnRoleCenterOpen | `src/Features/EDocuments/CDODOeDocsNotificHandler.Codeunit.al:6`; `src/Features/OutputProfiles/CDOConflictNotificHandler.Codeunit.al:8` |
| codeunit 6175375 OnBeforeSendCustomer | table 60 OnBeforeSend | `src/Features/EDocuments/CDOeDocsSendingProfileSubs.Codeunit.al:5` |

Program is right (the real edge is publisher → subscriber, class Event, which both graphs carry).

### S9 — trigger edge emitted although RunTrigger is absent or false (38 Trigger, compiler-only)

The compiler's `Trigger` edges are `OverApprox` and ignore the RunTrigger argument. None of these
runs the trigger.

- `Modify(false)`: codeunit 6175288 `src/Platform/Upgrade/CDODataUpgrade.Codeunit.al:786, 636, 1057`;
  6175310 `src/Features/SendOnPosting/CDOSubscribers.Codeunit.al:305`; 6175372
  `src/Features/EDocuments/CDOeDocsSendCodeMigration.Codeunit.al:469`; 6175379
  `src/Features/Email/Templates/CDOTemplateVariantMgt.Codeunit.al:995` (6 pairs).
- `ModifyAll(Field, Value)` (no third argument): 6175288 `CDODataUpgrade.Codeunit.al:1265`;
  6175338 `src/Platform/Setup/CDOSetupData.Codeunit.al:122, 134, 164`; page 6175305
  `src/DocumentTypes/RemittanceAdvice/CDOVendorRemittanceAdvice.Page.al:326` (5 pairs).
- `DeleteAll()` (no argument), 27 pairs: 6175288 `CDODataUpgrade.Codeunit.al:145`; 6175300
  `src/Platform/Upgrade/CDOVer20Convert.Codeunit.al:31`; 6175379 `CDOTemplateVariantMgt.Codeunit.al:1213`;
  page 6175284 `src/Features/Statements/CDOEMailPrintStatement.Page.al:483`; page 6175291
  `src/Platform/Setup/CDOSetup.Page.al:566`; page 6175295 `src/Platform/Setup/CDOSetupWizard.Page.al:278`;
  page 6175316 `src/Features/MergeFields/CDOFieldsWithRelation.Page.al:337`; page 6175339
  `src/Features/Email/Templates/CDOVariantEntryWizard.Page.al:250, 309`; page 6175458 `:106, :123`
  and page 6175459 `:58, :73` (their own files); report 6175271 line 183; table 6175283
  `CDOEMailTemplateHeader.Table.al:538, 553, 564, 570, 573` (5 of the 6 targets of that row);
  table 6175289 `src/Features/Email/Templates/CDOEMailTemplImpWorkshtL.Table.al:222, 318, 832, 858`;
  table 6175317 lines 156, 160; table 6175320 `src/Features/MergeFields/CDOMergeTableField.Table.al:578`;
  table 6175334 `CDODocumentTemplateLine.Table.al:435`.

Program is right in all 38 (it reads the literal `false`; it has no `deleteall`/`modifyall` rule,
which happens to be right for the no-`true` form).

### S11 — unqualified (implicit `Rec`) `Insert(true)` / `Delete(true)` has no trigger edge (8 Trigger, program-only)

Across all workspace callers the compiler's `Trigger` edges come only from qualified receivers
(`X.Insert…`); there are zero from an unqualified call. The program is right.

| Caller | Target | Evidence |
|---|---|---|
| page 6175344 NewFile | table 6175301 OnInsert | `src/Features/Email/Sending/CDOEMailAttachments.Page.al:97` `Insert(true);` (SourceTable "CDO File", :10) |
| page 6175467 OnAction | 6175301 OnInsert | `src/Features/Email/Sending/CDOEmailAttachmentsManage.Page.al:68` |
| page 6175394 OnAction | table 6175305 OnDelete | `src/Features/Printing/CDOPrintqueue.Page.al:106` `Delete(true);` (SourceTable "CDO Print document" = table 6175305) |
| table 6175274 ImportAttachmentFromClient | self OnInsert | `src/Features/Email/Templates/CDOEMailTemplateAttachment.Table.al:152` |
| table 6175301 AddAttachmentFromXml | self OnInsert | `src/Foundation/CDOFile.Table.al:886` |
| table 6175305 Create | self OnInsert | `src/Features/Printing/CDOPrintdocument.Table.al:115` |
| table 6175338 ImportAttachmentFromClient | self OnInsert | `src/Features/Email/Templates/CDOTemplateLineAttachment.Table.al:124` |
| table 6175320 ValueFromTableFieldArrayAssistEdit | self OnInsert | `src/Features/MergeFields/CDOMergeTableField.Table.al:258` (trigger at :236) |

### S12 — `Validate(Field, …)` does not produce an OnValidate edge (40 Trigger, program-only)

The compiler graph has **zero** `Trigger` edges into any `OnValidate` (it only records
`BuiltIn Table # Validate`, e.g. `CDOSetupData.Codeunit.al:833`). The program's rule is
field-precise (`TriggerSiteRule::admits`, `applicability.rs:199-206`), and spot checks confirm the
validated field has an OnValidate: table 133 "File Extension" (BaseApp `IncomingDocumentAttachment.Table.al:57`),
table 700 "Record ID" (`ErrorMessage.Table.al:24`), table 472 "Object ID to Run" (`JobQueueEntry.Table.al:102`),
table 6175335 "Filter Value" (`CDOTemplateVariantEntry.Table.al:43`), table 6175307 "One Page Document
Background" (`CDOEMailTemplLineReport.Table.al:188`), table 6175299 CU-ID fields
(`CDOEDocumentSendCode.Table.al:48…`), table 6175272 "E-Mail" (`CDOEMailRecipient.Table.al:89`).

Rows: codeunits 6175307 (`CDOElectronicDocumentMgt.Codeunit.al:181`), 6175309 (`CDOLegacyeDocDispatcher.Codeunit.al:283`),
6175338 ×5 (`CDOSetupData.Codeunit.al:833, 687, 957, 755, 426`), 6175358 (`CDOeCandidatesEventHandler.Codeunit.al:157`),
6175372 (`CDOeDocsSendCodeMigration.Codeunit.al:41`), 6175376 ×2 (`CDOeDocumentsDispatcher.Codeunit.al:346, 465`),
6175405 (`CDOSetupMigrationMgt.Codeunit.al:246`); pages 6175273 (:76), 6175275 (:59), 6175276 (:78…), 6175279 (:43),
6175339 ×3 (`CDOVariantEntryWizard.Page.al:440, 355, 293`), 6175423 (`CDOExtensionAppCard.Page.al:114`),
6175439 (`CDOMergeTableFields.Page.al:183`), 6175454 (:38); tables 6175283 ×4 (:1193, :232, :1092, :1142),
6175284 ×2 (`CDOEMailTemplateLine.Table.al:1038, 1077`), 6175289 (:735), 6175307 ×3 (:360, :399, :438),
6175315 (`CDOTemplateRecipientSetup.Table.al:78`), 6175317 (:732), 6175318 (`CDOMergeTableLink.Table.al:189`),
6175320 ×2 (`CDOMergeTableField.Table.al:514, 239`), 6175339 ×3 (:339, :378, :417).

### S13 — built-in page trigger event `OnOpenPageEvent` (5 Event, program-only)

Pages 6175286, 6175287, 6175289, 6175311, 6175312 → codeunit 6175402 subscribers, e.g.
`src/Features/OutputProfiles/CDOConflictNotificHandler.Codeunit.al:54-55`
`[EventSubscriber(ObjectType::Page, Page::"CDO UnhandledPostedSalesInv.", OnOpenPageEvent, …)]`.
The compiler graph has no publisher node for platform trigger events and no `Event` edge into these
subscribers (only the subscriber's own self-reference). Program is right.

---

## MAPPING shapes

### S5 — interface member → implementer edges (25 Call, compiler-only)

Callers are interface declarations (`interface "CDO eDocument Dispatcher"`, `"CDO Module License"`,
`"CDO eSeal Service"`, `"CDO IeDocumentDispatcherFactory"`, `"CDO eDocument Dispatcher Factory"`,
`"CDO IField Chain Evaluator"`, `"CDO Inv. Resp. Parser"`), e.g.
`src/Platform/Licensing/CDOModuleLicense.Interface.al:11-43` → codeunits 6175287 / 6175291. The edges
have compiler kind `Interface`, no line. They are dispatch-table facts, not calls. The program
models the same thing as call-site → implementer (`conditionalResolved`) and those agree (e.g.
`RunPrePostValidation` → CDN validators has no disagreement).

### S6 — instance run `Var.Run()` / `Var.RunModal()` / `Var.RUN` (63 Call, program-only)

Program emits caller → `OnOpenPage` / `OnRun` / `OnPreReport` of the variable's object; the compiler
records only `BuiltIn Page # RunModal` / `BuiltIn <codeunit> # Run` (e.g. page 6175285 line 416). These
are object runs, which the brief excludes, but the comparison only classes static `Page.Run(Page::X)`
as `Run`. Examples: `src/Features/HandledState/CDOUnhandledPostedSalesShipt.Page.al:416`
`WarningPage.RunModal()` (13 pages → page 6175461); `src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19, 34`
`ReminderIssue.RUN;` / `FinChrgMemoIssue.RUN;` (→ 393/395 OnRun); `src/Features/SendOnPosting/CDOSubscribers.Codeunit.al:1206, 1239`
`LogRunner.Run()`, `AutoSendRunner.Run()`; `src/Features/EDocuments/CDOInvResponseProcessor.Codeunit.al:71` `DocProcessor.Run(…)`;
`src/DocumentTypes/Warehouse/General/CDOWhseSalesOrderActions.Page.al:230` `CreateInvtPutAwayPickMvmtReport.RUNMODAL;` (4 pages → report 7323 OnPreReport);
plus page/table variables at `CDOEMailTemplateImportExport.Codeunit.al:687`, `CDOFunctions.Codeunit.al:384`,
`CDOMailManagement.Codeunit.al:124`, `CDORemittanceManagement.Codeunit.al:39`, `CDOSenderProfileErrorHndl.Codeunit.al:40`,
`CDOABSAuthSettingsMgt.Codeunit.al:16`, `CDOEMailTemplates.Page.al:157`, `CDOEMailTemplateCard.Page.al:290`,
`CDOCustomerCard.Page.al:251`, `CDOPrintqueue.Page.al:193`, `CDOEditHTMLEMailtemplate.Page.al:152, 178`, and others listed in the input.

### S7 — call inside an inactive `#if` arm (2 Call, program-only)

codeunit 6175280 SetSendingMethodAndFromAddress → SetFromAddressWithDOSMTPSetup, SetDOSMTPCode at
`src/Features/Email/Sending/CDOEMail.Codeunit.al:245, 247`, inside `#if DOSMTP` (:241-252). `app.json`
defines no `preprocessorSymbols`, so the compiler builds the `# else` arm. The program union-reads all
arms by design. Both are right for their own policy.

---

## PROGRAM BUG causes, ranked

1. **Missing RunTrigger argument is read as "may fire"; `Insert`'s slot is never read** — 20 pairs (S10).
   AL construct: `Rec.Modify()`, `Modify;`, `Insert()`, `INSERT;` (RunTrigger defaults to false).
   `src/program/resolve/applicability.rs:172-178`. Also hides ~135 wrong agreeing edges (see S10 note).
2. **Parens-less zero-argument call inside an expression is not a call site** — 10 pairs (S1).
   AL construct: `X.M` / `M` as an argument, `if` condition, `not` operand, `exit` value.
   `src/program/resolve/extract.rs:699-702, 730-732`.
3. **`DeleteAll(true)` (and `ModifyAll(F, V, true)`) not mapped to OnDelete/OnModify** — 4 pairs (S8).
   `src/program/resolve/resolver.rs:2015-2021`.
4. **Ternary `c ? a : b` lowered to opaque `Unknown`; calls inside are lost** — 3 pairs (S3).
   `crates/al-syntax/src/lower/mod.rs:1975-1982` (same for `in`/`is`/`as`/list literal).

## MAPPING rules to add to the comparison

1. Drop compiler edges of kind `Interface` whose **caller is an interface member** (object type
   `Interface`); keep call-site → implementer `Interface` edges (they match the program's
   conditional routes). (S5, 25 pairs)
2. Class a program edge as `Run` (and exclude it) when its target is an object entry trigger
   (`OnOpenPage`, `OnRun`, `OnPreReport`, `OnInitReport`, xmlport/query equivalents) reached from a
   `.Run`/`.RunModal`/`.RunRequestPage` call on a Page/Codeunit/Report/XmlPort **variable**, not only from
   static `Page.Run(Page::X)`. (S6, 63 pairs)
3. Drop program call sites that lie inside an `#if` arm that is inactive for the app's
   `preprocessorSymbols` (here: none defined). (S7, 2 pairs)
4. Key targets by object **type + number**, not number alone: number 6175383 is both a codeunit
   (`ProcessSingleResponse` → `6175383.onrun`) and the Customer table extension (`… → 6175383.onvalidate`).
   No false match was found in this run, but the key can collide.

## COMPILER LIMIT rules (filter or annotate compiler edges)

1. Drop a compiler `Direct` edge from subscriber S to publisher P when its line is S's
   `[EventSubscriber(…)]` attribute line (line < S's declaration line; callee = the subscribed event). (S4, 22)
2. Drop compiler `Trigger` edges for `Insert`/`Modify`/`Delete`/`DeleteAll` with no argument or literal
   `false`, and for `ModifyAll` with fewer than 3 arguments or a literal `false` third argument. (S9, 38;
   more after the program fix to cause 1)
3. Do not count missing compiler trigger edges for **unqualified** record ops (implicit `Rec` /
   `CurrPage` source record): the compiler models none. Accept program-only `OnInsert`/`OnModify`/
   `OnDelete` from unqualified `…(true)` as unconfirmable. (S11, 8)
4. Do not count missing compiler `OnValidate` edges: the compiler models no `Validate` → `OnValidate`
   trigger at all. Program-only `onvalidate` pairs are unconfirmable by this oracle. (S12, 40)
5. Do not count missing compiler `Event` edges from built-in trigger events (`OnOpenPageEvent`,
   `OnAfter/BeforeInsertEvent`, …): the compiler has no publisher node for them. (S13, 5)
