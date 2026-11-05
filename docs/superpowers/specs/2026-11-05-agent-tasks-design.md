# Agent tasks no tool serves: design spec (revision 2)

**Status:** design only. No code lands with this file.
**Date:** 2026-11-05. **Based on:** master `751385cd`.
**Revises:** revision 1 (`target/tmp/agent-tasks-design.md`, not committed) after two reviews:
astra (`target/tmp/agent-tasks-review-astra.md`, approve with changes, 16 findings) and Fable
(`target/tmp/agent-tasks-review-fable.md`, approve with changes, 16 findings). The owner's
decisions on the order and the open questions are applied (§13).
**Measured on:** the CDO baseline `/home/sshadows/Projects/DO-cdo-baseline/Cloud` (`bc3ccb18`,
passed as an argument; `CDO_WS` not set), with the installed `~/.al-sem/bin/alsem` (2.1.1).
Scratch: `target/tmp/agent-tasks-scratch/`. A number marked **measured** comes from those runs
(Appendix A). A number marked **estimate** is not measured; the milestone that builds the
feature measures it before it lands, and its measurement plan says how.

Words used here:

- **insertion point**: the routine (optionally a line in it) where an agent wants to add a
  COMMIT, an `Error`, or UI.
- **up-direction question**: a question about the routine's callers (the context R runs in).
  Today's prove questions are **down-direction**: what R and its callees already do. This spec
  calls their answers **existing effects**.
- **reverse cone**: every routine and object that may execute R, found by walking the
  may-execute graph backwards.
- **site**: one call site or one operation site (a record operation, a `Commit()`, an `Error`)
  in a routine body, with its own id and source location.
- **sited graph**: the may-execute graph with one edge per site (§3). The exported
  `alsem graph` file is a **collapsed** form of it: one edge per (caller, callee, kind).
- **entry kind**: what kind of thing starts a run (a page action, an API page, a job-queue
  entry, an install codeunit, a test, a public procedure other apps may call). It comes from
  what the source declares, not from the graph having no caller there (§3.3).
- **test universe**: a program rooted at a test app, with the workspace app loaded as a source
  dependency, so tests and the code they call are in one graph.
- **base revision**: the code before a change (a git commit), used for before/after deltas.
- **local path**: a path whose every hop is in the workspace app or a loaded test app.
- **refresh class**: the session's own classes (`src/program/session/classify.rs:627-634`):
  `BodyOnly` (B), `Declaration` (S; a file added, removed or renamed is S), `Package` (P).
- **revision vector**: the revisions (generation and input stamp) of every engine an answer read.

---

## Changes from r1

Every numbered finding of both reviews, what r2 does with it, and where. "Accepted" means the
change is in r2 as the reviewer proposed or stronger. "Changed" means accepted with a different
mechanism, stated. Nothing is rebutted outright. Three points are narrowed with code evidence:
a9 (not every hidden channel forces a fallback), f2 (TryFunction routines do have a statement
tree), f7 (master already dropped "safety"; the installed binary is older).

### astra

| # | Finding (short) | r2 | Where |
|---|---|---|---|
| a1 | The exported projection collapses sites by `(from, to, kind)`; A needs per-site order and guards | **Accepted.** Verified: `project.rs` builds `edges: BTreeMap<(Key, Key, Kind), E>` and keeps the minimum line. R0 walks a **sited graph**, one edge per site; the export is a collapse of it and must stay byte-identical. New test: two calls between the same routines with different transaction and GUI contexts | §3.1, R0-7 |
| a2 | One `Reach` per node and "no caller" entries lose information | **Accepted.** Entry kinds come from `root_classification` (semantic), separate from graph sources. The walk keeps the shortest path per **state** `(node, tier, crossedEvent)`; A adds its own state bits. Output counts entries and sites, never "paths" unless counted. `h:any`'s predecessors are walked once as `OnlyOpen`, never dropped | §3.2-3.4 |
| a3 | "Prior write before s" is not "pending write" | **Accepted.** A reuses `pending_writes::pending_before` (branch, loop and `Commit()` rules) and `is_checked_run` (own transaction). An earlier write is not pending after a Commit; a temporary-record trigger proves nothing; missing order or effect facts block `no`. The question is renamed `callers-have-pending-writes`. `CommitBehavior`/`InherentCommitBehavior` on callers are shown | §6.3 |
| a4 | TryFunction semantics wrong; attribute-only trap detection | **Accepted.** The trap is the **invocation edge into** a TryFunction with its result consumed; a discarded result is an ordinary call; an error in the argument evaluation is not trapped. Four kinds: catch-and-return-false, `asserterror`, collectible errors, propagation. The rollback text is corrected (TryFunctions do not roll back database changes, Microsoft docs). Traps do not cross session or task boundaries. Mandatory fixture: checked and unchecked calls to one TryFunction | §6.4, A-4..A-7 |
| a5 | `runs-without-ui` had no UI kind and no completeness rules | **Accepted (option 1).** `callers-may-run-noninteractive` asks only "may R run where no user can answer", independent of a UI kind. Missing publication config, unresolved background targets and unrecognized guards block `no`. Session context resets across async edges. Static background targets are read from the existing capability `start` fact now (also Fable f14) | §6.5 |
| a6 | A child light lane does not help: the MCP worker is blocked on its front end | **Accepted, second option.** Verified: `pool.rs` has one worker per engine owning an `EngineFront`; status is answered on the reader loop from snapshots (`server.rs:339-341`). r2 publishes an immutable **input descriptor** per revision into the worker's snapshot; status checks the disk against it on a checker thread in the MCP process, never on the reader loop and never through the busy front end. Refresh gets `queued/running/completed/failed`. F-3 runs status during a real heavy MCP call | §2.2 |
| a7 | Manifest reuse does not give the promised accounting or cost | **Accepted.** Verified: `changed_paths` drops `source-dependencies`, `al-walk`, `app.json-paths`; cached mode re-hashes source dependencies. r2 returns **input categories** (also for changes with no single path), extends the manifest with per-file stat entries for source dependencies and with injected test inputs, binds the result to `checkedRevision` with start/end times and incomplete reasons, uses the real refresh classes, and fixes the `freshness: verify` description (it rebuilds when stale: `session.rs:416-421`). The cached check is benchmarked with source dependencies | §2.2-2.4, F-6 |
| a8 | Test universes need a reproducible identity and a multi-revision protocol | **Accepted.** An immutable `TestUniverse` descriptor (test root, production root and app id, source overrides, package policy, config, consumed stamps) carried through the pool key, `session/open`, the manifest and cache keys. Answers carry a revision vector; a production-stamp mismatch is retried once, then refused with the reason | §4.3 |
| a9 | C's fallback policy contradicts its own example; missing roots over-interpreted | **Accepted.** Output has `runFirst`, `requiredFallback` and `selectionComplete`. Root statuses `reached`, `not-loaded`, `not-in-projection`, `no-known-path`, `analysis-incomplete` stay distinct. Discovery unions workspace tests and sibling apps; a cap gives "discovery incomplete". Narrowed: hidden channels are split into ones that can add unseen test callers (they force the fallback) and platform-raised table events whose raising writes are already edges (they do not); a C1 probe classifies CDO's 47 before the rule ships | §4.4-4.6 |
| a10 | B1 cannot reuse `HunkOutsideRoutines`: a mixed hunk hides its declaration part | **Accepted.** Verified: `digest_cli.rs:800-821` emits the diagnostic only when a hunk hits no routine. B1 accounts for **every changed line**: routine body, routine signature, object-level declaration (by kind), outside any object, unmapped. One comparison tuple is resolved once and used everywhere | §8.3-8.4 |
| a11 | B2 needs stable effect identity and per-side coverage | **Accepted.** Semantic effect keys are separate from display anchors; sites match by routine pairing plus normalized operation identity, ambiguity explicit; each side stores its coverage; "no longer observed" when the new side is incomplete; a count-preserving table swap shows. Pairing, preflight and policy are reused from the diff pipeline. Key changes: the shared diff engine gains key passes (it has none today, verified in `passes.rs`) | §10.3-10.5 |
| a12 | Resident integration assumes capabilities the host lacks | **Accepted** as milestone **M0**, first in the order: graph-only artifact ownership, dependency order independent of `Fold`, guard enrichment from pinned inputs, needs-aware row-cache invalidation, artifact admission/eviction/cancellation, multi-universe orchestration with no nested pool waits. B2 benchmarks lazy per-root base summaries before any all-routine table | §1, §10.6 |
| a13 | B2's cache is not managed by `cache_prune.rs` | **Accepted.** Verified: `cache_prune.rs` handles only dependency JSON artifacts. A separate namespace with schema version, integrity check, atomic publication, size caps, byte/age/LRU pruning, concurrent readers; the key includes source-dependency content and analysis options; at most 10 bases | §10.7 |
| a14 | D's consumer kinds need stricter semantics | **Accepted.** Extraction per operation (not membership in `FIELD_ARGS_OPS`; `Next` and `Get` arguments are not fields); `ReadWrite` counts as both uses on one site; site counts separate from use counts; TransferFields is a copy **candidate**; a literal field number needs a known table; `FieldIndex(n)` is not field n; the answer reports which bodies were searched. The D1 probe decides v1's scope | §7 |
| a15 | E cannot enumerate routes from the single-parent tree | **Accepted.** Verified: `literal_reach.rs` keeps the first parent per `(routine, bindings)` state. E is split: E1 (filters, error text, one route, no enumeration) and E2 (routes over the state-transition graph, with defined simple-path, recursion and cap rules; it reports candidates examined, not an exact count of ruled-out routes) | §9, §11 |
| a16 | Surface compatibility needs a contract | **Accepted, smaller.** The query-tool merge is **dropped** (owner): both query tools stay, so no aliases or tombstones are needed. Prove bumps to **2.4.0** (it is 2.3.0 today, `prove.rs:1595`). The legacy touches absence wording is fixed as an immediate prerequisite (I-3). Path routine names use an unambiguous overload handle, not `Name/arity` alone. A selector/mode validation table per new tool | §12, I-3 |

astra's "resident memory and latency risks" (its own numbered list):

| # | Risk | r2 |
|---|---|---|
| ar1 | Measurements are not resident ones; the test-app `explain` run failed (exit 2) | **Accepted.** The failed run is no longer quoted as an explain cost; it is "a build that then failed in selector resolution" (Appendix A). Every capability has a resident measurement plan |
| ar2 | 31,265 nodes is the pruned export | **Accepted.** R0's size is estimated from the unpruned site counts (§3.6) and measured in M0 with the construction peak |
| ar3 | No dynamic global memory scheduler; children share a fixed budget | **Accepted.** Verified: `child_budget` = min(own, total / max_children). `--max-children 2` stays (owner); no automatic raise |
| ar4 | Multi-child orchestration can deadlock | **Accepted.** M0-6: a composite request computes the main-side part, releases the main child, then schedules the next; it never holds one child while waiting for another, and answers with a resource reason when admission is impossible |
| ar5 | Default explain must not start test universes | **Accepted.** explain shows "discovered, not analyzed in this call" and never starts a test child |
| ar6 | Per-root materialization is the multiplier | **Accepted** (with Fable f10): multi-source and streaming walks; no `ReverseCone` per root is kept |
| ar7 | B2's all-routine table is unproven | **Accepted.** B2 starts with lazy per-root base summaries; the all-routine option is measured separately |
| ar8 | "48 changed routines" compares three counts only | **Accepted.** Relabelled as a count-change census (§8.5, Appendix A) |

astra's answers to r1's open questions are folded into §13 with the owner's decisions.

### Fable

| # | Finding (short) | r2 | Where |
|---|---|---|---|
| f1 | The resident revision does not keep what `project()` needs | **Accepted.** Verified: `build_view_model` lends the program to graph jobs inside a closure (`batch/mod.rs:368-375`); `Revision` keeps a model, `rel`, the store, no edges. The sited graph is built **inside the build closure** and kept as a per-revision artifact (`ArtifactKey::Sited`), interned to `u32`. This is M0, first in the order | §1.2 |
| f2 | The ordering rule has no fact in TryFunction callers and breaks under loops | **Accepted, mechanism changed.** A no longer reads `PCallSite.order`. It reuses `pending_before`, which walks the statement tree (which TryFunction routines do have: `ir_walk.rs:2096` builds it for every body) and counts a loop's whole body. Per the owner, a TryFunction caller still blocks `no` (its write-in-try semantics are not modelled) and is printed as its own row; any caller where the tree walk falls back to the whole body is an `order-unknown` blocker | §6.3 |
| f3 | Selecting a main-app routine in the test universe needs code; the explain measurement failed | **Accepted.** Selectors are scoped to the primary app (`digest_cli.rs:670-673`). C maps changed roots by stable id over the **test universe's sited-graph node table**, which holds dependency routines too; a miss is `not-in-projection`. The failed run is relabelled | §4.3 |
| f4 | A source dependency is `LocalSourceApproximate`; pin edge parity too | **Accepted.** C-2b: resolved out-edges of shared routines are equal in both universes (fixture exact; CDO ratchet). The tier is printed in the scope line | §4.8 |
| f5 | `UpdateLastUpload` has three one-hop tests the export cannot see | **Accepted** as the CDO ratchet C-9 | §4.8 |
| f6 | Through-dependency tests are the same for unrelated routines | **Accepted.** Honesty line "effectively the suite's posting tests, not specific to this change" when the dependency hub's cone is over half the graph; `--tier local|dependency|all` | §4.5 |
| f7 | Prove is already 2.3.0; the relabel missed three questions; "safety" in the instructions | **Accepted, one part already done.** New questions ship as **2.4.0**; all six existing questions are relabelled "existing effects". The word "safety": master's instructions no longer have it (`src/mcp/mod.rs:87` reads "A yes/no question about one routine (may it commit, …)"; `git log -S"safety questions"` shows wave 3b `fefcea35` removed it). The installed `~/.al-sem/bin/alsem` 2.1.1 predates that and still says "alsem_prove for yes/no safety questions"; rebuild and copy it. I-1 adds a test that no description or instruction contains "safe" | §5, I-1 |
| f8 | D's "no new IR" is false for `by-number`; `rule` and `copy` read gated or missing facts | **Accepted.** r2 says D needs new facts for `by-number` (a RecordRef `Field(<literal>)` site) and for TransferFields' source table; the `rule` kind reads the `TestField` record operation (no message text, no `rules` assembly). The D1 probe decides whether each new fact is in v1 or the kind is printed `not searched` | §7.2-7.3 |
| f9 | `session/check` placement and side effects | **Accepted, mechanism changed** (with a6): the check runs in the MCP process against the published descriptor; cached hashing on a checker thread, `verify` hashing on the same thread with its own deadline. The check never clears `dirty` and never counts a fresh hit | §2.2 |
| f10 | Impact over hundreds of roots must not keep a cone per root | **Accepted.** Multi-source walk labelling the nearest root, per-root streaming summaries; R0-6 counts allocations | §3.4 |
| f11 | Discovery facts on CDO differ from r1's example | **Accepted.** `TestFR` has an `app.json` but no `.al` file: skipped and listed. Counts: 139 files, 101 with `[Test]`, 1,860 `[Test]` lines, 1,844 test nodes loaded; the C1 probe explains the 16. The scan is cached | §4.2, C-3 |
| f12 | Citations | **Accepted.** Fixture is `tests/fixtures/test-selection-graph`. `HunkOutsideRoutines` carries only file and lines; B1's classification is new work. No `F`/`A-S` classes. "22,790 ancestors" was the test universe's count; the mock-ups say illustrative. Explain `/4` and query 3.2.0 were right | throughout |
| f13 | P0a not filed; #155 is a second commit hole | **Accepted.** P0a is in progress on branch `fix/commit-up-chain`; #155 (`Database.Commit()` gives no commit fact) is **P0c**. Both are hard dependencies of A and B2 | §0.2 |
| f14 | `runs-without-ui` can read the capability `start` fact today | **Accepted.** Verified: `extract_background` records `TaskScheduler.CreateTask` (arg0) and `StartSession` (arg1) with a classified value source | §6.5 |
| f15 | External callers include `internalsVisibleTo` friends | **Accepted.** External = public routine on a public object, or internal routine of an app with friends; `assumeNoDependents` is a separate flag from `closedWorld` | §3.5 |
| f16 | Status must name the universe of each engine | **Accepted.** Each engine row names its universe (`cross-app`, `single-app`, `tests:<app>`); refresh names the universe it refreshed; a base child is listed while it runs | §2.3 |

Fable's other notes, also applied: build the B2 base child in the `ON_DEMAND`/low-mem shape
(§10.6); stamp every non-archived input the base read (§10.6); E's guards under `--low-mem`
(r2 removes the problem: guards come from re-parsing pinned files, §1.2 M0-3); `h:any` had 0
callers on CDO, so R0-4 needs a fixture; status paths reuse `Manifest::changed_paths`'
spelling.

### r1 errors corrected

