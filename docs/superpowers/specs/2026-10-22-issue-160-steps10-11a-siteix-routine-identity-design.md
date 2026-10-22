# #160 child spec: steps 10 + 11a — `SiteIx` (the key join) and routine identity

Status: revised after spec panel round 1 (2026-10-22; findings R1-01 to R1-25 in
`.agent/issue-160/findings.json`). The row-indexed parity probe was re-run for that round
(ledger P15). One claim, one spec, one panel, one bench, one PR (epic Decision 57).
Child of `2026-10-18-issue-160-analysis-session-design.md` (the epic): §1 goal 7, §5.3, §5.4,
§5.12, §9 rows **10** and **11a**, E2, E11, E12, E14, E15, I2, I6, I7, I9, Decisions 5, 6, 20,
25, 37, 45, 54, 57, 58, open question 7. Follows the shape of
`2026-10-21-issue-160-step7b-closed-sets-fold-memo-design.md` ("7b" below). Where this
document and the epic disagree, this document is wrong, EXCEPT the numbered amendments of §9
(10A-1 to 10A-5, 11A-1 to 11A-4), each with its reason; until the epic takes them, they are
open questions for the panel.

It starts from two pre-drafts written 2026-10-21 outside the repo (step 10's and step 11's;
the boundary between them is the one the epic took in Decision 57). Every figure below was
measured at this spec's base; nothing from the pre-drafts is carried as a fact.

Base: master `ca35bf51` (step 7b's merge), worktree `/work/al-sem-issue-160-a4`. Since the
pre-drafts' base `afd5d88f`, `src/`, `crates/` and `tests/` changed only in
`capability_cone.rs`, `cone_derived.rs`, `detector_context.rs` and
`tests/cli/cli_build_counters.rs` (7b); no file this design edits except
`detector_context.rs`, whose cited lines are re-read here. Every `file:line` below is in
that tree. Every fact the design rests on is a row of the ledger's `## Assumption probes`
table (`.agent/issue-160/ledger.md`, rows P1-P15); the probe patches, scripts and raw output
are in `/work/bench/160-10-11a/` (`probe.patch`, `probe4.patch`; both reverted:
`git diff -- src crates tests` is empty in the worktree). The reference lists the tests
assert are in `/work/bench/160-10-11a/reference-cross-app.txt` and are reproduced in §3.5.

Words:
- **Model site** = a call site (`ModelRoutine.call_sites`, `PCallSite`), a record operation
  (`ModelRoutine.record_operations`, `ModelRecordOperation`) or an operation site
  (`ModelRoutine.operation_sites`, `POperationSite`).
- **Program site** = a `ClassifiedEdge` (`src/program/resolve/full.rs:99-102`) whose
  obligation is `ObligationId::CallSite` (`:89-96`).
- **FileIx** = a per-build number for one parsed file of one app. **SiteIx** = the key
  `(FileIx, ExprId)`: a file and the call expression in that file's IR arena (amendment
  10A-1: a key, not a dense number).
- **Span join (J1)** = today's pairing of model sites and program sites by
  `(unit, start line, start col, end line, end col)`. **Key join** = the pairing by `SiteIx`.
- **J4** = J1's source-text op name (`span_text` + `op_name`). **`texts`** = J1's unit → source
  text map. **J2** = the declaration-anchor join node → row (`by_decl`). **J5** = the event
  graph's span join row → node (`node_by_span`).
- **Row** = one entry of `ModelEntities::routines` (`ModelRoutine`, `workspace.rs:430`).
  **`RoutineIx`** = a row's position in `ws.routines` (11a). **Node** = a `RoutineNodeId`, the
  deduplicated program identity. **`RowNodes`** = the row/node table 11a records at assembly
  (§4.6).
- **Accepted pair** = a model site and the program edge the join hands to the conversion
  (`Join.calls` / `Join.ops`, `program_calls.rs:393-401`). **Candidate** = the edge a lookup
  returns before the shape/callee/caller checks.
- **Census** = `SiteCensus` (`program_calls.rs:153-271`); **`losses()`** its non-zero loss
  counters (`:279-315`).
- **Demand** = the set of routine nodes a cross-app model holds rows for
  (`cross_app_demand`, `program_calls.rs:921-928`; `demanded_population`, `workspace.rs:1586`).
- **`#if`-split routine** = one source routine the lowerer emits as several `RoutineDecl`s,
  one per preprocessor arm, whose bodies share source text. Each decl is a model row and each
  decl's sites are program sites; two decls' sites at one span are two expressions (P6, P15).

## Feasibility first

The two rows' acceptance items, checked against the measured data before the design.

