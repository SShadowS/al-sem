# Engine limits (from output validation, rounds 1–4, and the limit probes)

Written 2026-11-09, at master `58d0c94e`.

- **L1–L14** were found by output validation, rounds 1–4.
- **L15–L26** were suspected and then checked by probes on 2026-11-09. Their old ids, P1–P12,
  are kept in brackets.
- **L27–L34** are further limits the probes noticed along the way.
- **B1–B8** are wrong answers or a crash that the probes found. A wrong answer is worse than a
  limit: limits are stated in answers, these are not. They come first in the order.

**Where the evidence is:**
- The probes' full results are in `target/tmp/engine-limits/results.json`.
- Probe inputs and outputs are in `target/tmp/engine-limits/<area>/`, one folder per area:
  locks, values, errors, declarations, callers, server.
- Call ids (c08, n10, m10, …) refer to `target/tmp/output-validation/round*/bundle.md`.

**Sizes:** S is under 1 day, M is 1–3 days, L is more than 3 days. These are the probe agents'
estimates from reading the code. Nothing has been measured.

**Status:**
- **confirmed:** a probe showed the limit is real.
- **partial:** part of it is real; the row says which part.
- **found:** seen in the recorded validation calls.

## 1. Wrong answers and a crash (B1–B8)

Each of these gives a definite answer that the source contradicts, or kills the engine. None is
stated as a limit in the answer.

**Fixed in wave 0** (`integrate/w0`, 2026-11-09): B1, B3 and B8. CHANGELOG has a section for
each. B3 also decides the escape line of L7 for isolated events. What is still open:
- B3: forward `throws-error` still counts an isolated subscriber's error as the publisher's own.
  That is B7. **Done in wave 2** (`fix/w2-b7-traps`): it stays a `yes` (a may-answer: the error
  escapes when a write is pending), and the answer now says so.
- B8: the query `scope:` line still says "one dependency routine deep" when no dependency code is
  entered.
- B8: test-app discovery does not read `.gitignore`.