| r1 said | Correct |
|---|---|
| `explain` on the test-app workspace: 20.5 s, 3.1 GB, "the upper bound" | That run **failed** (exit 2: `no routine of the program has the id or name 'RunReportID'`). 20.5 s / 3.1 GB is a build plus a selector failure. Valid only as a bound on the build |
| `likelyClass` values `F`, `A-S`, `B-or-S` | The session's classes are `BodyOnly`, `Declaration`, `Package`. File add/remove/rename is `Declaration`. A `roots.config.json` change alone is `BodyOnly` (test `package_change_rebuilds_and_roots_config_is_b`) |
| fixture `tests/fixtures/test-selection-acceptance` | `tests/fixtures/test-selection-graph` |
| B1 reuses `HunkOutsideRoutines` "classified by what they touch" | It carries only file, start and end line, and only for hunks that hit no routine. Every-line accounting is new work (§8.3) |
| prove JSON 2.3.0 for the new questions | 2.3.0 is today's version. The new questions ship as **2.4.0** |
| "CDO's whole graph is 31,265 nodes and 78,936 edges" | That is the **pruned, collapsed export**. The unpruned sited graph is larger (§3.6) |
| "1,844 [Test] procedures" in the test app | 1,860 `[Test]` attribute lines in source; 1,844 test nodes in the loaded graph. The output prints what was loaded; C1's probe explains the 16 |
| "48 of the 176 changed routines changed behavior" | A **count-change census** over three counts (tables written, own commit sites, error kinds). It misses equal-count changes and other effect families |
| "~1,000 `.al` files" (verify cost) | CDO `Cloud` has 582 `.al` files and 70 MB in 11 packages (measured with `find`/`du`) |
| "Errors: a caller's earlier writes are rolled back" under any trap | A TryFunction does not roll back database changes. A checked `Codeunit.Run` runs its own transaction, which is rolled back on error |
| §4.2 discovery would find `TestFR` | `TestFR` has no `.al` file; it is skipped |

---

## 0. Summary, milestones and order

### 0.1 Milestones (owner order)

Sizes are rough: **S** about 1-2 commits and a day, **M** 3-4 commits, **L** 5 or more commits
or a new process shape.

| # | Milestone | What the agent gets | Size | Depends on |
|---|---|---|---|---|
| I | **Immediate fixes** | prove's six questions say "existing effects" (a test keeps "safe" out of every description); explain's `tests:` line says whether test code was loaded (P0b); touches no longer prints a whole-cone `no` before saying unresolved targets may be the table; explain's write rows drop path dumps by default | S-M (4 commits) | — |
| M0 | **Resident groundwork** | nothing visible; the sited graph per revision, graph-only universes, guard enrichment, needs-aware caches, artifact admission, composite requests | L (6 commits) | — |
| F | **Freshness** | `alsem_status` checks the disk against the revision it holds, without blocking on a busy engine; says what changed by category, the refresh class, the next action | M (3 commits) | M0-6 (descriptor publication) |
| R0 | **Reverse cone** | the shared up-walker: per-state shortest paths, semantic entry kinds, multi-source and streaming walks | M (3 commits) | M0-1 |
| C | **Test selection** | `alsem_tests`: scope first, `runFirst`, `requiredFallback`, `selectionComplete`, per-root status | L (6 commits) | M0, R0 |
| A | **Adding an effect** | `callers-have-pending-writes`, `callers-intercept-errors`, `callers-may-run-noninteractive`; explain's "callers' context" section | L (6 commits) | R0, **P0a**, **P0c** |
| D | **Field impact** (narrowed v1) | `query touches --field`: direct code consumers of a field by use kind, temporary ones equally visible | M (3 commits) | D1 probe; R0 only for `--from` |
| B1 | **Impact, current revision** | `alsem_impact`: every changed line accounted, one comparison tuple, callers and entries, current effects, tests | M-L (4 commits) | R0, C |
| E1 | **Debugging slice, narrow** | prove filters `match`/`site`/`op`, message text, one route with selected guards | M (2 commits) | M0-3 |
| B2 | **Impact, before/after** | a delta against a base with stable semantic effect keys and per-side coverage | L (6 commits) | B1, **P0a**, **P0c** |
| E2 | **Debugging slice, routes** | up to 10 distinct routes over the literal-reach state graph | M (2 commits) | E1 |

The query-tool merge of r1 is **dropped** (owner). `alsem_query_effects` and
`alsem_query_touches` stay; D adds `field` to touches.

### 0.2 Prerequisites outside this spec

- **P0a. The commit-up-the-chain fix** (Fable fact 1 of the MCP-output review: `RunReportID`
  loses `CDOFile.Table.al:128` and the commit beyond `Sales-Calc. Discount::OnRun`). In progress
  on branch `fix/commit-up-chain` (worktree `.claude/worktrees/commitchain`; it touches
  `digest.rs`, `digest_cli.rs`, `literal_reach.rs`, `prove.rs`, `routine_rollup.rs`). **Hard
  dependency** of A (pending writes are commit-sensitive) and B2 (a commit delta over a dropped
  fact shows phantom removals), and of E's commit slices. Each of those milestones starts with a
  regression test that names both lost commit sites.
- **P0c. #155: `Database.Commit()` produces no commit fact.** Open. Same dependency set. Until it
  is fixed nothing in A or B2 lands.
- **P0b** (explain's `tests: none reach it` must say whether test code is loaded) is milestone I's
  second commit; it does not wait for C.

### 0.3 Why this order

M0 first, because every later piece needs something the resident host does not do today: keep
edges past the build (Fable f1), answer a graph-only universe (astra a12), and run a request that
needs two children (astra ar4). F next: small, stops a wrong action today, and its control path
(the descriptor) is the M0 piece the test universe also needs. R0 is the one new engine piece
C, A and B share. C before A (both reviews): it depends only on R0, was shown feasible on CDO,
and its prerequisite P0b misleads every explain today. A after its two hard dependencies. D is
independent and cheap; its probe can run any time. B1 reuses R0, C and digest's root
resolution. E1 needs only M0's guard enrichment and is useful without route enumeration. B2 is
the only new process shape (a one-shot base child) and needs B1's output shape. E2 last.

---

## 1. M0. Resident groundwork

### 1.1 What the host does not do today (verified)

- `build_view_model` lends the `ResolvedProgram` to graph jobs inside a closure and drops it
  before returning the model (`src/engine/batch/mod.rs:368-375`). `Revision` keeps `inputs`, a
  `Model` target, `rel: ExecRelation`, `dependency_calls` and the artifact store
  (`src/engine/resident/revision.rs:258-295`). Nothing reachable from a revision has the
  projection's inputs (`ProgramGraph`, `ResolveIndex`, `DeclSurface`, `EdgeView`,
  `SourceFacts`, `project.rs:25-38`).
- A revision refuses graph demands and multi-view jobs (`revision.rs:1418-1440`:
  `d.needs.graph` → "needs a view this revision did not build").
- A session builds one fixed EXPLAIN-capable model (`session.rs:43-51`); retained rows carry no
  record of the needs they were built with.
- Engines are keyed by `(workspace, universe)` (`pool.rs:94-95`); the children bound evicts the
  least recently used idle child by count (`pool.rs:441-468`); per-child budgets are a fixed
  share of `--total-budget` (`mcp/mod.rs:159-169`).

### 1.2 Pieces

**M0-1. The sited graph as a revision artifact.** The projection builder (`project.rs`
`Builder`) is refactored to emit a **sited graph**: one edge per site, with the site id, its
source anchor, the edge kind, the confidence, and an **execution boundary**
(`Sync`, `CheckedRun` (own transaction), `UncheckedRun`, `BackgroundSession`, `BackgroundTask`,
`PageBackgroundTask`, `EventDispatch`, `TriggerOf { op, temp_state }`, `Lifecycle`,
`TestHandler`, `TestRunner`). Today's `Projection` is computed **from** the sited graph by
collapsing on `(from, to, kind)` with today's merge rule (minimum line, strongest confidence),
so `alsem graph` output stays byte-identical (M0-T1). Nodes and sites are interned to `u32`;
strings live once in a table.

It is built **inside the build closure**, where the program is lent, on every revision and on
every session refresh (`refresh_model` gets the program too; any B edit can move an edge). It
is stored as `ArtifactKey::Sited`, ready when the revision publishes. It does not depend on
`Fold` or `L4`: phase 0 of the dependency order.

Eager or lazy: lazy is not possible without keeping the program, which costs far more. So it is
eager, under a measured budget: if its build exceeds 300 ms or 60 MB on CDO cross-app (M0
measurement), it becomes a session option `engine-host --graph` that the pool sets only for a
universe that has served an up-question, and a first up-question on a session without it
answers after one rebuild, saying so.

**M0-2. Graph-only universes.** A session kind that builds the program and the sited graph and
no model (no L4, no fold, no trees): the test universe (§4) and the B2 base child (§10). It
retains what refresh needs (per-file parses and resolutions, `session/workspace.rs:67-93`), or,
under `--low-mem`, only the texts. A revision of this kind serves only jobs that declare
`needs.graph` and read the sited graph; any other job is refused with the reason. `alsem graph`
itself may then be served resident (a side benefit, not required by this spec).

**M0-3. Guard enrichment from pinned inputs.** E needs the enclosing conditions of a few sites
on the routes it prints. Instead of a level-1 assembly need for every site (r1's
`site_guards`, 15-130 MB estimated), the revision re-parses only the files of the routines on
the printed routes from its **pinned** `WorkspaceInputs` (workspace files) and package sources
(dependency files), with `al_syntax::parse`, finds each site by its anchor, and walks up the
enclosing `if`/`case`/loop nodes. The pinned inputs are the bytes the revision was built from,
so the text matches the revision even if the disk has moved. Cost: one parse per file on the
route (a few ms each). Works the same under `--low-mem`.

**M0-4. Needs-aware row caches.** Retained rows record the `AssemblyNeeds` they were built
with. A request whose needs exceed them rebuilds the rows it needs (reported as a refresh of
class `needs`) or, when the rebuild does not fit the budget, is refused with the reason. No
milestone in this spec adds a new gated assembly need (M0-3 removes the only one r1 had); this
piece makes the next one safe.

**M0-5. Artifact admission, eviction and cancellation.** New artifacts (`Sited`, the C walk
scratch, B2's base summaries) declare a `budget::Case` and an estimated size before the fill,
and join the store's reservations (`store.rs` reservations, `evict`/`evict_unheld`). A fill
checks the request's cancellation every 4,096 units of work, like literal reach
(`CANCEL_EVERY`).

**M0-6. Composite requests and the input descriptor.** Two pool changes:

1. **Descriptor publication.** When a worker sees a new generation (it already asks status
   after each call, `pool.rs:808`), it asks the child for the revision's **input descriptor**:
   the generation, the universe descriptor (§4.3), and the manifest entries with their stat
   data. The worker stores it in its `Snapshot`. It is immutable per generation. F's check
   reads it (§2.2).
2. **Composite requests.** A coordinator thread in the pool runs a request that needs several
   children (impact with tests; tests with production root resolution; B2) as a sequence of
   plain sub-requests. Rule: **a sub-request never holds one child while waiting for another
   child's admission.** The main-side part (root resolution, input accounting, revision stamp)
   is computed and returned to the coordinator, the main child is released (idle), and only
   then is the next child acquired. If a child cannot be admitted (every child busy, or the
   budget refuses), that part of the answer says why (`tests: not computed — the test engine
   could not start: 2 engines busy (max-children 2)`) and the rest is still returned.

### 1.3 Tests (M0)

| Id | Invariant | Discrimination break |
|---|---|---|
| M0-T1 | `alsem graph` output is byte-identical before and after the refactor (fixture `test-selection-graph`, the graph goldens, CDO under `cdo-gate`) | collapse keeping the first line instead of the minimum |
| M0-T2 | The sited graph keeps two calls between the same routines as two edges with different site ids and anchors | collapse sites in the sited graph |
| M0-T3 | After a B refresh that adds a call, the revision's `Sited` artifact has the new edge (resident test, both memory modes) | keep the previous revision's artifact |
| M0-T4 | A graph-only session refuses a model job with the reason, and answers a sited-graph job | let it fall through to the model path |
| M0-T5 | Guard enrichment reads the pinned bytes: edit the file on disk after the build; the guard text is the revision's | parse from disk |
| M0-T6 | A row cache built with fewer needs is rebuilt (counter) for a request that needs more | ignore the recorded needs |
| M0-T7 | A composite request with both children busy returns the main part and a resource reason, and never deadlocks (timeout test with two busy fake children) | wait on the second child while holding the first |
| M0-T8 | Cancelling during a `Sited` fill ends it within one check interval and leaves no published artifact | check cancellation only at the end |

### 1.4 Measurement plan (M0)

On CDO cross-app and single-app, each with default and `--low-mem`: cold build time and peak
RSS with and without the sited graph; the sited graph's resident bytes; its rebuild time on a
B refresh, an S refresh and a P refresh (`aldump --session-refresh <copy> --verify`); the peak
**during** its construction (it coexists with the lent program); the test universe as a
graph-only session, cold and after a B refresh. Pass bar: the sited graph adds at most 300 ms
to a B refresh and 60 MB resident; otherwise M0-1's option rule applies.

### 1.5 Commits (M0)

1. `refactor(graph): sited graph builder; the export collapses it` (M0-T1, M0-T2).
2. `feat(resident): ArtifactKey::Sited built in the build and refresh closures` (M0-T3, M0-T8).
3. `feat(resident): graph-only session kind` (M0-T4).
4. `feat(resident): guard enrichment from pinned inputs` (M0-T5).
5. `feat(resident): needs-aware row caches; artifact admission for new kinds` (M0-T6).
6. `feat(mcp): input descriptor in the worker snapshot; composite requests` (M0-T7).

---

## 2. F. Freshness: status checks the disk

### 2.1 The task

"Is the engine's answer up to date with my edits? Should I refresh?" Today `alsem_status`
returns the workers' snapshots and never looks at files (`server.rs:363-375`). An agent reads
`revision` and `lastBuild` as "current" when nothing was checked.

### 2.2 Control path

- The status call is handled as today on the reader loop up to the point of checking. For each
  engine it reports, it takes the worker's **published descriptor** (M0-6) and hands the check
  to a **checker thread** in the MCP process. The reader loop keeps reading; the response is
  sent when the checks finish. A busy engine child, a busy worker or a running build never
  delays the check: it does not go through the `EngineFront` at all.
- The check runs `Manifest::observe` (the MCP server is the same `alsem` binary) against the
  descriptor's manifest with `Hashing::Cached { prev, dirty: ∅ }`, or `Hashing::All` with
  `verify: true`. Only the MCP process reads the disk; the child is not asked.
- The check never clears the session's `dirty` set and never counts a fresh hit (Fable f9): it
  is not a `pin`.
- Source dependencies: today the cached mode re-reads and hashes them (`manifest.rs:353-357`,
  `source_deps.rs:123-180`). F1 extends the `ws:source-dependencies` entry with per-file stat
  data so the cached check stats them like `.al` files and hashes only the changed ones.
- Test universes: their descriptor includes the injected inputs (§4.3: the production source
  root, the extra package roots). The check observes those too, under the categories
  `injected-source` and `injected-packages`.
- Each check has a deadline (default 5 s for cached, 30 s for `verify`). A check that does not
  finish, or meets an unreadable input, returns `complete: false` with the reason; never
  `fresh`.

### 2.3 Output

`alsem_status` keeps JSON (status is not a CLI command), with `statusVersion: 2`, a `headline`
first, the operator data under `details`:

```json
{
  "kind": "status", "statusVersion": 2,
  "headline": "cross-app engine for /…/Cloud: STALE. 3 .al files changed on disk since generation 12. Refresh class: BodyOnly or Declaration (known after parsing them). Next: any command with freshness fresh (the default) refreshes first; or call alsem_refresh.",
  "engines": [{
    "workspace": "/home/sshadows/Projects/DO-cdo-baseline/Cloud", "universe": "cross-app",
    "check": {
      "checkedRevision": {"generation": 12, "inputsStamp": "…"},
      "startedAt": "…", "finishedAt": "…", "checkMs": 1.4,
      "hashing": "cached", "complete": true, "incompleteReasons": [],
      "fresh": false,
      "changes": [
        {"category": "al-file", "path": "src/Features/Email/Sending/CDOEMailHandler.Codeunit.al"},
        {"category": "source-dependency", "path": null, "detail": "al-sem.dependencies.json names a source whose files changed"}
      ],
      "changesTruncated": false,
      "refreshClass": {"class": "BodyOnly|Declaration", "certain": false,
                       "reason": "only .al file contents changed; whether a declaration changed is known after parsing them"},
      "residual": "a rewrite that keeps size and modification time is seen only with verify: true"
    },
    "refresh": {"state": "idle", "last": {"class": "BodyOnly", "generation": 12, "durationMs": 1312}},
    "details": {"lastBuild": {}, "activity": "idle", "memory": {}, "child": {}}
  }]
}
```

**Input categories** (from manifest keys, `manifest.rs:7-20`): `al-file` (`al:<rel>`),
`al-walk` (a file added, removed or renamed), `app-json`, `app-json-paths`, `roots-config`,
`coverage-config`, `source-dependency`, `package` (`pkg:<path>`), `package-list`,
`unreadable-package-folder`, `meta` (engine or grammar version), `injected-source`,
`injected-packages`, `test-config` (`testApps` in `.al-sem.json`). Paths are spelled as
`Manifest::changed_paths` spells them, so the LSP and MCP agree.

**Refresh class from categories** (real classes only):

| Categories present | `refreshClass.class` | certain |
|---|---|---|
| any `package`, `package-list`, `app-json`, `source-dependency`, `injected-*`, `meta` | `Package` | yes |
| `al-walk` (and no package category) | `Declaration` | yes |
| only `roots-config` | `BodyOnly` | yes (pinned by `package_change_rebuilds_and_roots_config_is_b`) |
| `coverage-config` | whatever `WorkspaceSession` does; F1 reads it from the session code and pins it with a test, no invented class | — |
| only `al-file` | `BodyOnly|Declaration` | no |
| `test-config` | not a refresh of this engine: "test scope changed; the next alsem_tests rediscovers" | yes |

Later (F2): parse just the changed files and compare their declaration projections with the
retained ones (`classify.rs`), which gives `BodyOnly` or `Declaration` exactly.

**Headlines** (exact texts are part of the contract; each names the universe, Fable f16):

| State | Headline |
|---|---|
| fresh | `<universe> engine for <ws>: FRESH: no input changed since generation N (checked just now: M files, K packages).` |
| stale | as above with the categories, the class and the next action |
| no engine | `no engine is running for <ws> (<universe>), so nothing was checked. The next command builds it (about 25 s cross-app on CDO).` |
| building | `<universe> engine: a refresh is running (class S, started 4 s ago). Disk compared with the last published revision (generation N): …` |
| incomplete | `<universe> engine: the check did not finish (<reason>); freshness unknown.` |
| failed | `<universe> engine failed 3 times (last: …). The next command starts a new one.` |

**Refresh state.** `refresh.state` is `idle | queued | running | completed | failed`, with the
class, start, end and generation; never `building: true` inferred from a request. `alsem_refresh`
answers `{"did": "already-fresh", "universe": …}`, `{"did": "rebuilt", "universe": …, "class":
"BodyOnly", "files": 3, "durationMs": 1312, "generation": 13}`, or with `wait: false`
`{"did": "queued", …}`.

**The `freshness: verify` description is corrected.** Today it says "answer from the built
revision and report whether it is stale" (`tools.rs:170`), but `pin(Verify)` hashes every input
and **rebuilds** when stale (`session.rs:384-421`). New text: "verify: like fresh, but hashes
every input instead of trusting size and modification time; rebuilds first when anything
changed." Its contract test pins the behavior it describes.

### 2.4 Honesty rules

- `fresh: true` only after a complete check in this call, bound to `checkedRevision`.
- The check sees the disk. Unsaved editor buffers are not inputs.
- With no published revision: `check: null` with the reason; never `fresh`.
- A check against a revision while a refresh runs says so: it compares with the last
  **published** revision.

### 2.5 Tests (F)

| Id | Invariant | Discrimination break |
|---|---|---|
| F-1 | Edit a fixture file between two status calls: first `fresh: true`, second `fresh: false` naming the file and category | answer from the last ask's result |
| F-2 | Status never builds: generation and build count equal before and after a stale status; `dirty` and `fresh_hits` unchanged | let the check call `pin` |
| F-3 | MCP-level: start a heavy `alsem_explain` (fixture with a test-only delay hook in the child), call `alsem_status`: it answers with a complete check within 200 ms while the heavy call is still running | route the check through the worker's front end |
| F-4 | Category → class, one fixture per row of the table above | map `app.json` to `BodyOnly` |
| F-5 | No engine: `check: null` and the "nothing was checked" headline | report the default as fresh |
| F-6 | A source-dependency file edit is reported as category `source-dependency`, and the cached check hashes only that file (counter) | drop the `source-dependencies` key as `changed_paths` does today |
| F-7 | A test universe's injected production source edit is reported on that engine | observe only the test root |
| F-8 | The `verify` description's contract test: a stale `verify` command rebuilds | keep the old text and behavior apart |
| F-9 | A check past its deadline answers `complete: false`, never `fresh` | default `fresh` on timeout |

### 2.6 Measurement plan (F)

Cached check and `verify` check on CDO cross-app (582 `.al` files, 11 packages, 70 MB), on
the test universe (with the injected production source), and with one source dependency;
each while idle and while a heavy explain runs on the same engine. Time from the status call
to its response, and the reader loop's latency for a second request sent during the check.
Pass bar: cached ≤ 10 ms on CDO; `verify` reported, no bar.

### 2.7 Commits (F)

1. `feat(manifest): per-file stat for source dependencies; injected-input entries; categories` (F-6, F-7).
2. `feat(mcp): status checks the disk off the reader loop; statusVersion 2; headlines; refresh state` (F-1..F-5, F-9).
3. `fix(mcp): the freshness verify description says it rebuilds` (F-8).

Consumers: MCP agents only. No CLI output moves.

---

## 3. R0. The reverse cone

### 3.1 Substrate: the sited graph

R0 walks M0-1's sited graph, not the collapsed export. Reasons (both reviews): the export keeps
one edge per `(from, to, kind)` (`project.rs`, `edges: BTreeMap<(Key, Key, Kind), E>`), so
`R(); Rec.Modify(); R();` is one edge, and a guarded and an unguarded call are one edge; and
pruning drops dependency leaves (`CDO Telemetry::UpdateLastUpload` is absent from the test
graph, Fable f5). The sited graph keeps the export's semantics (calls, interface dispatch,
event flow, implicit triggers, object runs and lifecycle, test handlers and runners, hubs,
`hiddenCallers`) with one edge per site.