| Item | Row | Verdict | Numbers (ledger row) |
|---|---|---|---|
| `losses() == []`, single-app, r0 | 10 | **feasible**, holds today | 0 losses on every r0-corpus and `tests/fixtures` dir with an `app.json`, 220 runs (P2) |
| `losses() == []`, single-app, CDO | 10 | **feasible**, holds today | `losses=[]`; 15,539 call sites, 20,721 program sites (P1) |
| `losses() == []`, cross-app, r0 | 10 | **feasible**, holds today | 0 losses, 220 cross-app runs (P2; most fixtures have no dependency, §5.2 adds a dependency fixture) |
| `losses() == []`, cross-app, CDO | 10 | **not feasible as written; needs 10A-2 and 10A-3** | Today: `program_only_site` 231,408, `implicit_trigger_unmatched` 18,532, `duplicate_program_span` 74, `duplicate_l3_span` 23, `shape_mismatch` 18, `caller_mismatch` 5, `adapter_l3_fallback_sites` 23 (P1). Counting only program sites of callers the model holds (10A-2): `shape_mismatch` 18, `caller_mismatch` 5, `duplicate_program_span` 24, `duplicate_l3_span` 23. Every span duplicate is two expressions at one span (74 of 74, P6) and the model has 0 duplicate keys in every site category (P15), so the key join has no duplicate counter. Left: **23 named sites, two root causes, neither a join defect** (§3.4, listed exactly in §3.5) |
| Span↔key parity (open question 7: every op the body walk numbers has an `ExprId` the extractor sees) | 10 | **yes** | Row-indexed, both joins' accepted pairs and their complete conversion compared (P15). Single-app CDO and every r0 dir: identical. Cross-app CDO: 81,699 accepted call pairs identical, **23 differ** (listed in §3.5), 6,271 accepted op pairs identical, 0 differ; **the converted calls are identical**: 165,361 `(from, site)` groups of edges, external targets and bindings, 84,717 edges on each side, 0 differing groups |
| Byte identity of the model's calls under the key join | 10 | **yes on CDO, both universes** | the 23 differing pairs convert to identical edges: 21 select an edge of equal kind, shape and routes; 2 (`GetAccessToken` cs4, cs6) select edges whose target nodes differ (`acquiretokenfromcache`/5, `sig_fp` 9686704692977790200 by span, 8102086367325882817 by key), and J2 maps both nodes to the same row (the first row at their shared anchor, §4.7). Hence 11a must keep that J2 answer (§4.7) |
| J4 replaceable without source text | 10 | **yes** | the extractor's own op name gives the same `can_fire_trigger` answer as `op_name(span_text)` on all 135,013 `ImplicitTrigger` sites of cross-app CDO and all 5,124 of single-app CDO, 0 disagreements; same on r0 (P7) |
| `edge_order_is_call_sites_then_triggers` green | 10 | **feasible by construction** (order comes from model routine order, §4.3) | — |
| Attach saving ≥ parent `b3.adapter.join` − band(join), if join median ≥ 2 × band | 10 | **feasible; a speed step, its bounds can fail** (§8) | base, 5 interleaved rounds, census off (P4): cross-app `b3.adapter.join` 8,108.9 ms, band 571.8 (14.2 × band); `b3.adapter` 8,334.6, band 591.9; margin `8,108.9 − 571.8 − 591.9 = 6,945.2 ≥ 2 × 591.9`; `analyze.total` 26,140.4 band 1,792.9, net margin `8,108.9 − 571.8 − 1,792.9 = 5,744.2 ≥ 2 × 1,792.9`. Single-app `b3.adapter.join` 80.7 ms, band 2.4; `b3.adapter` 151.8, band 8.6. Where the cross-app join's time goes (probe sub-spans, P3): J4's loop 9,806.5 ms of 10,319.5 (95 %), the program map 317.5, the model loop 146.6, `texts` 9.2; the caller check 24 ms |
| Both peaks ≤ parent + band | 10 | **feasible for the process peak; the post-attach peak cannot be read on Linux** (10A-4) | trace `peak.*` read 0 on Linux (#204); kernel peak RSS (`VmHWM` via `getrusage`), base, 3 runs (P5): cross-app 3,222.6 MB (band 31.0; the bound's 3 % band is 96.7 MB), single-app 704.8 MB (band 15.6; 3 % = 21.1 MB). Expected growth ≈ 12 MB at attach (§4.13), so B2 cannot see it; the census (§8 part 1) reports it |
| Byte-identical (I2) on the 8 golden targets | 10 + 11a | **feasible**: the goldens' corpus has no differing pair at all (P2) | — |
| `RowNodes` replaces `caller_mismatch`; J2, J5 deleted; output-neutral | 11a | **feasible** with recorded compatibility targets (§4.7-§4.9) | CDO cross-app: 2 collision groups (4 rows), both one node, both Base Application `PageManagement`; three `#if`-split routines with two rows at one anchor: `AcquireTokenFromCache` on two nodes (the cross-node alias, §3.2; its edges differ in `sig_fp`), `GetAccessToken` and `GetFreshAccessToken` each on ONE node (two string ids, so no collision group; corrected from P15's `from=` on both edges, final panel FP-16), one arm J5 cannot map; single-app 0 groups; r0: one group of two symbol-only rows (`ws-cross-object-chain`, cross-app) (P8, P9) |
| 11a non-speed, no bench (Decision 58) | 11a | **yes** | `b3.adapter.by_decl` 9.6 ms (band 0.6) cross-app, 0.8 ms (band 0.1) single-app, against an analyze wall band of 1.8 s / 0.1 s (P4) |

So the rows are delivered with five amendments for 10 (10A-1 to 10A-5) and four for 11a
(11A-1 to 11A-4); none drops an item.

## 1. Goal

**Step 10.** The model and the program engine name a call site by one key, `(FileIx,
ExprId)`. The adapter pairs them by that key. The span join, the `texts` map and J4 are
deleted. The span becomes a debug assertion on every accepted pair.

Why: the span join is the largest single phase of model assembly on cross-app CDO, and most
of it is J4. On this base, cross-app `analyze` walks **25,289,392,125 bytes** of source text
inside `span_text` (P3): for each program site that no call site took (367,587 of them,
almost all dependency-body sites of routines the model does not hold), it scans its file from
byte 0 to the site's line, twice. The join is 8,108.9 ms (band 571.8) of a 26.1 s cross-app
`analyze` (P4), and 95 % of it is that loop (P3).

**Step 11a.** At assembly, each row's node, each node's program declaration anchor, and the
answers today's J2 and J5 give are recorded once (`RowNodes`). The adapter's caller check,
J2's route lookup and J5's event-graph lookup read that table instead of re-deriving them
per use from a `DeclSurface` lookup, a `format!`ed unit and a span. A collision census
counts rows that share a string id. Output does not change; where today's answers are
wrong (a cross-node alias in J2, two J5 mapping gaps) the table records today's answer as a
named compatibility target, and the correction is 11b's (10A-3, 11A-3).

## 2. Non-goals

- **No re-keying of engine tables by `RoutineIx`, no collided-id rule, no correction of J2's
  or J5's compatibility answers.** That is 11b (Decision 57).
- **No change to resolution.** Routes, shapes, completeness and the histogram are unchanged
  (CDO `aldump --program-call-graph-stats` SHA equal, §8 part 1). The resolver only records
  the key and one bit (§4.4) it already knows.
- **No serialized field and no `Debug`-visible field on an L2 feature type.** The
  `tests/ir-l2-goldens/l2_features.snapshot` golden hashes the `Debug` text of every
  routine's `PFeatures` (`tests/l2_ir/ir_l2_snapshot.rs:66-74`), so a new field on
  `PCallSite`, `PRecordOperation` or `POperationSite`, `serde(skip)` or not, moves it. The keys
  therefore live on `ModelRoutine` (§4.2; P12).
- **No fix of the two residual cross-app causes** (§3.4). Both move output; both are filed
  as discoveries (10A-3).
- **J3 (the ABI zip, `workspace.rs:1497-1546`) stays.** Its value stays a string id in 11a.
- **No rename of `src/engine/l4/routine_interner.rs`'s `RoutineIx`** in this claim (11A-4).
- **The `pd` arena is not here** (G-4).
- **The LSP keeps its own, unstamped resolution path** (§4.2a): it resolves files without a
  `ProgramContext` file table and reads no key.

## 3. Background: the joins today

### 3.1 The adapter (`program_calls.rs`)

`attach_program_calls_with` (`:655-701`) runs `adapter` under `b3.adapter` (`:662-665`).
`adapter` (`:1011-1126`) runs `join` under `b3.adapter.join` (`:1019-1022`), builds
`by_decl` (J2) under `b3.adapter.by_decl` (`:1026-1039`), then converts (`:1069-1116`,
`b3.adapter.convert`). `join` (`:403-611`):

1. `texts` (`:412-427`): model unit → source text, every workspace file and every dependency
   file of a model app.
2. J1 program side (`:430-441`): `SiteLinks::build(view)` (`site_links.rs:62-77`) keys every
   call-site edge of `view.all_edges()` by `(caller app, (String path, lines, cols))`; for each
   model app, `sites_of(app)` filters the whole map (`site_links.rs:89-97`) and re-keys each
   site with `model_unit` (a `format!` for a dependency, `program_calls.rs:356-362`). First
   edge per span wins; the rest count `duplicate_program_span`.
3. J1 model side (`:466-536`): per routine, each operation site and record op is re-keyed by
   `l3_key` (`site_links.rs:31-43`, a `String` per site) into `l3_ops` / `l3_record_ops`; each
   call site is looked up, then checked for shape, callee fingerprint and caller. The caller
   check (`:518-531`) compares the edge's `from` node's declaration anchor
   (`DeclSurface::get_with_path`) with the row's declaration anchor.
4. J4 (`:538-568`): every program site no call site took is classified with its source text
   (`span_text`, `:321-331`, a line walk from byte 0) and `op_name` (`:335-339`).

The only readers of the census are the two loss gates
(`tests/program_resolve_harness.rs:10612-10654`, single-app only: `adapter_census_for_workspace`,
`program_calls.rs:1969-1972`, builds the workspace scope) and the `--lib` tests. The
cross-app census is discarded (`:664` keeps `.0`).

### 3.2 J2 and J5

- **J2**: `by_decl` maps `(unit, line, col)` → the FIRST row with that anchor, over every row
  of the model (`entry().or_insert`, `:1035-1037`); `Converter::l3_routine` (`:1178-1191`;
  callers `:1197`, `:1878`) goes node → `DeclSurface` declaration (last write wins per node,
  `decl_surface.rs:288-292`) → anchor → row. When two NODES' declarations share an anchor,
  both map to the first row there (the cross-node alias, P15: `AcquireTokenFromCache`'s two
  nodes both map to row `…4f4ac631`).
- **J5**: `program_event_graph` (`events.rs:269+`) maps rows to nodes by
  `(file, start, end)`: workspace rows from the graph's physical rows (first wins,
  `:288-293`), dependency rows from the surface's one declaration per node (`:305-319`,
  first wins). Two rows at one span map to the first node inserted there; a dependency row
  whose span is not its node's surface declaration does not map (`unmapped`, `:385-388`).

### 3.3 What both walks already know

- The extractor's site has `expr: ExprId` (`extract.rs:162-164`); it is resolved per
  `(app, ParsedFile)` (workspace `full.rs:1230-1252`; dependency bodies `:1630-1672`).
  `ExprId` is a `u32` arena index with a public `index()`; it derives `Copy, Clone,
  PartialEq, Eq, Debug, Hash`, no `Ord` (`crates/al-syntax/src/ir/mod.rs:50-62`).
- The body walk keys its numbering by the call's `ExprId` (`ir_walk.rs:75-80`); `eid` is in
  hand where it pushes a record op (`:997`), an operation site (`:1045`) and a call site
  (`:1086`). Every record op also pushes an operation site with the same `eid` (`:997-1053`;
  measured: 33,290 of 33,290 record-op keys are also operation-site keys, P15).
- Both walk the SAME `ParsedFile` (an `Arc<AlFile>`): the model projects `pf.file` from
  `ctx.parsed()` (`workspace.rs:1666-1735`) and from `ctx.dep_bodies()` (`:1549-1578`), and
  both already look a per-file resolver output up by `(AppRef, virtual_path)`
  (`ParenlessCalls`, `full.rs:121-124`, read at `workspace.rs:1718-1720` and `:1563-1565`).
- Production builds `ClassifiedEdge` at exactly two places: call sites (`full.rs:1053`) and
  publisher event flow (`:1291`). The other 10 literals are in `#[cfg(test)]` code (P10).
- `resolve_file_obligations` (`full.rs:900`) has THREE production callers: the workspace
  resolution (`:1245`), the dependency bodies (`:1657`), and the LSP's per-file recompute
  (`src/lsp/snapshot.rs:1145-1163`, `recompute_file`, called by the batch snapshot
  `:764` and the incremental updater `src/lsp/updater.rs:554`, `:829`), which has no
  `ProgramContext` at hand. Test callers: `full.rs:4292`, `:4382-4383`.

### 3.4 The cross-app residual (P1, P15)

With the program side restricted to callers the model holds, 23 sites remain. Both causes are
disagreements between the two walks, not join defects:

- **18 `shape_mismatch`**, all in Base Application `ICOutboxImpExp.XmlPort.al` (11 ×
  `X.DeleteAll`, 7 × `X.Insert`): the receiver is an XmlPort table element. The extractor
  treats table elements as record variables (S9.0e, `full.rs:936-946`), so it emits a record
  op; the body walk's record-receiver set does not include them, so the model has a call
  site. Each gets one `Unknown(NoProgramSite)` edge today.
- **5 `caller_mismatch`**, all in Base Application `PageManagement.GetPurchaseHeaderPageID`,
  which has two rows (two `#if` arms at lines 324 and 353, one string id, one node;
  collision group 2 of §4.10). The surface keeps the LAST arm's declaration, so the first
  arm's 5 call sites fail the anchor check. They too get `Unknown(NoProgramSite)` edges.

Their `adapter_l3_fallback_sites` is the same 23 sites. Both are filed as discoveries (10A-3).

### 3.5 The reference lists (P15)

These are what T1, T2, F2, F3, 10A-3 and 10A-5 assert, as SETS (any identity not on the
list fails closed). Units are the model's; `dep:<BaseApp>` is the Base Application's
`dep:<guid>` unit, `dep:<CC>` the dependency that holds `AccessToken.Table.al` and
`CoreSessionManager.Codeunit.al` (the test spells the guids out, read from the run).

**R-PAIRS: the 23 call sites whose accepted pair differs, key vs span** (cross-app CDO; 0 in
single-app CDO and in every r0/`tests/fixtures` dir; 0 op pairs):

