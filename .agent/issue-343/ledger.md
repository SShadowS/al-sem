# Issue #343 ledger

## Claim

- Run `20261025-235440-f107d0`, attempt 1.
- Worktree `/work/al-sem-issue-343-a1`, branch `issue/343-agent-discovery-l5-d39-callerpe-a1`, base `506149da`.
- Body hash: `5dc4ca75762ca1c187e90cdb60c1e7d02818b038e07893cd83a9c9bf31df67eb`.
- Step owner: design author (steps 2 to 4). No commits.
- Revision 2 (after spec round 1: sol S1-S17, Opus OP1-OP7). Gate base stays `506149da`.

## Classification

**Bounded.** One detector (d39) gains one more skip; no L4 summary, no id, no
output shape changes. The only engine-side change is one in-memory-only field on
`PCFNNode` (`path_hint: Option<PathHint>`, `#[serde(skip)]`, outside `PartialEq`), so
no golden that serializes the tree can move; the `Debug`-digest L2 snapshot does
(see probes). `## Design` below is the spec.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| How `callerPersists` works today | Source order only: any own `Modify`/`Insert`/`Rename` on the source variable whose anchor is after the call's anchor. No helper, no paths (an exit before it, or a persist in another arm, still counts). Unchanged by this issue. | `src/engine/l5/detectors/d39.rs` `persisted_after` (`PERSIST_OPS`, `before_anchor`) |
| Is there an L4 "helper persists" fact? | Only a MAY fact: `RecordRoleSummary.persists_current_record = Yes` when the routine has any persist op on the param on any path (`summary_runner.rs:670`). No MUST ("on every path") fact. There is no `persists_param`. | `src/engine/l4/summary.rs:259`, `summary_runner.rs:626-718` |
| Does the L4 walker compose a helper's persist into the caller's dirt? | Only from `Pristine`: `persists == Yes && dirty == Pristine -> Persisted`. A `DirtyV` record stays dirty across a persisting helper, so the caller's own `dirty_at_exit` cannot answer this. | `src/engine/l4/cfg_walker.rs:1402` |
| Receiver persist (`Rec.InsertJob()` on a table procedure) | No L4 receiver role on master (#303 added one, not merged). A table procedure's own `Rec` ops are visible in its body, and `is_table_receiver_rec` marks the receiver row. | `mod.rs::is_table_receiver_rec`; `grep receiver_binding src` = 0 hits |
| Path tools that exist | `branch_exclusive`, `load_point_after`, `control_flow::{terminates, branch_termination}`, `pending_writes`. None answers "every path from leaf X to an exit passes a leaf Y". The L4 `cfg_walker` is the only forward path walk, fixed to `PerParamState`. | `src/engine/l5/detectors/mod.rs`, `src/engine/l5/pending_writes.rs`, `src/program/body/control_flow.rs` |
| Does `statement_tree` carry what a path walk needs? | Leaves (op/call/error), `if`/`case`/loops, `exit`, `break`/`continue`, `try`. It does NOT carry: which `if` arm runs on which return value of a call; an `exit`'s literal value; that an `other` node is an `asserterror`; that a leaf comes from one alternative of a ternary (`harvest` flattens both). A `for` header's bounds are its `condition_leaves`. | `src/program/body/ir_walk.rs::build_statement`, `harvest` |
| The CDO shape | `if not InitRecurringDailyJob(.., JobQueueEntry) then exit; .. InsertJob(JobQueueEntry, ..)`. The guard `exit` runs right after the dirtying call with no persist, so a strict rule KEEPS all 3 CDO rows. They go only when the walk knows the `then exit` arm runs when the callee returned `false`, and that the callee is clean on every `exit(false)`. | `CDOSetupData.Codeunit.al:368-454` (CDO baseline) |
| `InsertJob` persists on every path? | Yes: arm 1 `Void := Insert(true); exit;`, arm 2 `if Insert(true) then Restart()`. | same file, 441-454 |
| The `ProcessInNAV` shape (#303's wrong removal) | `exit(ProcessActiveQuote(CRMQuote, SalesHeader))`: the call is in the `exit`'s own value, so the walk sees an `exit` with the dirt unsaved. | `ws-baseapp/src/Integration/D365Sales/CRMQuotetoSalesQuote.Codeunit.al:103-116` |
| Base numbers (`506149da`, release-fast) | Base App d39 363 (`callerPersists` 165); CDO primary 3, CDO all 3. | `runs-local/base.{ba,cdo-primary,cdo-all}.json` |
| Does the L2 snapshot see in-memory fields? | Yes, on purpose: `ir_l2_snapshot` digests the `Debug` form. The new field moves every routine line: 988 changed, 0 added, 0 removed. The snapshot stores one hash per routine, so it cannot show WHAT changed; that evidence is the code diff (`path_hint` is set only in `IrCfn::build_statement` for `if` / `exit` / `asserterror` and in `harvest` for ternary alternatives; every other literal sets `None`). No other golden moves: `check-goldens` 1310 / 1311, the 1 is this snapshot. | `tests/l2_ir/ir_l2_snapshot.rs:9-16`; `scratchpad/343/goldens-r2.log` |
| Who resolves a call for the proof? | `ctx.resolved_call_edge_by_callsite` holds only edges with a target (first per site; an interface dispatch reads only the smallest implementer). An unresolved call has an uncertainty edge on its call site (`ctx.own_uncertainty_edges`); a builtin has neither. | `detector_context.rs:2093-2115`, `:1025-1036`; `taxonomy.rs` `Resolution`, `DispatchKind` |

## Revision 2: what changed for each round-1 finding

### R2-S1 `for` header
`walk_paths` runs a `for` / `foreach` header's leaves ONCE before the loop; only a
`while` condition runs on every pass. Fixture `ForHeader` (`for I := 1 to
SaveAndCount(Job) do Dirt(Job);`) reports; revision 1 removed it. Break B9 (replay the
header) -> `ForHeader` vanishes, RED. Unit test `loops_run_zero_or_more_times` pins it too.

### R2-S2-OP1 reload, Init or copy before the persist
Three guards, all fail closed:
- the skip now also needs `!reset_after_call()` (the shared d39/d40 load-point rule);
- in the caller walk and in every helper walk, a leaf that reloads, inits or copies
  into the record while its dirt is unsaved (`record_flow_role` in
  `loadsFromDb`/`initialises`/`copiesInto`, or a `var` call whose role
  `puts_in_loaded_state()` or has any of those Unknown) FAILS the proof (`VarEffect::Resets`);
- a bare `Var := Other` assignment (after the call in the caller, anywhere in a helper) fails it.
Fixture `HelperReinits` (helper `Init(); ..; Insert()`) and `CallerReloads`
(`Dirt; Get; InsertJob`) report; revision 1 removed both. Break B12 (own reload ops
no longer reset) stayed GREEN: the `reset_after_call` gate catches both rows on its
own. Breaking the gate alone also stays GREEN (the walk catches both). Breaking both ->
both rows return, RED. The two guards are redundant on these shapes; recorded as such,
each alone is sufficient. S2's second half (`CleanReturn` treats `Validate; reload;
exit(false)` as clean) is correct for the question it answers: on that exit the record
holds no dirt from this run. Dirt carried in from before the call is R2-S3's job.

### R2-S3 earlier loop pass (sol and Opus disagreed; settled by fixture)
Fixture `LoopGuard`:
`repeat if not MaybeDirty(Job) then exit; until Job."No." <> ''; InsertJob(Job, false);`.
Revision 1 REMOVED it (`scratchpad/343` run of `alsem-proto` r1 on the fixture), so sol
is right: pass 1 returns true and dirties, pass 2 returns false and exits, and the
pass-1 write is lost. Opus's argument misses that the exit is inside the loop, not at
its normal exit. Fix: a new state `CARRIED`. When the dirtying call runs again while
dirt is unsaved, that dirt becomes `CARRIED`; the clean-return arm settles only this
run's `PENDING`, never `CARRIED`; only a persist settles it. If the dirtying callee may
reload the record, a second run with unsaved dirt fails the proof. Break B8 (drop
`CARRIED`) -> `LoopGuard` vanishes, RED.

### R2-S4-OP2 clean-return proof ignored receiver calls, unresolved calls, publishers
`CleanReturn::leaf` now fails closed on any call that takes the parameter (whole-record
argument or receiver): DIRTY unless it is a builtin record method (no target, no
uncertainty edge, the record only as receiver), a resolved by-value copy, or a resolved
direct/method `var` call to a non-publisher whose role never exits dirty. A member call
on the parameter to a table procedure is DIRTY (no receiver role on master). A `var`
event publisher is DIRTY (subscribers are not read). Fixtures `ReceiverInClean`
(`Job.DirtyMe()` before `exit(false)`) and `PublisherInClean` (`OnBeforeInitJob(Job,
IsHandled); if IsHandled then exit(false)`) report; revision 1 removed both. Breaks B13
(receiver call no longer dirties) and B14 (publisher no longer dirties) -> each row
vanishes, RED.

### R2-S5-OP3 `asserterror`
The `asserterror` statement node carries `PathHint::AssertError`; `walk_paths` treats it
as opaque (the proof fails). Fixture `AssertErrorShape` (`Dirt(Job); asserterror
Error('expected');`) reports; revision 1 removed it. Break B10 -> vanishes, RED. Unit
test pins it too. The d39 module doc names it.

### R2-S6 `parse_incomplete`
Every proof (caller walk, helper walk, clean return) reads the body through
`proof_tree`, which needs `body_available && !parse_incomplete`. No fixture: a fixture
with a parse error would also move other detectors' rows and the L2 corpus. Pinned by
code review only; stated.

### R2-S7 ternary alternatives
`harvest` marks every leaf from a ternary's `then` / `else` value
`PathHint::MayRun`; `walk_paths` joins "ran" with "did not run" for such a leaf.
Fixture `TernaryShape` (`Dirt(Job); I := Flag ? SaveAndCount(Job) : 0;`) reports;
revision 1 removed it. Break B11 -> vanishes, RED. AL `and` / `or` evaluate both
operands, so they need no hint.

### R2-S8 walker coverage
Unit tests `walk_paths_tests` in `src/engine/l5/detectors/mod.rs` (6 tests, hand-built
trees, a toy question): sequence and exit, nested `if` joins, `case` with/without
`else`, `while` zero passes / `repeat` one pass / `break` / dirt saved after the loop /
`for` header once, `error` ends a path / `try` / stray `break` / `asserterror` opaque,
`MayRun` leaf and both `CondCall` polarities. Breaks, each RED then green
(`scratchpad/343/u-*.log`): U1 `case` never falls through; U2 `while` exits only after a
body pass; U3 `break` states dropped; U4 `error` does not end its path; U5 `try`
transparent; U6 `if` drops the `then` arm. CLI depth boundary: `ThreeLevels` (persist 3
helpers down, no row) and `FourLevels` (4 down, row). B6 (`MAX_HELPER_DEPTH = 1`) ->
`NestedHelper` and `ThreeLevels` return, RED; B6b (`= 4`) -> `FourLevels` vanishes, RED.

### R2-S9 gate script
`scratchpad/343/gate.sh` now exits 1 unless both analyze runs exited 0 with a findings
array, the non-d39 multisets are identical, no d39 is added, and the removed d39 set
equals `scratchpad/343/expected-removed.<run>.fps` exactly. Self-test: with a wrong
expected set it printed the diff and exited 1; with the right set it exited 0.

### R2-S10 memo key
No memo is added (no timing claim either way: see R2-OP7). If one is added, the
`helper_persists` key is (routine pointer, variable, remaining depth), and
`clean_return` is cached separately per (routine pointer, parameter index). Stated in
the `helper_persists` doc.

### R2-S11-OP5 kept-row sample
Revision 1 claimed 4 kept rows sampled and called `IsHandled` "the most common" kept
shape. Correct statement: 2 kept rows were read (`TransferWarehouseMgt.TransLine2ReceiptLine`
:230, `WhseWorksheetCreate.FromWhseShptLine` :35), both kept because the persisting
helper exits early on `IsHandled`. No census of the other 52 was done, so no claim is
made about the distribution. This does not gate admission: the change only removes
findings, and every removal was read.

### R2-OP4 polarity break and helper-side `IsHandled`
B7 (swap the arms' values) -> `CdoShape` returns and `GuardOnDirtyValue` vanishes, RED.
Fixture `IsHandledHelper` (helper `OnBeforeInsertJob(Job, IsHandled); if IsHandled then
exit; Job.Insert();`) reports.

### R2-OP6 field size and docs
The two revision-1 fields (`cond_call: Option<(String, bool)>`, `exit_value`) are one
`path_hint: Option<PathHint>` (a 1-byte-payload enum: `CondCall { negated }`,
`Exit(bool)`, `AssertError`, `MayRun`). The call of a `CondCall` is the `if`'s last
top-level condition leaf (`cond_call` sets the hint only when that holds). Revision 3:
`cond_call_site` returns a borrowed `(&String, bool)`, so no string is cloned or
stored. The `PCFNNode` / `PartialEq` docs and the snapshot test's field list name it.

### R2-OP7 timing
Every wall time in this ledger comes from a single run, under varying load. They show
neither a cost nor a saving for the helper walks, and no claim is made about either.
F4 below stands.

## Revision 3 (after spec round 2: Opus APPROVE, sol re-raised S2, S4, S8, OP6, OP7, new S18; Opus OPR2-1, OPR2-2)

Supersedes R2-S2-OP1 and R2-S4-OP2 above where they disagree (they stay as history).
Base App fixture lines refer to `tests/fixtures/issue-343/ws-helper/src/H.al`.

### R3-S2-OP1 no L4-role trust for an intervening call
Revision 2 fell back to the callee's L4 role for a `var` call between the dirtying
call and the persist; a role does not see a whole-record assignment (`Job := Other`)
or a receiver call, so `Replace(Job, Other)` read as harmless (sol). Revision 3:
`call_effect` gives such a call `Persists` only if `helper_persists` proves it, `None`
only if `helper_preserves` proves from the callee's own body (path-insensitive, every
op and every call site) that it does not reload, init, copy into or reassign the
record, and that every call it makes on it is itself proven, to depth 3; otherwise
`Unknown`, which fails the proof. An event publisher never preserves (F5). The
dirtying callee's "may reset on a re-run" flag is the same body proof
(`!helper_preserves`), not its role. Fixture `IndirectReplace` reports (revision 2
removed it); `PreservingHelper` (a read-only `Touch(Job)` between) stays skipped.
Breaks: B16 (`helper_preserves` failure ignored) -> `IndirectReplace` and
`PublisherBeforePersist` vanish, RED; B16b (`helper_preserves` always false) ->
`PreservingHelper` appears, RED.

### R3-S4-OP2 clean-return proof trusts only bodies
`call_keeps_clean` replaces the role check: a resolved direct / method `var` call
keeps the parameter clean only if `helper_clean` proves from its body that it makes
no `Validate` / copy into it and every call it makes on it keeps it clean (to depth
3); a receiver call on it, an unresolved or interface call, and an event publisher
make it dirty. Fixture `WrappedReceiverInClean` (`Wrapper(Job)` = `Job.DirtyMe()` before
`exit(false)`) reports (revision 2 removed it); `GuardWithCleanHelper` (a read-only
helper before `exit(false)`) stays skipped. Breaks: B17 (trust every resolved `var`
call) -> `WrappedReceiverInClean` and `PublisherInClean` vanish, RED; B17b
(`helper_clean` always false) -> `GuardWithCleanHelper` appears, RED.

### R3-S8 `continue`
Unit test `continue_joins_the_loop_condition`: `d; repeat if c then continue; p; until x`
fails (the `continue` path reaches `until` with dirt), and the same loop with `p` after
it passes. Break U7 (`continue` states dropped) -> RED (`scratchpad/343/u-continue.log`).
7 walker tests in all.

### R3-S18 `LoopGuard` really runs pass 1 dirty, pass 2 clean
`MaybeDirty` now returns false, untouched, when `Name` is already `'x'`, and otherwise
validates `Name` to `'x'` and returns true. `LoopGuard` sets `Job.Name := ''` first.
Pass 1: `''` -> validated to `'x'`, true; `until Job."No." <> ''` is false (`"No."` stays
blank), so pass 2 runs: `'x'` -> false -> `exit` with pass 1's write unsaved. Same line
count, so every fixture line number is unchanged. B8 (no `CARRIED`) re-run on revision
3 -> `LoopGuard` vanishes, RED.

### R3-OP8 (OPR2-1) `Var := Other` on a loop back edge
The re-run branch of `PersistAfter::leaf` now fails on `callee_resets ||
record_reassigned(routine, var, None)`. Fixture `ReassignInLoop` (`repeat Job :=
Template; Dirt(Job); until ..; InsertJob(Job, false)`) reports (revision 2 removed it).
Break B15 (drop the `record_reassigned` term) -> `ReassignInLoop` vanishes, RED. It fires
only when the dirtying call runs twice, so it cannot move a removal outside a loop.

### R3-OP9 (OPR2-2) gate on the final code
The release-fast binary was built from the frozen revision-3 sources (sha256 of
`d39.rs`, `mod.rs`, `ir_walk.rs`, `features.rs` recorded before the build and checked
after it and after the golden run: unchanged). All three gates ran on that binary, one at
a time. Results under Measurements.
Evidence (scratchpad `343/`, worktree `.agent/issue-343/runs-local/`):
- `r3-src.sha` (01:01:06, 4 files) and `r3-src-full.sha` (all 9 changed `src/` files);
  `sha256sum -c` passes for both on the current tree.
- Every changed `src/` file's mtime is before the build; the last edit is `mod.rs` at
  01:00:12. `build-proto-r3.log` says "Compiling al-sem ... Finished release-fast in 1m 17s",
  ending at 01:02:32, the mtime of `target/release-fast/alsem`. `find src -newer
  target/release-fast/alsem` is empty.
- The gate outputs `proto.cdo-primary.json` (01:02:53), `proto.cdo-all.json` (01:03:14) and
  `proto.ba.json` (01:04:21) come after the binary, with `removed-d39.*.fps` and `fp.*`
  beside them; `gate.sh` exits 1 on any difference from `expected-removed.*.fps`.

### R3-OP10 F5 decided: publishers fail closed
Coordinator decision (sol's option, the safe direction): a `var` event publisher
reached while dirt is unsaved makes the persist proof `Unknown`, and in the clean-return
proof it dirties. Fixture `PublisherBeforePersist` (`OnBeforeInsertJob(Job, H);
Job.Insert();` in the helper) reports; break B18 (publishers trusted in
`helper_preserves`) -> it vanishes, RED. The expected Base App removed set in
`scratchpad/343/expected-removed.ba.fps` was changed deliberately from the 5
fingerprints to empty for this decision, before the revision-3 runs.

## Prototype (revision 3)

`runs-local/proto-r3.diff` (tracked files; uncommitted in the worktree; base
`506149da`). New untracked files: `tests/cli/cli_d39_helper_persists.rs`,
`tests/fixtures/issue-343/ws-helper/src/{H.al,HExt.al}`, `app.json`. Earlier:
`runs-local/proto.diff` (r1), `proto-r2.diff` (r2); outputs `runs-local/proto-r1.*`,
`proto-r2.*`.
- **L2** (`features.rs`, `ir_walk.rs`): `PCFNNode.path_hint`; set on `if` (whole
  condition `C(..)` / `not C(..)`), `exit(<bool literal>)`, `asserterror`, ternary
  alternatives. The 9 other struct literals set `None`.
- **Shared walker** (`detectors/mod.rs`): `walk_paths(tree, init, &mut dyn PathQuestion)`
  and its unit tests (see R2-S1, S5, S7, S8 for the semantics).
- **d39** (`d39.rs`): skip `helperPersists` after `callerPersists`, gated by
  `!reset_after_call()`; questions `persisted_on_every_path_after` (BEFORE / PENDING /
  CARRIED / DONE), `helper_persists`, `helper_preserves`, `helper_clean`,
  `clean_return`; shared `var_effect` = `op_effect` + `call_effect` (None / Persists /
  Resets / Unknown), `call_keeps_clean`, `certain_callee` (resolved direct or method
  only), `is_builtin_site`, `is_event_publisher`, `proof_tree`, `record_reassigned`.
  No L4 role is read by the new skip any more.

## Measurements (revision 3, release-fast, one analyze at a time; `heavy-ok` checked before the runs)

Binaries: `scratchpad/343/alsem-base` (`506149da`), `scratchpad/343/alsem-proto`
(revision 3: base + `proto-r3.diff`, built from the frozen sources, R3-OP9). Scripts:
`scratchpad/343/measure.sh`, `gate.sh` (strict, R2-S9). Expected sets:
`scratchpad/343/expected-removed.{ba,cdo-primary,cdo-all}.fps` (Base App changed to
empty for F5, R3-OP10). Outputs: `runs-local/{base,proto}.{ba,cdo-primary,cdo-all}.json`,
`removed-d39.*.tsv`; revision 2's in `runs-local/proto-r2.*`.

| run | total base -> proto | non-d39 diff | d39 base | added | removed | `helperPersists` | gate |
|---|---|---|---|---|---|---|---|
| Base App (`--single-app --scope all`) | 71957 -> 71957 | **0** | 363 | **0** | **0** | 0 | PASS |
| CDO primary (default mode) | 2217 -> 2214 | **0** | 3 | **0** | **3** | 3 | PASS |
| CDO all (default mode) | 30006 -> 30003 | **0** | 3 | **0** | **3** | 3 | PASS |

Revision 3 removes only the 3 CDO false positives (acceptance holds). The 5 Base App
removals of revisions 1-2 all come back, as F5 predicted: each has a `var` publisher
between the dirtying call and the Insert. `ProcessInNAV` (`0c806c7019a7dd60`) still
fires. `check-goldens`: 1310 / 1311, the 1 is `l2_features.snapshot` (988 changed, 0
added, 0 removed). Dev clippy (`--lib --tests -D warnings`): clean.

Fixture `ws-helper`, 27 shapes: base reports all 27; revision 1 reported 8 of the first
21; revision 2 reported 18 of 27 (it wrongly skipped `ReassignInLoop`,
`IndirectReplace`, `WrappedReceiverInClean`, `PublisherBeforePersist`); revision 3
reports 20. Skipped by revision 3: `CdoShape`, `PlainHelper`, `ReceiverCall`,
`NestedHelper`, `ThreeLevels`, `PreservingHelper`, `GuardWithCleanHelper`.

## Triage (every move; source read for each)

Base App: **no removal** in revision 3. The 5 rows revisions 1-2 removed (read in
revision 1, all with a correct persist path) stay reported because each passes an
event publisher taking the line by `var` before the persist (signatures read):
`OnBeforeCreateShptLineFromTransLine`, `OnFromPurchLine2ShptLineOnBeforeCreateShptLine`,
`OnBeforeCreateShptLineFromSalesLine`, `OnFromServiceLine2ShptLineOnBeforeCreateShptLine`
in the callers, and `OnBeforeWhseShptLineInsert` in `CreateShipmentLine` (4 warehouse
rows); `OnBeforePostPlanningLine` in `PostPlanningLine`
(`JobPostLine.InsertPlLineFromLedgEntry`). Fingerprints `7032a70935d6a0d7`,
`f18a3bf03339cc51`, `cb331a2b79b2d625`, `3091d957223b1277`, `22350e653693453e`.

CDO (primary = all, same 3): **3 correct.** `CreateEmailQueueJobQueueEntry` :376
(`5fcb917069429b4f`), `CreateStatementJobQueueEntry` :386 (`065cffc76dd0a963`),
`CreateInvResponseJobQueueEntry` :404 (`1c237aab732f9816`). `if not
InitRecurringDailyJob(..) then exit;` runs only on `false`; both `exit(false)` come
before any `Validate`, and no resolved routine call takes the record before them (only
builtin record methods `WritePermission`, `SetRange`, `IsEmpty`); on the other path
`InsertJob` inserts on both its paths, with no call on the record before the Insert
(`CSCEnvInfo.IsSaaSInfrastructure()` does not take it). All 3 were false positives.

Kept rows read for the false-negative list: 2 (see R2-S11-OP5).

30% rule: no new finding, 3 removals, 0 wrong.

## Design

**Rule.** d39 skips a candidate (`helperPersists`, after `callerPersists`, so no
existing skip changes) when no load point follows the call (`reset_after_call`) and, on
every path in the caller from the dirtying call to an exit, the source record is
persisted (own Modify / Insert / Rename, or a later call handing it `var`, or as the
receiver of a Table / TableExtension procedure, to a helper that itself persists it on
every path) with nothing reloading, initing, copying into or reassigning it first. A
path needs no persist for THIS run's dirt when it runs only on the `if C(..)` /
`if not C(..)` arm where the callee returned the Boolean literal on which it provably
leaves the record clean; dirt from an earlier run (a loop) always needs a persist.
Every fact about a called routine comes from its own body, never from an L4 role:
a `var` call between the call and the persist counts only if its body provably
persists the record on every path or provably leaves it be; a call before a clean
`exit` counts only if its body provably makes no dirt.
Fails closed (keeps the finding) on: an `exit` before the persist, `try`,
`asserterror`, a stray `break`, an incomplete parse, a tree deeper than 256, an
unresolved / interface / builtin-with-argument call taking the record while its dirt
is unsaved, a `var` event publisher (F5), a table procedure on it that does not
provably persist, a `Var := Other` that may run on a loop back edge, more than 3
helper levels.

**Where it lives.**
- `src/program/body/features.rs`, `ir_walk.rs`: `PCFNNode.path_hint` / `PathHint`
  (in-memory only). The lowerer (`crates/al-syntax`) is not touched.
- `src/engine/l5/detectors/mod.rs`: `PathStates`, `PathQuestion`, `walk_paths`, unit tests.
- `src/engine/l5/detectors/d39.rs`: the questions, the skip and its stat
  (`helperPersists`), module doc paragraph.
- `tests/l2_ir/ir_l2_snapshot.rs` doc names the field; `l2_features.snapshot` regenerated.
- CHANGELOG `Fixed`: d39 no longer reports a record that a later helper persists on every path.

**Tests.**
1. CLI `tests/cli/cli_d39_helper_persists.rs` (real `run_analyze_with_exit`, default
   mode, primary scope) over `tests/fixtures/issue-343/ws-helper`, exact `(routine,
   line)` rows. 27 hand-stated shapes; expected 20 rows: `ExitBeforeHelper L90,
   ExitWithCall L101, HelperOnOnePath L117, GuardOnDirtyValue L133, GuardDirtyOnFalse
   L142, PersistInOtherArm L160, HelperReinits L179, CallerReloads L187, LoopGuard L206,
   ForHeader L224, AssertErrorShape L231, TernaryShape L240, IsHandledHelper L263,
   ReceiverInClean L281, PublisherInClean L307, FourLevels L344, ReassignInLoop L356,
   IndirectReplace L370, WrappedReceiverInClean L394, PublisherBeforePersist L411`.
   Red on base (27 rows).
2. CLI breaks (each Edit, run, revert; logs `scratchpad/343/r2-*.log`, revision-1 logs
   `brk-b*.log`), all RED unless noted. B1, B6-B14 recorded on revision 2, B8 and
   B15-B18 on revision 3:

   | break | change | effect |
   |---|---|---|
   | B1 | no `CondCall` hint | `CdoShape` returns (re-proved on r2) |
   | B2 | an `exit` never reports | 4 exit shapes vanish (r1) |
   | B3 | no receiver-call persist | `ReceiverCall` returns (r1) |
   | B4 | `helper_persists` always true | `HelperOnOnePath` vanishes (r1) |
   | B5 | no exit literal | `CdoShape` returns (r1) |
   | B6 / B6b | `MAX_HELPER_DEPTH` 1 / 4 | `NestedHelper`, `ThreeLevels` return / `FourLevels` vanishes |
   | B7 | arm polarity swapped | `CdoShape` returns, `GuardOnDirtyValue` vanishes |
   | B8 | no `CARRIED` | `LoopGuard` vanishes |
   | B9 | `for` header replayed | `ForHeader` vanishes |
   | B10 | `asserterror` hint ignored | `AssertErrorShape` vanishes |
   | B11 | `MayRun` hint ignored | `TernaryShape` vanishes |
   | B12 | own reload ops do not reset | GREEN (redundant with the gate); with the gate also broken: `HelperReinits`, `CallerReloads` vanish |
   | B13 | receiver call does not dirty in clean return | `ReceiverInClean` vanishes |
   | B14 | publisher does not dirty in clean return | `PublisherInClean` vanishes (revision 2; in revision 3 B17 covers it) |
   | B8 (r3) | no `CARRIED`, on the revised `LoopGuard` | `LoopGuard` vanishes |
   | B15 | re-run ignores `Var := Other` | `ReassignInLoop` vanishes |
   | B16 | `helper_preserves` failure ignored | `IndirectReplace`, `PublisherBeforePersist` vanish |
   | B16b | `helper_preserves` always false | `PreservingHelper` appears |
   | B17 | clean return trusts every resolved `var` call | `WrappedReceiverInClean`, `PublisherInClean` vanish |
   | B17b | `helper_clean` always false | `GuardWithCleanHelper` appears |
   | B18 | publishers trusted in `helper_preserves` | `PublisherBeforePersist` vanishes |

   The implementer re-runs B2-B5 and B12-B14 on the final code (recorded on revisions
   1 and 2).
3. Unit tests `walk_paths_tests` (7) with breaks U1-U7 (R2-S8, R3-S8).
4. `l2_features.snapshot` regenerated: exactly 988 changed, 0 added, 0 removed.

**Expected moves (exact fingerprint gate, `gate.sh` must exit 0).** Non-d39 sets
identical; d39 added 0; d39 removed exactly:
- Base App: none (F5; was the 5 fingerprints listed under Triage until revision 2).
- CDO primary and CDO all: `5fcb917069429b4f`, `065cffc76dd0a963`, `1c237aab732f9816`.
- Goldens: only `l2_features.snapshot`.

**False-positive shapes (a wrong removal could still come from):**
- Name-only matching when a side has no record-variable id.
- A builtin taking the record only as receiver (`Job.TableCaption()`) is trusted to
  leave it be; a builtin with the record as an argument is not.
- A persist whose return says it failed (`if not Insert() then ..`) counts: the write
  was attempted or refused, not silently discarded.

**False-negative shapes (kept by design):** an `IsHandled` exit or any `var`
publisher before a helper's persist (all 5 Base App candidates of revisions 1-2); a
helper persisting on some paths only, through a RecordRef, `Codeunit.Run`, or more than
3 levels down; a helper whose body cannot be proven (no body, interface, incomplete
parse); a compound guard or a non-literal return; any unresolved, `try` or
`asserterror` code on the way.

**Fallbacks (stated in advance).**
- F1: if the implementation's d39 sets differ from the prototype's, stop and find the
  cause; no golden regen to make them match.
- F2: if a reviewer finds one of the 3 removals wrong, fix the rule at its cause and
  re-triage; never just count it.
- F3: if the `PCFNNode` hint is rejected, the guard carve-out has no other source of
  the `if` polarity; the CDO acceptance then cannot be met by a sound rule; escalate.
- F4: if helper walks show up in repeated timing, memoize with the R2-S10 keys.
- F5 (decided, R3-OP10): a `var` publisher fails the proof. Measured: Base App
  removals 5 -> 0, CDO unchanged at 3.

## Implementation (step 5 onward; spec converged in round 4, 28 / 28 accepted)

- Rebased onto `origin/master` `258da610` (#340, #353); stash popped cleanly; the
  restored diff equalled `runs-local/proto-r3.diff` byte for byte.
- Plan: `docs/superpowers/plans/2026-10-26-issue-343-helper-persists.md` (1 task).
- Cleanups: `LoopGuard` preparation split onto its own lines (rows from L206 on +2);
  `helper_clean` comment on the copy asymmetry with `CleanReturn`; module-doc F5 line.
- Two lib tests the prototype never ran failed in `ci-steps task-affected` and were
  fixed at their tripwires: `serde_skip_list_is_complete` (the INTERNAL-ONLY list now
  names `PCFNNode.path_hint`, count 11) and the `FIXTURE_ROOTS` count for
  `tests/fixtures` (46 -> 47, the new `issue-343/ws-helper`), read by
  `exec_relation::relation_equals_the_legacy_base_per_field` and
  `capability_cone::derived_only_matches_raw_fold_on_fixtures`.
- Breaks re-run on the final code (logs `scratchpad/343/r2-fin-*.log`), each reverted:

  | break | effect on the final code |
  |---|---|
  | B2 an `exit` never reports | RED: ExitBeforeHelper, ExitWithCall, GuardDirtyOnFalse, GuardOnDirtyValue, LoopGuard, ReceiverInClean, PublisherInClean, WrappedReceiverInClean vanish |
  | B3 no receiver-call persist | RED: ReceiverCall appears |
  | B4 `helper_persists` always true | RED: HelperOnOnePath, IsHandledHelper, FourLevels, PublisherBeforePersist vanish |
  | B5 no exit literal | RED: CdoShape, GuardWithCleanHelper appear |
  | B12 own reload ops do not reset | GREEN (the `reset_after_call` gate covers it, as recorded) |
  | B12 + gate broken | RED: HelperReinits, CallerReloads vanish |
  | B13 receiver call keeps clean (`call_keeps_clean`) | RED: ReceiverInClean, WrappedReceiverInClean vanish |
  | B14 publisher clean in `helper_clean` | RED: PublisherInClean vanishes |

  Final green: `r2-fin-green.log` (20 rows).
- Golden: only `tests/ir-l2-goldens/l2_features.snapshot`, triaged hash-only before the
  regen (receipt `.agent/golden-triage.md`): 988 keys / 1013 lines changed, key column
  identical. `scripts/check-goldens` after regen: 1322 / 1322. `scripts/ci-steps
  task-affected`: 4504 / 4504.

## Final panel round 1 fixes (sol F1-F6, Opus OF1-OF2, code-review CR1-CR9)

The proofs moved to `src/engine/l5/detectors/d39_persist.rs` (`Proofs`): one struct per
d39 run holding the context, a per-routine index (op ids, call-site ids, opaque flag)
and memo tables keyed (routine pointer, variable, depth) / (routine pointer, parameter
index). Fixture `ws-helper` now has 36 shapes; base (a `506149da` build, no #343)
reports all 36; the revision-3 binary skipped 8 of the 9 new ones wrongly
(`AliasSave`, `AssertInHelper`, `AssignDirectClean`, `AssignFromCall`,
`AssignInHelperClean`, `GlobalTouch`, `OnAfterModify`, `TryHelperShape`; the ninth,
`EarlyExitHelper`, it already reported); the final code reports 29 rows.

### FR1-CR1 `[TryFunction]` helper
`proof_body` refuses a `[TryFunction]` routine. Fixture `TryHelperShape` (A28). Break
B19 (TryFunction readable) -> `TryHelperShape` vanishes, RED.

### FR1-CR2 bare table-procedure call on the implicit `Rec`
`bare_on_rec`: while the record is `rec`, a bare call resolved to a table procedure is
treated as a call on the receiver (`call_effect`: only a proven persist with no alias
counts, else Unknown; `call_keeps_clean`: dirty). Fixture `OnAfterModify` in `HExt.al`
(A36: `M.DirtPublic(Rec); ReloadSelf(); M.SaveRec(Rec);`). Break B20 -> vanishes, RED.

### FR1-CR3 any whole-record assignment
`record_reassigned` counts every assignment whose `lhs_name` is the variable, whatever the
right side (call result, member, parenthesized). Fail-closed side effect: a field
assignment whose member name equals the variable's name counts too. Fixture
`AssignFromCall` (A29, `Job := GetTemplate()`). Break B21 (require `rhs_identifier`) ->
vanishes, RED.

### FR1-CR4 object-global source record
`binding.source_kind == "global"`: in the caller walk, every non-builtin call while the
dirt is unsaved is Unknown (even one not passing the record; even a persisting helper).
Fixture `GlobalTouch` (A30, `Dirt(GJob); ResetGlobal(); InsertJob(GJob, ..)`). Break B22
-> vanishes, RED.

### FR1-F1 whole-record assignment in the clean-return proof
`helper_clean` and `clean_return` both fail on `record_reassigned` of the parameter.
Fixtures `AssignInHelperClean` (A31, through `CopyIn`) and `AssignDirectClean` (A32,
direct). Breaks B23 (`helper_clean` ignores it) and B24 (`clean_return` ignores it) ->
each row vanishes, RED.

### FR1-F2 receiver also passed as an argument
A call on the record whose arguments also take it (an alias) is Unknown in
`call_effect`. Fixture `AliasSave` (A35, `Job.SaveAlias(Job, Template)`, `Alias :=
Template; Insert();`). Break B25 -> vanishes, RED.

### FR1-F3 opaque nodes in helper bodies
`proof_body` refuses a body whose walk meets an opaque node (`try`, `asserterror`, stray
`break`), found by an `OpaqueProbe` walk; this covers every proof (path-sensitive and
path-insensitive). Fixture `AssertInHelper` (A33). Break B26 -> vanishes, RED.

### FR1-F4 reachable dirty `exit(false)` in the receiver fixtures
`InitViaReceiver` / `InitViaWrapper` take a `Stop: Boolean` and return false on it right
after the dirtying call; the callers pass their own parameter. Same line count.

### FR1-F5 helper-side early exit without a publisher
Fixture `EarlyExitHelper` (A34, `if Skip then exit; Job.Insert();`). Break B27
(`HelperPersists::exit` never fails) -> `EarlyExitHelper` vanishes (with
`HelperOnOnePath`, `IsHandledHelper`, `PublisherBeforePersist`), RED.

### FR1-F6 receipt counts
`.agent/golden-triage.md` and the CHANGELOG now separate distinct keys (988 of 1002
changed) from physical entries (1013 of 1027).

### FR1-OF1 final-head gate
Head `3b007585` (the r1 commit), base `258da610` (the merge-base), both release-fast,
built from clean trees (head tree hash recorded before the build and re-checked after
the runs: unchanged; base built in a detached worktree, since removed). Provenance:
`scratchpad/343/gate-final/{provenance.txt,build-head.log,build-base.log,gate.log}`;
binary sha256 head `b0f70997..`, base `710f3efa..`. One analyze at a time, heavy-ok
present. `gate.sh` (strict): Base App PASS (non-d39 0, added 0, removed 0 = expected
empty, F5), CDO primary PASS (removed the expected 3), CDO all PASS (the same 3). The
CDO 3 are the same fingerprints as in revisions 1-3: no change to triage.

### FR1-OF2 stale base in the test comment
The CLI test doc now says "Without #343 all 36 report (measured on a `506149da` build)".

### FR1-CR5 memoization and laziness
`helper_persists` / `helper_preserves` / `helper_clean` memoized per (routine pointer,
lowercased variable, depth); `clean_return` per (routine pointer, parameter index). The
skip computes `clean_return` only when a guard arm asks for it, and the callee's
re-run reset proof only on a re-run.

### FR1-CR6 lookups
One per-routine index (op id -> op, call-site id -> site), built once per routine row.
Not `ctx.call_site_by_id`: rows of a collided id share site ids and that map keeps the
last row's site (`detector_context.rs`, "Class P per call-site id"), so it can return
another row's site.

### FR1-CR7 one `record_args`
`call_effect` and `call_keeps_clean` share `record_args` (receiver + whole-record
argument indexes).

### FR1-CR8 second walker (refuted)
The spec panel rejected trusting L4 roles or `cfg_walker` state for these proofs (S2,
S4, F5): `cfg_walker` composes callee roles and is blind to whole-record assignments
and receiver calls. `walk_paths` is the ratified fail-closed proof walker; it answers a
different question (must-persist / clean-return) on a different state set.

### FR1-CR9 `record_reassigned` vs `bare_rec_assignments` (refuted)
Deliberately different: d37 / d11 count only `Rec := <bare identifier>` as a reset
(#299 S14), while d39's suppression proof must fail closed on any right-hand side. The
doc comment on `record_reassigned` says so.

### Break table on the final r1 code (logs `scratchpad/343/r2-fin1-*.log`)

| break | effect |
|---|---|
| B1 no `CondCall` hint | RED: `CdoShape`, `GuardWithCleanHelper` appear |
| B2 `exit` never reports | RED: 11 rows vanish |
| B3 no receiver persist | RED: `ReceiverCall` appears |
| B4 `helper_persists` always true | RED: 6 rows vanish |
| B5 no exit literal | RED: `CdoShape`, `GuardWithCleanHelper` appear |
| B6 / B6b depth 1 / 4 | RED: `NestedHelper`, `ThreeLevels` appear / `FourLevels` vanishes |
| B7 polarity swapped | RED: `CdoShape`, `GuardWithCleanHelper` appear, `GuardOnDirtyValue` vanishes |
| B8 no `CARRIED` | RED: `LoopGuard` vanishes |
| B9 `for` header replayed | RED: `ForHeader` vanishes |
| B10 no `asserterror` hint | RED: `AssertErrorShape`, `AssertInHelper` vanish |
| B11 no `MayRun` hint | RED: `TernaryShape` vanishes |
| B12 own reload ops no reset | GREEN (the `reset_after_call` gate covers it) |
| B12 + gate | RED: `HelperReinits`, `CallerReloads` vanish |
| B13 receiver keeps clean | RED: `ReceiverInClean`, `WrappedReceiverInClean` vanish |
| B14 publisher clean | RED: `PublisherInClean` vanishes |
| B15 re-run ignores reassignment | RED: `ReassignInLoop` vanishes |
| B16 `helper_preserves` failure ignored | RED: `IndirectReplace`, `PublisherBeforePersist`, `AssertInHelper` vanish |
| B16b `helper_preserves` false | RED: `PreservingHelper` appears |
| B17 clean return trusts `var` calls | RED: `PublisherInClean`, `WrappedReceiverInClean`, `AssignInHelperClean` vanish |
| B17b `helper_clean` false | RED: `GuardWithCleanHelper` appears |
| B18 publisher preserves | RED: `PublisherBeforePersist` vanishes |
| B19-B27 | as above, each RED |

Final green: `r2-fin1-green.log` (29 rows). `scripts/ci-steps task-affected` 4504 / 4504;
`scripts/check-goldens` 1322 / 1322 (no regen needed).

## Timeline (merge)

- Final round 2: both ship (sol: no findings; Opus: OF3, wording). OF3 fixed in 4558675
  (CHANGELOG + d39 doc comment). Final round 3: both accepted OF3, ship. Register: 46
  entries, all accepted by both. Follow-up noted by Opus: page procedures have the same
  bare implicit-`Rec` gap as CR2 (filed as a discovery).
- Rebased onto 700251ec (#339). CHANGELOG conflict resolved by keeping both entries; the
  code is identical to 4558675; OF3 re-anchored to CHANGELOG.md:L71-L73 (same hash).
- Gates on H = ff0c896b (B = 700251ec): check-goldens-coverage 0, ci-steps-all 0,
  check-goldens 0, cdo-gate 0. Final-head fingerprint gate (3b007585 code, unchanged
  since): see `gate-final/provenance.txt`.