| # | What is wrong | Probe evidence | Size |
|---|---|---|---|
| B1 | **`alsem_prove` with a filter (`match`, likely also `site`, `op`, `table`) crashes the resident engine** (signal 6). The resident revision is built for explain (`ViewNeeds::EXPLAIN`, `src/engine/resident/session.rs:52`, no site guards). `ProveJob::demands` asks for site guards when a filter is given (`prove.rs:3791-3801`). The function that would widen the needs, `pin_needs` (`session.rs:279`), has no production caller. Panic: `workspace.rs:907 'site guards not built'`. The one-shot CLI answers correctly. | server/ (MCP, 1-codeunit fixture, single-app and cross-app) | S |
| B2 | **`#if` arms are run one after another.** An `exit` or `Error` inside one arm cuts off the code after the group in every configuration. So prove answers a wrong `no (every candidate decided)` and a wrong `unconditional` (`ir_walk.rs:1897`). A routine declared in two arms cites the second arm's line for the first arm's write. **Fixed in wave 2** (`fix/w2-b2-preproc-arms`, CHANGELOG "engine limits wave 2, B2"): a statement `#if` chain is an IR `PreprocGroup` whose arms are alternatives (a `case` in the statement tree, with an implicit empty arm when there is no `#else`). The code after an `if`, `case` (a group included) or loop that only MAY `exit` is `conditional` (`control_flow::may_exit`; this also fixes the older plain-AL `if a then begin if b then exit; end; Insert()` shape), and every row of a routine declared in two arms has its `top-level` sites lifted to `conditional`. **Left:** the two-arm routine's line citation is NOT fixed: both arms share site ids (`…/op0`) by the #160 step 11b collided-id design, and readers resolve an id to one row (the last). Per-arm ids (a site-number stride) fix it but break 19 tests that build the 11b precondition from two `#if` arms and change answers the 11b design settled (two producers per duplicated declaration, an interface call counted per arm, d1 keeping one seed); that needs an owner decision (per-arm ids with the 11b contract restated by assignment, or the owning row carried with each fact). No configuration is chosen (L22 later steps), so an arm's effect is `conditional`, never `unconditional`, even in the build that compiles it; the `earlier exit if …` guard is not carried past a group, so such an `Insert` reads plain `conditional`; split shapes (`#if` inside an `if` header, a split call or `case` header) keep their union reading; collecting walkers (record-ref tables, closed enums, first-assignment) still read the arms in document order as one list; site guards list no `#if` arm as a guard. | values/probe-preproc.txt | M (step 1 of L22) |
| B3 | **`callers-intercept-errors` answers a wrong `no` for a subscriber of an isolated event.** Isolated dispatch catches the error, but it is not treated as a trap. On CDO, `OnCompanyOpenCompleted`'s subscriber gets `unknown` where the answer is `yes`. | errors/iso.out, cdo-iso-intercept.out | S (part of L7) |
| B4 | **`[CommitBehavior(Error)]` is ignored.** A `Commit()` under it raises an error, but `throws-error` says `no (every candidate decided)` and `commits-on-success-path` says `yes`. Under `Ignore`, the commit is still reported as a success-path commit. **Fixed in wave 2** (`fix/w2-b4-commit-behavior`, CHANGELOG "engine limits wave 2, B4/L16"). | locks/p2-pure-error.txt | M (L16) |
| B5 | **Fixed in wave 2** (`fix/w2-b5-flowfield-reads`): a `CalcFields`, a filter on a FlowField and a read after `SetAutoCalcFields` now read the `CalcFormula`'s table (source or symbols); an undecided one is `unknown`, never `no`. CDO: `Balance` vs `Detailed Cust. Ledg. Entry` is now `down: yes`. ~~`query touches --direction down` says a definite `no` for a table read only through a FlowField's `CalcFormula`.~~ What is left is in L19. | declarations/out | M (L19) |
| B6 | **`prove writes-table` says a definite `no` for a table written only by an XmlPort's automatic insert.** **Fixed in wave 2** (`fix/w2-b6-dataset-ops`): the platform's dataset operations are record operations of the running call, with their reason; see L20 for what is left. | declarations/out | M (L20) |
| B7 | **An error behind a trap is reported as escaping.** A trap is a checked `Codeunit.Run`, a `[TryFunction]` whose result is used, or an isolated event. `throws-error --site/--match` answers `yes`, and the condition reads `unconditional on success`, which describes the callee and not the route. **Fixed in wave 2** (`fix/w2-b7-traps`, CHANGELOG): an error a definite trap catches on every route is no longer counted by prove (with or without a filter), explain or digest; an isolated event's error stays `yes` and states its condition; a route trapped on some paths says which. A checked run with a write that may be pending is no definite trap (L28), and a TryFunction traps only where its value is tested. Round 2 (`fix/w2r2-b7`): a `Commit()` that `[CommitBehavior(Ignore)]` or `(Error)` stops no longer ends the write before a checked run, and explain and digest state the callers' assumption a caught ruling rests on, as prove does. Left: see L23 and L28. | errors/p9.out, cdo-sendqueue-site.out | M (L23) |
| B8 | **Small wrong output.** Each item is S. |  | S together |

The B8 items:
- The human fingerprint prints `W` for an insert right (`fingerprint_query.rs:303-311`); AL uses
  `I` (`snapshot_full.rs:1145-1152` is right).
- In single-app mode, throws-error suggests `prove … callers-intercept-errors --single-app`, which
  the CLI rejects.
- The help text of `mcp`/`engine-host --memory-budget` says the default is `6G`; it is 8 GiB
  (`src/bin/alsem.rs:235-236, 267-269`).
- `locks: none observed` always blames dependency depth, even with 0 dependencies, or when the real
  cause is an UpdLock read.
- An interface call with no implementer is counted as `(0 in this workspace)`, yet listed under
  `gaps in workspace code`.
- A stale answer is labelled `served: fresh`. The size-and-time residual shows only in status.
- Explain repeats the same five dependency routine names once per place (about 5 KB on CDO
  `SetHandled`).
- Test-app discovery stops at 64 `app.json` files when the workspace sits under a folder with
  many fixtures.

## 2. All limits

