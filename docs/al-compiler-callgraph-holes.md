# Holes in Microsoft's AL compiler call graph

This register lists every known place where **Microsoft's** AL call graph (`altool graph`:
`extract`, `extract-whole`, `stitch`, `query` and its SARIF export) is missing, wrong or lossy.
For each hole it says how **the owner's tools** do in comparison. It feeds the competitive
feature matrix at the end.

**Keep the two sides apart.** Throughout this file:

- **Microsoft's tools:** the AL compiler (`alc`), `altool graph`, the official AL Language
  extension's LSP, and the official AL MCP (`almcp`).
- **The owner's tools:** al-sem (binaries `al-call-hierarchy`, `alsem`, `aldump`),
  bc-dev-mcp, al-lsp-for-agents and LethAL.

Microsoft's graph is used here as an **oracle** (the CDO compiler audit,
`src/program/resolve/compiler_golden.rs`) and as bc-dev-mcp's **fallback** engine. Its gaps are
competitive evidence. They are not bug reports to Microsoft.

**Keep this file current.** When a compiler/engine disagreement turns out to be an altool limit,
add or update its entry in the same change. That applies to a new `Verdict` in
`compiler_golden.rs`, a spec saying "the oracle cannot see X", and a bc-dev-mcp workaround for
altool. A hole that lives only in a CHANGELOG line or a code comment is lost.

**Versions.** "v18" is alc 18.0.41.45789 (extension 18.0.2732683), which minted the CDO golden.
"v30" is extension 30.0.2863738 (alc 30.0.43.45776). The v30 probe app from 2026-10-13 is
committed in [`al-compiler-callgraph-holes-probe/app/`](al-compiler-callgraph-holes-probe/app/),
and altool's raw output for it (stitched `g.jsonl`, the shard, and the SARIF exports) is in
[`al-compiler-callgraph-holes-probe/altool-v30-output/`](al-compiler-callgraph-holes-probe/altool-v30-output/).
Build the app against BC28 symbols in `.alpackages`, then run `altool graph extract` + `stitch`
to reproduce. The output's absolute paths are hole A27 itself.

**Sources** used for evidence:
- **Triage:** [`docs/s9-oracle/cdo-workspace-triage.md`](s9-oracle/cdo-workspace-triage.md)
- **Spec:** [`docs/superpowers/specs/2026-10-13-callgraph-export-design.md`](superpowers/specs/2026-10-13-callgraph-export-design.md)

**Confidence.** Every entry in section A was reproduced by a probe on v30 unless it says otherwise.

## A. Holes in `altool graph`

### Edges that should exist but are missing