| Row (name) | Sites | Unit, lines | Equivalent edge? |
|---|---|---|---|
| `AcquireTokenFromCache` (`…b353d8a0`) | cs0-cs7 | `AccessToken.Table.al` 64:8, 65:8, 67:8, 72:42, 74:42, 76:24, 77:24, 81:45 | yes (8) |
| `GetAccessToken` (`…6b4aafd9`) | cs0, cs1, cs3, cs5, cs7 | `CoreSessionManager.Codeunit.al` 59:8, 60:8, 73:38, 81:11, 89:15 | yes (5) |
| `GetAccessToken` (`…6b4aafd9`) | cs4, cs6 | 76:11, 85:11 (`AccessToken.AcquireTokenFromCache(…)`) | **no**: span → node `acquiretokenfromcache`/5 `sig_fp` 9686704692977790200, key → `sig_fp` 8102086367325882817; both convert to row `…4f4ac631` (J2 alias) |
| `GetFreshAccessToken` (`…61a9c530`) | cs0-cs3 | `CoreSessionManager.Codeunit.al` 101:8, 103:8, 104:11, 107:12 | yes (4) |
| `GetSalesHeaderPageID` (`…f09488ab`, second twin row) | cs0-cs3 | `dep:<BaseApp>` `PageManagement.Codeunit.al` 297:8, 298:11, 299:19, 299:42 | yes (4) |

The converted output of all 23 is identical on both joins (P15: 0 differing
`(from, site)` groups, edges, external targets and bindings included).

**R-RESIDUAL: the 23 loss sites of cross-app CDO under 10A-2** (each `(reason, unit,
start, end)`):
- `shape_mismatch`, `dep:<BaseApp>` `src/Finance/Intercompany/Outbox/ICOutboxImpExp.XmlPort.al`:
  656:8-656:33, 657:8-657:35, 658:8-658:33, 659:8-659:36, 660:8-660:37, 661:8-661:36,
  662:8-662:37, 663:8-663:33, 664:8-664:37, 665:8-665:31, 666:8-666:35 (`…DeleteAll`);
  671:16-671:38, 679:24-679:48, 689:24-689:46, 698:24-698:49, 707:24-707:50, 716:24-716:49,
  725:24-725:50 (`…Insert`).
- `caller_mismatch`, `dep:<BaseApp>` `src/Utilities/PageManagement.Codeunit.al`:
  329:8-329:39, 330:11-330:46, 331:19-331:84, 331:42-331:83, 350:8-350:70.
- `adapter_l3_fallback_sites`: the same 23 sites.

## 4. Design

### 4.1 `FileIx` (step 10; 11a reuses it)

```rust
// src/program/site_key.rs (new)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileIx(pub u32);
/// `SiteIx` (epic §5.3): a call expression in one file. 8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SiteIx { pub file: FileIx, pub expr: u32 }   // expr = ExprId::index()
```

`expr` stores `ExprId::index()` as a `u32`: `ExprId`'s field is private to `al-syntax` and it
derives no `Ord` (`crates/al-syntax/src/ir/mod.rs:50-62`), so storing the index keeps the
type free of both limits and lets a test state a key no expression has (`u32::MAX`; the arena
is indexed from 0 and a file never holds 2³² expressions). `SiteIx` derives no `Ord`:
nothing orders sites by key (order comes from rows, §4.3).

`ProgramContext` (`full.rs:1490-1506`) gains `files: FileTable`, filled once where the
context is built (`full.rs:1861`): every `(AppRef, virtual_path)` of `parsed`, then of the
dependency bodies when the profile keeps them, numbered in that order. `ctx.file_ix(app,
path) -> Option<FileIx>`. The key carries the app, so two apps with the same virtual path get
two `FileIx` (T4b pins it). It is the only file table (Decision 57); 11a reuses it. The LSP's
exhaustive destructure (`src/lsp/snapshot.rs:692-700`) gains `files: _`.

Rejected: `Arc<AlFile>` pointer identity (not deterministic, breaks on any re-parse);
position arithmetic (the model sorts files by name first, `workspace.rs:1698-1699`, `:1555`).

### 4.2 Where the key is stamped

| Side | Where | What |
|---|---|---|
| program | `ClassifiedEdge.site: Option<ProgramSite>` | `ProgramSite { at: SiteIx, fires: bool }` (12 bytes; 16 as an `Option`). `Some` at `full.rs:1053` when the caller passed a file; `None` at `:1291` (publisher edges), on the LSP path (§4.2a) and in every test literal |
| program | `resolve_file_obligations` (`full.rs:900`) | gains `file: Option<FileIx>`; the workspace and dependency callers look it up once per file (`:1245`, `:1657`) and pass `Some`; the test callers pass a looked-up one |
| model | `ModelRoutine.file: Option<FileIx>` and `ModelRoutine.site_exprs: SiteExprs` | `SiteExprs { calls: Vec<u32>, record_ops: Vec<u32>, op_sites: Vec<u32> }` (expression indices, §4.1), parallel to `call_sites`, `record_operations` and `operation_sites` (one entry each, same order) |
| model | the walk | `walk_spine`'s context pushes `eid` into the parallel vector at each of the three push sites (`ir_walk.rs:997`, `:1045`, `:1086`); a new entry `project_routine_features_ir_keyed` returns `(PFeatures, SiteExprs)`; `project_routine_features_ir[_with]` keep their signatures and discard it (so the l2 snapshot sees the same `PFeatures`) |
| model | `project_ir` (`workspace.rs:759`) | gains `file: Option<FileIx>` (looked up by both callers next to the `ParenlessCalls` lookup) and stores it and the `SiteExprs` on each row |

Why not on the feature types: §2 (P12). `ModelRoutine` is in no golden's `Debug` text
(searched: `rg '\{:?\}' tests`; only failure messages).

A symbol-only (ABI) row has no sites, so `file == None` is normal there. A row WITH sites and
`file == None` cannot occur in production (every site-bearing row comes from `project_ir`,
which receives the file); it trips a `debug_assert!` and counts each of its sites unpaired
(§4.3). The walk's parallel vectors are filled at the same three pushes that number the
sites, so their lengths equal the site vectors' by construction; a `debug_assert_eq!` on the
three lengths at the end of `project_ir` pins it.

`ModelRoutine` literals to update (`file: None, site_exprs: SiteExprs::default()`): 11 in
`src` (2 production: `workspace.rs:1375`, `abi_rows.rs:183`; 9 in test code:
`root_classification.rs:822`, `return_summary.rs:894`, `l5/test_support.rs:117`,
`l4/cfg_walker.rs:1869`, `l4/db_effect_solver.rs:1814`, `l5/d1_temp.rs:275`,
`gate/format_pr_summary.rs:311`, `l5/fingerprint.rs:305`, `l5/detectors/d17.rs:385`) and 3
in `tests` (`temp_state/temp_state_shadowing.rs:100`, `temp_state/temp_state_path.rs:63`,
`l4_summary_differential.rs:1289`); `rg -n 'ModelRoutine \{'` excluding the definition and
function signatures (`program_calls.rs:2269` is one).

### 4.2a The LSP path

`recompute_file` (`src/lsp/snapshot.rs:1145-1163`) is the LSP's single definition of "what a
file contributes", shared by the batch snapshot (`:764`) and the incremental updater
(`updater.rs:554`, `:829`). It reads only `LspEdge`s, never a key, and it has no
`ProgramContext` file table when the updater re-resolves one file. It therefore calls
`resolve_file_obligations(…, None)`: the explicit unstamped path. The LSP's edges are
unchanged by construction. T10 (§5.11) pins it at `resolve_file_obligations` itself (called
directly with `None` and with `Some`), because `recompute_file` returns `LspEdge`s and a
stamp cannot be observed through it; the existing batch/incremental `recompute_file` tests
keep pinning the LSP's output.

### 4.3 The key join (step 10)

`join` keeps its outputs (`calls: (ri, ci) → edge`, `ops: (ri, oi) → edge`) and its census.