### 3.2 What a walk computes

```rust
pub struct WalkState { node: NodeIx, tier: Tier, crossed_event: bool, ext: u8 /* consumer bits */ }
pub enum Tier { Local, ThroughDependency, OnlyOpen }   // best first

pub trait ConeVisitor {
    /// Each state the first time it is reached at its shortest hop count.
    fn reach(&mut self, s: WalkState, hops: u32, via: SiteIx);
    /// A node in the cone carrying hiddenCallers.
    fn hidden(&mut self, n: NodeIx, ch: HiddenChannel);
    /// A node in the cone with an entry kind (semantic, §3.3).
    fn entry(&mut self, n: NodeIx, kinds: &[EntryKind], s: WalkState, hops: u32);
    /// A node apps outside the program may call (§3.5).
    fn external(&mut self, n: NodeIx, why: ExternalWhy);
    /// A cap fired.
    fn limit(&mut self, l: Limit);
}
```

- **States.** The walk keeps the shortest hop count and one parent site per **state**
  `(node, tier, crossedEvent)`, not per node, so the shortest local path and a shorter
  dependency path are both kept. A consumer adds its own bits (`ext`): A uses them for "a
  commit was passed", "inside a trap", "past an async boundary" (§6). With at most 3 tiers ×
  2 × 2^k consumer bits, the state count is bounded by nodes × 6 × 2^k; A uses k ≤ 3.
- **Tier.** `Local`: every node of the path is in the workspace app or a loaded test app.
  `ThroughDependency`: the path passes dependency code. `OnlyOpen`: every path to this state
  uses an `Open` edge (a hub). Best over all paths (R0-5).
- **Order.** Breadth-first over reversed edges, ascending node id then site id per level:
  deterministic.
- **`h:any`.** The universal hub stands for "a caller of every node". Its predecessors are
  walked **once**, as `OnlyOpen` states, and the hub is reported as a hidden channel. They are
  never dropped (astra a2). C treats an `h:any` caller as `requiredFallback` (§4.5).
- **Counts are of entries and sites.** Output says "41 page-action entries", "6 call sites",
  never "49 paths" unless paths were counted.
- **Budget.** A state budget (default 2,000,000). A cap that fires is a `Limit`, printed, and
  blocks every `no`.

### 3.3 Entry kinds (semantic)

An **entry** is a node whose source declares a way to start a run, whether or not the graph
has an edge into it: from `engine::root_classification` (13 kinds: 11 derived from source,
`web-service-exposed` and `job-queue-entrypoint` from `roots.config.json` only,
`root_classification.rs:28-40`), plus page actions with captions (explain's `run_by` and
`card_text.actions`), plus static background targets (§6.5). A page action stays an entry
when a lifecycle edge reaches it; a public procedure stays externally callable when it also
has internal callers. **Graph sources** (nodes with no incoming edge) are reported separately
and are not entries by themselves.

Print order, "a user or a job will hit this" first: `page-action` (by caption), `api-page`,
`web-service-exposed`, `job-queue-entrypoint`, `background-start`, `report-trigger`,
`trigger-page`, `trigger-table`, `event-subscriber` (to a platform or dependency event),
`install-codeunit`, `upgrade-codeunit`, `onrun-codeunit`, `test-procedure`, `external`.

### 3.4 Many roots: multi-source and streaming walks

Impact and tests ask for hundreds of roots (699 on the 94-file CDO range, measured). One
resolved cone is about 22,452 nodes (RunReportID, workspace universe, measured), so a retained
map per root would be about 15 million entries.

- **`walk_multi(roots, visitor)`**: one breadth-first walk from all roots at once; each state
  is labelled with its nearest root (ties by root order). It gives per-root summaries (entries
  by kind, nearest test, hops, tier counts) in O(states) memory. Used by impact's ranking and
  tests' selection.
- **`walk_one(root, visitor)`**: a streaming walk for one root; the visitor keeps what it
  needs (first K witnesses per entry kind). The scratch (visited bitsets, queue) is reused
  across roots; nothing per root outlives its call.
- No `ReverseCone` struct with a full `reached` map is ever stored.

### 3.5 External callers (open world, up-direction)