### Locks and transactions

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L1 | Reads, locks and validations listed one dependency routine deep. Deep reads and lock facts already ride the cone, so this is mostly output. Validations need a new cone fact. | found | M | — |
| L3 | No lock state along call paths and no lock order. Answers must list held tables, never give a "safe" verdict. Lock escalation and SQL range locks are not modeled. | found | L | L1, L15 |
| L15 (P1) | `ReadIsolation` UpdLock/RepeatableRead and `FindSet(true)` are plain reads, never locks. On CDO, `RunMigration :307-308` takes an update lock and the answer says `locks: none`. Approach: record isolation per read in the body walk, then emit a lock fact. | confirmed | M | — |
| L16 (P2) | **Fixed in wave 2 (B4).** `[CommitBehavior]` now decides every forward commit and error answer (prove, its `--site/--match` slice, digest, explain) through the rule the callers' question reads (`commit_scope::declared`). A route that crosses a checked `Codeunit.Run` keeps its commit as a commit whatever is declared (Learn: the run's implicit commit is not affected by `CommitBehavior`); an unchecked object run only *may* carry the scope (Learn is silent). What is left: (1) `InherentCommitBehavior` is parsed (the probe was wrong: `workspace.rs` reads it) but is not on Microsoft Learn, so it only *may* apply: it never removes a commit, and under `Error` adds a possible error. (2) **The implicit commit of a checked `Codeunit.Run` has no fact.** A checked run whose codeunit has no explicit `Commit()` answers `may-commit no` (on master too). The rule above uses an explicit commit inside the run as its stand-in, so only the no-explicit-commit case is still wrong. (3) `alsem fingerprint` (and `policy check`, which reads its capabilities) does not apply the rule: it says `commit: yes` for a root whose only commit is ignored or errors. That over-states, it never says a wrong `no`. (4) A commit turned into an error inside a used `[TryFunction]` or an isolated event is reported as escaping, like every trapped error (B7/L23). (5) `query effects` lists table effects only, so it has no commit to change. (6) A slice site that is a commit-turned-error names no reason (the unfiltered answer does). (7) Whether the scope follows an unchecked `Codeunit.Run` was not checked on a BC server (the BC 28 sidecar). | fixed (remainder stated) | M | — |
| L27 | `callers-have-pending-writes` says `no` when a caller holds a `LockTable` but wrote nothing, so an agent about to add a lock gets no hint. | new | S | L3 |
| L28 | A checked `Codeunit.Run` inside an open write transaction is a run-time error in BC. Nothing models it. CDO avoids it by committing first (`CDOQueueManagement:46`). Not probed on BC. **Wave 2 (B7 review):** the forward error answers read it: a checked run at which a write may be pending (a write before it in its routine, a callee that may leave one, or a caller in the root's reach that calls with one pending; a `Commit()` on the way clears it, also one earlier in the same loop iteration) is no definite trap, and a caught ruling that may rest on a checked run states that it assumes the root's callers have no write pending. **Round 2 (`fix/w2r2-b7`):** a `Commit()` clears only when it runs normally on every route from the root, by B4's `[CommitBehavior]` rule (`CbGraph::commit_surely_runs`): under `Ignore` the write stays pending, under `Error` the commit raises an error and commits nothing (Learn, "CommitBehavior attribute"). Before, `[CommitBehavior(Ignore)] … ModifyAll; Commit(); if Codeunit.Run(Thrower) then;` answered a wrong `no`. A run that is clean only because a `Commit()` ends the write adds a second assumption: the callers do not call the root under `[CommitBehavior(Ignore)]` or `(Error)`. explain and digest now state both assumptions too (header assumptions line, the caught line, JSON `summary.assumesNoPendingWriteAtEntry` / `assumesNoCommitBehaviorAtEntry` and `coverage.assumptions`), not only prove. **Left:** the run's own error is not added as an error site (the answers keep the callee's errors instead, with the condition), and the commit answers do not model it. Platform-side database writes before a checked run (`IsolatedStorage.Set`, Media import and similar) do not count as a pending write; whether BC counts them as a write transaction is not established. An event with no loaded subscriber before a checked run counts as no write; that rests on the loaded-program assumption every answer states. A `Commit()` inside a callee before the run still ends nothing (the pending-writes walk's known over-approximation). The transaction spans and the callers' questions still read every `Commit()` as a commit (B4's to decide). An explicit commit inside a checked run is read as possibly under its caller's scope (Learn is silent); no current answer depends on it, since an error inside a clean checked run is caught by that run anyway. | partial | M | L23 |