1. **Program side.** One pass over `view.all_edges()`: keep `ObligationId::CallSite` edges
   whose caller app is a model app (`model_apps`, `:366-380`) and whose caller the model
   holds (§4.5); insert `site.at → edge`, first wins, the rest count
   `duplicate_program_span` (name kept for the census's continuity). No `SiteLinks`, no
   `String`.
2. **Model side.** Per row, `(row.file, row.site_exprs.*[i])` is the key. Duplicates are
   counted per category: a repeated call-site key, a repeated record-op key, a repeated
   operation-site key each count `duplicate_l3_span`; a record op and its own operation site
   sharing one key is the walk's design (§3.3) and is not a duplicate. Shape and callee
   fingerprint checks run as today; the caller check reads `RowNodes` (§4.8).
3. **Unconsumed program sites (old J4).** `ImplicitTrigger`: `fires == false` →
   `operation_site`; else a model record op at the same key → `implicit_trigger_matched`
   (the pair), else `implicit_trigger_unmatched`. `Commit` → `operation_site`. A key among
   the model's operation sites → `op_shape_mismatch`. Else `program_only_site`. Same buckets
   and rules, no source text.
4. **The span as a debug assertion** (from Task 2, kept after the deletion): for every
   accepted pair (call and op), `debug_assert_eq!` the model anchor's span with the program
   site's span, the unit spelled by `model_unit`. Release builds pay nothing. It fires only
   on a pair, so a test that unpairs a site by key (§5.3) never reaches it.
5. `UnmatchedSite.text` for a program-side miss becomes `""` (it was the span's text); its
   span fields stay. No serializer reads `SiteCensus` (searched). Stated.
6. The two bucket identities (`:581-596`) stay.

**Order.** The conversion iterates model rows (`:1077-1116`) and reads the maps by
`(ri, ci)`/`(ri, oi)`, so edge order cannot change; `edge_order_is_call_sites_then_triggers`
(`program_calls_adapter_tests.rs:1370-1406`) pins it (I7).

**What the key join changes (P15).** At an `#if`-split routine, two decls' sites sit at one
span with two expressions. The span join hands the second decl's site the FIRST decl's
edge; the key join hands each site its own decl's edge. On cross-app CDO that is exactly
R-PAIRS (§3.5): 23 call sites, 0 ops. Their converted calls are identical, because for 21
the edges are equivalent and for 2 J2 maps both target nodes to one row. 11a keeps that J2
answer (§4.7), so the claim is byte-neutral on the model's calls. The census's duplicate
counters fall from 24/23 to 0 (span duplicates are distinct keys; P6, P15).

### 4.4 J4's replacement: `fires` on the program side

The extractor parsed the op (`CalleeShape::RecordOp { op, .. }`, `extract.rs:76`; folded at
`full.rs:774`). `fires = can_fire_trigger(&op_lc)` is stamped with the key. Measured equal to
the J4 answer on every `ImplicitTrigger` site (P7), so the census rules are unchanged,
including an `ImplicitTrigger` edge with no model record op (`operation_site` when it cannot
fire). `can_fire_trigger` moves next to the resolver so one function decides on both sides.
`span_text`, `op_name` and `texts` are deleted.

Rejected: reading the op name from the paired model record op. An unpaired non-firing op
would change bucket from `operation_site` to a loss, so
`l3_ops_with_no_program_site_are_counted_by_trigger_capability` (`:2318-2350`) would change
its expected counts: a census-semantics change in a byte-neutral step.

### 4.5 The program side counts callers the model holds (amendment 10A-2)

Dependency bodies are resolved whole (`full.rs:1630-1647`); the cross-app model holds only
demanded routines. So today every edge of a non-demanded dependency routine is a
`program_only_site` or `implicit_trigger_unmatched` loss by construction (231,408 + 18,532
on CDO, P1), not a lost site. The program side therefore keeps an edge when its caller app
is a model app (unchanged, `model_apps`) AND its caller is a primary routine, or is in the
demand set the model was built from, or the model was built without demand.
`attach_program_calls_with` gains `demand: Option<&HashSet<RoutineNodeId>>`;
`assemble_and_resolve_cross_app_program` already holds it (`program_calls.rs:921-928`). The
predicate is the one `demanded_population` applies to the same node ids
(`workspace.rs:1583-1585`, `:1611`), so a demanded routine's missing site is still a loss.

On CDO the filter drops 334,145 of 449,383 cross-app program sites before any hashing (P1).
Single-app is unchanged (no demand, primary only).

### 4.6 `RoutineIx` and `RowNodes` (step 11a)

- **`RoutineIx` = the row's position in `ws.routines`** (no stored field; accessors
  `ModelEntities::routine(ix)` / the enumerate index). Rows are pushed in today's order
  (workspace files, then ABI rows, then dependency source files, `workspace.rs:1442-1450`);
  nothing reorders them. Epic §5.4's "rows gain `ix`" is met by position (11A-2).
- **`RowNodes`**, built at the end of assembly from the rows, the program graph and the
  `DeclSurface` (all in hand there), returned in `ModelRows` beside the rows:

  ```rust
  pub struct RowNodes {
      /// The true node of each row (per RoutineIx): workspace rows from the physical
      /// row's `node`; dependency source rows from `source_routine_node_id`; ABI rows
      /// from J3's `node_id`. Read by the collision census; 11b's corrections read it.
      node_of: Vec<Option<RoutineNodeId>>,
      /// The program's declaration anchor `(FileIx, start line, start col)` of every node
      /// that is the `from` of a program edge the join reads (after the 10A-2 filter),
      /// captured from `DeclSurface::get_with_path` and never derived from a row. That is
      /// exactly the set the caller check reads (`decl_anchor[from]`, §4.8), including a
      /// `from` no held row stands for (the cross-node alias), so it is sized by callers,
      /// not by model-app nodes. A `from` with no surface declaration is absent, and the
      /// check reads absence as `caller_mismatch`, as today's failed surface lookup does.
      decl_anchor: HashMap<RoutineNodeId, (FileIx, u32, u32)>,
      /// J2's answer, recorded (§4.7). Built by walking EVERY node of a model app
      /// (`model_apps`) that has a surface declaration, whether or not a held row stands
      /// for it (J2's domain: `l3_routine` asks the surface for any model-app node,
      /// `:1178-1191`), and storing, for a node whose declaration anchor some
      /// non-symbol-only row has, the FIRST such row over all rows. A node with no
      /// declaration, or whose anchor no row has, is absent (J2's `None`), so the map
      /// holds about one entry per row, not one per model-app node.
      j2_row: HashMap<RoutineNodeId, u32>,
      /// J5's answer, recorded (§4.9): per row, the node today's span map gives an
      /// event-subscriber row, or None (every other row: J5 is read for subscribers only).
      j5_node: Vec<Option<RoutineNodeId>>,
  }
  ```

  Workspace rows' nodes come from `physical.rs:45-56` (`rows_population`,
  `workspace.rs:730`, already walks them); dependency source rows' from
  `source_routine_node_id`, which `demanded_population` (`:1586`, the call at `:1611`)
  already computes and forgets (computed the same way when demand is off); symbol-only rows'
  from J3 (`:1531-1541`).
- **Lifetime:** build-local, read by the adapter and `program_event_graph`, both before the
  program body is released (Decision 35). It is not retained (§4.13 sizes it).

### 4.7 J2 replaced by its recorded answer