Apps that depend on the workspace are not part of the program (engine-switch S7.4). A node is
**external** when (a) it is a routine with public access on a public object (`Access` property
not `Internal`), or (b) it is an `internal` routine and the app names any friend in
`internalsVisibleTo` (`R3a5CrossAppBase.friends`, `capability_cone.rs:4704`), unless
`assumeNoDependents` (CLI `--no-dependents`, MCP `assumeNoDependents: true`) is given. The
output says which reason applies. `assumeNoDependents` is a separate flag from `closedWorld`
(which is about interface dispatch and appears in prove's existing JSON as `closedWorldRows`).
Neither flag closes unknown platform scheduling.

### 3.6 Cost

The export is 31,265 nodes and 78,936 collapsed edges (pruned, measured). The sited graph is
larger. From the graph's own histogram (measured, `graph-cdo.out` meta): 11,639 + 266
workspace call edges resolved to source or conditional, and 191,084 dependency-body call edges
resolved to source, so about 200,000 routine-to-routine call sites, plus trigger, event,
lifecycle and test edges. **Estimate:** 250,000-350,000 sited edges at 20-24 bytes, 6-9 MB,
plus a node table of about 40,000 nodes; build time under 300 ms. A walk over it is
milliseconds. M0's measurement settles it.

R0 answers off the **published** revision's sited graph: a stale engine gives a stale cone.
Every R0 consumer prints the generation in its coverage line (F's headline is the cure).

### 3.7 Tests (R0)

| Id | Invariant | Discrimination break |
|---|---|---|
| R0-1 | Fixture `tests/fixtures/test-selection-graph`: the cone of `ApplyDecay` contains the test `NightlyRecalcDecays`, tier `Local`, hop count counted by hand | follow forward edges |
| R0-2 | A dependency leaf called only from the workspace, absent from the pruned export, is in the cone | walk the export |
| R0-3 | explain's `callers:` count and page actions equal the cone's direct-caller sites and page-action entries (fixture; CDO `RunReportID`) | keep explain's workspace-only adjacency |
| R0-4 | Fixture with an `h:any` caller: its predecessors appear once as `OnlyOpen` states and the hub once as a hidden channel | treat `h:any` as an ordinary caller per node |
| R0-5 | A node reached both locally and through Base Application has a `Local` state with the local hop count and a `ThroughDependency` state with its own | keep the first-found tier per node |
| R0-6 | A 1,000-root `walk_multi` on the fixture allocates O(states), not O(states × roots) (allocation counter) | retain one map per root |
| R0-7 | Two calls `A → R` in one routine, one before a `Modify` and one after, under different `GuiAllowed` guards, are two reverse edges with their own sites | walk the collapsed export |
| R0-8 | A page action reached by a lifecycle edge is still a `page-action` entry; a public procedure with internal callers is still `external` | entries = nodes with no caller |
| R0-9 | An internal procedure of an app with a friend is `external` (reason `friend`); with `assumeNoDependents` it is not | drop the friend check |

### 3.8 Measurement plan (R0)

On CDO cross-app and the test universe: `walk_one` for RunReportID, IssueReminder,
UpdateLastUpload (time, states); `walk_multi` for the 699 roots of the 94-file range (time,
peak); both in default and `--low-mem`, and while another heavy request runs on the same child.

### 3.9 Commits (R0)

1. `feat(graph): reverse walks over the sited graph (states, tiers, h:any)` (R0-1, R0-2, R0-4, R0-5, R0-7).
2. `feat(graph): semantic entry kinds; external callers with friends; assumeNoDependents` (R0-8, R0-9).
3. `feat(graph): multi-source and streaming walks; explain's callers read the cone` (R0-3, R0-6). explain's text does not change; R0-3 proves it on fixtures; any CDO difference is a found bug, named in the commit.

---

## 4. C. Test selection as a first-class answer

### 4.1 What was measured on CDO

CDO keeps its tests in a sibling app, `DO-cdo-baseline/Test` ("Continia Document Output 365 -
Test"): 139 `.al` files, 101 with `[Test]`, 1,860 `[Test]` attribute lines, 101
`Subtype = Test` codeunits and one `Subtype = TestRunner` (Fable, measured). `TestFR` has an
`app.json` that depends on Cloud and no `.al` file. `FR`, `OnPrem`, `DemoApp` depend on Cloud
and hold no test codeunit. The workspace app has no `[Test]`. So every `tests: none reach it`
in today's outputs means "no test code loaded" (callgraph spec limit F7).

With the test app copied to scratch, `al-sem.dependencies.json` naming `Cloud` as a source
dependency, and Cloud's `.alpackages` copied in (measured, `alsem` 2.1.1):

| Run | Wall | Peak RSS | Result |
|---|---|---|---|
| `alsem graph Cloud` | 12.4 s | 2.54 GB | 31,265 nodes, 78,936 edges (pruned export), 0 tests |
| `alsem graph TestApp` | 13.5 s | 2.59 GB | 30,149 nodes, 1,844 test nodes; 2,267 Cloud nodes by app id, 2,100 with `sourcePath`; Cloud is `localApproximate`; primary-scoped `unknown` 4,699 (the test libraries were not in `.alpackages`) |
| stable ids | — | — | all 2,100 shared ids equal in both graphs (`f4b69b55-…:Codeunit:6175303#83ca…` for RunReportID) |
| `explain TestApp RunReportID` | 20.5 s | 3.12 GB | **failed, exit 2**: selectors are scoped to the primary app; a build bound only |

Reverse walks over the test graph (Python over the JSONL; re-derived exactly by Fable):

| Routine | Tests, resolved edges | Tests, any edge | Tests by a local path | Nearest |
|---|---|---|---|---|
| `CDO E-Mail Handler::RunReportID` | 369 | 622 | **68 in 7 codeunits**, 3-7 hops | `CDO Combine To One PDF Tests` (3 hops) |
| `CDO Issue Document::IssueReminder` | 369 | 622 | **0** | `CDO Setup Tests` (6 hops, through Base Application) |
| `CDO Telemetry::UpdateLastUpload` | pruned from the export | — | 3 tests in `CDO Telemetry Tests` exist, one hop (by name) | — |

Lessons: the engine can load tests today; ranking by tier is the answer (RunReportID and
IssueReminder reach the **same** 369/622 tests through Base Application posting, Fable f6); the
walk must not use the pruned export; and without the test libraries' symbols the test universe
has 4,699 unknown calls, so any selection there is incomplete (§4.5).

### 4.2 Surface and scope discovery

CLI: `alsem tests <ws> (--routine R… | --diff P [--diff-root D] | --file F… | --base REV)
[--test-app DIR…] [--tier local|dependency|all] [--max-tests N] [--format human|json]`.
Changed-root resolution is digest's (`resolve_changed_roots`) in the production universe.

MCP: new tool `alsem_tests` with `routines`, `diff`, `diffRoot`, `files`, `base`, `testApps`,
`tier`, `maxTests`, `format`, and the common properties.

**As built in C (2026-11-05): `--base REV` / MCP `base` are not part of milestone C.** They need
B1's comparison tuple and changed-line accounting (§8.2-8.3) and B2's base build (§10.6), so they
ship with those milestones; C takes `--routine`, `--file` and `--diff` only.

**Test scope** is the union of:

1. `[Test]` procedures in the workspace app itself (a nested test folder is folded into the
   primary app today), always;
2. the sibling test apps from the first rule that applies:
   a. `testApps` given in the call;
   b. `testApps` in `.al-sem.json` (`testApps: []` turns sibling discovery off);
   c. **auto-discovery**: folders under the git top level, sorted, at most 3 levels deep, at most
      64 `app.json` files read, skipping `.alpackages`, `.git`, `node_modules` and `target`;
      an app qualifies when its `app.json` lists the workspace app's `id` in `dependencies`
      **and** it holds a codeunit with `Subtype = Test`. An app with no `.al` file is skipped and
      listed `skipped: no source`. A cap that fires makes the scope **"discovery incomplete"**,
      never "no app depends on this one". The scan is cached, keyed by the git top level and the
      `app.json` paths and mtimes.

On CDO this finds `Test`, skips `TestFR` (no source) and lists `FR`, `OnPrem`, `DemoApp` as
"depends on the workspace, no test codeunit".

**As built: a project-collection marker.** A folder holding a file `.al-sem-projects` says each
of its subfolders is a separate project; for a workspace under it, auto-discovery's search root
is the subfolder that holds the workspace when that is nearer than the git top level. This
repository marks `tests/r0-corpus/` and `tests/fixtures/`: without it every fixture searched the
repository, hit the 64-file cap among 270-odd fixture `app.json` files, and its goldens depended
on the repository's layout.

### 4.3 Test universes

**Descriptor.** Each sibling test app is one test universe, described by an immutable
`TestUniverse`:

```rust
pub struct TestUniverse {
    test_root: PathBuf, test_app_id: Guid,
    production_root: PathBuf, production_app_id: Guid,
    /// The production app as an in-memory source dependency (no file is written
    /// into the user's test app), as al-sem.dependencies.json would name it.
    source_overrides: Vec<PathBuf>,
    /// Package discovery: the test app's .alpackages and ancestors, then the
    /// production root's (CDO's layout: only Cloud/.alpackages exists).
    package_policy: Vec<PathBuf>,
    /// The testApps config entry (or "auto") that selected it.
    config: ScopeOrigin,
}
```

Its blake3 is the universe key (`tests:<hash>`), used in the pool key, the `session/open`
request (a new `universe: {kind: "tests", descriptor}` field), the manifest (entries
`inj:source:<path>` and `inj:pkg-root:<path>`, observed like their real counterparts), and
every cache key. The session is graph-only (M0-2).

**Production stamp.** Both universes compute a `productionSourceStamp`: blake3 over the sorted
`(relative path, content hash)` of the production app's `.al` files and its `app.json`, the
same function on both sides. The production universe records it in its revision; the test
universe records the one it consumed.

**Revision vector and consistency.** A tests answer reads the production revision (root
resolution) and one revision per test universe. It carries `revisions: {production:
{generation, inputsStamp, productionSourceStamp}, tests: [{app, generation, inputsStamp,
productionSourceStamp}]}`. If a test universe's stamp differs from production's, the
coordinator refreshes the lagging side once (fresh mode) and compares again; if they still
differ (the disk is moving), the answer is refused for that test app with
`test universe built from different production bytes (generation …); retry`.

**Selector path into the test universe** (Fable f3). Changed roots are resolved in the
production universe to stable ids. The test universe's sited graph has a node table over every
routine row, dependency rows included; C looks the ids up there. A miss is
`not-in-projection` (§4.4). This lookup is used only by `tests` and `impact`; user-facing
selectors keep their primary-app scope.

### 4.4 Per-root status

| Status | Meaning |
|---|---|
| `reached` | at least one test reaches the root |
| `no-known-path` | the root is in the test universe's graph and no test reaches it, with a complete cone |
| `not-in-projection` | the stable id is not in the test universe's node table |
| `not-loaded` | the test universe does not load the root's body (no demand from test code); missing demand is **not** proof that no test reaches it |
| `analysis-incomplete` | a cap fired, or a selection-relevant hidden channel is in the cone (§4.5) |

`not-in-projection` and `not-loaded` are reported with the nearest ancestor (in the production
universe's cone) that the test universe does have, as "nearest tested caller", when one exists.

**As built in C (2026-11-05): "nearest tested caller" is deferred to a later milestone.** C
reports `not-in-projection` and `not-loaded` roots with their status and sets
`requiredFallback`, and `nearestTestedCaller` is `null`. Computing it needs an up-walk in the
production universe's sited graph that then maps each ancestor into the test universe; the
production child builds no sited graph today (M0's lazy-graph decision). It belongs with B1
(`alsem_impact`), which needs the same up-walk.

### 4.5 Selection, ranking and fallback

- Walk `walk_multi` from the mapped roots; keep `test-procedure` entries; rank by tier (`Local`
  first), then hops, then codeunit id; group per codeunit.
- **`runFirst`**: the ranked subset, local tier by default (`--tier` changes it).
- **`requiredFallback`**: the whole discovered suite, with the reasons. It is set when any root
  is `not-in-projection`, `not-loaded` or `analysis-incomplete`, when an `h:any` caller is in a
  cone, or when test code has unresolved calls into symbol-less libraries that could call back
  into production code (CDO's scratch test universe: 4,699 unknown primary calls).
- **`selectionComplete`**: `true` only when `requiredFallback` is empty.
- **Hidden channels, split** (narrowing astra a9 with code evidence). `project.rs:943-956`
  marks platform table publishers hidden even when trigger edges exist. A channel is
  **selection-relevant** when it can add test callers no edge shows (`h:any`, an
  `unraisedPublisher` that is not a table event, an opaque `Run` hub reached from test code).
  An `unraisedPublisher` on a table event (`OnAfterModifyEvent` …) is **platform-raised,
  covered** when every write of that table in the program is an edge into the event (the
  platform raises it only on writes, and tests run program code). Only selection-relevant
  channels force the fallback. **C1 probe:** classify CDO's 47 hidden nodes (all
  `unraisedPublisher`, Fable) before the rule ships; if a covered one cannot be proven covered,
  it counts as relevant.
- **Dependency-tier sameness** (Fable f6): when a root's through-dependency tests are reached
  only via a dependency node whose own cone covers more than half of the graph, print `through
  dependency code: N tests — effectively the suite's posting tests, not specific to this
  change`.

### 4.6 Output

Human (mock-up; per-tier test counts and the local codeunit count are from §4.1; ids, paths
and the fallback reason are illustrative):

```
test scope: workspace 0 [Test]; 1 test app — Continia Document Output 365 - Test (/…/Test, auto-discovered; 1,844 tests loaded; production app loaded as local source, tier localApproximate); skipped: TestFR (no source)
revisions: production generation 12, tests generation 3 (same production bytes)
changed: 1 routine (Codeunit CDO E-Mail Handler::RunReportID) — reached
run first — local path, 68 tests in 7 codeunits:
  Codeunit 70xxxx CDO Combine To One PDF Tests: 29 tests, 3 hops (first: PrintEmailAll_CombineDecision_ComesFromTheLineThatActuallySends; via CDO Combine To One PDF::Print… → CDO E-Mail Handler::SendMail → RunReportID)
  Codeunit 70xxxx CDO Doc Print Functions Tests: 19 tests, 4 hops
  … +5 codeunits (20 tests)
through dependency code: 301 tests — effectively the suite's posting tests, not specific to this change
selection complete: no — run the whole suite as well (requiredFallback): test code makes 4,699 calls that resolve nowhere (test libraries without symbols: Library Assert, Tests-TestLibraries, …); a library may call back into this code
run first: codeunit filter 70xxxx|70xxxx|…  (the 68 local-path tests)
this is static reachability, not coverage: a reached test may not exercise the changed lines
```

No scope:

```
test scope: none — no [Test] procedure in the workspace app, no testApps configured, and no app under the git top level (searched 7 app.json files) depends on this one with a test codeunit. No test was considered. This does not mean no tests are needed.
```

JSON schema `al-sem.tests/1`:

```json
{"schema": "al-sem.tests", "version": 1, "lineBase": 1,
 "scope": {"workspaceTests": 0, "discovery": "auto" | "configured" | "call" | "off",
           "discoveryComplete": true,
           "apps": [{"name": "…", "root": "…", "appId": "…", "files": 139, "testsLoaded": 1844,
                     "found": "auto", "productionTier": "localApproximate"}],
           "skipped": [{"root": "…", "reason": "no-source" | "no-test-codeunit"}]},
 "revisions": {"production": {}, "tests": []},
 "changed": [{"stableId": "…", "display": "…", "anchor": {}, "status": "reached",
              "nearestTestedCaller": null}],
 "runFirst": [{"stableId": "…", "codeunitId": 70000, "codeunit": "…", "procedure": "…",
               "anchor": {}, "tier": "local", "class": "direct" | "event" | "dynamic",
               "hops": 3, "reaches": ["<changed stableId>"], "path": []}],
 "byTier": {"local": 68, "throughDependency": 301, "onlyOpen": 253},
 "requiredFallback": [{"reason": "unresolved-test-calls", "count": 4699, "detail": "…"}],
 "selectionComplete": false,
 "hidden": [{"node": "…", "channel": "unraisedPublisher", "selectionRelevant": false}],
 "assumptions": [], "limits": []}
```

`class` uses bc-dev-mcp's words (`direct`/`event`/`dynamic`, callgraph spec §8 B5).

explain's `tests:` line (P0b, milestone I) reads the same scope rules without starting a test
child: `tests: 1 test app discovered (Continia Document Output 365 - Test); not analyzed in
this call — alsem_tests selects them` or `tests: no test code found (…)`. When a test
universe with a matching revision vector is already resident, explain may show its counts.

### 4.7 Children and memory

- `--max-children 2` stays the default (owner). The test child starts only on an
  `alsem_tests` or `alsem_impact` call, never on explain.
- With three universes in play (for example single-app, cross-app and one test app), the LRU
  idle child is evicted (`pool.rs:441-468`), a ~13 s rebuild on its next use; status shows the
  eviction.
- Two test apps run **sequentially** through the composite request (M0-6), one test child at a
  time, unless both fit under the children bound.
- **C2** (test apps folded into the main program as extra roots, changing engine-switch S7.4)
  is a later, separate design.
- bc-dev-mcp keeps its JSONL walk for now and adopts `alsem tests --format json` later, once
  `al-sem.tests/1` is pinned by fixtures and `--keep-local-sources` has shipped.

### 4.8 Relation to `alsem graph`

The walk reads the sited graph, whose collapse is the export, so the two cannot disagree about
edges. `alsem graph` gains, additively: `meta.apps[].sourceRoot` for local-source apps and
`--keep-local-sources` (default on when a local-source app is loaded), so pruning keeps every
routine of a local-source app (closes F7 for bc-dev-mcp: it joins changed main-app files on
`(appId, sourcePath)`).

### 4.9 Honesty rules

- Scope first, with counts, how each app was found, and what was skipped.
- "No test found" never means "no test needed".
- A root that is not loaded or not in the graph is never "not reached".
- Static reachability is not coverage; said once, at the end.
- `selectionComplete: false` always comes with the fallback reason and is printed before the
  `run first` filter.

### 4.10 Tests (C)

| Id | Invariant | Discrimination break |
|---|---|---|
| C-1 | Fixture `test-selection-graph`: `ApplyDecay` selects `NightlyRecalcDecays`, tier local, hops as counted | rank by codeunit id only |
| C-2 | A routine's stable id is equal in the production and the test universe (fixture with a sibling test app; CDO ratchet over all shared ids) | mint the id with the app's role |
| C-2b | The resolved out-edges of every shared production routine are equal in both universes (fixture exact; CDO ratchet ≥ 99 %, the rest listed) | resolve local-source calls with the approximate tier's fallback |
| C-3 | Discovery: a sibling that depends on the workspace with a test codeunit is found; one with no `.al` is `skipped: no source`; one without the dependency is not found; `testApps: []` finds none; a 64-file cap gives `discoveryComplete: false` | drop the dependency check; report a cap as "none" |
| C-4 | A changed dependency leaf pruned from the export is `reached` or `no-known-path`, never missing | walk the export |
| C-5 | No scope: the "none — no test was considered" text, exit 0 | print `tests: none reach it` |
| C-6 | A selection-relevant hidden channel sets `requiredFallback` and `selectionComplete: false`; a covered platform table event does not | treat every hidden channel alike |
| C-7 | explain's `tests:` line distinguishes "no test code found" from "discovered, not analyzed" and never starts a test child (child count) | start the test child from explain |
| C-8 | `alsem graph` is byte-identical when no local-source app is loaded | turn `--keep-local-sources` on always |
| C-9 | CDO (`cdo-gate`): `alsem tests Cloud --routine "CDO Telemetry::UpdateLastUpload"` lists exactly the 3 `CDO Telemetry Tests` tests (`UpdateLastUpload_ShouldSetLastTelemetryUploadToToday`, `…_WhenNeverUploaded_…`, `…_WhenCalledMultipleTimes_…`), tier local, 1 hop | walk the pruned export |
| C-10 | A production edit after the test universe was built: the answer refreshes the test side or refuses with the stamp reason; never answers with mismatched stamps | skip the stamp comparison |
| C-11 | A root missing from the test universe's demand is `not-loaded` and sets `requiredFallback` | report it as `no-known-path` |
| C-12 | A root reached by tests only through Base Application posting gets the sameness line | omit the line |

### 4.11 Measurement plan (C)

On CDO: the test universe cold (graph-only), warm, after a B refresh of a production file
(both universes refresh) and of a test file; in default and `--low-mem`; with **two test apps**
(the fixture's two, and on CDO a scratch copy of `Test` with a new app id) run sequentially
and, with `--max-children 3`, side by side; an `alsem_tests` call while a heavy explain runs on
the production child; cancellation of a cold test-universe build; peak RSS during the test
universe's sited-graph construction. C1 also records the 16 tests between 1,860 attribute lines
and 1,844 loaded nodes (`#if` arms, tests outside Test codeunits, parse errors).

### 4.12 Commits (C)

1. `feat(tests): test universe descriptor; injected source and package roots; session/open` (C-2, C-2b).
2. `feat(tests): scope discovery (bounded, cached, skips); explain's tests line reads it` (C-3, C-7) — explain goldens move.
3. `probe(tests): CDO hidden channels by class; the 16 missing tests` (recorded in the commit and in this spec's Appendix).
4. `feat(tests): alsem tests, al-sem.tests/1, runFirst/requiredFallback/selectionComplete` (C-1, C-4..C-6, C-11, C-12) — new family `tests/cli-tests-goldens/`.
5. `feat(mcp): alsem_tests through a composite request; revision vector` (C-10).
6. `feat(graph): --keep-local-sources, meta.apps[].sourceRoot` (C-8) and the CDO ratchet C-9.

---

## 5. Shared rule for A, B and D: "existing effects" wording

Milestone I relabels the six existing prove questions (`may-commit`, `commits-on-success-path`,
`writes-table:<T>`, `publishes-event:<E>`, `reaches-ui`, `throws-error`, `prove.rs:60-98`):
their human first line starts `existing effects:`. Their JSON does not change in I (human text
only). On master the instructions already say "A yes/no question about one routine (may it
commit, …): alsem_prove" (wave 3b removed "safety questions"; the installed 2.1.1 copy still
shows the old text until it is rebuilt). I-1 makes the prove description say "about the
routine's existing effects" and, from milestone A, "or its callers' context", and adds a
contract test that no tool description or instruction contains "safe". The words "safe" and
"unsafe" appear in no output.

---

## 6. A. "What context does R run in?" (adding an effect)

### 6.1 The task

The agent is about to add a `Commit()`, an `Error(...)`, or UI (`Message`, `Confirm`,
`Page.Run`) to routine R. What breaks depends on R's **callers**:

- **COMMIT**: a caller with a write pending when it calls R would have that write committed by
  the new COMMIT (posting half-done); a caller that runs R under a checked `Codeunit.Run`
  expects an error to roll back the run's own transaction; a caller object with
  `InherentCommitBehavior` or a `[CommitBehavior]` attribute changes what a COMMIT does at all.
- **Error**: a caller that intercepts errors turns the new error into a `false` result; an
  uncaught path surfaces it to a user, a Job Queue log or a web-service fault. A TryFunction
  that catches it does **not** roll back the database changes made before the error (Microsoft
  docs, "Handling errors using Try methods"); a checked `Codeunit.Run` rolls back its own
  transaction.
- **UI**: a path from a job queue, a background session or task, a web service or API page, or
  an install/upgrade codeunit has no user to answer.

### 6.2 Decision

Three prove questions (yes/no/unknown with witnesses; the shape ConTinAI's `verify.py` parses),
prove JSON **2.4.0** (additive):

- **`callers-have-pending-writes`**: may some caller have a database write pending (written and
  not yet committed, in the same transaction) when R runs?
- **`callers-intercept-errors`**: may an error raised in R be intercepted by a caller before it
  ends the run (caught and turned into `false`, collected, or expected by `asserterror`)?
- **`callers-may-run-noninteractive`**: may R run in a session where no user can answer UI?

Each human first line ends `(callers' context; not about R's own effects)`. An explain section
`callers' context` shows the three answers in one line each. There is no "safe" verdict. A
single `alsem_safety` tool stays rejected (a fourth answer shape; the facts are useful alone).

All three walk R's reverse cone (R0) with A's state bits and evaluate each reverse edge
`caller C --site s--> R'`. A starts only after **P0a** and **P0c**; each A commit begins with
P0a's regression (both lost commit sites present).

### 6.3 `callers-have-pending-writes`

Reuse, not a new rule: `pending_writes::pending_before(C, [s])` (`pending_writes.rs:95`) gives
the sites of C that may run before s on C's control-flow tree: earlier statements in the block,
conditions on the way down, only the branch that holds s, a loop's whole body (an earlier
iteration), and a `Commit()` statement on the way clears what was collected
(`walk_list`, `:312`). A site's effects are its physical table writes, plus for a call the
callee's cone writes (`ConeDerivedStore`).

Walking up from R, per state:

1. At edge `C --s--> R'`, compute `pending_before(C, [s])`. A physical write in it is a
   **witness** (kind `pending-write`, evidence = that write's site).
2. A `Commit()` before s in C (the walk saw a commit statement; `walk_list` is extended to
   report it) sets the state bit `committed`: callers above C cannot have a write pending at
   R through this path, so the walk stops there for this state.
3. A **checked** `Codeunit.Run` edge (`is_checked_run`, `pending_writes.rs:232`) starts a new
   transaction: callers above it are not in R's transaction. The walk stops there.
4. A `TriggerOf { op }` edge (R runs inside an `Insert`/`Modify`/`Delete`/`Rename`): a witness
   of kind `inside-write` only when the op's record is physical. A temporary record proves
   nothing; a caller-dependent one is a witness marked `caller-dependent`. `OnValidate` (from
   `Validate`) is not a write by itself.
5. An event edge from a publisher P in C: the subscribers of the same event run in an unknown
   order (the documented under-approximation in `pending_writes.rs:21-24`). Their writes are
   counted as may-pending (over-approximation, keeps `no` sound) and listed as
   `sibling-subscribers`.
6. A commit inside a callee called before s is a may-commit and does **not** clear (keeps
   `yes` an over-approximation and `no` sound).
7. Async boundaries (`BackgroundSession`, `BackgroundTask`, `PageBackgroundTask`) start a new
   session and transaction: callers above do not count.

**Blockers of `no`:** a caller where `pending_before` fell back to the whole body (no tree, or
the site not found: `order-unknown`); a TryFunction caller (owner decision, Fable f2: writes
inside try functions follow a server setting the engine does not model; printed as its own row
because a COMMIT under a try function is decision-relevant on its own); a caller whose cone
effect facts are incomplete (unresolved calls before s); hidden callers; `OnlyOpen` ancestors;
external ancestors; a limit; P0a/P0c not yet landed (no A commit lands before them).

**Commit behavior rows.** A caller object with `InherentCommitBehavior` (`ModelObject`,
`workspace.rs:73-82`) or a routine with `[CommitBehavior(CommitBehavior::Ignore|Error)]` on the
path is printed: `commit behavior: Ignore on Codeunit X (an added COMMIT below it is ignored)`.

### 6.4 `callers-intercept-errors`

Per reverse edge, classify how an error in R' crosses the edge:

| Kind | Rule |
|---|---|
| `catch-and-return-false` (TryFunction) | the edge is the **invocation of** a TryFunction T (R' = T or R is in T's cone and this edge enters T), and the call site's result is consumed (not `in_statement_position`, `features.rs:237-245`). An error raised while evaluating the call's **arguments** happens in the caller and is not caught: a site whose anchor lies inside the argument span of the TryFunction call is not trapped by it |
| `catch-and-return-false` (checked run) | `is_checked_run(site)`: `if Codeunit.Run(...) then`, or a codeunit variable's `.Run` on `OnRun` with the result used. Its own transaction is rolled back |
| `asserterror` | `under_asserterror == Some(true)` (tests) |
| `collectible` | a caller with `[ErrorBehavior(ErrorBehavior::Collect)]`: intercepts only errors raised as collectible (`ErrorInfo` with `Collectible = true`); an ordinary `Error('…')` is not collected. Printed as `collectible errors only` |
| `propagates` | anything else, including a TryFunction call whose result is discarded (it behaves like an ordinary call) |

Traps do not cross session or task boundaries: an async edge ends the path; the error goes to
the session or task (printed as entry `background-start: the error ends the background
session`). The answer always prints the **uncaught** side too: the entry kinds reached with no
trap, nearest first. `yes` = some path is intercepted; `no` = every path propagates to an entry,
with the cone complete (same blockers as §6.3 minus the TryFunction-caller one).

### 6.5 `callers-may-run-noninteractive`

Independent of the UI kind (astra a5, option 1). A path is a **witness** when its entry is
non-interactive:

- `api-page`, `web-service-exposed`, `job-queue-entrypoint` (the last two from
  `roots.config.json`);
- `install-codeunit`, `upgrade-codeunit`;
- a **background start target**: read now from the capability `start` facts
  (`dispatch_background.rs`: `TaskScheduler.CreateTask` arg0, `StartSession` arg1) with a
  literal codeunit target, as `non-interactive-entry` witnesses "via a background start fact,
  not an edge"; plus `CurrPage.EnqueueBackgroundTask`. When callgraph F3 later turns literal
  start targets into edges, R0 picks them up unchanged.

**Session context resets across async edges**: below a background start, the run is
non-interactive whatever the starter was; a `GuiAllowed` guard above the start site does not
apply below it.

**GuiAllowed guards** (new plain fact `gui_guard` on every call and operation site, A1a):
`Inside` (then-branch of `GuiAllowed`/`GuiAllowed()`, or a conjunction containing it; or after
an early `if not GuiAllowed() then exit;` in the same block), `Outside` (else-branch, or under
`not GuiAllowed`), `Unrecognized` (a condition that mentions `GuiAllowed` in any other shape),
`None`. A path through an `Inside` site is not a witness below it. `Unrecognized` never removes
a witness and is listed.

Tests are not witnesses (a test session handles UI through handlers); they are printed
separately: `test entries: 12 (UI needs a handler in each)`.

**Blockers of `no`:** no `roots.config.json` in the workspace (job-queue and web-service
publication live in data; `publication-unknown`); a `WebServicePublish.xml` in the workspace
that is not read (CDO has one; A2 reads it); a `start` fact or `EnqueueBackgroundTask` with a
non-literal target anywhere in the program (`unresolved-background-target: N`); an
`Unrecognized` guard on a path; plus the common ones (hidden, `OnlyOpen`, external, limit).

### 6.6 Output (mock-ups; values illustrative)

```
callers-have-pending-writes: yes — a caller may have a write pending when Codeunit CDO E-Mail Handler::RunReportID runs (callers' context; not about R's own effects)
  nearest witness (4 hops, local): Page CDO Posted Sales Inv. List action "Send" → Codeunit CDO Document Mgt::SendDocuments: Modify "CDO File" at ws:src/…/CDODocumentMgt.Codeunit.al:212, then the call at :219 (no Commit between) → … → RunReportID
  witnesses by entry kind: page-action 41 entries, event-subscriber 6 (posting events), trigger-table 2 (inside Modify of a physical record)
  stopped: 3 paths at a Commit before the call; 2 at a checked Codeunit.Run (own transaction)
  commit behavior: none on the paths
  TryFunction callers: 1 (Codeunit CDO Preview::TryRender) — a COMMIT under a try function is not modelled
coverage: generation 12; cone complete for 22,452 nodes; hidden callers 47 (first: unraisedPublisher Table "CDO File" OnAfterModifyEvent); external: RunReportID is public — apps that depend on Continia Document Output are not loaded
next: alsem_explain RunReportID lists the callers; alsem_prove RunReportID callers-intercept-errors
```

```
callers-intercept-errors: yes — 2 call sites intercept an error raised in RunReportID (callers' context)
  catch-and-return-false: if Codeunit.Run(Codeunit::"CDO Send Mail", Rec) then at ws:…/CDOQueueHandler.Codeunit.al:134 (5 hops; the run's own transaction is rolled back)
  catch-and-return-false: if TryRender(…) then at ws:…/CDOPreview.Codeunit.al:61 (2 hops; a TryFunction does not roll back database changes)
  not intercepted: TryRender called as a statement at ws:…:88 (its result is discarded, so it is an ordinary call)
  uncaught at: page-action 38 entries (first: "Send by E-Mail" on Page CDO Posted Sales Invoices), event-subscriber 6
coverage: … (as above)
```

Explain section, after `callers:`:

```
callers' context (for adding effects; the sections above are existing effects):
  pending writes: yes — 49 entries reach it with a write pending (41 page actions, 6 posting subscribers, 2 inside Modify triggers)
  intercepted errors: yes — 2 sites (1 checked Codeunit.Run, 1 TryFunction); 44 entries uncaught
  non-interactive: unknown — no non-interactive entry found; blocked: no roots.config.json (job queue, web services), WebServicePublish.xml not read, 2 background starts with a non-literal target
  ask: alsem_prove RunReportID callers-have-pending-writes (one witness per entry kind)
```

JSON (prove 2.4.0): the same `result` object as every prove answer (`answer`, `evidence[]`,
`blockedBy[]`, `assumptions[]`). Evidence rows:

```json
{"kind": "pending-write" | "inside-write" | "catch-and-return-false" | "asserterror" | "collectible" | "non-interactive-entry",
 "evidence": {"file": "ws:src/…", "line": 212, "sourceKind": "source"},
 "entry": {"kind": "page-action", "display": "…", "caption": "Send"},
 "hops": 4, "tier": "local",
 "path": [{"routine": "<stableId>", "handle": "<overload handle>", "site": {"file": "…", "line": 219}, "boundary": "sync"}]}
```

`evidence.file/line` is the decisive site (the pending write, the trap, the entry), so
`verify.py` reads it as it reads today's rows (T-W2-21). `blockedBy[]` kinds: `order-unknown`,
`tryfunction-caller`, `effects-incomplete`, `hidden-callers`, `only-open`, `external-callers`,
`publication-unknown`, `unread-publication-file`, `unresolved-background-target`,
`unrecognized-guard`, `limit`.

### 6.7 Honesty rules

- `yes` is a may-answer: a path exists in the static graph.
- `no` needs the whole cone complete with no blocker; on a public procedure it is rare without
  `assumeNoDependents`, and the output says why.
- A `GuiAllowed` guard removes a witness only for the non-interactive question; it is not a trap.
- A TryFunction is never described as rolling back.
- No "safe" or "unsafe".

### 6.8 Tests (A)

| Id | Invariant | Discrimination break |
|---|---|---|
| A-0 | P0a's regression: `RunReportID`'s commit sites include `CDOFile.Table.al:128` and the one beyond `Sales-Calc. Discount::OnRun`; `Database.Commit()` gives a commit fact (P0c) | — (prerequisite) |
| A-1 | Caller `Modify`s then calls R → `yes`, the write as evidence; calls R then `Modify`s → `no` | ignore the statement order |
| A-1b | Loop: `repeat R(); Rec.Modify(); until …` → `yes` (an earlier iteration) | walk only earlier statements, not the loop body |
| A-1c | `Modify(); Commit(); R();` → `no` from this caller | ignore the Commit reset |
| A-1d | A TryFunction caller → `unknown` with `tryfunction-caller`, printed as its own row | let the tree walk answer for it |
| A-2 | A callee before s that writes counts; one after s does not | use the caller's whole cone |
| A-3 | `TriggerOf` from `Modify` on a physical record → `inside-write`; on a temporary record → no witness | ignore the temp state |
| A-3b | A checked `Codeunit.Run` between the writer and R stops the path | follow checked runs |
| A-4 | Same TryFunction called twice, `if T() then` and `T();`: only the first is `catch-and-return-false` | trap on the attribute alone |
| A-5 | A call to R inside the arguments of `if T(R()) then` is not trapped by T | trap every site on the line |
| A-6 | `ErrorBehavior::Collect` caller → `collectible`, not `catch-and-return-false` | treat collection as catching |
| A-7 | A trap above a `StartSession` does not intercept an error below it | propagate traps across async edges |
| A-8 | `if GuiAllowed then R()` is not a non-interactive witness; the else branch is; an `Unrecognized` shape keeps the witness and is listed | ignore `gui_guard` |
| A-9 | A literal `StartSession(…, Codeunit::X)` makes X's `OnRun` cone non-interactive; a non-literal target blocks `no` | ignore the `start` facts |
| A-10 | No `roots.config.json` → `callers-may-run-noninteractive` cannot answer `no` (`publication-unknown`) | treat a missing config as empty |
| A-11 | Public routine, no other blocker → `unknown` with `external-callers`; with `assumeNoDependents` → `no` and the assumption listed; an internal routine of an app with a friend behaves like public | drop the friend check |
| A-12 | A hidden caller in the cone blocks `no` | ignore `hidden` |
| A-13 | The six existing questions keep their JSON byte-identical except `schemaVersion` 2.4.0 | change their JSON |
| A-14 | CDO (`cdo-gate`): `RunReportID callers-have-pending-writes` is `yes`; `UpdateLastUpload`'s evidence names `QueueUpload` or answers `unknown` with a named blocker | — (ratchet) |
| A-15 | `gui_guard` adds less than 1 MB to the CDO model (measured, recorded) | — |

Golden families that move: `cli-b-goldens/prove` (version, new question files),
`cli-output-goldens` (prove, explain), `cli-explain-goldens` (new section), CDO rendering
goldens. Must not move: `r4f-goldens`, graph, fingerprint, snapshot.

### 6.9 Measurement plan (A)

On CDO cross-app, resident: each question for RunReportID, UpdateLastUpload and IssueReminder,
cold and warm, after a B and an S refresh, default and `--low-mem`; the explain card with the
new section (time added); peak during the walk; cancellation mid-walk; model memory with and
without `gui_guard`.

### 6.10 Commits (A)

1. `feat(body): gui_guard site fact (plain, measured)` (A-8 fixture part, A-15).
2. `feat(l5): pending_before reports a Commit on the way; A's reverse state bits` (A-1..A-3b).
3. `feat(prove): callers-have-pending-writes; prove 2.4.0` (A-0, A-1d, A-11..A-13).
4. `feat(prove): callers-intercept-errors` (A-4..A-7).
5. `feat(prove): callers-may-run-noninteractive (start facts, publication blockers)` (A-9, A-10).
6. `feat(explain): callers' context section; al-sem.explain/4` (A-14) — explain families move.

---

## 7. D. Field impact (narrowed v1)

### 7.1 The task

"I'm changing table T's field F. Who touches it, from where?" A schema change breaks every
consumer, temporary or not. Today touches is table-level only.

### 7.2 Extraction per operation

Not by membership in `FIELD_ARGS_OPS` (`record_op.rs:8-30`; `Next` is there for its step
argument, `Get` and `Find*` arguments are values). Each kind names the argument it reads:

| Kind | Source | Argument |
|---|---|---|
| `read` | `field_accesses` `Read`, and `ReadWrite` | the access |
| `write` | `field_accesses` `Write`, and `ReadWrite`; `ModifyAll` | the access; `modify_all_field` |
| `validate` | `Validate` | arg 0 |
| `filter` | `SetRange`, `SetFilter` | arg 0 |
| `key-use` | `SetCurrentKey` | every arg |
| `load` | `SetLoadFields`, `AddLoadFields` | every arg |
| `calc` | `CalcFields`, `CalcSums` | every arg |
| `rule` | `TestField` record operation | arg 0 (no message text in v1, so no `rules` assembly) |
| `copy-candidate` | `TransferFields` | the receiver's table, and the argument's table when D1 finds it; every field of both tables is a candidate unless matched-field details are known |
| `by-number` | a RecordRef `Field(<literal>)` site | needs a new fact (D1) |
| `declared-key` | the table's keys containing F (`ModelKey.fields`) | declaration |

A `ReadWrite` access (`Rec.F += 1`) is one **site** with two **uses**. The output counts sites
and uses separately. A field access whose record variable's table is unknown (a Variant, an
unresolved type) is a **candidate**, not a match. A literal field number counts only with a
known table. `FieldIndex(n)` is not field number n (it is the n-th field) and is never matched.

### 7.3 D1 probe decides v1's scope

Before building, D1 measures on the fixture and CDO:

1. an unqualified field name in a table or page trigger (implicit `Rec`) is a `PFieldAccess`
   with record variable `Rec`;
2. `FieldNo(F)` / `FieldCaption(F)` are captured somewhere;
3. whether the TransferFields argument's variable (and so its table) is kept;
4. what adding a RecordRef `Field(<literal>)` site fact costs (Fable f8: there is no `Field`
   record op today; `receiver_is_record_ref` marks operations on a RecordRef, not `Field`
   accesses);
5. how many dependency rows carry `field_accesses` under `FULL`, and under `--low-mem`.

Each finding is a scope decision recorded in the D1 commit: the fact is added in v1 (a plain IR
or body fact, measured, through the `add-al-construct` or `declared-needs` skill), or the kind
is printed under `not searched`. r1's "no new IR in v1" is withdrawn.

### 7.3a D1 result (measured 2026-11-05, branch `m-d-fields`)

Probe: a fixture with every kind (`aldump --l2` on it), and `aldump --field-census <ws>`
(`engine::l4::field_touches::census`, new) on the CDO baseline, cross-app (the model `query`
reads), `cdo` profile. The census counts the model's field facts by role.

| # | Question | Finding | v1 decision |
|---|---|---|---|
| 1 | Bare field in a table / page trigger | **Not captured.** `Amount := Amount + 1` in a field's `OnValidate` gave no `PFieldAccess`; only `X.F` member forms are recorded. A bare `TestField("No.")` was a record op on `Rec` | **Added**: `ModelRoutine::implicit_field_accesses` (body walk, `IrSpine::implicit_field_accesses`): a bare identifier in an implicit-record frame (`Rec`, a `with` record) that no parameter, local, global, dataitem or procedure takes, with its access kind; a record method's field-naming argument is not one. Kept apart from `field_accesses`, so no detector input or serialized feature moves. CDO: 3,966 workspace + 18,741 dependency rows (2,478 + 15,823 name a field of the record's table), **5.3 MB** against 16.9 MB for `field_accesses` and a 2.96 GB peak build: a plain fact, no gate |
| 2 | `FieldNo(F)` / `FieldCaption(F)` | **Captured** as call sites with every argument text (also `FieldError`, `SetAutoCalcFields`, `CopyFilter`, `GetFilter`). CDO workspace: 504 metadata calls, 177 filter readers, 15 `SetAutoCalcFields` | No new fact. New kind `metadata` (`FieldNo`, `FieldCaption`, `FieldName`, `FieldActive`): a rename or removal breaks them. `FieldError` is `rule` from its call site, not from the rules-gated fact |
| 3 | TransferFields' argument | **Not kept** (`TransferFields` is not in `FIELD_ARGS_OPS`; no call site either) | **Added**: `ModelRecordOperation::transfer_fields_source`, read at assembly from the IR beside `modify_all_field`. CDO: 57 + 303 TransferFields, every source captured and its table resolved |
| 4 | RecordRef `Field(<literal>)` | **Already a call site** with its argument text (Fable f8 looked for a record op); `FieldIndex(n)` too | No new fact. A literal counts only when the routine alone decides the RecordRef's table (a local, every `Open`/`GetTable` naming one table, never assigned or passed): CDO workspace 25 literal sites, 0 decided; dependency 273, 13 decided. The rest are candidates; 64 + 119 non-literal numbers are counted as "any field", never matched |
| 5 | Dependency rows with `field_accesses` | Under `FULL` (cross-app) every dependency routine the model holds has a body: 27,808, 8,554 with field accesses (60,868 accesses). Code the workspace does not demand is not in the model at all. `--low-mem` (context server only) drops dependency syntax trees between revisions, not model rows; that the answer is the same is checked with the feature (measurement below) | The `searched` line counts bodies by role and states that undemanded dependency code is not loaded |

Cost: the census over every fact took 278 ms on CDO (the build 22 s); `--field` scans the same
facts for one field. The query schema is 3.2.0 on master already (output step 1b), so
`--field` ships as **3.3.0** (additive), not 3.2.0.

**As built (commit 2 of D).** `engine::l4::field_touches` (extraction and census) and
`effect_query_cli::render_field_touches`. Differences from §7.2-7.4:

- A `metadata` kind (`FieldNo`, `FieldCaption`, `FieldName`, `FieldActive`), and the filter
  readers (`GetFilter`, `GetRangeMin/Max`, `CopyFilter`'s first argument) count as `filter`.
  `Validate` is `validate` only (as §7.2), not also `write`.
- `TransferFields` is listed only when the other table has a field with F's number (it copies
  by number); with the other table not resolved it is a candidate (`copy-table-not-resolved`).
  `copySide` says `receiver` (F may be written) or `source` (F may be read).
- `declared-key` is the `declaredKeys[]` list, not a site.
- Without `--from` the job's plan is `session::field_plan()` (the model alone: no `l4`, no tree);
  with `--from` it is `query_plan()`. **Down** (R's cone) is the solve graph: R and its
  transitive callees, dependency code one routine deep, as `query touches` down reads it.
  **Up** (R's callers) is **R0's reverse walk** (`impact_graph::reverse::walk_one`) over the
  sited graph, as `alsem tests` and impact read it: through dependency code, each up site with
  its tier (`local`, `throughDependency`, `onlyOpen`), and the walk's external callers, hidden
  channels and limit stated as reasons. Its demand declares `ViewNeeds::sited` beside the model
  (a model demand that also reads the sited graph is new: the one-shot driver keeps the graph it
  builds off the lent program and hands it to the compute, `ViewCx::sited`; the engine host pins
  such a request with `pin_sited`, `alsem.rs`' `query_field_reads_sited`). `--single-app` lends
  its workspace-scoped program for the same graph (`build_view_model`'s single-app arm now lends
  it to a model demand that declares the sited graph), so both universes walk R0, and with no
  dependency they answer alike (master's `without_dependencies_cross_app_equals_single_app`
  failed while single-app still used the solve graph). The solve graph names callers only when
  the sited graph did not build (stated as `sited-graph-not-built`); on code with no dependency
  the two agree (`field_callers_agree_with_the_reverse_walk_on_local_code`).
  Measured on CDO, `WaitForMailLimit`: the solve graph named 967 callers, R0 names 4,827 with a
  body in the model (local 1,030, through dependency code 2,726, only through an open site
  1,071) and 20,242 more the model holds no body for (stated as not searched). The sited graph
  costs ~1.5 s and 58 MB (134,385 nodes, 318,084 sites); the whole answer 15.0 s / 3.19 GB peak.
- An answer with no site always says `unknown` ("no indexed consumer observed"): in v1 the
  declaration-level consumers are never searched, so coverage is never complete.

**Measured on CDO** (`cdo` profile, cold one-shot CLI, `/usr/bin/time -v`; hand-checked
against source): `CDO E-Mail Log` field 22 "Sent with": 4 sites — `SetCurrentKey` (Send Mail
Management :308), `SetRange` (:312), the implicit write in the table's `InitNewEntryCore`
(:385), and a by-number candidate in CDN's `GetLineUnitPrice` (`RecRef.GetTable(<Variant>)`
then `Field(22)`); the `"Sent with"::SMTP` enum values are rightly not sites, the page
columns are the stated declaration gap. Cross-app 23.2 s / 3.14 GB peak; `--from
WaitForMailLimit` (cone 1; callers by R0, above) 15.0 s / 3.19 GB on a quieter machine;
`--single-app` 7.6 s / 0.74 GB.
`CDO Merge Table Field` field 10 "Merge Table No.": 11 sites — 3 `Validate` in field
triggers, `TestField`, the implicit read as `Field.SetRange(TableNo, "Merge Table No.")`'s
value, implicit read/write in the XML export/import, 3 reads through `MergeTableField."Merge
Table No."` in another table, 1 by-number candidate; the two CalcFormula uses (:86, :94) are
the stated gap, and the other tables' same-named fields are not counted. 32.2 s / 3.11 GB.
The build dominates; the field scan itself is the census's order (278 ms for every fact).
`--low-mem` exists only on the context server and was not measured here.

### 7.4 Surface and output

CLI: `alsem query touches <ws> --table T --field F [--from R] [--direction …] [--all]`. MCP:
`field` on `alsem_query_touches` (the merge is dropped). With `--from R`: only R's cone (down)
and its callers (up, R0).

```
field: Table CDO E-Mail Log (6175281) field 12 "Sent with" (Enum "CDO Sent With", Normal)
searched: workspace bodies 582 files; dependency bodies kept under FULL (2,031 routines with field facts); not searched: dependency bodies not demanded, page/report/query/XMLport columns, CalcFormula, TableRelation (D2), by-number (no fact yet)
consumers: 41 sites in 17 routines of 9 objects — uses: read 18, write 6 (1 also read), filter 9, validate 2, key-use 1, copy-candidate 3; on temporary records 11, persistent 27, caller-dependent 3; candidates: 2 accesses on a record of unknown table
  Codeunit CDO E-Mail Handler: 9 sites in 3 routines (write 2, filter 3, read 4; temporary 0)
     CreateLogEntries ws:src/…:612 write, :640 filter …
  Table CDO E-Mail Log: declared key 3 ("Sent with", "Sent Date"); SetCurrentKey in 2 routines
  … +5 objects (all: true lists every site)
```

JSON: query **3.2.0** adds `field` (identity), `fieldTouches[]` (`kind`, `uses[]`, `site`,
`routine`, `tempState`, `object`, `candidate`), `fieldSummary` (sites and uses by kind and temp
class), `searched` (bodies and kinds). Rows of `touches` without `--field` keep their shape;
their wording follows milestone I's absence fix (I-3).

### 7.5 Cost

A `FieldIndex` built in `query_plan` only when `--field` is given (the query command owns its
pipeline). **Estimate:** under 200 ms on CDO cross-app; D1 measures it.

### 7.6 Later (D2)

Declaration consumers: CalcFormula, TableRelation, page field controls, report/query/XMLport
columns, as owned IR fields through the `add-al-construct` skill.

### 7.7 Honesty rules

- Every answer says what was searched and what was not.
- A non-literal or unknown-table access is a candidate or `unknown`, never "no".
- Temporary consumers are counted with persistent ones, not hidden.

### 7.8 Tests (D)

| Id | Invariant | Discrimination break |
|---|---|---|
| D-1 | Fixture with every kind, temporary and persistent: each site once with its kind and uses; hand-counted totals | count only writes |
| D-2 | `Rec.F += 1` is one site with uses read and write | count two sites |
| D-3 | `Next(2)` and `Get('X')` are not field consumers | read every `FIELD_ARGS_OPS` argument as a field |
| D-4 | A dependency routine touching a dependency table's field is listed | restrict to primary role (as `field_writes` does) |
| D-5 | `TransferFields` lists `copy-candidate` for both tables' fields | skip it |
| D-6 | A field access on an unknown-table record is a candidate, not a match | match by field name alone |
| D-7 | `FieldIndex(12)` never matches field 12 | treat it as `Field(12)` |
| D-8 | Each kind D1 left out appears under `not searched` | omit the list |

### 7.9 Commits (D)

1. `probe(query): D1 field-fact census on the fixture and CDO` (the scope decision in the message).
2. `feat(body): <the facts D1 admitted>` (only if D1 admits any; measured).
3. `feat(query): touches --field; query 3.2.0` (D-1..D-8) — `cli-query-goldens` new files.

Measurement plan: CDO cross-app, cold and warm, default and `--low-mem` (the `searched` line
must change, not silently shrink), with and without `--from`.

---

## 8. B1. PR blast radius on the current revision: `alsem_impact`

### 8.1 Decision

New CLI command `alsem impact` and MCP tool `alsem_impact`. Digest stays the effect inventory
(schema 3.0.0, file-only in MCP; bc-dev-mcp pins 1.7.1 and reads `entries[].routine`). Impact
answers human inline by default in MCP (folded to 40 KiB), JSON on request. B1 is the current
revision only; B2 adds the base delta. B1 reserves the delta block, printed "no base given".

### 8.2 Inputs and the comparison tuple

`alsem impact <ws> (--diff P [--diff-root D] | --file F… | --routine R… | --base REV)
[--tests] [--format human|json] [--max-roots N]`.

One **comparison tuple**, resolved once, printed, and used for root resolution, accounting,
materialization (B2) and the delta (B2):

| Field | Meaning |
|---|---|
| `requestedRef` | what the caller named (`origin/master`, a commit, none) |
| `resolvedCommit` | the commit id it names (the MCP front end resolves it before sending) |
| `mergeBase` | `git merge-base resolvedCommit HEAD` |
| `comparisonBase` | the commit the change is measured against: `mergeBase` when `--base` is given alone; with `--diff`, the commit whose blobs match the diff's pre-images (checked, below) |
| `head` | `HEAD`'s commit, and the working tree's `inputsStamp` (the revision's manifest stamp) |
| `diffHash` | blake3 of the diff text used |

Rules:

- `--base REV` alone: the change is `comparisonBase` → working tree: committed, staged and
  unstaged changes, plus **untracked** `.al` files under the workspace (listed as `untracked`).
- `--diff P`: the diff text is the change. Its post-images are compared with the working tree
  (by the `index` blob ids when present, else by applying hunks' added lines): a mismatch is
  an accounting row `diff does not match the working tree: N files` and those files' roots are
  marked `position-uncertain`. With `--base` too (B2), the pre-images must match
  `comparisonBase`'s blobs, or the delta is refused with the files named.
- Old-only / new-only routines are classified `deleted` / `added` only from **complete
  inventories** of both sides (B2), never from changed-root sets.

### 8.3 Every changed line accounted

New work (today `HunkOutsideRoutines` carries only file and lines, and only for a hunk that hits
no routine, `digest_cli.rs:800-821`). For each file, every changed line (added on the new side,
removed on the old side when a base is available) lands in exactly one bucket:

| Bucket | From |
|---|---|
| `routine-body` (with the routine) | the routine span index (`build_routine_span_index`) minus the signature span |
| `routine-signature` | the routine's declaration line(s) and attributes |
| `object-declaration` by kind: `field`, `key`, `fieldgroup`, `global-var`, `property`, `action`, `layout`, `trigger-decl`, `enum-value`, `other` | IR declaration spans (new: a declaration span index over `FieldDecl`, keys, var sections, properties, actions, layout) |
| `outside-object` | lines before or after any object |
| `unmapped` | a file not in the workspace, a non-AL file, a parse error, a deleted file without a base |

Invariant: the buckets' line counts sum to the diff's changed-line count per file. A hunk that
touches a global and a routine reports both.

### 8.4 What it computes

1. **Input accounting** (always first): the tuple; changed routines resolved; every bucket with
   counts and the first items; untracked files; diff/working-tree mismatches; every
   `ChangedRootsDiagnostic`.
2. **Per changed root, up** (R0 `walk_multi`): direct caller sites with `file:line`, ancestor
   count, entries by kind (page actions by caption, API pages, job queue, web services,
   reports, event subscribers with the event, install/upgrade, tests), external callability.
3. **Per changed root, down (current):** wave 2's `RollupTotals` (tables written by class,
   commits, errors, events, UI, HTTP, file, background) — totals, not witness paths.
4. **Tests:** C's answer through a composite request (M0-6): the main part first, then the test
   child; if it cannot be admitted, `tests: not computed — <reason>`.
5. **Ranking:** roots reached from user or job entries first, then by ancestor count; fold
   after `maxRoots` (default 20) with full counts.

### 8.5 Output (mock-up)

Measured context (Appendix A): the range `bc3ccb18~5..bc3ccb18` (94 files) gave, on the new
side, 699 resolved roots and 99 digest diagnostics (93 hunks outside routines, 5 unmatched
files, 1 non-AL file). Joined with the reversed diff on the base tree: 176 roots in both, 523
only new, 49 only in the base. A **count-change census** over three counts (tables written,
own commit sites, error kinds) found 48 of the 176 differing; it misses equal-count changes and
other effect families, so it is not a behavior delta. Everything else below is illustrative.

```
impact of the working tree against comparison base 3ae74b8 (merge-base of origin/master 9c1e… and HEAD 51d0…); diff 94 files
input: 699 changed routines resolved (no base given: added/deleted need a base); changed lines 12,395 = routine bodies 10,880, signatures 212, object declarations 1,190 (fields 84, keys 6, global vars 140, properties 311, actions 402, layout 247), outside objects 3, unmapped 110 (5 files not in the workspace, 1 non-AL file); untracked .al files: 0
current effects of the 699 changed routines (no base given, so no before/after); first 20 by entries:
  Codeunit CDO E-Mail Handler::SendMail (ws:src/…:140): writes 3 tables (physical 2), commits 0, errors 7
      reached from: 12 page actions (first: "Send by E-Mail" on Page CDO Posted Sales Invoices), 6 posting subscribers; 1,008 ancestors
      tests: 29 local-path (CDO Combine To One PDF Tests, 3 hops)
  … +679 routines (maxRoots: 50, or format json)
deleted files: 21 (their routines need a base to resolve)
schema: 6 key and 84 field lines changed in tables (a base gives the schema findings)
tests: 1 test app; 412 tests reach changed routines (186 by a local path); selection complete: no (see alsem_tests); run first: 9 codeunits
not seen: generation 12; 16 workspace gaps, 265 dependency gaps on the changed routines' cones
```

JSON schema `al-sem.impact/1`: `comparison` (the tuple), `input` (buckets, diagnostics,
mismatches), `roots[]` (stable id, handle, display, anchor, status
`changed|added|deleted|signatureChanged|positionUncertain`, `up`, `effects`, `delta: null`,
`tests`), `schema: null`, `tests` (the `al-sem.tests/1` block or a not-computed reason),
`revisions`, `coverage`, `assumptions`, `limits`, `base: null`.

### 8.6 Honesty rules

- Accounting first, always; a line the engine could not map is listed before any conclusion.
- Without a base: "current effects; no before/after". Never infer a delta.
- The reverse dependents are static may-paths with R0's coverage line.

### 8.7 Tests (B1)

| Id | Invariant | Discrimination break |
|---|---|---|
| B-1 | Accounting comes first in human and JSON; every `ChangedRootsDiagnostic` appears | print accounting after roots |
| B-1b | Per file, the buckets sum to the changed-line count; a hunk touching a global and a routine reports both | report `HunkOutsideRoutines` only |
| B-2 | A root's `up` equals R0's walk and explain's `callers:` (R0-3) | compute `up` from digest's crossings |
| B-3 | `--base REV` alone resolves `comparisonBase` = merge-base; an untracked `.al` file is listed and resolved | diff against `REV` itself |
| B-3b | A `--diff` whose post-image differs from the working tree is reported and its roots marked `positionUncertain` | trust the diff's line numbers |
| B-8 | MCP default human ≤ 40 KiB on the CDO 94-file range with folding; full counts kept | drop the `+N` fold |
| B-10 | Tests not admittable → `tests: not computed` with the reason; the rest of the answer present | wait for the test child while holding the main one |

Golden family: new `tests/cli-impact-goldens/`.

### 8.8 Measurement plan (B1)

CDO 94-file range: cold, warm, after a B refresh; default and `--low-mem`; with and without
tests (two test apps on the fixture); while another heavy request runs; cancellation mid-walk;
peak during the 699-root `walk_multi` and the accounting.

### 8.9 Commits (B1)

1. `feat(digest): declaration span index; every-changed-line accounting` (B-1b).
2. `feat(impact): comparison tuple; untracked files; diff/working-tree check` (B-3, B-3b).
3. `feat(impact): alsem impact, al-sem.impact/1, current effects and up` (B-1, B-2).
4. `feat(mcp): alsem_impact through a composite request with tests` (B-8, B-10).

### 8.10 As built in B1 (2026-11-05)

- **Parts.** `impact --part main` (production model: roots, accounting, digest's plan and the
  per-routine `RollupTotals`, the tests scope), `impact --part up` (production sited graph:
  `walk_one` per root with one reused scratch and a counting visitor, plus one `walk_multi` for
  the union), then C's select parts unchanged (`test_select::run::select_parts`, split out of
  C's `orchestrate`). The CLI runs each part in-process (a one-shot run builds the production
  program twice: once for the model, once for the sited graph); the MCP tool runs them on the
  resident engines.
- **Tests default on** (`--no-tests` / `tests: false`), not opt-in as §8.2's `[--tests]` reads:
  an agent reviewing a PR wants them, and a resident test universe answers in 0.2 s warm.
- **Nearest tested caller** (§4.4, left `null` by C): the up part keeps each root's first 32
  ancestors with a stable id in reach order; for a root every test universe reports
  `not-in-projection` / `not-loaded`, they are probed there (`tests --part select --probe-id`,
  status walk only, never joining the selection) and the nearest reached one is reported.
- **Accounting kinds** come from a line scanner over the analyzed text (§8.3's "IR declaration
  spans": the IR carries no origin for keys, field groups or layout), with three buckets §8.3 did
  not list: `object-header`, `comment-or-blank`, and `removed-routine-text` (removed lines that
  held a routine no new-side span holds, named from their header lines, never called deleted).
- **The diff check** compares each hunk's new-side lines (context and added) with the analyzed
  text, a byte-order mark ignored; the `index` blob ids are not used (no SHA-1 in the build).
- **inputsStamp.** A one-shot CLI run has none (printed "one-shot run"); the MCP answer prints the
  main part's revision stamp, so its bytes differ from the CLI's in that one clause.
- **`--base` with `--diff`** is refused (B2).

### 8.11 Warm latency on the resident engine (fix, 2026-11-05)

B1 measured warm `alsem_impact` calls of 35-58 s for one routine and a digest of 9-45 s after
them. Re-measured on CDO with the store's counters per call, five causes, all fixed:

1. **The parts were not served.** `impact` and `impact-up` were missing from
   `SERVED_COMMANDS`, so the child ran each part through the one-shot job path (a whole
   program build per part per call). §8.10's "the MCP tool runs them on the resident engines"
   was true only of the child, not of the artifacts.
2. **M0-1's "one rebuild on the first ask" lost the store.** The rebuild published a new
   revision with an empty store. `pin_sited` now ADDS the graph to the published revision:
   the program is rebuilt from the workspace session's retained parses over the revision's own
   read (a program refresh, not a model build), its graph seeded into the revision's store.
   This revises §1.2 M0-1 ("lazy is not possible without keeping the program"): the program is
   not kept, but the session can remake it cheaply for the revision it built.
3. **The witness batch reserved per root** at the batch's observed peak; now once.
4. **Witnesses are memoized only for reads of at most 64 roots** (`WITNESS_MEMO_MAX_ROOTS`):
   a larger read computes its entries itself and leaves the store as it found it.
5. **Estimates are per size class** (`budget::size_class`, by the roots a request's trees,
   raw facts, tree and witness batch cover): the observed peak of a 699-root request is no
   longer the next one-routine request's estimate.

---

## 9. E1. Debugging slice, narrow

### 9.1 The task

"Why did this error or this write happen?" E1 gives filters and the message text, with one
route and its selected guards. No route enumeration (E2).

### 9.2 Surface

`alsem prove <ws> R throws-error --match "<text>"`, `prove R writes-table:T --op Insert`,
`prove R <question> --site <file:line>`. MCP: `match`, `op`, `site` on `alsem_prove`.

- `match`: case-insensitive substring of the static message text (literal, label value,
  TextConst) or the label or variable name. Message text comes from the `Rules` artifact
  (`ArtifactKey::Rules`, already resident). E1's probe checks whether rules are built for
  dependency rows; if not, extending them is part of E1.
- `site`: keep only effects whose terminal operation is at that location.
- `op`: keep only writes of that operation kind.

### 9.3 Filtered verdicts

- `yes`: a matching site is reachable.
- `no`: no site can match **and** every reachable error site has a known static message (and the
  cone is complete).
- `unknown`: otherwise, with blocker `message-unknown: N reachable error sites have no static
  text` (a computed message, a `GetLastErrorText` rethrow, an implicit platform error). An
  unknown message never supports "no error matching this text". `ERROR_IMPLICIT` sites (a `Get`
  without `if`) match only by their platform kind words when `match` names them, and say so.

### 9.4 Guards

From M0-3 (pinned-input enrichment): for each site on the printed route, at most the 2
innermost enclosing conditions, each with its polarity (then/else/case arm) and its source text
(≤ 120 characters). They are **selected context, not all necessary conditions**: the output
prints `guards shown: 2 of 5 enclosing conditions`. A `while` condition is a **loop-entry**
condition; a `repeat … until` condition is a **continuation** condition; both are labelled.

```
throws-error matching "already issued": yes — 1 matching site reachable from Codeunit CDO Issue Document::IssueReminder (static candidates, not an observed trace)
site: dep:Base Application:Sales/Reminder/ReminderIssue.Codeunit.al:72  Error(AlreadyIssuedErr) — "Reminder %1 has already been issued." (Label AlreadyIssuedErr)
  route (6 hops): IssueReminder → [run] Reminder-Issue::OnRun → … → CheckReminder (ReminderIssue:72)
     guards shown (2 of 3): at ws:…/CDOIssueDocument.Codeunit.al:31 then-branch of `if not ReminderHeader.IsEmpty() then`; at ReminderIssue:68 then-branch of `if ReminderHeader."Issued" then`
other error sites: 14 reachable, 3 with no static text (blocked a "no" for other texts)
to confirm: set a breakpoint at ReminderIssue:68 and read ReminderHeader."Issued"; al-sem reads no runtime traces
```

### 9.5 Tests (E1)

| Id | Invariant | Discrimination break |
|---|---|---|
| E-2 | `match` filters by label value and label name; a site without static text never matches | match against the anchor text |
| E-6 | A cone with one computed-message error and no matching site answers `unknown`, not `no` | treat unknown text as non-matching |
| E-4 | `if X then A else B`, route through B prints `else` | drop polarity |
| E-7 | `while C do R()` prints C as loop-entry; `repeat R() until C` as continuation | label both the same |
| E-8 | Guards come from the pinned bytes (M0-T5) and print "k of n" | print guards as complete |
| E-5 | Without `match`/`site`/`op`, prove's output is byte-identical | always compute guards |

### 9.6 Commits (E1)

1. `feat(prove): match/site/op filters; message text from Rules; filtered verdicts` (E-2, E-6, E-5).
2. `feat(prove): selected guards on the route from pinned inputs` (E-4, E-7, E-8).

Measurement plan: CDO, resident, warm; with a dependency-code error site (re-parse cost of the
dependency files on the route); `--low-mem`.

### 9.7 As built (E1, 2026-11-05)

- **Guards are a gated assembly fact, not M0-3's re-parse** (coordinator's decision for E1; M0-3
  is not built). `ModelRoutine::site_guards` holds each routine's `if`/`case`/loop blocks with
  the condition text (folded, ≤ 120 characters), built only behind `ViewNeeds::site_guards`,
  which only `alsem prove` with a filter declares (`ViewNeeds::PROVE_SLICE`, with `rules` for the
  message text). Measured on CDO cross-app (`IssueReminder`, `ALSEM_TRACE`
  counters `prove.site_guard_arms` / `prove.site_guard_bytes`): **49,428 arms, 5.6 MB**, below
  r1's 15-130 MB estimate. Peak RSS of `alsem prove IssueReminder throws-error`: 3.14 GB
  unfiltered, 3.14 GB with `--match` (no measurable change). The slice itself (`prove.slice`
  span: a second `LiteralReach`, the root's live tree, the terminal records) took 0.45 s.
- The slice is a level-3 read of prove's tree (`TreeDemand::slice`), so it runs one-shot and
  resident alike, while the tree is alive. Sites come from the tree's terminal records (every
  direct fact, dependency code included), kept when the root's guarded search reaches a state
  where the site can run; a route is `GroupPaths::own_path` over the same search.
- Filters: `--match` (throws-error), `--table` (throws-error: errors a record operation on that
  table raises; the coordinator's addition), `--op` (throws-error and writes-table),
  `--site` (throws-error, writes-table, may-commit, reaches-ui). `publishes-event` and
  `commits-on-success-path` take none in E1.
- Two more `no` blockers than §9.3 names: `message-placeholders` (a text with `%1` that did not
  match: the value may hold the text) and `message-platform-wrapped` (a `FieldError` text). On
  CDO `IssueReminder` has 5,278 reachable error sites, 4,454 of them with no static text, so a
  filtered `no` is out of reach there; the `yes` side is what serves debugging.
- Guards are shown per route site (at most 2 innermost each), so `k` in "k of n" sums over the
  route's sites. A route through a TryFunction is not marked as trapped
  (`callers-intercept-errors` answers that question).
- A filtered document is schema 2.5.0; an unfiltered one stays 2.4.0, byte for byte.

---

## 10. B2. Impact before/after

### 10.1 What it adds

Per root present in both revisions, the delta of its behavior; per deleted root, its base
callers and page actions; per added root, its current summary marked `new`; schema findings.
Starts after **P0a** and **P0c**, with P0a's regression as its first test.

### 10.2 Labels

The base is analyzed against **today's** dependency environment (packages are gitignored, so
the base commit has none). Every answer says: `base analyzed under the current dependency
environment`, with the package hashes (`pkg:<path>` blake3) of both sides and every
non-archived input the base read (`roots.config.json`, `al-sem.dependencies.json`, source
dependencies outside the archive), each with its hash. When the diff changes `app.json`
dependencies, the delta of dependency-reaching effects is marked `unreliable: the dependency
declaration changed`, and the changed dependencies are named.

### 10.3 Semantic effect keys

A display anchor (`file:line`) is not an identity: a comment inserted above an unchanged COMMIT
would make a removal and an addition. Effects are keyed semantically:

- **routine identity**: the routine pairing (§10.4), not the stable id alone;
- **effect key** = `(family, normalized target, op)`: `write(T, Modify, class)`, `commit`,
  `error(kind, message-key)` where message-key is the label name or the static text's hash,
  `event(publisher id)`, `ui(kind)`, `http`, `file`, `background(target)`, `callee(stable id)`;
- per routine, the effect keys form a **multiset** (own sites counted by key) plus the cone's
  key **set** (beyond dependency entry points). Deltas are multiset/set differences; a key whose
  count changes prints `Modify on T: 2 → 1 sites`.
- When one key on one side maps to several sites and the pairing of sites is needed for display,
  sites are matched by normalized operation identity (op, receiver variable's table, argument
  shape), in statement-tree order; an unresolved match is printed `ambiguous`.

A count-preserving table swap (`Modify T1` → `Modify T2`) appears as `-T1 Modify, +T2 Modify`.

### 10.4 Pairing, preflight and policy reused from `alsem diff`

Routine pairing reuses `gate/diff/renames.rs` (signature pairing with its member and ABI
safeguards, `:176-235`), not "same object and name". The diff pipeline's preflight and its
coverage-sensitive loss policy (`gate/diff/policy.rs:1-6, 55-116`) decide the wording of a
loss: when the new side's coverage is incomplete for that root (unresolved calls in the cone,
literal analysis incomplete, write class uncertain), a key present only on the base side is
printed **"no longer observed"**, not "removed". Each side's summary stores its coverage,
literal-analysis outcome and write-class uncertainty.

Schema: the diff engine's schema, ABI, events and permissions passes over base and new
(`gate/diff/passes.rs`). It has field and enum passes but no key passes (`passes.rs:421-579`);
B2 extends the **shared** diff engine with `TableKeyAdded`, `TableKeyRemoved`,
`TableKeyFieldsChanged` (so `alsem diff` gains them too, its goldens move), each finding with a
pointer to D (`alsem_query_touches table:T field:F`).

### 10.5 Output (mock-up, illustrative)

```
impact of the working tree against comparison base 3ae74b8 …; base analyzed under the current dependency environment (packages: 11, hashes equal on both sides)
input: 176 changed, 523 added, 49 deleted (complete inventories); …
behavior changed in 61 of the 176 changed routines; first 20 by risk:
  Codeunit CDO E-Mail Handler::SendMail (ws:src/…:140): + write CDO E-Mail Log Entry Insert (physical); errors: + "Recipient %1 is blocked." ; commits 0 → 0
  Upgrade codeunit CDO Upgrade::OnUpgradePerCompany: + writes 4 tables …; - write CDO Setup Modify: no longer observed (the new side has 2 unresolved calls in its cone)
deleted: 49 routines; 3 still have callers in the base outside the diff (first: Codeunit CDO Ver20 Convert::ConvertSetup, called from CDO Upgrade::OnUpgradePerCompany at base:…:88)
schema: table-field-removed 1 (Table CDO Setup field 260), table-field-added 6, table-key-fields-changed 2; consumers: alsem_query_touches table:"CDO Setup" field:260
```

### 10.6 How the base is built

| Option | Verdict |
|---|---|
| a second resident revision in the same child | rejected: ~4.7 GB + ~3.4 GB exceeds the 8 GiB cross-app budget |
| a one-shot base child, graph-and-summary shape, cached per root | **chosen** |
| `fingerprint` snapshots + `alsem diff` | reused only for schema/ABI/events/permissions |

1. **Materialize**: `git archive <comparisonBase> <workspace subtree>` into the server's private
   folder (0.17 s for CDO's `Cloud`, measured). Packages are not copied; the base reads the
   current package discovery. Source dependencies inside the archive are read from it, others
   from disk (and stamped, §10.2).
2. **Build** in a one-shot `alsem engine-host --one-shot` child in the `ON_DEMAND`/low-mem shape
   (no tree stores; Fable), under the pool's budget.
3. **Admission**: no oversubscription (owner). The coordinator asks the pool for a child slot
   and budget; if the resident children leave no room, it waits for an idle eviction up to a
   deadline (default 60 s), then refuses with the reason (`base not built: the resident engines
   hold 7.9 of 8 GiB; retry when idle, or stop an engine`). The current side's part of the
   answer is still returned (B1's output).
4. **Lazy per-root summaries first** (astra a12, ar7): the base child computes the summary
   (§10.3 keys, coverage, base callers, page actions) only for the roots the request needs, and
   the cache stores them per root. An all-routine table is a separate option, measured against
   lazy before it is considered.
5. The current side computes the same summaries from the resident revision; the delta is
   computed per paired root.

### 10.7 Cache

Separate bounded namespace `~/.al-sem/cache/impact-base/v1/` (through `state_paths.rs`), not
`cache_prune.rs` (which handles only dependency JSON artifacts):

- **Key** = blake3 of: the comparison base's tree id of the workspace subtree; the
  `closure-v2:` stamp and every package hash; source-dependency content hashes; analysis
  options (universe, profile, `closedWorld`, low-mem); engine version; grammar commit; summary
  schema version.
- **Entry**: one file per base, holding per-root summaries; a header with schema version and
  the payload's blake3. Written to a temporary name and renamed (atomic publication). Readers
  verify the hash; a bad entry is deleted and rebuilt.
- **Limits**: at most 10 bases (owner); per entry 64 MB compressed, 256 MB decoded (refused
  above); folder total 256 MB; entries older than 30 days pruned; LRU by last read.
- **Concurrency**: two servers may share the folder; publication is by rename, so a reader sees
  a whole entry or none; two writers of one key both write and the last rename wins (same
  content).

### 10.8 Tests (B2)

| Id | Invariant | Discrimination break |
|---|---|---|
| B-0 | P0a's regression and #155's `Database.Commit()` commit fact hold | — (prerequisite) |
| B-3 | Base fixture: an added `Modify` on T shows `+T Modify`; a removed `Commit` shows `-commit`; an unchanged routine shows no delta | diff totals instead of keys |
| B-3c | A comment inserted above an unchanged COMMIT gives no delta | key effects by `file:line` |
| B-3d | `Modify T1` replaced by `Modify T2` shows both | compare counts |
| B-3e | A key missing on the new side whose cone is incomplete is "no longer observed" | print "removed" regardless |
| B-4 | A deleted routine (complete inventories) is resolved in the base and listed with its base callers | resolve deleted files against the new model |
| B-5 | A signature change pairs by `renames.rs`; two overloads stay deleted + added with the reason | pair by name only |
| B-6 | Cache: a second run with the same key does not build (counter); a changed package hash or option builds again; a corrupted entry is rebuilt | key by commit id only |
| B-6b | The cache never exceeds 10 bases or 256 MB (fixture with small limits) | no pruning |
| B-7 | The base child never writes into the workspace or the base tree's source; it reads packages from the current workspace | copy `.alpackages` into the base tree |
| B-9 | Schema findings equal `alsem diff base new --format json`'s, keys included | re-implement a field diff |
| B-11 | Admission refused → the current side and the reason are returned; no oversubscription (budget counter) | spawn over budget |

### 10.9 Measurement plan (B2)

CDO 94-file range: first impact against a new base (materialize, base child build, lazy
summaries for 225 roots), peak of the base child and of the whole process tree (resident child
plus base child), a second impact against the same base (cache hit), a changed package
(cache miss); lazy against all-routine summaries; cancellation during the base build; the
same with `--low-mem` on the resident child.

### 10.10 Commits (B2)

1. `feat(impact): materialize the comparison base; one-shot base child (low-mem shape)` (B-7, B-0).
2. `feat(impact): pool admission for the base child; refusal with the reason` (B-11).
3. `feat(impact): semantic effect keys and per-root summaries with coverage` (B-3, B-3c, B-3d).
4. `feat(impact): delta with diff pairing/preflight/policy; deleted and signature-changed roots` (B-3e, B-4, B-5).
5. `feat(diff): table key passes in the shared diff engine` (B-9) — `alsem diff` goldens move.
6. `feat(impact): bounded impact-base cache` (B-6, B-6b).

---

## 11. E2. Debugging slice, multiple routes

`--routes N` (default 1, at most 10) on prove's effect questions. Enumeration runs over the
literal-reach **state-transition graph** (states `(routine, bindings)`, edges = sites), not the
retained single-parent tree (`literal_reach.rs` keeps the first parent per state).

- **Route** = a simple path in the state graph from the root's start state to a state at the
  terminal site; recursion is allowed once per routine (a state may repeat with different
  bindings, a `(routine, bindings)` state may not).
- **Distinct** = different sequences of sites.
- **Order**: shortest first (hops), then by site ids.
- **Caps**: the state graph is the one literal reach already builds (its cap `STATE_CAP`
  400,000 states); route enumeration has its own cap of 10 routes and 50,000 edge expansions. A
  cap that fires is printed as `limit` and the answer says routes may be missing.
- **Counts reported**: routes shown, candidate edges examined, candidate edges rejected by
  literal arguments. Never "N routes ruled out" as an exact count.
- Guards per route from M0-3, as in E1.

Tests: E-1 (two routes to one Error: `routes: 3` lists exactly 2, shortest first, distinct site
sequences; break: dedupe by terminal only), E-3 (a literal-rejected edge is counted, not listed;
break: enumerate over the unfiltered graph), E-9 (reconverging paths give two routes; break:
walk the parent tree), E-10 (a cap gives `limit`; break: stop silently). Commits:
`feat(literal): keep the state-transition graph for one root on demand`;
`feat(prove): --routes`. Measurement: state-graph memory for RunReportID and IssueReminder on
CDO, routes 1/3/10, warm.

### 11.1 As built (E2, 2026-11-05)

- `--routes` goes only with a filter (`--match`/`--site`/`--op`/`--table`); `--routes 1` is E1's
  answer byte for byte (E1's route off the search tree, no `routes` key, no `filter.routes`).
  Above 1, routes are enumerated for the first 20 matching sites (E1's `MAX_ROUTES`), each with
  its own 50,000-edge budget. E1's `route` stays on each row; an enumeration that lists none
  falls back to it, labelled.
- The state graph is the literal search itself run once with a recorder
  (`LiteralReach::state_graph`, module `literal_reach::routes`): same states, same order, same
  first parents, so the rest of the slice reads its tree unchanged. Transitions are kept as
  (target state, edge index) pairs, plus their live reverse and the reversed routine graph of
  the root's guard-free reach.
- A route ends at its FIRST state where the site can run (a longer one passes the site first).
  Enumeration is best-first over partial routes ranked by (hops + exact hops left, site-id
  sequence), so routes come out in the spec's order without a sort.
- Rejected edges are split four ways: literal arguments at the edge's own site; literal
  arguments further on (no live way on, but the callee reaches the site's routine with guards
  ignored); back to a state on the route; callee cannot reach the site at all. Counts are of
  edges, each time a partial route reaches them.
- Cancellation: the request's stop reaches the slice through `read_tree_shared`; the resident
  tree read checks it again after the read. Checked at the first edge and every 1,024.
- Equal-length routes are ordered by site ids, so `--routes 3`'s first route can differ from
  `--routes 1`'s (E1's is the search tree's first). Both are shortest.
- CDO (pinned baseline, `release-fast`, cold CLI; the machine had a load average near 380, so
  times are relative only): `RunReportID throws-error --op Error --routes 10`: state graph
  21,556 states, 40,178 transitions, 3.2 MB beyond the tree; 20 sites enumerated in 8.6 ms with
  153,211 candidate edges examined in all. `IssueReminder --match issued --routes 3`: 21,818
  states, 40,626 transitions, 3.2 MB, 58 edges. Peak RSS 3.09-3.15 GB at routes 1, 3 and 10 (no
  measurable change). The `prove.slice` span with routes is no longer than without (the graph
  replaces the tree: the same search).

---

## 12. Surface, consumers and versions

### 12.1 Tools

After this spec: `alsem_explain`, `alsem_prove`, `alsem_impact`, `alsem_tests`, `alsem_digest`,
`alsem_query_effects`, `alsem_query_touches`, `alsem_status`, `alsem_refresh`. The merge is
dropped, so there are no aliases and no tombstones. Each tool's description is updated in the
commit that ships its feature.

Selector and mode validation (enforced by clap in the CLI and by the MCP front end before
sending, with the same error text):

| Tool | Exactly one of | Optional | Refused combinations |
|---|---|---|---|
| `alsem_tests` | `routines`, `diff`, `files`, `base` | `testApps`, `tier`, `maxTests`, `diffRoot` (with `diff`) | `diffRoot` without `diff` |
| `alsem_impact` | `routines`, `diff`, `files`, `base` | `base` with `diff` (B2), `tests`, `maxRoots`, `diffRoot` | `base` with `routines` |
| `alsem_prove` (new) | — | `match` (only `throws-error`), `op` (only `writes-table:`), `site`, `routes` (E2), `assumeNoDependents`, `at` | `match` on another question |
| `alsem_query_touches` | `table` | `field` (with `table`), `from` | `field` without `table` |

Routine names in paths print a stable **overload handle** (`Name/arity` when unambiguous in the
object, else `Name/arity#<short signature hash>`), so two same-arity overloads never print the
same. JSON paths carry the stable id.

### 12.2 Consumers and versions

| Consumer | Reads | Change |
|---|---|---|
| ConTinAI `effects.py` | `query effects` JSON | none |
| ConTinAI `graphs.py` | `alsem graph` JSONL, `fingerprint --inventory-only` | graph additive (`sourceRoot`, `--keep-local-sources`; byte-identical otherwise, C-8, M0-T1) |
| ConTinAI `verify.py` | prove `result.answer`, `evidence[0].evidence.{file,line}`, `blockedBy`, `assumptions` | prove 2.4.0 additive; existing questions byte-identical except the version (A-13) |
| bc-dev-mcp (pinned 1.7.1) | digest 2.x, `alsem graph` | nothing breaks; may adopt `--keep-local-sources`, later `al-sem.tests/1` |
| MCP agents | tool list, descriptions, instructions | per milestone |

New or bumped: `al-sem.tests/1`, `al-sem.impact/1`, prove 2.4.0, query 3.2.0, explain
`al-sem.explain/4` (A's section; milestone I's write rows and tests line also move explain
goldens, under `/3` because they change text, not the schema), `alsem diff` key findings,
MCP status `statusVersion: 2`. CHANGELOG in each commit.

### 12.3 Instructions' "which tool" line (final form, after A, B1, C, D, E1)

> Before editing a routine: alsem_explain. Adding a COMMIT, an error or UI: alsem_prove with
> callers-have-pending-writes / callers-intercept-errors / callers-may-run-noninteractive
> (callers' context); the other prove questions ask about R's existing effects. Reviewing a
> diff or a PR: alsem_impact. Which tests to run: alsem_tests. The full downstream inventory of
> roots: alsem_digest. Record operations of a routine: alsem_query_effects; who touches table T
> or field F: alsem_query_touches. Why an error or write happened: alsem_prove with match/site.
> Is the engine current: alsem_status (it checks the disk).

---

## 13. Milestone I and the settled decisions

### 13.1 Milestone I (immediate fixes)

| Id | Fix | Goldens |
|---|---|---|
| I-1 | prove's six questions: first line `existing effects:`; the prove description says "existing effects"; a contract test that no description or instruction contains "safe" (§5). Human text only; JSON unchanged | `cli-b-goldens/prove`, `cli-output-goldens` prove |
| I-2 | P0b: explain's `tests:` line says "no test code found in the workspace app" or "N tests loaded; none reaches it" (scope rules of §4.2 without sibling discovery; discovery joins in C) | explain families |
| I-3 | touches' absence wording: a whole-cone `no` is never printed before the warning that unresolved targets may be the table (`effect_query_cli.rs:1790-1794, 1849-1859`); the answer becomes `not observed (N unresolved calls may reach it)` when the cone has unresolved targets | `cli-query-goldens` |
| I-4 | explain write rows without path dumps by default (owner): per table keep table, op kinds, write class, first site, "via `<callee>`, N hops"; commit, UI and error paths stay; `maxPaths ≥ 1` or `detail` expands; overload handles (§12.1). T-W2-5 becomes "every workspace table row names its first site and first hop; with `maxPaths ≥ 1` it has a witness" | explain families, CDO rendering goldens |

Each with a discrimination proof (break the new wording's condition, see the test fail,
revert). Measurement: explain card size on CDO `RunReportID` before and after I-4.

### 13.2 Settled decisions (owner, from both reviews)

| r1 question | Decision |
|---|---|
| Q1 test-universe memory, C2 | keep `--max-children 2`; start the test child on demand; C2 is a later separate design |
| Q2 discovery | bounded auto-discovery: skip apps with no `.al`; `testApps: []` disables; a cap gives "discovery incomplete"; workspace tests always included |
| Q3 base budget, cache | no oversubscription: wait for an eviction or refuse with the reason; ≤ 10 bases in its own bounded namespace |
| Q4 base closure | base under the current `.alpackages`, labelled "base analyzed under the current dependency environment", package hashes on both sides |
| Q5 explain write paths | off by default; keep table, kinds, class, first site, first hop, hop count; commit/UI/error paths stay; explicit expansion; unambiguous overload handles (I-4) |
| Q6 merged query | **dropped**; both tools stay |
| Q7 question names | `callers-have-pending-writes`, `callers-intercept-errors`, `callers-may-run-noninteractive` |
| Q8 external callers | separate `assumeNoDependents`; friends included |
| Q9 F3 before A | not required: A reads the capability `start` fact now; F3 later becomes edges R0 picks up |
| Q10 bc-dev-mcp | adopts `alsem tests --format json` later, once pinned |
| Q11 P0a | in progress on `fix/commit-up-chain`; with #155 (P0c), a hard dependency of A and B2 |

### 13.3 Still open

1. M0-1's eager sited graph: if CDO measurement exceeds 300 ms per B refresh, is the
   "graph-capable after first up-question" option acceptable (one rebuild on the first ask)?
2. C's split of hidden channels (§4.5) depends on the C1 probe; if CDO's 47 cannot be shown
   covered, every CDO selection carries `requiredFallback` until the test libraries' symbols
   are present. Accept that, or decide a weaker rule?
3. The 4,699 unresolved test-code calls on CDO come from test libraries missing from the
   scratch `.alpackages`. Should C print a specific next action ("add Library Assert,
   Tests-TestLibraries symbols to <test app>/.alpackages") when it sees the standard Microsoft
   test library ids unresolved?

---

## 14. Cost summary on CDO

| What | Measured / estimate | Number |
|---|---|---|
| Manifest check (status, per engine, packages only) | measured (context-server §5.3) | 1.2-1.7 ms; with source dependencies: measure in F |
| `.al` files / packages in `Cloud` | measured | 582 files; 11 packages, 70 MB |
| `alsem graph` cold, workspace universe | measured | 12.4 s, 2.54 GB |
| `alsem graph` cold, test universe | measured | 13.5 s, 2.59 GB |
| `explain` on the test-app workspace | measured, **failed** (exit 2) | 20.5 s, 3.12 GB: a build bound only |
| `fingerprint --inventory-only` cold | measured | 21.1 s, 3.12 GB |
| `diff ws ws` | measured | 8.6 s, 1.94 GB |
| `digest --diff` 94-file range, new side (699 roots, paths) | measured | 63.0 s, 5.24 GB, 42 MB JSON |
| same, reversed diff on the base tree (225 roots) | measured | 42.3 s, 4.07 GB |
| `git archive` of `Cloud` | measured | 0.17 s |
| Export (pruned, collapsed) | measured | 31,265 nodes, 78,936 edges |
| Call sites resolved (graph histogram) | measured | workspace 11,905; dependency bodies 191,084 |
| Sited graph per revision | estimate | 250-350k edges, 6-9 MB, < 300 ms |
| Reverse walk per root | estimate | ms |
| `walk_multi`, 699 roots | estimate | < 1 s, O(states) |
| `gui_guard` | estimate | < 1 MB |
| Field index per `--field` query | estimate | < 200 ms |
| Test universe resident (graph-only) | estimate from `graph` | ≤ 2.6 GB peak, less resident |
| Base child (low-mem shape) + lazy summaries | estimate | 2.5-3 GB peak, 25-35 s first time; cache hit: seconds |

---

## Appendix A. How the numbers were produced

- Binary: `~/.al-sem/bin/alsem` (al-sem 2.1.1). Workspace argument
  `/home/sshadows/Projects/DO-cdo-baseline/Cloud` (clean, `bc3ccb18`); `CDO_WS` not set. Each run
  under `/usr/bin/time -f "%e s %M KB"`, one at a time. Scripts and outputs:
  `target/tmp/agent-tasks-scratch/` (`measure.sh`, `measure2.sh`, `times.txt`, `times2.txt`).
- `explain-testapp-run` **exited 2** (`explain: no routine of the program has the id or name
  'RunReportID'`, `explain-testapp-run.out`): it measured a build plus a selector failure, not an
  explain.
- Test universe: `TestApp/` = a copy of `DO-cdo-baseline/Test` (`app.json`, `src/`), Cloud's
  `.alpackages` copied in (the test libraries were not available and stay unresolved: 4,699
  primary-scoped unknown calls in the test graph's histogram), `al-sem.dependencies.json` =
  `{ "sources": [".../Cloud"] }`, `git init` so package discovery stops there. The baseline was
  not modified.
- Reverse walks: Python (`-I`) over the `graph` JSONL: reversed edges, BFS; "resolved" skips
  `confidence: Open`; "local" keeps only nodes whose `appId` is the CDO app or the test app.
  Re-derived independently by the Fable review with the same figures.
- Base tree: `git archive bc3ccb18~5 Cloud` into `base/`, Cloud's `.alpackages` copied in.
  `fwd.patch` = `git diff bc3ccb18~5 bc3ccb18 -- Cloud`, `rev.patch` the reverse. New side:
  `digest Cloud --diff fwd.patch --diff-root DO-cdo-baseline --format json --compact`; base side:
  `digest base/Cloud --diff rev.patch --diff-root base …`. Roots joined by `routine.stableId`.
  The "48 of 176" is a **count-change census**: it compared `summary.mayWriteTables.count`,
  `summary.commits.ownSites` and `summary.kinds.errors` only.
- Source counts (Fable, on the baseline): `Test/` 139 `.al` files, 101 with `[Test]`, 1,860
  `[Test]` lines, 101 `Subtype = Test`, 1 `Subtype = TestRunner`; `TestFR/` no `.al`. `Cloud`
  file and package counts by `find … -name '*.al' | wc -l` and `du -sh .alpackages` (this
  revision).
- Call-site counts: the `histogram` in the `meta` line of `graph-cdo.out` (`wholeProgram`
  resolvedSource 11,639 + conditionalResolved 266; `dependencyBodies` resolvedSource 191,084).

### Milestone C's probe (C1, measured 2026-11-05)

With `alsem tests` itself (branch `m-c-tests`, release-fast; the real `DO-cdo-baseline/Test`,
auto-discovered, the production app injected, nothing written into it):

- **Hidden channels in RunReportID's resolved cone: 48** (R0 counted 48, one more than the 47
  above: `Check Dimensions::OnErrorMessageDrillDown`). By class: **47 `platform-table-event`**
  (synthetic `OnAfter*Event`/`OnBefore*Event` publishers a subscriber binds to) and **1
  `unbound-subscriber`**. None of the 47 can be shown covered: a record write routes to the
  table's declared triggers only (`resolve_implicit_trigger`), so no write is an edge into a
  platform publisher; they are reached through the record hub alone. By the owner's rule (one
  that cannot be shown covered counts), all 48 are selection-relevant and the CDO selection stays
  `selectionComplete: false`. Making them coverable needs a resolver fact (each record write's
  table as a route into its platform events), not a weaker rule.
- **The 16 tests** between "1,860 `[Test]` lines" and 1,844 loaded nodes: `grep '\[Test\]'` finds
  1,847 occurrences in `Test/`; 3 are commented out (`// [Test]`, in `CDOOpenDocumentsTests` and
  twice in `CDOSendCustStatementTests`); the other 1,844 are attribute lines, and the loaded
  graph has 1,844 test nodes with the same count in every file. No test is lost; the 1,860
  figure did not reproduce.
- **Dependency-tier sameness**: RunReportID's 301 through-dependency tests reach it via 7
  dependency nodes (`Page 42` 160 tests, `Page 144` 109, `Table 60::onbeforesend` 22, …). Their
  *reverse* cones are 23,567 of 137,196 nodes (17 %); their *forward* (may-execute) cones are
  91,684 (67 %). C reads §4.5's "own cone" as the forward one: a node that may execute two
  thirds of the program is why the same tests reach unrelated routines.
- **Unresolved test calls: 4,702** (sites, by call-site anchor, from test-app code); the test
  app declares 6 Microsoft libraries that are not loaded (`Tests-TestLibraries`, `Test Runner`,
  `Library Assert`, `Permissions Mock`, `Any`, `System Application Test Library`) and
  `Continia Core Internal Activation App`.

### Milestone C's open items (measured 2026-11-05)

Binary: `alsem` release-fast at `d6fc10ab` (branch `m-c-tests`). Workspace: a scratch copy of
`DO-cdo-baseline/Cloud` and `Test` (`bc3ccb18`) under one folder with an empty `.git`, so
auto-discovery finds `Test` and the B edit never touches the baseline. Driver:
`target-mc/scratch/tools/mcp_measure.py` (stdio JSON-RPC to `alsem mcp`; peak RSS is the sum
over the server and its children, sampled every 100 ms). Every call is
`alsem_tests {routines: ["CDO E-Mail Handler::RunReportID"]}`. The machine ran other builds
(load average 25-230 on 22 cores), so the cold times are upper bounds; the cold default run was
measured twice (64.6 s at load ~200, 23.0 s at load ~40).

| Run | default | `--low-mem` |
|---|---|---|
| cold (both engines build, one after the other) | 23.0 s, peak 6.0 GB | 24.0 s, peak 4.6 GB |
| warm | 0.18-0.20 s | 0.18 s |
| after a B edit of `CDOEMailHandler.Codeunit.al` (a comment in RunReportID's body) | 13.2-14.3 s, peak 6.5 GB; both universes at generation 2 | 14.2 s, peak 5.0 GB |
| warm after the edit | 0.17-0.19 s | 0.18 s |
| resident RSS after (production / test universe) | 3.85 / 2.16 GB | 2.80 / 1.21 GB |

- **A B edit costs the test universe a full rebuild.** For the test universe the production file
  is an injected input, whose refresh class is `Package` (§2.3), so a body-only edit rebuilds it;
  its cold build alone took 9.9 s (below), so most of the 13-14 s is likely that rebuild (the
  split between the two engines was not measured). A body-level
  refresh of injected sources is the next step if this matters.
- **Cancelling a cold build** (the call cancelled 3 s after it was sent, while the production
  engine built its first revision): the build is not interrupted (the child reads a cancel
  between phases, and the build is the `pin` phase). It ran on and the engine was idle 10.0 s
  after the cancel; the call was not answered, no test engine started (item 2), and the built
  revision was kept: the next `alsem_tests` took 9.9 s, the test universe's cold build alone.
- **Item 1** (explain's summary): on CDO the three explain cards' summary now says `tests: none
  in the workspace; 1 test app discovered (not analyzed here; alsem tests)`, as the `tests:` line
  below it does.
- **Item 4**: `alsem tests Cloud --routine "CDO E-Mail Handler::RunReportID"` (human, cdo
  profile, 33.4 s, 3.12 GB one-shot) prints `through dependency code: 301 tests — effectively the
  suite's posting tests, not specific to this change`; the CDO ratchet pins the line.
- Not measured here (still open from §4.11): two test apps side by side, `alsem_tests` during a
  heavy explain, a test-file B edit, peak RSS during the sited-graph construction alone.