| # | Hole | Repro (what Microsoft emits) | Seen | Owner's tools |
|---|------|------------------------------|------|---------------|
| A1 | **No object-run edges at all.** `Run` is in altool's own edge-kind enum and `--edge-kinds` help, but altool never emits one. | `Codeunit.Run(Codeunit::"X")` gives `BuiltIn 0 Codeunit # Run` only, with no target and no edge to `OnRun`. `Page.Run`/`RunModal`, `Report.Run` and report-variable `RunModal` behave the same. `query --to "obj:X" --edge-kinds Run` returns 0 nodes. | v18 (CDO: 63 instance-run pairs in Triage S6; 147 `ObjectRun` verdict pairs), v30 | **Better.** al-sem has `EdgeKind::Run` to the entry trigger plus extension triggers. `alsem graph` adds `Run` and lifecycle `Trigger` edges. Live on Cronus28, bc-dev-mcp selects the test for D4 (`ApplyDecay`, reached only through `Codeunit.Run`) with no coverage map; on altool it was "uncoverable". |
| A3 | **Calls into symbol-only dependencies disappear silently.** There is no node and no edge, not even a built-in one. `IndirectUnresolved` is in the enum but never emitted. Cross-app edges appear only when you have source for every app and use `extract-whole`. | With BC28 symbols: `NoSeries.GetNextNo('X')`, `Cust.Insert(true)` and `Cust.Validate(Name,'x')` leave only the caller's self-edge. | v18 (Continia Document Output: 0 nodes from 37 packages; 1 event edge of 108 publishers), v30 | **Better.** al-sem resolves through `SymbolReference.json` (`resolvedAbiExternal`). `alsem graph` has `symbolOnly` nodes and a callbacks hub. |
| A9 | **No trigger edges from record operations without a receiver.** | `Insert(true)` / `Delete(true)` on the implicit `Rec` (page triggers, table procedures) give a built-in edge only. | v18 (Triage S11: 8 pairs), v30 | **Better** (`Verdict::UnqualifiedRecordOp`). |
| A10 | **No `Validate` → field `OnValidate` edges.** CDO has zero `Trigger` edges into any `OnValidate`. | `R.Validate(Amount, 5)` gives `BuiltIn 0 Table # Validate` only. | v18 (Triage S12: 40 pairs), v30 | **Better** (an edge to the exact field's `OnValidate`; `Verdict::ValidateTrigger`). |
| A11 | **Table-extension record triggers are never reached.** | A `tableextension` `OnAfterInsert`, or `modify(F) { trigger OnAfterValidate }`, gets no incoming edge from `Insert(true)` / `Validate`. | v30 | **Same hole in al-sem** (Spec F5), but `alsem graph` flags it with `unmodeledRecordTrigger`; altool says nothing. *To win this row: fix F5.* |
| A12 | **Platform events are never raised.** The shard records `sub` facts but no `pub` facts, so `stitch` connects nothing. | Subscribers to `OnAfterInsertEvent`, `OnAfterValidateEvent` and `OnOpenPageEvent` get no `Event` edge. | v18 (Triage S13: 5 `OnOpenPageEvent` pairs), v30 | **Page events: better** (synthetic publishers, `Verdict::PlatformPageEvent`). **Table events: same hole** (Spec F1), flagged with `unraisedPublisher`. *To win this row: fix F1.* |
| A16 | **Self-recursion is invisible.** Every routine gets a self-edge at its declaration line; the real recursive call is dropped. | `Recurse(n - 1)` produces no edge at its line. | v18, v30 | **Engine: better** (the site is kept). **`alsem graph` export:** emits no self-edges, so it cannot show recursion either. That is harmless for reachability. |
| A21 | **Built-ins that run an object never reach their static target.** | `Session.StartSession(Sid, Codeunit::"X")`, `TaskScheduler.CreateTask(Codeunit::"X", 0)`, `Report.SaveAsPdf(Report::X, …)`, `RecordRef.Insert(true)` after `Open(Database::"T")`, and `FieldRef.Validate` all end at a built-in node. | v30 | **Sound but coarse.** `alsem graph` sends these to `Run{kind}` / record hubs, so nothing is lost silently. Exact targets are still open (Spec F3). |
| A22 | **Runs set by a property produce no edge.** | An action with `RunObject = page "P"` produces nothing. Inferred: `LookupPageId` / `DrillDownPageId` are the same. | v30 | **Better** (IR `run_objects` + lifecycle rule 1). |

### Edges that are wrong

| # | Hole | Repro | Seen | Owner's tools |
|---|------|-------|------|---------------|
| A7 | **Trigger edges ignore RunTrigger.** | `Insert()`, `Insert(false)`, `Modify()`, `Delete()`, `DeleteAll()` with no argument, and `ModifyAll(F, V)` with no third argument all get a `Trigger/OverApprox` edge, yet run no trigger on BC28. | v18 (Triage S9: 38 pairs; 145 `RunTriggerFalse` pairs), v30 | **Better.** al-sem reads the RunTrigger argument; a non-literal value keeps the edge, marked `runTriggerGuarded`. |
| A14 | **The `[EventSubscriber]` attribute is recorded as a call to the publisher.** | The subscriber gets a `Direct/Resolved` edge at the attribute line, so `query --to "obj:Pub"` lists the subscriber as a caller. | v18 (Triage S4: 22 pairs), v30 | **Better** (no such edge; `Verdict::SubscriberAttribute`). |
| A17 | **Interface declarations appear as callers.** | `Interface I # Go` → each implementation, as `Interface/OverApprox` edges. These are dispatch-table facts, not calls, and they make path queries return duplicate paths. | v18 (Triage S5: 25 pairs), v30 | **Better** (al-sem models only call site → implementer). |

### Positions and duplicates

| # | Hole | Repro | Seen | Owner's tools |
|---|------|-------|------|---------------|
| A4 | **Only the first edge per (caller, callee) pair is kept.** Every later call site is dropped. | Calls `T.A()`, `T.B()`, `T.A()` and `T.A(); T.B();` on lines 28–31: only lines 28 and 29 appear. | v18 (CDO; 42 repeat calls in S9.5c), v30 | **Engine and LSP: better** (every site with its span; LSP incoming calls return every site). **`alsem graph` export: same hole** (one row per (caller, callee, kind), smallest line). *To win this row: emit every site.* |
| A5 | **No column on any edge or SARIF region**, so two calls on one line cannot be told apart. | `T.B(); T.A();` gives two edges, both at line 38. CDO has 269 ambiguous same-line (caller, class, line) groups. | v18, v30 | **Engine and LSP: better** (byte and column spans; the #28 span key is unique over 46,317 CDO edges). **`alsem graph` export: same hole** (line only). *To win this row: add the column to the export.* |
| A6 | **`Interface` and `Event` edges carry no line or path**, and their SARIF steps have no region. This contradicts Microsoft's own changelog ("SARIF steps open at the exact line"). | The call-site `Interface` edge carries only `detail`, yet the same call has line 49 on its `Direct` edge. | v18 (golden stores `line: 0`), v30 | **Interfaces: better** (the edge sits on the call-site span). Event edges in the export: not checked. |
| A8 | **A trigger edge's first line can point at an operation that does not fire it.** This follows from A4 + A7. | `R.Insert();` on line 35 then `R.Insert(true);` on line 36 places the `OnInsert` edge at line 35. | v18 (CDO: CDOMailManagement 180/208, CDOEMailTemplateManagement 1530/1564), v30 | **Better** (the first line is the real firing site; pinned by #28's `FirstLineDiffers` audit). |
| A15 | **Duplicate rows.** The shard writes `header` and `pub` twice, so the stitched graph has two identical `Event` edges and SARIF repeats results. | Any published event. | v30 | **Better** (the export merges duplicates). |

### Identity and metadata

| # | Hole | Repro | Seen | Owner's tools |
|---|------|-------|------|---------------|
| A2 | **A run through a variable becomes a fake built-in named after the user object.** | `T.Run()` on `Codeunit "X"` gives `BuiltIn 0 X # Run` under the null GUID. `obj:X` matches it, so `RunVar` is listed as a caller and `Codeunit.Run(Codeunit::"X")` is not. Naming is inconsistent: `ReportInstance # RunModal` but `Page # RunModal`. | v30 | **Better** (one object identity, however the run is spelled). |
| A13 | **Platform event nodes lose their owner.** | A field event is `BuiltIn 0 Amount # OnAfterValidateEvent`: the field sits in the object slot, with no table and no app. Inferred: same-named fields on different tables collide. | v30 | **Table events: better** (synthetic publisher per table). Field events: not checked. |
| A18 | **Trigger nodes do not name their field, action or dataitem.** Two field `OnValidate` triggers differ only by an opaque hash. | `# OnValidate#-2033899177` and `# OnValidate#-779430849`, with `memberName: "OnValidate"`. | v30 | **Better** (`enclosing_member` plus location). With altool, bc-dev-mcp falls back to all tests for a changed trigger; with al-sem, D5 selects 1 test (live). |
| A19 | **The trigger methodId rule is undocumented.** `OnRun` is `-401248892` in two different codeunits and apps, so the hash looks independent of the object. The procedure rule would give `289754967`. | Compare the fixture `call-graph.jsonl` with the probe. | v30 | **altool is AHEAD here.** al-sem emits `methodId: null` for triggers (Spec F6). *To close: crack the rule (F6).* |
| A20 | **Record operations are not bound to their table.** | `Table # Insert/Modify/Validate/Rename` are null-GUID built-ins with object id 0. | v18, v30 | **Better** (record operations on a named table, db effects). |
| A23 | **Subscribers are not marked as subscribers.** | Subscriber nodes have `eventRole: "None"`, although the enum has `Subscriber`. | v30 | al-sem knows every subscription. Whether the export has a role field: not checked. |
| A24 | **Edge kinds that are declared but never produced:** `Run` and `IndirectUnresolved`. | Seen 0 times in the probe, the bc-dev-mcp fixture and the Continia Document Output composition. | v18, v30 | **Better** (`Run` and open-site edges are produced and pinned by tests). |

### Robustness and output

| # | Hole | Repro | Seen | Owner's tools |
|---|------|-------|------|---------------|
| A25 | **A broken compile still produces a graph, with no signal.** | On code alc rejects (AL0135/AL0132/AL0296), `extract` prints "Extracted:", silently leaves out the unresolvable call, and keeps an edge for a call that is illegal in its scope (OnPrem). | v30 | **Better on signal** (`meta.recoveredFiles`, `parse_incomplete`, open-hub edges). al-sem does not type-check either. |
| A26 | **SARIF export gaps.** | A one-ended export (`--to` only) gives 0 results. With no selectors it gives only sink-reachability results. Other gaps: `startLine` only; absolute `file:///` URIs; `informationUri` points to Microsoft's internal `microsoft.ghe.com`; every result is a `warning`; 2 generic rules; A6 and A15 also apply. | v18, v30 | **A different product.** `alsem analyze` SARIF has named findings; `alsem graph` is JSONL. |
| A27 | **CLI rough edges.** | A wildcard in an `id:` selector crashes with a .NET stack trace that shows internal build paths (documented on v18, not re-probed). `--paths` needs both ends. Only `ns:` supports globbing. JSONL `sourcePath` values are absolute paths on the extracting machine, so graphs are not portable (verified on v30). | v18, v30 | **Better on paths** (`sourcePath` is relative to the app root). |

## B. Not altool holes: BC runtime semantics, or places where altool was right

- **Interface dispatch over-approximates** to every implementer, because the runtime picks the
  target. al-sem does the same (bc-dev-mcp D2 selects 10 tests; only 2 reach the DE implementer).
- **Event edges have no call site.** Publisher → subscriber is not a call; the raise site is the
  `Direct` edge to the publisher.
- **`BindSubscription` and `Codeunit.Run(<integer>)`** are dynamic.
- **altool was right and al-sem was wrong, since fixed in al-sem:** inactive `#if` arms (Triage
  S7), parens-less calls inside expressions (S1), ternaries (S3), `DeleteAll(true)` /
  `ModifyAll(…, true)` firing triggers (S8), and overload binding (CHANGELOG S9.5c, where the
  compiler is the authority).

## C. Microsoft's official AL LSP and AL MCP

- **The official AL LSP has no call hierarchy.** Its protocol DLL has `textDocument/references`,
  `implementation`, `typeHierarchy/*` and `codeLens`, but no `prepareCallHierarchy` /
  `incomingCalls` / `outgoingCalls`. (Inferred from
  `Microsoft.Dynamics.Nav.EditorServices.Protocol.dll` strings; not exercised live.)
  **Owner's `al-call-hierarchy` serves all three, with every call site's position.**
- **The official AL MCP has no call-graph tool.** Its tools are `al_build`, `al_compile`,
  `al_publish`, `al_run_tests`, `al_downloadsymbols`, `al_symbolsearch`,
  `al_symbolrelations`, `al_getdiagnostics` and others. `al_symbolrelations` covers only
  SourceTable, TableRelation, Implements, Extends, ExtensionOf, ExtendedBy, ImplementedBy and
  UsedAsSourceTable: no calls, callers or subscribers. (Verified from the extension's
  `package.json` and `almcp.dll`.) **Owner's bc-dev-mcp selects tests over a whole-program
  graph, and `alsem` answers callers, events and impact.**

## D. Feature matrix: call graph, test selection and verification

Microsoft's column = the AL compiler, `altool graph`, the official AL LSP and the official AL
MCP. The owner's column = al-sem, bc-dev-mcp, al-lsp-for-agents and LethAL. The wider
toolbox-vs-Microsoft matrix lives in
`bc-dev-mcp/scratch/blogpost-bc-toolbox-vs-microsoft-mcp.md`.

| Capability | Microsoft | Owner's toolbox |
|------------|-----------|-----------------|
| Call hierarchy in the editor (incoming/outgoing) | No (C) | Yes: al-call-hierarchy, cross-app, every call site |
| Whole-program call graph export | Yes, `altool graph` (with holes A1–A27) | Yes, `alsem graph` (schema `al-sem.callgraph` v1) |
| Object runs (`Codeunit.Run`, `Page.Run`, run variables, `RunObject`) | No (A1, A2, A22, A24) | Yes |
| Calls into symbol-only dependencies | Silently dropped (A3) | Yes, via `SymbolReference.json`, plus callbacks hub |
| Record-trigger edges, RunTrigger-aware | Wrong (A7, A8); missing for implicit `Rec` (A9) | Yes |
| `Validate` → field `OnValidate` | No (A10) | Yes |
| Table-extension record triggers | No (A11) | Not yet (F5), but flagged |
| Platform events raised (table / page) | No (A12) | Page: yes. Table: not yet (F1), but flagged |
| Every call site, with column | No: first site per pair, line only (A4, A5) | Engine and LSP: yes. Export: not yet |
| Subscriber attribute not counted as a call | No (A14) | Yes |
| Trigger identity (field / action / dataitem) | No (A18) | Yes |
| Trigger methodId | Yes (rule undocumented, A19) | Not yet (F6) |
| Signal for incomplete code | No (A25) | Yes |
| Diff-scoped test selection | No | Yes: bc-dev-mcp `bcdev_test_select` over `alsem graph`. Live Cronus28: 0 missed tests on D1–D7, 13% of test executions on D1–D6 |
| Sound fallback (never silently miss a test) | No: holes are silent | Yes: hubs plus `hiddenCallers` force a wider selection |
| Mutation testing | No | Yes: LethAL (alpha), runs through bc-dev-mcp. Idea, not built: choose each mutant's tests statically with `alsem graph` instead of from coverage only |

**Rows to turn into clean wins on the owner's side:** A4/A5 in the export (emit every site,
with column), A11 (F5), A12 table events (F1), A19 trigger methodId (F6).