### Values (literal pruning)

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L4 | **Cause corrected:** literals are carried through any number of calls. m10 fails because **platform option types** (`TelemetryScope`, `Verbosity`, `DataClassification`, `ObjectType`, …) are not a value domain (`literal_values.rs:298 domain_of`). | partial | S | L14 |
| L5 | Pruning gaps by design: globals, conversions, unresolved labels, values from data (91 of CDO's 323 `case` statements switch on a record field), and arithmetic. Each keeps both branches live. | confirmed | L | L4, L14, L21 |
| L29 | **Text and Code literals are not a value domain:** `case T of 'A':` keeps every arm live. | new | S | L14 |
| L6 | The company a write targets after `ChangeCompany` is unknown. All 12 CDO sites pass a variable, so it needs values from data. | confirmed | M | L5, L21 |
| L22 (P8) | `#if`: no configuration is chosen. Step 1 (B2, wave 2) is done: statement arms are alternatives, not flattened. Later steps: follow the app's `preprocessorSymbols` and a file's `#define`/`#undef`, infer each dependency's symbols, state the configuration in the answer, and make routines that exist in only one arm agree with the resolver's filter (a chosen configuration lets a two-arm routine's arm read `unconditional` again in the build that compiles it). | partial | L | — |

### Errors

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L7 | Whether a subscriber's error escapes its publisher. The answer is decidable locally: it escapes unless the event is isolated (see B3). Implicit errors (`Insert` failing because the record exists) name the operation, not the receiver or the reason. | partial | S | L10 |
| L8 | Guards past an unanalyzed dependency entry. 0 CDO calls reach it, because every dependency ships its source. The answer should name the routine without a body. | confirmed | S | — |
| L23 (P9) | Traps are not applied in the forward error answers (see B7). CDO: 34 TryFunctions and 8 checked runs; Base App: 327 and 30. The fix touches the literal reach, so measure speed. **Fixed in wave 2** (one trap rule, `src/engine/l5/traps.rs`, read by both directions). **Left:** (1) a row that may stand for unseen sites (a site search stopped at its depth bound) is never ruled caught, and a dependency boundary group is ruled only by the route to its crossing (a crossing that is itself a trap, or an event crossing, keeps the group escaping); (1b) the cone's obligations (open-world and unresolved calls) are cone-wide, so a filtered answer whose only matching site is caught is `unknown`, not `no`, when an unresolved call sits anywhere in the cone, even inside the trapped part; (2) an interface call into a TryFunction implementer is not a trap; (2b) the trap-free walk ignores guards (for speed), so an error whose only trap-free route the literal arguments rule out stays escaping; (3) the writes of a failed checked run are rolled back, but `writes-table` is a may-answer and says nothing about it, which stays correct; (4) a checked run inside an open write transaction raises its own error (L28): the forward error answers now treat a checked run at which a write may be pending (within the root) as no definite trap (`escapes-if-write-pending`), and a caught ruling that may rest on a checked run states, in prove, explain and digest alike, that it assumes the root's callers have no write pending (and, when a `Commit()` before the run ends the write, that they call it under no `[CommitBehavior(Ignore|Error)]`; see L28). An effect row that stands for several error sites (one `ERROR_THROW` row whose origins are a site behind a checked run with a write pending and a site with no trap, e.g. `Report.Run(R); if Codeunit.Run(Thrower) then;` where the report raises an error too) is judged as a whole: it escapes on a trap-free route, so it carries no trap verdict and its condition does not mention the pending write at the run (the answer `yes` is right). The commit answers (`may-commit`, spans) still do not model that error, and the transaction spans' own checked-run test (`transaction_spans::is_checked_codeunit_run`) does not count a codeunit variable's `.Run`; (5) a TryFunction call traps only as the whole right-hand side of `Ok := …` or the whole condition of `if`/`while`/`until`, through parentheses and `not`. Learn names `OK :=` and `if-then`; `while`/`until` and `B := not TryIt()` are read the same way, which goes slightly beyond Learn and was not checked on BC. Used as an argument, in `exit(…)` or inside `a and …` it is counted as an ordinary call (safe; whether BC traps there is not established, no BC probe was run); (6) not modeled: errors in a try call's arguments (Learn: they are evaluated inside the try function, so caught; the answers report them as escaping, which is safe), and a write inside a try method on premises with `DisableWriteInsideTryFunctions` (a run-time error the answers do not add; Learn does not say whether the try call catches it, so this one is NOT known to be safe: `throws-error` may under-report it). | fixed (wave 2) | M | L7, L14 |
| L30 | `ErrorBehavior::Collect` is modeled only in the callers' question, not in forward `throws-error`. 5 uses in CDO, 13 in Base App. **Wave 2:** a forward `Error` row whose route passes a collecting procedure says it may be collected and raised when that procedure ends (the error is kept: Learn says a collected error is still raised unless the code clears it). **Left:** whether the `Error` is collectible (an `ErrorInfo` with `Collectible = true`) is not read, and `ClearCollectedErrors` is not followed, so the answer never drops a collected error. | partial | S | L23 |

### Declarations and objects

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L2 | A rename misses consumers in declarations: CDO has 101 `CalcFormula`, 210 `TableRelation`, and about 1,750 page field controls. One sample field had 3 code consumers and 12 in declarations. Approach: one index of field names in declarations, used only by field answers. | confirmed | M | — |
| L17 (P3) | Permission failures are not predicted. Object `Permissions` (32 CDO objects), permission sets (252 entries) and `InherentPermissions` are never read. Ship as advisory first. | confirmed | L | L19, L20, L1, L2 |
| L19 (P5) | **Code reads fixed in wave 2 (B5).** Left: (1) a page that shows a FlowField calculates it when displayed; no routine's cone holds that read, so it needs L2's page columns. The same holds for a report or query column bound to a FlowField: the dataset reads (B6) name the dataitem's table, not the FlowField's source table. (2) A record passed to another routine: a caller's `SetAutoCalcFields` and the callee's `Find*`/`Get`/`Next` are not linked (the caller can answer `no`); a caller's filter on a FlowField is placed at the filter (the caller answers `yes`) but the callee's own answer can be `no`. Needs resolved calls with record arguments, which L3 resolve lacks; 0 such `SetAutoCalcFields` cases in CDO. (Object globals and the implicit `Rec` across routines of one object, and a bare `SetAutoCalcFields`, are covered.) (3) Reads stay one dependency routine deep (L1). | partial | S (left) | L2, L1 |
| L20 (P6) | The platform's own dataset operations are missing: report and query dataitem reads, and XmlPort import inserts (see B6). Trigger code is analyzed. CDO: 14 reports, 1 query, 0 XmlPorts. **Fixed in wave 2** (`program::resolve::dataset`, CHANGELOG "engine limits wave 2, B6"): report run/print/save/execute and query open/save read each dataitem's table (report extensions' too); an XmlPort import looks up, inserts (AutoSave) and modifies (AutoUpdate/AutoReplace) each table element's table, an export reads it, `Run` follows `Direction`; the insert may run `OnInsert` (Learn is silent) and raises the table events; a symbol-only or unparsed dataset gives table-less operations, so `unknown`, never `no`. Review fixes: `Report.SaveAs*`/`Execute`/`Print` and `Query.Open`/`SaveAs*` (static and on a variable) now run the object's triggers like `Run` (a query's `OnBeforeOpen`); a dynamic id (`Report.SaveAsPdf(Id, …)`, `Query.SaveAsCsv(Id, …)`) is a dynamic run, so `unknown`, never `no`; `Xmlport.Run(X, false, Import)` follows a literal `Import`. **Left:** the program graph's own edges (`alsem graph`, the call hierarchy, impact) do not carry these operations' trigger and table-event edges, only the model does; a dynamic run target stays the run's own unknown (no table is named); `RunRequestPage` runs the request page's triggers through the catalog hub only; an XmlPort import's insert and modify sit at the call site with its loop and condition context, while the platform does them once per imported record (possibly never), which matters for in-loop detectors and wording, not for yes/no answers; a property set in both arms of an `#if`/`#else` reads as "may" (safe, imprecise); an XmlPort import's error on an existing record (AutoUpdate and AutoReplace both false) is not an error fact; a `Direction` written in one `#if` arm alone reads as written in every build; DataItemTableView/DataItemLink filters are not modelled (every read is "may read"). | fixed (wave 2) | M | — |

### Callers, targets and the program universe

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L10 | One bucket, `not resolved`, covers three cases: (1) literals passed by callers, which could be resolved (one CDO routine passes 15 literal tables into `RecRef.Open`); (2) truly run-time values (Job Queue `Object ID to Run`); (3) engine failure. Step 1 is labels; step 2 resolves case 1. | confirmed | L | L4, L5 |
| L11 | The subscriber, caller and implementer universe is the loaded apps. CDO's own friend and dependent apps (Test, Demo, FR, OnPrem: 21 subscribers on CDO objects) sit on disk next to it and are never loaded. | confirmed | L | L12 |
| L18 (P4) | Background sessions: literal targets are named, but their writes, commits and errors are not reported to the starter, even as "in another session". Job Queue entries written in code are invisible. | partial | M | L10, L4 |
| L31 | **The `unresolved background target` blocker is global.** Two `TaskScheduler.CreateTask` calls in Continia Core block every `callers-may-run-noninteractive` question, so CDO can never get a `no` there (`callers_context.rs:750-781`). | new | S | L18 |
| L24 (P7) | Published SOAP/OData objects count as entries only through `roots.config.json`. CDO publishes 2 codeunits in `WebServicePublish.xml`, which is not read. OData actions on non-API pages are not recognized. | partial | S | L11 |
| L32 | `impact` does not apply the `roots.config.json` overlay, so configured entries differ between impact and prove/explain. | new | S | — |
| L21 (P10) | Every write through a `var` record parameter is "caller-dependent", even for local procedures and internal ones without friends, whose caller set is closed. CDO: 180 local and 66 internal such procedures. | partial | M | L11 |
| L25 (P11) | Three interface cases print the same "open world": implementers followed but others possible; no implementer loaded; an implementer whose body is not loaded. The data to separate them exists. | confirmed | S | L11 |
| L33 | `query touches --all` lists a routine as touching a table when its only use is passing a temporary record to a `var` parameter. | new | S | — |

### Server, tests and process

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L9 | Test-selection reach covers 128 changed routines. One CDO file, `CDOEvents.Codeunit.al` with 155 procedures, crosses it alone; about 7% of recent commits touch more. | confirmed | S | — |
| L12 | Digest doesn't load the test app. The CDO workspace app has 0 tests and the test app has 1,844, so `digest --tests` is always empty on CDO. | confirmed | M | L9 |
| L13 | Memory climbs to about 8 GB before it frees any. The dependency syntax trees (about 1 GB on CDO) are never freed while the child lives, and `--low-mem` can only be set at start. | confirmed | M | L14 |
| L14 | No speed or memory gate on CDO. It must be a branch-gate step on release-fast and tolerate a loaded machine. Cost: 2–4 min. **Fixed in wave 1:** `cdo-gate`'s `speed` phase (`scripts/cdo-speed`), A/B against the merge base on resident CDO calls, CPU and wall time (lost parallelism shows in the warm-up's wall time); timing 178–301 s; round 4's slowdown fails it at x2.1 (CHANGELOG). Linux only. | confirmed | M | — |
| L26 (P12) | Freshness: a rewrite that keeps size and time is served from the old revision. Unsaved editor text is never an input. Checking ctime too costs nothing. | confirmed | S | — |
| L34 | **Baseline input gap, not engine:** 7 test-library apps are missing from `DO-cdo-baseline/Test/.alpackages`, which leaves 4,701 unresolved test calls. Every CDO `alsem tests` answer is incomplete because of it. The baseline is pinned, so changing it is an owner decision. | new | — | — |

## 3. Proposed order

The rule: wrong answers first, then the speed gate (most later items risk speed), then cheap
precision, then the larger capabilities in an order where each builds on the last.

| Wave | Items | Why here | Size |
|---|---|---|---|
| 0 | **B1** (crash), **B3** (isolated events), **B8** (small output fixes) | A crash and a wrong answer, each with a small, known fix | 1–2 days |
| 1 | **L14** speed gate | Needed before B2, B7, L4, L5, L10 and L23, which all change the reach search; round 4 already lost 45% once | 1–3 days |
| 2 | **B4/L16** CommitBehavior, **B7/L23** traps (+ L30), **B5/L19** FlowField reads, **B6/L20** dataset ops, **B2** `#if` arms as alternatives | The remaining wrong answers | 1–2 weeks |
| 3 | **L4** platform option types, **L29** Text/Code, **L8**, **L9**, **L25**, **L24**, **L32**, **L33**, **L26**, **L31** | Small precision and wording wins, each S | about 1 week |
| 4 | **L15** isolation as locks, then **L1** deep reads/locks/validations, then **L27**, then **L3** caller-held locks | One chain; L3 is the big one | 2–3 weeks |
| 5 | **L2** declaration index, then **L17** permissions (advisory) | L17 needs L2, L19 and L20 | 1–2 weeks |
| 6 | **L11** load sibling/dependent apps, then **L21**, **L12**, **L18** | They need a wider universe | 2 weeks |
| 7 | **L13** memory eviction | Independent; after L14 so it can be measured | 1–3 days |
| 8 | **L5** value domains (globals, data, arithmetic), **L10** step 2, **L6**, **L28**, **L22** later steps | Largest precision work, highest speed risk | open |

L34 (the missing test libraries) is an owner decision on the pinned baseline, separate from these
waves.

## 4. Verification gaps (no limit suspected)

astra named these cases as fixture-tested only. Add one real CDO call for each to the next
sample:
- touches with `opFamily: "write"`;
- an assumption whose `subscribersConsidered` is not empty;
- a digest in human format for an ambiguous name (the `differs in:` lines);
- a test whose code path was not kept.