`Converter::l3_routine(node)` becomes `row_nodes.j2_row.get(node)`, restricted to model apps
and to non-symbol-only rows (a symbol-only row's empty unit never equals a declaration's,
so J2 never returned one; the restriction makes that explicit). `j2_row` is exactly J2, on
J2's own domain: building `j2_row` walks every node of a model app that has a surface
declaration (not only nodes a held row stands for), so a route into a node no row stands
for, but whose declaration anchor coincides with some row's, gets that row exactly as
today; the anchor read per node is the surface's declaration (last write wins per node), and "first row
at that anchor over all rows" is `by_decl`'s `entry().or_insert`. Only matched nodes are
stored, so the walk costs time, not memory. A node with no surface
declaration, or whose anchor no row has, is absent from `j2_row` (J2's `None`). It therefore keeps the
cross-node alias (both `AcquireTokenFromCache` nodes → row `…4f4ac631`, which is what keeps
R-PAIRS' two non-equivalent pairs byte-identical) and the two collision shapes:
`GetPurchaseHeaderPageID` (two anchors → the row at line 353) and `GetSalesHeaderPageID`
(two rows at one anchor → the first). The alias is a known wrong answer; correcting it to
`node_of`-based selection is 11b's (11A-3, the 11b discovery of 10A-3).
`by_decl`, `b3.adapter.by_decl` and the surface lookup in `l3_routine` are deleted.

### 4.8 `caller_mismatch` replaced (the boundary of Decision 57)

The caller check of §4.3 step 2 becomes: `decl_anchor[ce.edge.from] == (row.file, row anchor
line, row anchor col)`. The left side is the program's own declaration, captured
independently of any row at assembly (R1-03); the right side is the row as it is at join
time. So moving a row's anchor still fails the check
(`caller_anchor_disagreement_is_caller_mismatch`, `program_calls.rs:2298-2315`, stays as it
is), and the answer equals today's on every site, including the 5 `caller_mismatch` sites of
R-RESIDUAL. One `HashMap` lookup and a tuple compare per pair; no surface lookup, no `format!`.

The caller check costs 24 ms on cross-app CDO today (P3), so this is an identity change,
not a speed one.

### 4.9 J5 replaced by its recorded answer

`program_event_graph` reads `j5_node[ri]` instead of building `node_by_span`; its dependency
surface pass (`events.rs:298-319`) and `node_by_span` (`:288-293`) are deleted. `j5_node` is
computed at assembly by J5's own two rules, from the same inputs: a workspace row takes the
node of the first physical row with its `(file, start, end)`; a dependency row takes the
first node (graph routine order, model apps, non-primary) whose surface declaration span
equals the row's span, else `None`. So it keeps J5's two wrong answers exactly (P9): the
second row of each two-rows-at-one-anchor routine maps to the first row's node, and
`GetPurchaseHeaderPageID`'s line-324 arm maps to nothing. The `processed` rule (first MAPPED
row per string id, `:376-391`) is unchanged. Correcting the mapping to `node_of` is 11b's.

J5 is recorded for event-subscriber rows only (final panel r1, FP-10): the event graph is
J5's only reader and it reads no other row's node, so every other row's `j5_node` is `None`
by construction, not a mapping gap.

### 4.10 The collision census (11a)

One line printed with the other `rel.*` census lines (`ALSEM_RETAINED_CENSUS`), per model:
`routine_rows`, `distinct_ids`, `collision_groups`, `collision_rows`, each group split into
one node / several nodes by `node_of`, and by role (primary, dependency source,
symbol-only); and `j2_alias_nodes` (nodes whose `j2_row`'s `node_of` is another node) and
`j5_compat_rows` (event-subscriber rows whose `j5_node` differs from `node_of`), so 11b's
corrections start from a count. `j5_compat_rows` counts subscriber rows only (final panel
r1, FP-01): J5 answers for no other row (§4.9), so counting every row would count each
non-subscriber's `None` against its node, a non-answer and not a gap. A non-subscriber
`#if` duplicate (e.g. `PageManagement.GetPurchaseHeaderPageID`'s rows) is J2's shape and
is counted by `j2_alias_nodes` and the `caller_mismatch` residual, not here.
`j5_compat_rows_counts_the_subscriber_gaps_only` (r4) pins the count on a fixture with both. Measured by the probe (P8, P9): CDO cross-app 31,658 rows, 31,656 ids, 2
groups (both one node, dependency source, `PageManagement` codeunit 700); single-app 5,479
rows, 0 groups; r0 `ws-cross-object-chain` cross-app, one group of two symbol-only `OnRun`
rows of codeunit 60105.

### 4.11 What is deleted

| Item | Where | After |
|---|---|---|
| J1 program side | `program_calls.rs:430-441`; `SiteLinks` (`site_links.rs:58-98`) and its test `links_keep_every_edge_and_qualify_by_app` | deleted (I2 exception, §5.12); its app-qualification half is restated by T4b |
| J1 model side | the `l3_key` calls in `join` (`:472`, `:483`, `:502`) | key lookups; `model_key`/`program_key` move into `program_calls.rs` for the debug assertion; `site_links.rs` deleted |
| `texts` | `:411-427` | deleted |
| J4 | `span_text` `:320-331`, `op_name` `:333-339`, their use `:546-548` | deleted; `fires` replaces them |
| J2 | `by_decl` `:1026-1039`, `Converter::by_decl`, the surface lookup in `l3_routine` `:1178-1191` | `j2_row` (§4.7) |
| the caller check's surface lookup | `:518-531` | `decl_anchor` (§4.8) |
| J5 | `events.rs:288-319` | `j5_node` (§4.9) |
| module doc "Join keys" | `program_calls.rs:10-21` | rewritten |

`census::anchor_key` keeps its readers in `census.rs:134, :160, :234` (the physical census
tool) after J5's use (`events.rs:385`) goes.

### 4.12 Delivery order inside the claim

Each task is one commit (or a few), green on `scripts/ci-steps task`.

1. **Task 1: keys and tables, the old joins still in use.** §4.1, §4.2, §4.2a, `RowNodes`
   (§4.6) and the collision census (§4.10). Production reads nothing new. Test-only
   functions compare old against new on PRISTINE assembly (no hand-mutated input): T1
   (`key_join_parity`, §5.1), T7 (`j2_row` vs J2), T8 (`j5_node` and the event graph vs
   J5). They are called by tests, never from `join` or the event graph, so negative tests
   that move anchors or keys never meet them. `scripts/cdo-gate` builds the `cdo` profile
   (`Cargo.toml:266-267`, release), so each has a CDO test (both universes).
2. **Task 2: the switch.** `join` uses the key join (with the span debug assertion);
   `l3_routine` reads `j2_row`; the caller check reads `decl_anchor`; the event graph reads
   `j5_node`. The demand filter (§4.5) and the cross-app loss gates (T2) land. The whole debug
   suite runs the span assertion on every accepted pair here, while J1 still exists (A10.6).
3. **Task 3: the deletion.** §4.11. The old paths and the three comparison functions are
   deleted with the code they compare; the permanent tests of §5.10 stay.

**The run that licenses the switch and the deletion** (7b §4.7's rule): on the commit just
before Task 2 and on the commit just before Task 3, the implementer runs
`python scripts/agentflow run --name cdo-gate --timeout 45 --cwd <worktree> -- bash scripts/cdo-gate`;
the ledger's `## Gate results` records the commit SHA, the exit status and the T1/T7/T8 CDO
lines. Task 2 / Task 3 is committed only after that record exists and is green.

### 4.13 Memory

| Item | Lives | Size on CDO cross-app (P11, P15) | Charged in |
|---|---|---|---|
| `ClassifiedEdge.site` (`Option<ProgramSite>`, 16 B) | the program body, released at attach (Decision 35) | 16 B × ≈ 477,700 edges (46,328 workspace + 431,248 dependency body + publisher) ≈ 7.6 MB | `census.program` (`ResolvedProgram::census`), new bucket `program.edge_site` |
| `ModelRoutine.{file, site_exprs}` | the rows, assembly → attach: only the join and the census read them, so the attach clears them (the key vectors are freed; the inline 80 B per row stays in the rows' buffer, charged by `model`) (final panel r1, FP-12) | 4 B × (81,745 + 33,290 + 33,493) sites + 3 × 24 B vector headers + 8 B per row × 31,658 rows ≈ 3.1 MB | new bucket `model.site_exprs`, emitted at the attach (`census.program`) before they are cleared |
| `FileTable` | the `ProgramContext` | one entry per parsed file (`AppRef` + `String` path + map slot, ≈ 100-150 B); the file count was not read at spec time | `census.program`, new bucket `program.file_table` |
| `RowNodes` | build-local (assembly → attach) | `node_of` and `j5_node` 72 B × 31,658 ≈ 2.3 MB each; `decl_anchor` (callers of read edges only) and `j2_row` (matched nodes only, ≈ one per row; the extra entries are the cross-node aliases) ≈ 31,700 entries × (72 + 12 or 4 + slot) / 0.875 load ≈ 3.2 MB and 2.9 MB; ≈ 11 MB | `census.program`, bucket `program.row_nodes`, marked at attach |
| deleted: the join's `String`-keyed maps (`SiteLinks`, `program`, `consumed`, `l3_*`) | transient inside the join | not measured (P3 has no byte census) | — |

Expected net growth at the process peak (set in attach, epic §5.12): ≈ 12-20 MB up, minus the
deleted maps. B2's 3 % band (96.7 MB cross-app) is about 5-8 × that, so B2 can only catch a
gross mistake; the census (§8 part 1) is the real check and is reported per bucket (§10 F8).
The census command: `ALSEM_RETAINED_CENSUS=1 alsem analyze <ws> --format sarif`, one run per
side and universe.

## 5. Test plan

Every new test lists its break (made with Edit, reverted with Edit), the failing assertion
and the revert, and both outcomes are recorded (I9, `discrimination-proof` skill). A scripted
break asserts it applied.

### 5.1 T1: key/span parity (Task 1; deleted at Task 3)

`key_join_parity(view, ctx, ws, demand) -> ParityReport` (test-only) runs both joins and
both conversions on the SAME pristine model and returns three separate comparisons:
1. **Accepted pairs, row-indexed:** for each `(ri, ci)` and `(ri, oi)`, the edge each join
   accepted (or none).
2. **Converted output:** the full `ResolvedCalls` of each, grouped by `(from, callsite id)`:
   every `CallEdge` (`Debug`), every `ExternalTargetRef`, every `upgraded_bindings` entry.
3. **Census:** both censuses, under the same demand setting (cross-app: the 10A-2 filter,
   as production runs at Task 2), compared counter by counter against EXACT stated deltas.
   A span duplicate is two keys, so each extra edge the span join set aside as
   `duplicate_program_span` is classified on the key side like any other edge: a call edge is
   consumed by its own arm's call site (no counter), a non-firing operation edge becomes an
   `operation_site`, a firing one an `implicit_trigger_matched`. Expected, derived from P15
   (not measured in key mode; the probe's key mode recomputed pairs, not the census):
   - single-app CDO and every r0 / `tests/fixtures` dir: every counter equal (0 duplicates
     either side, P2, P15);
   - cross-app CDO: `duplicate_program_span` 24 → 0, `duplicate_l3_span` 23 → 0,
     `operation_site` +1, every other counter equal. The 24 span duplicates are the 23
     call edges of R-PAIRS' second arms (consumed by their own sites on the key side; the
     23 model call-site span duplicates are those same sites) plus one operation edge, the
     non-firing record op at `AccessToken.Table.al` 68:11 of `AcquireTokenFromCache`'s
     second arm (P6 listed it; P15 found 0 differing accepted op pairs, so it does not fire),
     which the key side counts as an `operation_site`.
   T1 asserts these deltas exactly at Task 1; any other delta is F2.

Tests:
- `key_join_equals_span_join_on_the_fixtures` (r0 dirs with an `app.json` and
  `tests/fixtures`, both universes): pairs, conversion and census identical (P2 measured 0
  differences on these dirs).
- `key_join_equals_span_join_on_cdo` (`cdo_ws_or_enforce`, both universes): single-app
  identical; cross-app: the set of differing accepted pairs EQUALS R-PAIRS (§3.5) as a set of
  `(row id, site id, unit, start)`; the two non-equivalent pairs carry the hand-stated target
  nodes of §3.5 (`acquiretokenfromcache`/5, `sig_fp` 9686704692977790200 by span,
  8102086367325882817 by key); conversion identical; census as point 3.
Break: swap the first two call sites' `site_exprs` entries of one fixture row → pairs differ
→ fails. Revert → green.

### 5.2 T2: the loss gates, both universes (kept)

- Existing single-app gates (`tests/program_resolve_harness.rs:10612-10654`), unchanged.
- New: `adapter_loses_nothing_cross_app_on_the_fixtures` (`losses() == []`) and
  `adapter_cross_app_losses_on_cdo`, through a new entry
  `adapter_census_for_workspace_cross_app(ws)` that shares
  `assemble_and_resolve_cross_app_program`'s code (a helper both call, not a copy), so the
  test pins the use. The CDO gate asserts `losses()` equals `[("caller_mismatch", 5),
  ("shape_mismatch", 18), ("adapter_l3_fallback_sites", 23)]` AND the census's `unmatched`
  set equals R-RESIDUAL exactly (`(reason, unit, start, end)`; set equality, so a new loss
  replacing a fixed one fails).
- New `--lib`, dependency fixture (`program_calls_adapter_tests.rs`, a source-bearing
  dependency via `test_apps`, as `interface_into_a_source_bearing_dependency_keeps_its_implementer`
  does): `cross_app_census_has_no_losses_with_a_dependency_body`. Stated preconditions,
  asserted first: (a) a demanded dependency routine `DepA` whose call `DepB()` is `matched`;
  (b) a NON-demanded dependency routine `DepUnused` with one ordinary call `DepB()` (a
  `Call`-kind program edge, asserted present in the program's edges and absent from the
  model's rows). Then `losses() == []`.
  **Epic discrimination (row 10):** in the key join, replace `DepA`'s call-site key with
  `u32::MAX` (an index no expression has, §4.1, so it can pair nothing and the
  span assertion is never reached) → `no_program_site` 1 and `program_only_site` 1 (`DepA`'s
  now-unconsumed edge), asserted by name → fails. Revert → green.
  **10A-2 discrimination:** drop the demand filter → `DepUnused`'s edge is a
  `program_only_site`, asserted by name → fails. Revert → green.

### 5.3 T3: tests restated on the key (they unpaired by moving an anchor)

Moving a site's anchor no longer unpairs it; moving a row's anchor no longer changes J2.
Each test below keeps its expected outcome and states its precondition on the new input:
- `l3_ops_with_no_program_site_are_counted_by_trigger_capability` (`program_calls.rs:2318-2350`):
  replace each original expression index `e` of routine `P` by `u32::MAX - e` (its 2
  record ops and 3 operation sites). Distinct originals stay distinct, so no within-category
  duplicate is created, and each record op keeps the same key as its own operation site
  (the walk's correspondence, §3.3); no unused key equals a real one (the test asserts every
  index of the file is below `u32::MAX / 2`).
- `a_site_without_a_program_edge_is_unknown_not_legacy_resolved` (`:2632+`): same rule
  (`u32::MAX - e`), for the one call site.
- `duplicate_l3_span_is_counted` (`:2357-2374`): the clone of a call site also copies its
  `site_exprs` entry (stated in the test).
- `unmatched_record_op_gets_no_l3_trigger_edge` (`program_calls_adapter_tests.rs:1326-1341`)
  and `unmatched_site_is_unknown_not_l3` (`:1347+`): the same `u32::MAX - e` rule, for the
  record op AND its own operation site (one key, kept shared) / for the call site.
- `interface_implementer_outside_l3_is_unknown` (`program_calls_adapter_tests.rs:396-415`):
  remove `Go`'s node from `row_nodes.j2_row` instead of moving `Go`'s anchor (the
  `adapt_with` helper gains access to `RowNodes`).
- `can_fire_trigger_admits_the_resolver_ops` (`program_calls.rs:2070-2092`): its `op_name`
  half goes with `op_name`; its `can_fire_trigger` half moves with the function; T6 restates
  the text-to-op half on the resolver side.
- Kept as they are: `callee_text_disagreement_is_callee_fp_mismatch`,
  `caller_anchor_disagreement_is_caller_mismatch` (§4.8: the captured `decl_anchor` keeps it
  failing), and the span-convention tests (`non_ascii_before_call_on_same_line_pairs_by_byte_column`,
  `nested_same_callee_calls_on_one_line_pair_exactly`, `bom_file_pairs_line_zero_calls`,
  `non_utf8_byte_before_call_pairs`, `implicit_trigger_and_operation_sites`): after Task 2
  they exercise the span debug assertion.
Break: make the key join pair ops by `(ri, oi)` position instead of key → the first test's
`l3_op_no_program_site_*` drop to 0 → fails. Revert.

### 5.4 T4: duplicates by key; T4b: app qualification

- `duplicate_program_key_is_counted` (`--lib`): a copy of one edge pushed into the view (as
  `site_links.rs:123-135` does today) → `duplicate_program_span == 1` and `losses()` names it.
  Break: last-wins without counting → fails. Revert.
- `duplicate_model_key_per_category` (`--lib`): two call sites of one row given one key by
  assignment → `duplicate_l3_span == 1`; a record op and its own operation site at one key
  (the walk's normal shape) → 0. Break: count across categories → the second assertion fails.
- **T4b** `two_apps_same_path_same_expr_pair_within_their_app` (cross-app, `--lib`): two
  source-bearing dependencies each holding `src/Main.al` with IDENTICAL text (so identical
  `ExprId`s), each calling its own `Helper()`. Preconditions asserted: the two files have
  different `FileIx` and their call's `ExprId` is equal. Then each app's site pairs with its
  own app's edge (`from.object.app`). Break: key `FileTable` by path only → one app's site
  pairs with the other's edge → `caller_mismatch` → fails. Revert.

### 5.5 T5: the span assertion can fire

`#[cfg(debug_assertions)] #[should_panic]`: a call site whose key is right and whose anchor
is moved one column → the §4.3 step 4 assertion panics. Break: delete the assertion → no
panic → fails. Revert.

### 5.6 T6: `fires`

`record_op_site_fires_matches_can_fire_trigger` (`--lib`, resolver side): `Rec.Insert`,
`Rec.SetRange`, `Rec.ModifyAll`, implicit-`Rec` `Modify()`, `T.ModifyAll(Amount, 0, true)` →
true, false, true, true, true. Break: `fires = true` always → the `SetRange` assertion fails,
and the T3 test moves `operation_site` 2 → 1. Revert.

### 5.7 T7: J2 oracle (11a; Task 1, deleted at Task 3)

`j2_row_equals_decl_join`: for every node with a model row (and every route target the
adapter resolves), old `l3_routine` and `j2_row` return the same row, on r0,
`tests/fixtures`, the cross-app gap fixtures, and (CDO variant, both universes) CDO. Asserts
0 differences. Break: compute `j2_row` within `rows_of[node]` only (per node) → the T7b
cross-node fixture differs → fails. Revert.

### 5.8 T8: J5 oracle (11a; Task 1, deleted at Task 3)

`event_graph_j5_node_equals_span_join`: `program_event_graph` old against new on the same
fixture set and CDO (both universes). Comparator: `format!("{:?}", graph)` equality
(`EventGraph`, `EventSymbol`, `EventEdge` hold only `Vec`s, `Option`s and scalars, so the
`Debug` text is complete and order-sensitive, `events.rs:14-111`; it includes the
`conditional` and `mixed_readings` fields the serialized projection drops) AND
`EventCensus` equality (it derives `PartialEq`, `:179`). Asserts equality everywhere.
Break: leave dependency rows out of `j5_node` → a dependency subscriber's edge disappears in
the cross-app fixture → fails. Revert. If no existing fixture has a source-bearing dependency
subscriber, T8 hand-states one in the T2 dependency fixture.

### 5.9 T9: the caller check, the collision rules, the census

- `caller_check_reads_captured_decl_anchor`: assemble a fixture, then set
  `row_nodes.decl_anchor[node]` of the caller's node one column off by assignment (the row is
  untouched) → its sites are `caller_mismatch`. Break: compare the row's anchor with
  `j2_row`'s row anchor (a row-derived side) → the sites match → fails. Revert. Together with
  the unchanged `caller_anchor_disagreement_is_caller_mismatch` (row moved, captured anchor
  fixed), both sides of the comparison are pinned.
- The epic's 11a proof: `collision_rules_hand_stated` (beside `detector_context`'s collision
  tests): two `OnAction` triggers with different bodies, B's `id` and `stable_routine_id` set
  to A's by assignment (the technique of `detector_context.rs`'s
  `hand_stated_id_collision_keeps_a_real_summary_and_derived_row`). It asserts today's three
  rules: edges union (R1, `combined_graph.rs` `edges_by_from`), direct facts and coverage
  last-wins (R2, `det_cone_inputs`, `detector_context.rs:1240-1261`), summary first-wins
  (R3, `:1388-1389`). It passes on 11a unchanged (11a changes no rule). Break:
  `direct_full.insert` → `entry().or_insert` at `:1261` → the R2 assertion fails. Revert.
  11b flips it to the union rule.
- `collision_census_counts_groups`: the r0 fixture `ws-cross-object-chain` cross-app gives
  1 group of 2 symbol-only rows (P8). Break: count distinct rows instead of ids → 0 → fails.

### 5.10 Permanent shape fixtures (replace the oracles' coverage after Task 3)

Hand-stated AL, each asserting its shape precondition first (two rows, anchors, nodes, as
stated), so a lowerer change that removes the shape fails the precondition rather than
passing silently. The real `#if` precedent is `r4_cross_app_program.rs`'s
`if_dup_dependency_workspace` (a dependency `#if`/`#else` duplicate).
- **T7a different anchors, one node** (the `GetPurchaseHeaderPageID` shape): a procedure
  declared in `#if A` / `#else` arms at two lines, same signature. Asserts: one node, two
  rows; `j2_row` = the second arm's row; the first arm's sites `caller_mismatch` (today's
  answer, R-RESIDUAL's shape).
- **T7b one anchor, two nodes** (the `AcquireTokenFromCache` shape): a split header whose arms
  differ in a parameter type, so the two decls have different `sig_fp`. Asserts: two nodes,
  two rows at one anchor; `j2_row` of BOTH nodes = the first row (the recorded alias).
- **T7c one anchor, one node, two rows** (the `GetSalesHeaderPageID` twins): asserts
  `j2_row` = the first row.
- **T8a J5 shapes**: in a source-bearing dependency, T7a's and T7b's shapes declared as event
  subscribers. Asserts `j5_node` of T7b's second row = the FIRST row's node, and of T7a's
  first arm = `None`; and the event graph's edges per subscriber as hand-stated.
- **A8 (#103) on the 11a side** (`tests/r4/r4_cross_app_program.rs:1725-1782`,
  `a_dependencys_if_duplicated_subscriber_gets_each_edge_once`): its stated limit ("the first
  row does not map" is NOT asserted, the span map was internal) is replaced by an asserted
  precondition: `row_nodes.j5_node[first row] == None` and `j5_node[second row] == Some(_)`,
  read through a `#[doc(hidden)]` entry that returns the cross-app assembly's `ModelRows`
  with its `RowNodes` before attach (`RowNodes` is build-local, so production never keeps it). It keeps discriminating break 8 through 11a.
  11b, which corrects `j5_node` to `node_of`, fails that precondition by design and must
  restate the test (recorded for 11b's spec).
Breaks: T7a/T7c `j2_row` = last row → T7c fails; T7b per-node selection → fails; T8a
`j5_node = node_of` → both J5 assertions fail. Revert each.

### 5.11 T10: the LSP's unstamped path

`recompute_file` returns `LspEdge`s (`src/lsp/snapshot.rs:1145-1155`), not
`ClassifiedEdge`s, so no test can see a stamp through it. The stamp is therefore pinned at
the function the LSP calls, and the LSP's output separately:
- `resolve_file_obligations_unstamped_and_stamped` (`--lib`, `full.rs` tests): for an r0
  fixture file, call `resolve_file_obligations(…, None)` directly (the exact call
  `recompute_file` makes) → every edge has `site == None`; call it with `Some(f)` → every
  call-site edge has `Some` with `at.file == f`, and the two edge lists are equal apart from
  `site`. Break: stamp `Some(FileIx(0))` when `file` is `None` → the `None` assertion fails.
  Revert.
- The existing `resolve_file_obligations_matches_full_run_per_file_and_in_concatenation`
  (`full.rs:4199`) passes `Some(ctx.file_ix(app, path))` and additionally asserts every
  call-site edge's stamp equals it.
- LSP output parity: the existing batch/incremental tests of `recompute_file` (updater
  rung-1/rung-2 against a fresh snapshot) stay green unchanged; they compare `LspEdge`s,
  which carry no key, so they show the unstamped path gives the LSP the same edges.

### 5.12 Tests deleted or rewritten with the code they pin (I2's exception)

Deleted: `links_keep_every_edge_and_qualify_by_app` (with `SiteLinks`; T4b restates its
app half, T4 its every-edge half); T1, T7, T8 (Task 3, with the old paths). Rewritten: the
seven tests of §5.3. Mechanical: the 12 `ModelRoutine` (9 in `src`, 3 in `tests`) and 10 `ClassifiedEdge` test literals
(§4.2, §3.3) gain the new fields; the two `resolve_file_obligations` test callers pass a file.
Changed by assertion only: A8 (§5.10).

## 6. Acceptance matrix

| # | Epic item | Evidence |
|---|---|---|
| A10.1 | `SiteIx` on edges, call sites, ops | §4.2: `ClassifiedEdge.site`, `ModelRoutine.{file, site_exprs}` (10A-1) |
| A10.2 | Join by key, span as a debug assertion | §4.3; T5 |
| A10.3 | Delete J1, `texts`, J4 | §4.11; `rg 'span_text|op_name|SiteLinks|texts:' src/program/model` empty after Task 3 |
| A10.4 | `losses() == []` both universes on r0 + CDO | T2: single-app r0 and CDO `[]`; cross-app r0 and the dependency fixture `[]`; cross-app CDO = R-RESIDUAL exactly (10A-2, 10A-3) |
| A10.5 | `edge_order_is_call_sites_then_triggers` green | §4.3, unchanged test |
| A10.6 | Debug span assertion on the whole suite before deletion | Task 2's `scripts/ci-steps task` (debug) runs the assertion on every accepted pair while J1 still exists; T1's CDO variant in the recorded pre-Task-2 `cdo-gate`; both in the ledger before Task 3 |
| A10.7 | Mis-pair one dependency site → the loss gate fails | T2's discrimination, both outcomes recorded |
| A10.8 | Attach saving ≥ parent `b3.adapter.join` − band(join), if join median ≥ 2 × band | B1 and B3 (§8); void checks first |
| A10.9 | Both peaks ≤ parent + band | B2 (§8): kernel peak RSS (process); post-attach not readable (10A-4) |
| A10.10 | Mints `FileIx`, keeps `caller_mismatch` for 11a | §4.1; §4.8 |
| A11a.1 | `RoutineIx`, dependency rows included | §4.6 (position, 11A-2); `node_of` covers workspace, dependency source and symbol-only rows (T8, T9, T10) |
| A11a.2 | `RowNodes` at assembly replaces `caller_mismatch` | §4.8; T9 |
| A11a.3 | J2, J5 deleted | §4.11; T7, T8 before; `rg 'by_decl|node_by_span' src` empty after Task 3; no `b3.adapter.by_decl` span in the head trace |
| A11a.4 | Reuses step 10's `FileIx` | §4.1 (one table) |
| A11a.5 | The collision census | §4.10; T9's census test; CDO line both universes in the numbers file |
| A11a.6 | Output-neutral, no rule change; the hand-stated collision fixture passes unchanged | T9 `collision_rules_hand_stated`; T1 (conversion identical); T7, T8; E2 below |
| A11a.7 | Non-speed, no bench (Decision 58) | §8: 11a has no bound; its CDO evidence is `cdo-gate` + identity |
| E2/I2 | Byte-identical | the 8 golden targets (`scripts/check-goldens`) and `cargo test -p al-sem --lib` outcomes unchanged (apart from §5.12); CDO histogram SHA unchanged; CDO SARIF at `--scope primary` and `--scope all`, both universes, identical |
| E14 | CDO ratchets | `scripts/cdo-gate` green |

**I6 for every bound.**

| Bound | Timer | Median at this base | Band at this base | ≥ 2 × band | Re-checked |
|---|---|--:|--:|---|---|
| B1 (A10.8) | `b3.adapter.join` (P4); bound read on `b3.adapter` | 8,108.9 ms (`b3.adapter` 8,334.6) | 571.8 (591.9) | yes, 14.2×; margin 6,945.2 ≥ 1,183.8 | the final bench's parent runs; void blocks |
| B3 (A10.8, net) | `b3.adapter.join` against `analyze.total` | 8,108.9 ms (`analyze.total` 26,140.4) | 571.8 (1,792.9) | margin 5,744.2 ≥ 3,585.8 | same |
| — single-app join (reported, 10A-4) | same | 80.7 ms (151.8) | 2.4 (8.6) | yes, 33.6× | reported only |
| B2 (A10.9) process peak | kernel peak RSS (P5) | 3,222.6 MB cross-app, 704.8 MB single-app | 3 % of the parent median (96.7 / 21.1 MB; observed max − min 31.0 / 15.6) | n/a | same |
| — `b3.adapter.by_decl` (11a, reported) | span | 9.6 ms / 0.8 ms | 0.6 / 0.1 | — | non-speed (Decision 58) |

Invariants touched: I1 (no build counter moves), I2, I6, I7 (§4.3, §4.6), I9 (§5), I10.
I3, I4, I5, I8, I11 untouched.

## 7. False-positive and failure shapes

- **Silent mis-pair by key.** Two sites with one key on one side. Guard: both duplicate
  counters are losses (T4); 0 duplicate program keys and 0 duplicate model keys per category
  measured on CDO (P6, P15); the span assertion catches an accepted pair whose spans differ
  (T5, debug).
- **Two apps, one path** (a `FileTable` keyed without the app): T4b.
- **The key join pairs differently from the span join beyond R-PAIRS**: T1 fails closed (set
  equality).
- **A filter that hides a loss** (10A-2): the predicate is `demanded_population`'s; T2's
  dependency fixture states a matched demanded site and a non-demanded ordinary call.
- **A site-bearing row with `file == None`**: a loss plus a `debug_assert!`.
- **The cross-app residual moves** (up, down or sideways): T2's CDO gate asserts the exact
  set.
- **`j2_row` ≠ J2 or `j5_node` ≠ J5**: T7, T8 before the switch; T7a-c, T8a after.
- **The LSP stamps or reads a key**: T10.
- **A vacuous speed bound**: the void checks of §8.
- **A peak the platform hides**: 10A-4; the census buckets of §4.13.
- **Noise.** server1 is shared (a rust-analyzer process and other sessions ran during the
  probes; load averages are in the ledger). §8's re-run rule.

## 8. Measurement plan

One CDO bench, run ONCE after the final panel, by the conductor through
`agentflow run --name bench --timeout 45 -- <cmd>`, in 7b's three-part shape (7a's
`check.sh` fail-closed rules reused). `B=/work/bench/160-10-11a-final`, `WS` = the pinned
CDO workspace read from `cdo_ws` in the main checkout's `.agent/config.json` at run time
(never written into a file), `P` = the branch base, `H` = the branch head.
`export TREE_SITTER_AL_PATH` to the main checkout's grammar clone; `RAYON_NUM_THREADS` unset;
`release-fast`; `uptime` before, between and after the parts.

**Peaks are read with the kernel's high-water mark, not by sampling** (R1-25). The operator's
instruction for this claim was psutil RSS sampling (`read_rss` is a stub off Windows,
`src/engine/perf_trace.rs:174-177`, #204). `bench_ab.py`'s psutil number samples RSS every
20 ms (`scripts/bench_ab.py`), so a peak shorter than the interval is missed.
`/work/bench/160-10-11a/maxrss.py` reads `getrusage(RUSAGE_CHILDREN).ru_maxrss` after the
child exits: the child's `VmHWM`, the same quantity (peak resident set) tracked by the kernel
on every page fault, with no sampling gap. At the median the two agreed within 3 MB on the
probe runs (P5: cross-app 3,219.9 psutil vs 3,222.6 MB `VmHWM`; single-app 707.4 vs 704.8); B2 binds the `VmHWM` reading, and the psutil one is reported beside it.

- **Part 1 (identity; 10 and 11a).** Build both sides from clean trees. Histogram SHA equal.
  SARIF `cmp` at `--scope primary` and `--scope all`, cross-app and `--single-app`. The
  collision-census line, both universes, head. One `ALSEM_RETAINED_CENSUS=1` analyze per side
  and universe: every bucket of §4.13, head against parent. The CDO variants of T1, T7, T8 are
  read from the recorded pre-Task-2 `cdo-gate` (they no longer exist at the head).
- **Part 2 (bounded; 10 only).** `bench_ab.py run --rounds 5`, interleaved, commands
  `a` = `analyze $WS --format sarif` and `s` = `analyze $WS --single-app --format sarif`.
  Read `b3.adapter`, `b3.adapter.join`, `analyze.total` and `crossapp.attach_program_calls`
  with `trace_summary.py --compare`. Then 3 rounds per side and command through `maxrss.py`.
- **Part 3 (reported, no bound; Decision 58: 3 rounds, touched commands only).** `digest
  --diff` and `fingerprint --format json` cross-app (the other commands that run the adapter),
  walls and kernel peaks. 11a adds no command.

**Bounds** (bands ONLY from this bench's parent runs, I6, Decision 20):

| Bound | Reading | Rule | Void check (read first) |
|---|---|---|---|
| B1 (A10.8) | span `b3.adapter`, cross-app `analyze` (it holds the join on both sides; `b3.adapter.join` stays on the head as the key join's span) | `Pa − Ha ≥ Pj − Bj − Ba` | `Pj ≥ 2 × Bj` and `Pj − Bj − Ba ≥ 2 × Ba` |
| B3 (A10.8, net) | span `analyze.total`, cross-app `analyze` (it encloses the moved preparation work: the `FileTable` in context build, the stamps in resolution, `site_exprs` and `RowNodes` in assembly) | `Pt − Ht ≥ Pj − Bj − Bt` | `Pj − Bj − Bt ≥ 2 × Bt` |
| B2-a / B2-s (A10.9) | kernel peak RSS, cross-app and single-app `analyze` | `Hm ≤ Pm + 0.03 × Pm` | none (the 3 % band is the epic's, §5.12) |

`P` / `H` = the parent's / head's median, `B` = the parent's band (max − min), `a` =
`b3.adapter`, `j` = `b3.adapter.join`, `t` = `analyze.total`, `m` = kernel peak. The
single-app join is read and reported with B1's formula, not bound: its expected saving
(≈ 70-75 ms) sits within a few ms of its bound (69.7 ms), so it would measure the noise of a
3.8 s command, not the step (10A-4).

Expected (P3/P4, not a claim): the head's key join costs about what the probe's `program` +
`model` sub-spans cost without their `String` keys, a few hundred ms at most; `Pa − Ha` ≈
7.8-8.0 s against B1's ≈ 6.9 s; `Pt − Ht` about the same minus the preparation work (tens of
ms) against B3's ≈ 5.7 s.

**A void bound (pre-stated, binding; 7b §8).** A void bound never passes. If either void
check is void, re-run Part 2 once (fresh 5 rounds per side, same exclusive slot) and read
both checks and all bounds from the re-run alone. Void again → A10.8 **FAILS** (§10 F9). It
is never "merge with void recorded" and never a re-label.

**A met-not-void bound MISSED (pre-stated, binding).** §10 F10.

Numbers go to `docs/measurements/2026-10-22-issue-160-steps10-11a.md`.

## 9. Amendments this spec asks the epic to take

- **10A-1: `SiteIx` is the key `(FileIx, ExprId)`, not a dense `u32`.** A dense index needs
  a minting pass both walks agree on and has no reader before step 6 (whose arena keys on
  `EdgeIx` / `DirectFactIx`, G-4). The key is 8 bytes, `Copy`, deterministic per build.
- **10A-2: the cross-app census counts program sites of callers the model holds** (§4.5).
  Without it, "`losses() == []` cross-app" is false by construction (249,940 non-demanded
  losses on CDO, P1).
- **10A-3: the cross-app CDO loss gate asserts R-RESIDUAL (§3.5), not `[]`.** 18
  `shape_mismatch` (XmlPort table-element record ops the body walk misses) and 5
  `caller_mismatch` (the collision group's first `#if` arm), with their 23
  `adapter_l3_fallback_sites`. Fixing either moves output, so neither is in this byte-neutral
  claim. Discoveries filed with this claim: (1) "body walk: XmlPort table elements are record
  receivers (18 Base Application sites on CDO)"; (2) "11b: J2's cross-node alias, J5's two
  mapping gaps and the collision group's 5 `NoProgramSite` sites" (the compatibility
  answers of §4.7-§4.9, with `RowNodes.node_of` as the correction's input). Cross-app r0 and
  the dependency fixture stay `[]`.
- **10A-4: the speed bound is B1 on `b3.adapter` plus B3 on `analyze.total`, both
  subtracting the band of the timer read; B2 is the kernel peak RSS, and the post-attach peak
  is not bounded on Linux.** `b3.adapter.join` survives as the key join's span; B1 reads the
  enclosing adapter span so cost moved into `by_decl`/`convert` cannot hide, and B3 reads the
  whole command so cost moved upstream (context, resolution, assembly) cannot hide either;
  I6 subtracts the band of the timer read (7a's 7A-1, 7b's 7B-3). The bounds are on cross-app
  `analyze` (the command and universe step 0 measured the join on); the single-app join is
  reported with B1's formula (P4). `read_rss` is a stub off Windows, so `peak.process_mb` and
  `peak.post_attach_mb` read 0 on server1 (#204, deferred by the operator); the kernel's
  per-process high-water mark replaces the process peak (§8), and there is no Linux reading
  of a phase peak. The post-attach peak is reported "not measured", and the epic's row-10
  "both peaks" is met for the process peak only until #204 lands.
- **10A-5: "byte-identical" for step 10 is read on the converted model, not on the accepted
  pairs.** At the 23 R-PAIRS sites the key join accepts a different program edge than the
  span join (each `#if` arm's own); the converted calls are identical (P15), and T1 asserts
  both facts. No output allowance is needed.
- **11A-1: steps 10 and 11a in one claim** — taken by the epic (Decision 57); listed for
  completeness.
- **11A-2: `RoutineIx` is the row position, with no stored field.** Epic §5.4 says "rows gain
  `ix`"; position gives the same number, and no reader holds a row apart from its vector in
  11a. 11b adds a field if a reader needs one.
- **11A-3: "J2, J5 deleted" means their per-use derivations are deleted; their answers are
  recorded at assembly (`j2_row`, `j5_node`), wrong answers included.** The epic row says
  11a is output-neutral; P15 shows J2's cross-node alias is what keeps R-PAIRS' two
  non-equivalent pairs identical, and J5's gaps decide which `#if` row emits event edges. So
  11a keeps today's answers as named compatibility targets beside the true `node_of`, and the
  corrections are 11b's (10A-3 discovery 2).
- **11A-4: the L4 interner's `RoutineIx` is not renamed in 11a.** 11a introduces no type of
  that name (11A-2); 11b, which introduces stored `RoutineIx` keys, renames the interner's
  type (`SolveIx`, the step-11 pre-draft's D11-1).

## 10. Fallbacks (every "if the measurement shows X")

Every fallback that changes code re-runs EVERY identity gate it could affect (T1, T2, T7,
T8, the golden targets, and §8 part 1 if the bench already ran) before the claim continues;
a fallback that cannot restore identity stops the claim at Task 1 (keys and tables stamped,
nothing switched), with the reason in the ledger.

| # | If | Then (applied without a new panel round; recorded in the ledger) |
|---|---|---|
| F1 | T1 on r0 or `tests/fixtures` shows ANY difference (pairs, conversion or census) | Task 2 does not start. Triage the site's shape; if the extractor and the walk key different nodes for one construct, align the walk to the extractor's node in `ir_walk.rs` (the extractor decided the edge) in its own commit with its own test; re-run T1. Not closed by that one fix → stop at Task 1 |
| F2 | T1 on CDO: the differing accepted pairs are not exactly R-PAIRS, or any conversion differs, or the census deltas of §5.1 point 3 differ | Same as F1 for the new shape. R-PAIRS is never extended to admit a site whose conversion differs; it may be extended only by a pair whose conversion is identical AND whose shape is `#if`-split, with the evidence in the ledger. The census deltas were derived from P15, not measured: a wrong derivation may be corrected (spec and ledger, with the measured numbers) only when the pairs are exactly R-PAIRS, every conversion is identical, and every counter that moved is traced to an `#if`-split duplicate; anything else is a defect (F4's rule) |
| F3 | Part 1: CDO SARIF (either scope, either universe) or the histogram SHA differs | A defect (T1 says the converted calls are identical, so nothing downstream can move). Fix and re-run Part 1 and T1/T7/T8; not fixed in one attempt → the claim stops (blocked), nothing merges |
| F4 | Part 1: a golden target or a `--lib` outcome outside §5.12 differs | Same as F3 |
| F5 | T7 finds a node where `j2_row` ≠ J2 | `j2_row` is by construction J2's two maps evaluated once; a difference means the surface or the row set seen at assembly is not the adapter's. Make assembly read the same surface and rows (one source), re-run T7; not closed → stop at Task 1 |
| F6 | T8 finds `j5_node` / the event graph ≠ J5's | Same rule as F5 for J5's two maps (the physical rows; the surface's spans in graph-routine order), including `None`; not closed → stop at Task 1 |
| F7 | The cross-app residual on CDO is not exactly R-RESIDUAL at Task 2 | T2 fails by design. Triage: a key-join or filter defect → fix, re-run T1 and T2; the set is never updated to admit a new identity in this claim. Not closed in one attempt → stop at Task 1 |
| F8 | Part 1's census: a bucket of §4.13 exceeds its estimate by more than 50 %, or B2 misses | Name the bucket. Store `u32` indices into `graph.routines` instead of `RoutineNodeId` clones in `RowNodes`, and pack `fires` into `FileIx`'s top bit (`ProgramSite` 8 bytes, `Option` 12); re-run Part 1 and Part 2 once. B2 still missed → B2 FAILS (F10's rule) |
| F9 | B1 or B3 void twice (§8) | A10.8 **FAILS**: the claim stops at blocked; nothing merges. The ledger records both runs' readings. No re-labelling after the fact: the epic row's "else a non-speed step" was a feasibility test on the step-0 median, and it was met (8,108.9 ≥ 2 × 571.8) |
| F10 | B1, B3 or B2 met-not-void but MISSED | Name the miss with `trace_summary.py`: if the head's `b3.adapter.join` is not ≈ 0, profile it with sub-spans (the probe patch's `b3.join.*` shape); for B3 find the grown span; for B2 the grown census bucket. ONE fix, then re-run Part 2 once. Still missed → the bound FAILS: the claim stops at blocked, nothing merges (7b §8's rule: there is no second design attempt within the claim) |
| F11 | The panel rejects one claim for 10 + 11a | Not applicable: Decision 57 is the epic's |

## 11. Risks and open questions

1. **The compatibility answers keep known wrong pairings** (J2's alias, J5's gaps, the 5
   `caller_mismatch` sites). That is the price of a byte-neutral 11a; 11b's discovery carries
   the count (`j2_alias_nodes`, `j5_compat_rows`, §4.10).
2. **Does demand equal the model's population for every dependency file?** The filter uses
   the same node ids as `demanded_population`; a dependency app `graph.apps.find` misses gets
   `whole_file_population` (`workspace.rs:1559`) and is counted whole. T2 states a
   non-demanded routine; CDO's exact residual (F7) is the population check.
3. **B2 is coarse** (§4.13): it catches only a gross memory mistake; the census buckets carry
   the real reading.
4. **The l2 snapshot is a `Debug` hash.** A future change that moves the keys onto the feature
   types would move it; §2 states why they are not there.
