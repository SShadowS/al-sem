# ENG-165: can a COMMIT run between site A and site B?

Status: revision 3.1, confirmed by Fable (all review findings closed); N1–N8 from the
confirmation are folded in (§11). Ready to implement. Task card: `/coord/tasks/ENG-165/task.md`.

- Revision 2 decided §10's questions 1, 2, 3 and 6, measured questions 4 and 5 on BC (§2.4), and
  found two places in the DO workspace that match the owner's description (§2.5, counts only;
  engine findings, not confirmed by the owner).
- Revision 3 answers Fable's review of revision 2: 5 blocking findings, 6 should-fix findings
  and one scope change. Each is fixed below; §11 maps every finding to the section that answers
  it. Two findings needed new BC measurements (§2.4, round 2). Both changed the design: a
  `Commit()` inside an isolated subscriber commits the caller's work, and `Page.RunModal` with a
  pending write is an error when no test handler stands in.
- Revisions 3.2 to 3.6 answer the reviews of the implementation (Fable rounds 2 to 6, sol
  rounds 5 and 6): the blockers for a loop above the meet routine, recursion on a route, sibling
  subscribers, other edges at a leave site, a step that may run its target more than once (bulk
  operations, per-row and re-running triggers, manually bound subscribers), a sure-B cut only at
  edges that surely fire, and a table of every edge kind with the check that covers it (§4.1);
  the `route-search-capped` relation and what a path that ends before B leaves out (§5.3).

## 1. The question

The owner investigated one codeunit's mail and eDocument bookkeeping on DO (2026-11-20). The
useful question was an ordered one:

> Between this delivery (a send) and its log entry (an `Insert`), can a `COMMIT` run?

Answering it by hand found two places where a sent mail or eDocument can lose its log entry.
The send happens and a commit makes it durable. Then an error stops the code before the
`Insert`. The send is not undone, and the log row is never written.

al-sem answers "does this routine commit?" and "where are its commits?". It does not answer
"does a commit run *after A and before B*?". This spec adds that question, and the errors that
can follow such a commit before B.

Words used below:
- **site**: one statement in the source: a call or a record operation, named by file and line.
- **route**: a call path from the root routine to a site, in the format `prove --site --routes`
  already prints (`witness: {hops, terminal}`).
- **cone**: everything a routine may do, its callees and event subscribers included.
- **trap**: something that catches an error: a checked `Codeunit.Run`, a `[TryFunction]` whose
  result is used, an isolated event.
- **pending write**: a database write not yet committed.

## 2. What exists today (measured)

Principle 4: measured on a fixture and on CDO with master `c6f61f04`, `cdo` profile build.

### 2.1 The fixture

An alc-clean fixture (alc 18.0.41, no warnings), kept outside the repo at
`/work/.al-sem/engine-notes/eng165/fx/`. `Send()` inserts into `CB Outbox` (A is the call to
it, so A leaves a pending write). B is `L.Insert()` on `CB Mail Log`. One routine per shape:

| Routine | Shape | The right answer to "commit between A and B?" | `prove may-commit` today |
|---|---|---|---|
| S1Straight | `Send; Commit; L.Insert` | yes | yes (unconditional) |
| S2NoCommitBetween | `Send; L.Insert; Commit` | **no** | yes (unconditional) |
| S3CalleeCommits | `Send; Helper (commits); L.Insert` | yes, through a callee | yes, path `→ Helper` |
| S4Branch | `Send; if Flag then Commit; L.Insert` | yes, when `Flag` | yes, guard `Flag` |
| S5Loop | `for: Send; L.Insert; Commit` | **no**: B runs on every iteration, so the next B after each A comes before the commit | yes, guard "loop body" |
| S6Event | `Send; OnAfterSend; L.Insert`, a subscriber commits | yes, through a subscriber | yes, path `→ [event] HandleAfterSend` |
| S7CommitThenError | `Send; Commit; if Amount < 0 then Error; L.Insert` | yes, **and an error may run after the commit, before B** | yes (unconditional) |
| S8CheckedRun | `Send; if Codeunit.Run(W) then; L.Insert` | **no**, with a `run-error-rolls-back-a` error row: `Send`'s write is pending at the run, so the run raises an error (L28) and commits nothing of the caller's | yes, "checked Codeunit.Run" (conditional) |
| S9Reversed | `L.Insert; Send; Commit` | **no** (B never runs after A) | yes (unconditional) |
| S10CommitThenExit | `Send; if Flag then begin Commit; exit end; L.Insert` | **no** path A→commit→B, but a commit after A on a path where B never runs | yes, guard `Flag` |
| S11ExclusiveArms | `if Flag then Send else Commit; L.Insert` | **no** (the commit is in the other arm) | yes, guard `not Flag` |

(Revision 3 corrects S8: revision 2 still said `yes` against its own §4.3 rule. Principle 9.)

So `may-commit` says `yes` in all eleven. Six of those (S2, S5, S8, S9, S10, S11) are a "no"
for the ordered question. `may-commit` cannot tell them apart, because it has no notion of order.

The other commands, on the same fixture:
- `prove may-commit --site <file:line> --routes N` gives routes from the **root** to one commit.
  It says nothing about A or B. `--site` only keeps sites of the question's own kind, so it
  cannot name A (a call) or B (an `Insert`) at all.
- `prove throws-error --site` finds the error in S7 and its guard (`Amount < 0`). It does not
  say that the error runs after the commit.
- `explain` lists the commits, the writes and the errors as three separate lists. Its "starts by"
  line shows the first statements in order, but it is a start, not a path.
- `query effects` lists writes and the route to the first write. Commits are not in it.
- `digest --detail` grades order with the ordering engine (`src/engine/l5/ordering*.rs`):
  `COMMIT_DOMINATES_RETURN`, `COMMIT_ON_SUCCESS_PATH`, `WRITE_COMMITTED_BEFORE_RETURN` and,
  for HTTP/FILE only, `EXTERNAL_IO_BEFORE_COMMIT` and `IO_BEFORE_ESCAPING_ERROR`. These are
  pairwise labels (one write or IO against one commit or the return), mostly inside one routine.
  In S1 the write inside `Send` gets no write-vs-commit label at all. No label relates two
  sites the user picks.

### 2.2 What exists that this question can reuse

- **Order inside a routine.** `pending_writes.rs` (`walk_list`, `Before`) already computes "the
  sites that MAY RUN BEFORE a target" on the control-flow tree (`PCFNNode`): earlier statements
  of the same block, the branch that holds the target and not its siblings, the whole body of a
  loop around it. It counts operation-sited edges (triggers, table events) as sites too.
  `program::body::control_flow` (`branch_termination`, `terminates`) says which branches always
  leave by `exit` or by an error. It reads `break` and `continue` as falling through, and it has
  no "must run" helper (§4.2 adds one). `ordering.rs` has the same ideas over
  `orderId`/`frameId` (`may_co_execute`, `ordered_before`, `dom`).
- **Commits in callees.** The cone keeps a COMMIT fact per root. ENG-09 makes a checked
  `Codeunit.Run` a commit of its own (`commit_scope::is_run_commit`, resource `checked-run`).
  `commit_scope` applies `[CommitBehavior]` (`Ignore` removes a commit, `Error` turns it into
  an error) and `InherentCommitBehavior`. A plain `Commit()` inside a codeunit that a checked run
  starts is read by the cone as an ordinary commit (engine limits L16 item 2); this question must
  class it by the route (§4.3).
- **Run edges.** `pending_writes::is_run_edge` tells a checked-run edge apart, and
  `RunCleanCones` walks a callee's cone while leaving out what runs inside checked runs.
- **Routes.** `literal_routes.rs` enumerates distinct routes from the root's start state to a
  site, with literal arguments followed, caps and a complete/not-complete flag. `prove_slice.rs`
  prints them with guards. This is the evidence format to reuse.
- **Errors and traps.** `terminal_facts.rs` (`TerminalSite`) holds the explicit and implicit
  error sites. `literal_reach` decides traps (`TrapAt`, `TrapVerdict`, engine limits B7).
  `TrapVerdict::Caught` means "the error does not escape the **root**" (`literal_reach.rs`
  ~2062). That is not the scope this question needs (§5.1).
- **Worlds.** prove's assumption labels (`loaded-subscribers`, `closed-world`) and its blockers
  (unresolved call sites, open interface dispatch, truncated cone).

### 2.3 CDO (counts only)

`/work/src/cdo-baseline/Cloud` at `bc3ccb18`. A textual census over the workspace source
(5,512 routines, split crudely at `procedure`/`trigger` lines):
- 55 routines hold a direct `Commit()`.
- 1 routine holds both a direct `Commit()` and a direct `.Insert(`, and its commit comes after
  the inserts.
- 0 routines hold a send, then a direct commit, then an insert.

So on CDO the shape is never inside one routine. **The commit sits in a callee or a
subscriber, or B sits in a callee.** Callees and subscribers are the core of this question,
not an extension.

One engine run on CDO, in the CDO slot: `prove may-commit` on the print routine of the mail
template code (A is the print dialog's `RunModal`, B is the log `Insert` inside a callee, and the
routine's only direct `Commit()` is just before A). It answered `yes` with 6 COMMIT effects (4 of
conditionality `unknown`, 2 `guarded-by-IsHandled`). The direct commit before A is one of them.
None is placed before or after A or B. The right answer to the ordered question there is `no`
(only a status check runs between A and B). That commit right before a `RunModal` is the shape
T18 (§2.4) explains: a pending write at a `Page.RunModal` is an error. Locations are in the private
note `/work/.al-sem/engine-notes/eng165/cdo-shapes-PRIVATE.md`.

### 2.4 Runtime facts, measured

alc 18.0.41 compiles every shape below without a warning. The runtime facts were measured on the
**BC 28 Linux sidecar** (`http://bc`, instance `BC`, platform 28.0.54946.56477), 2026-11-25,
with a test codeunit that raises its results through `Error('MEASURED …')`. Pending-write
oracle (ENG-09): a checked `Codeunit.Run` inside a `[TryFunction]` fails if and only if a write
is pending. Probe source and results: `/work/.al-sem/engine-notes/eng165/bcprobe/` (the app was
unpublished after each round).

Round 1 (revision 2):

| Test | Shape | Result |
|---|---|---|
| T01 / T02 | `Insert`; `Insert; Commit` (controls) | pending / none pending |
| T03 | caller `Insert`, then `Page.RunModal`, **under the test's modal handler** | no error; still pending |
| T04 | `Page.RunModal` (handler) of a page whose `OnOpenPage` inserts | still pending |
| T05 | caller `Insert`, then `Report.RunModal` (processing-only, no request page; no handler) | no error; still pending |
| T06 | a report that inserts in `OnPreReport` | still pending |
| T07 | nothing pending, isolated event, the subscriber inserts | none pending; the row is kept (committed) |
| T08 | nothing pending, isolated subscriber inserts then errors | the caller goes on; the row is gone (rolled back) |
| T09 | caller `Insert`, then an isolated event whose subscriber inserts | no error; still pending |
| T10 | nothing pending, plain event, the subscriber inserts (control) | pending |
| T13 | caller `Insert`, then an isolated subscriber inserts and errors | the error is **not** trapped: it stops the caller |
| T11 | evaluation order | `Log(Send())`: Send, Log. `Log2(Send(), Send2())`: Send, Send2, Log2. `if Send() then Log`: Send, Log |

Round 2 (revision 3, for review findings 2 and 4):

| Test | Shape | Result |
|---|---|---|
| T14 | caller `Insert`, then an isolated subscriber runs `Commit()` | none pending: **the caller's write was committed** |
| T15 | same with a plain subscriber (control) | none pending |
| T16 | inside a checked run: caller `Insert`, isolated subscriber `Commit()`, then `Error` | the run returns false; **the caller's row is kept** |
| T17 | same with a plain subscriber (control) | the run returns false; the caller's row is kept |
| T18 | caller `Insert`, then `Page.RunModal`, **no handler** | **error at the `RunModal`**: "An error occurred and the transaction is stopped." (the L28 text) |
| T19 | nothing pending, `Page.RunModal`, no handler (control for T18) | no error |
| T20 | nothing pending, `Report.RunModal` (control) | no error; none pending |

What the rounds settle:
- **Order inside a statement** (T11): arguments left to right, then the call; a condition
  before its branch.
- **`Page.RunModal` with a pending write is an error** (T18 against T19). T03's "no error" came
  from the test's modal handler, which stands in for the page. Revision 2's "not an error" was
  wrong for a real run; revision 3 corrects it (§4.3).
- **`Report.RunModal`** of a processing-only report without a request page, with a write pending:
  no error, no commit (T05). A report **with** a request page was not measured (no client).
- **Isolated events**: with nothing pending, each subscriber's writes are committed and its error
  is trapped (T07, T08). With a write pending, the event commits nothing of the caller's and traps
  nothing (T09, T13). **A `Commit()` inside an isolated subscriber commits the caller's pending
  write** (T14, T16), as in a plain subscriber (T15, T17).

Scope: one shape each, on a server session run by the test runner, not an interactive client.
Not decided, and no answer depends on it: in T09, whether the isolated subscriber's own row
commits apart from the caller's pending write.

**Windows rule (repo CLAUDE.md).** A fact measured on BC 28 on Linux is confirmed on a Windows
Cronus before it is recorded as a platform fact. Until then the rules in §4.2 and §4.3 rest on
the Linux measurement, and the spec says so. The engine-limits entry the implementation writes
carries the label "BC 28 Linux; Windows confirmation pending" and the probe app's location, so
the owner can rerun it. The `Page.RunModal` and request-page rows matter most for that check: an
interactive web client on Windows Cronus is the one setting not measured here.

### 2.5 The DO workspace (counts only)

A clean DO checkout (`b3b4459`, 2026-10-05) was searched. Its source is private: no names or
text here. The owner's 11-20 session was on a later revision, and the owner does not remember
which two places he found. **The two places below are engine findings: two places the engine
finds in the DO workspace that match the owner's description. They are not confirmed by the
owner.** The future CHANGELOG entry uses the same wording.
- Textual census (§2.3's script): 5,665 routines, 56 with a direct `Commit()`, 0 with send →
  commit → insert in one routine. A second census for "a delivery call, then a log call, in one
  routine" found 8 routines (CDO: 8), 7 with other calls between the two (CDO: 5), **0 with a
  direct `Commit()` between** (CDO: 0).
- Engine runs (copy of the workspace with the CDO baseline's dependency packages, same
  versions; in the CDO slot). On the eDocument dispatch routine of the codeunit that matches the
  owner's description, `may-commit` answers `yes` with 9 distinct commit sites. Placed by the
  first call on their path in that routine: 6 lie behind the call that sends the document, 3
  behind the calls that finalize and log the delivery, **0** behind the calls between those two.
- **The first place (an engine finding).** The send goes down into a dependency app. There the routine sets the
  document's status, commits, makes the delivery call, and **on success commits again**, then
  returns. Back in the workspace routine, two explicit error calls (and the implicit errors of the
  record operations between) can run before the call that writes the log entry. That matches the
  shape the owner described: sent, committed, then an error before the log `Insert`.
- **The second place (an engine finding).** The mail send goes through System Application's `Email.Send`, which runs
  `Commit()` and then a checked `Codeunit.Run` of its dispatcher (the dispatcher sends, in its own
  transaction). Back in the workspace, a handled-field update (a record `Modify`) runs between the
  send and the log call; its errors can stop the code before the log entry. Engine run on the
  mail routine: `may-commit` answers `yes` with 3 commit rows: two at the `Email.Send` crossing
  (the explicit commit, and the checked-run commit), one behind the log call itself, **0** behind
  the record update between them.

**How the design finds them** (revision 3):
- **eDocument.** Name A as the delivery call inside the dependency (sites may sit in dependency
  source, §3). The commit after it lies in **A's tail** (§4.1), a class-1 commit (§4.3), so the
  answer is `yes`. The error calls back in the workspace routine lie in the meet routine's middle:
  `errorsAfterCommit` rows. (Named at the workspace level instead, at the call into the
  dependency, the same commits are `commitsInsideA` rows, §5.4.)
- **Mail.** Name A as the workspace's call that sends. The explicit `Commit()` in `Email.Send`
  runs **inside A**, before the dispatcher sends; the dispatcher's own commit is a class-2 commit
  (own transaction) inside A. So the between-verdict is `no` (nothing commits between A's return
  and B), with two `commitsInsideA` rows and the `Modify`'s errors in `errorsBetween` (§5.1).
  That is the honest reading: the mail is sent and kept by the dispatcher's own transaction, and
  the risk is an error after A and before B, which `errorsBetween` lists without any commit.
  Named deeper, at the dispatcher's checked run, A's own run-commit makes A durable; the answer
  then lists the same errors.
- An error row guarded by a callee's result (in the eDocument case, an error under "if the send
  reported failure", seen on the path where the send succeeded) is listed with its guard, not
  evaluated. The literal flow does not follow return values; the row says so.

## 3. The question form

```
alsem prove <ws> <routine> commit-between --after <FILE:LINE[:COL]> --before <FILE:LINE[:COL]>
            [--routes N] [--closed-world] [--detail] [--format json]
```

- `commit-between` is a new prove question. `--after` names A; `--before` names B. "After A and
  before B" is how the answer reads, so the flags say it.
- **The file.** A source file path or its tail, matched at a `/` boundary, ASCII
  case-insensitive, as `--site` matches today. It may be workspace or dependency source (the
  `dep:<app>:<path>` path the evidence prints, or a tail of it). **A tail that matches two files**
  (a workspace file and a dependency file, or files of two apps) is refused, and the message lists
  the full paths.
- **The line** is 1-based. **The column** is optional, 1-based, counted in UTF-16 code units as
  the editor shows it (the engine stores 0-based UTF-16; the parser converts once).
- **Parsing** is deliberate about both forms. `SiteFilter::parse` uses `rsplit_once(':')`, so
  today `f.al:12:5` would read as file `f.al:12`, line 5. The new parser splits the last two
  `:`-segments: when both are numbers, it is `file:line:col`; when only the last is, it is
  `file:line`. A Windows drive letter (`C:/…`) never ends in two numeric segments, so it is not
  misread. Unit tests pin `f.al:12`, `f.al:12:5`, `C:/x/f.al:12`, `C:/x/f.al:12:5` and
  `dep:App:src/f.al:7`.
- **A candidate** on a line is a call site or a record operation (with its trigger and
  table-event edges, which belong to the operation, §4.1). Each nested call is its own candidate
  (`Log(Send())` has two: `Log` and `Send`), told apart by column.
- A line with no candidate, or more than one with no column given, is refused. The message lists
  the candidates with their columns (principle 5: never guess a site).
- A or B that the root cannot reach (guards ignored) is refused with the reason. A or B that the
  root reaches only on routes the literal arguments rule out answers `no` with relation
  `a-dead-under-literals` or `b-dead-under-literals`, as prove already drops literal-dead sites
  and counts them (`literalDeadSites`).

**Rejected:** an operation selector such as `insert:"Mail Log"` or `call:Send`. One routine can
hold several, and the owner's question was about one specific send and one specific log line.
File and line is what an investigation has in hand. Principle 13 (decision 10.1).

**Rejected:** `commit-between:<A>..<B>` as one question string. Paths hold `:` already, and a
site is a filter like `--site`, not part of the question's name.

## 4. What "between" means

An execution of the root **runs A, and later runs B, and runs a commit C after A has returned
and before B starts**. A commit that runs while A itself runs (in A's own cone, when A is a call
or an operation with triggers) is not "between": it is listed apart (§5.4), and so is a commit
inside B before B's write.

### 4.1 The meet routine and the region

A and B may sit in different routines. Take a route to A and a route to B, both from the root's
state graph (`literal_routes`), so the root's literal arguments hold on both. They share a
prefix. The **meet routine** M is the last routine on that shared prefix. In M, A's route leaves
at call site `a` (or `a` is A itself, when A is in M), and B's route leaves at `b` (or `b` is B).

The **between region** has three parts:
1. **A's tail.** In each routine on A's route below M: the statements that may run after the call
   toward A (or after A itself), up to the routine's end. Example: a commit later in the same
   dependency routine, after the delivery call.
2. **M's middle.** In M: the statements that may run after `a` and before `b`.
3. **B's head.** In each routine on B's route below M: the statements that may run before the
   call toward B (or before B itself).

**What a region statement contributes:** its own effects, and the cones of everything it starts:
- a call's callee cone, and an event raise's subscribers (the existing event edges);
- a record operation's **trigger and table-event edges** (`Insert(true)`, `Modify(true)`,
  `Delete(true)`, `Rename`, `Validate`, and the table events that run whatever `RunTrigger` says),
  as `pending_writes` treats operation-sited edges. A or B can be such an operation too: an
  `OnInsert` that commits before B's own write is a `commitsInsideB` row (§5.4).

A commit C is between A and B when it is contributed by a region statement.

**Every route pair is checked.** Several routes to A or to B give several meet routines and
regions. Every pair is checked, up to the route cap. A cap that fires leaves the answer
`unknown`, never `no` (principle 5). The answer lists the pairs examined and whether the
enumeration completed.

**A loop above the meet routine** (revision 3.2, Fable review r2): when a routine on the shared
prefix can call toward M again (the call site toward M reaches itself in that routine's graph),
M runs again and the next B after A may be in a later run of M. That is not followed yet: the
answer carries the blocker `loop-above-meet` and is `unknown`, never `no` (principle 1: a false
`no` is the one answer the question cannot give). A prefix routine with no model row is not known
not to loop, so it blocks the same way. Judging each shallower level as a meet routine with
`a = b =` that call site is the full fix (leftovers card). Likewise, when a route leaves a
publisher by its event, the event's other subscribers run in no defined order around the route's:
the blocker `sibling-subscribers`.

**Recursion on a route** (revisions 3.3 and 3.4, Fable reviews r3 and r4): the route search
never repeats a state, so a real stack `M → X → X → A` is listed as `M → X`, and what the outer
run of X does after the inner run returns is in no part. When any routine on either route (A's
route from the root to A's routine, B's route below M to B's routine) can reach itself over the
edge graph (every edge kind, guards ignored), the answer carries the blocker
`recursion-on-route` and is `unknown`. Shapes: S41 (M runs again: `Outer: B; Inner()`,
`Inner: A; if C then begin Commit; Outer(false) end`), S43 (a routine below M on A's route:
`Rec: if Again then begin Again := false; Rec(); Commit end else A`, root `Rec(); B`). The mirror
on B's route (S43b) is already `yes` without the blocker: recursion before the first B passes
through B's head, which is walked; B's route is checked too, as the safe side. Every routine of both routes is checked because any real stack the search
does not list repeats a routine, and that routine stays on a listed route. Revision 3.3 checked
only the prefix and M, which Fable's review r4 showed was too narrow.

**Every route edge fires once per pass** (revisions 3.4 and 3.5, Fable reviews r4 and r5, sol
review r5). The parts read each route edge as running its target exactly once each time its site
runs. Where that does not hold, the answer is `unknown` or the cut is not taken. The graph already
marks the edges whose target may not run, or runs once per row, `TriggerFires::Maybe`:
- *More than once, on A's route, the shared prefix included.* A step that may run its target
  more than once runs A's part, or M itself, again; those runs are in no part. Two blockers:
  `bulk-operation-on-route` for a trigger or table-event step of a bulk record operation
  (`ModifyAll`, `DeleteAll`: once per matching row), or of a record operation the model does not
  hold (not known not to be bulk); `repeated-trigger-on-route` for a variable-typed call or object
  run marked `Maybe` (a report dataitem's or XmlPort element's trigger, once per row; a page's
  `OnAfterGetRecord`). Shapes: S44 (`OnDelete: if "No." = 2 then Commit; A`, root
  `D.DeleteAll(true); B`), S45 (on the prefix, so M is the trigger itself: `OnDelete: B; A;
  Commit`, root `D.DeleteAll(true)`; row 1's A, the commit, row 2's B), S47 (a batch job: root
  `Report.Run(…)`, the dataitem's `OnAfterGetRecord` is `B; A; Commit`). B's route below M is
  checked too, as the safe side.
- *Zero times, on B's route: no sure-B cut.* A sure-B cut (§4.2) stands only on a plain edge that
  must run its one target whenever its site runs: a direct call, or a variable-typed call,
  implicit trigger or object run not marked `Maybe`, with no other edge at the same site. Never a
  table event (a bulk operation may match no row; a subscriber may be bound by hand), an
  interface call (one implementer of several runs, open or closed world) or an event edge.
  Otherwise the path is not cut there, the site's cone enters the region, and the answer is `yes`
  or blocked. Shapes: S46 (a loop of `A; SetRange; DeleteAll(true); Commit` whose first pass
  matches no row: `yes`) and S48 (a loop of `A; Insert(I = 2); Commit`, B in `OnInsert`: the
  non-literal RunTrigger runs B only on pass 2: `yes`).
- *Zero times, on A's route.* Needs nothing: on a pass where A's edge does not fire, A does not
  run, and the question is about the runs after A.

Neither rule changes a `yes`: the blockers only add reasons, and fewer cuts only widen the
regions. Fewer cuts can turn a `no` into a `yes` (S46, S48, S52, S53): the may-reading of a B
that is not sure, as S5d.

**Every edge kind, and which check covers it** (revision 3.6, Fable and sol reviews r6). Three
checks read a route step: `may_repeat` (the step may run its target more than once: a blocker on
A's route, the prefix included, and on B's route below M), `fires_surely` (the step must run its
one target: needed for a sure-B cut) and `site_co_edges` (other edges at the step's site that run
in the same statement: a blocker).

| Edge kind | Runs its target | `may_repeat` | `fires_surely` | `site_co_edges` |
|---|---|---|---|---|
| direct call | once; one of several if ambiguous | no | yes, when alone at its site (pinned with the interface arm by S53; the engine resolves a `Variant` argument to one overload, so no direct-call shape with two targets was found) | not counted (alternatives) |
| variable-typed call, not `Maybe` | once; a run on an object variable fans out to each lifecycle trigger | page trigger other than `OnInit`/`OnOpenPage`/`OnClosePage` | yes, when alone at its site | counted (S50) |
| variable-typed call, `Maybe` | zero times, once, or once per row | yes (`repeated-trigger-on-route`) | no | counted |
| interface dispatch | one implementer of several | no | no (S53) | not counted (alternatives) |
| object run, not `Maybe` | once (a codeunit's `OnRun`, a report's or XmlPort's object triggers, a page's lifecycle triggers) | page trigger other than `OnInit`/`OnOpenPage`/`OnClosePage` (S51) | yes, when alone at its site | counted |
| object run, `Maybe` (report dataitem, XmlPort element, request page, page record triggers) | zero times, once, or once per row | yes (S47) | no | counted |
| implicit trigger of a single-row operation, `Always` | once | no | yes, when alone at its site | counted (S42) |
| implicit trigger, `Maybe` (non-literal RunTrigger) | zero times or once | no | no (S48) | counted |
| implicit trigger or table event of `ModifyAll`/`DeleteAll`, or of an operation not in the model | once per row, possibly zero | yes (`bulk-operation-on-route`, S44, S45) | no (S46) | counted |
| table event of a single-row operation | once, or zero when the subscription is skipped | no | no (S52) | counted |
| event dispatch to a plain subscriber | once per raise | no | no (not a guarded site) | `sibling-subscribers` for the event's other subscribers |
| event dispatch to a subscriber whose subscription may carry a condition (`Manual`, a skip on a missing license or permission) | once per bound instance, possibly zero | yes (`repeated-subscriber-on-route`, S49) | no | `sibling-subscribers` |
| unresolved object run (`Codeunit.Run(Id)`) | not known | no route goes through it | n/a | n/a: a region call no edge leaves blocks (`region-unresolved-call`) |

A loop or recursion around a step is control flow, read by `loop_above_meet` and
`recursion_on_route`, not by these three.

**Other edges at a leave site** (revision 3.3, Fable review r3): a record operation or object
run that a route leaves by can have several edges at the same site: an implicit trigger, a table
event's subscribers, an object's several triggers. They all run in one execution of that
statement, before or after the route's target, and no part walks them (A's tail and M's middle
start after the leave node; `commitsInsideA` is A's own cone). At every guarded leave site of
either route from M (index k) down, another edge of those kinds at the same site, with a
different target, blocks: `site-other-edges` (shape S42: A in a table's `OnInsert`, an
`OnAfterInsertEvent` subscriber commits, root `Rec.Insert(true); B`). Interface implementers and
other alternatives (one of them runs instead of the route's) do not count.

### 4.2 Order inside one routine

"May run after X and before Y" is computed on the routine's control-flow tree, as
`pending_writes` computes "may run before". The rules, each pinned by a fixture (§8):
- **Blocks.** Statements after X and before Y in the same block are in. So are the conditions
  on the way down to Y.
- **Branches.** Only the branch that holds X (going up) and the branch that holds Y (going down)
  count. A sibling arm of either is out (S11). An `if` or `case` **between** X and Y counts
  with all its arms (S4).
- **Branches that leave.** An arm between X and Y that always ends by `exit` or by an error
  (`branch_termination`) never reaches Y. Its commits are not between A and B (S10). They go to
  `commitsWithoutB` (§5.3). `branch_termination` reads `break` and `continue` as falling
  through; this question does not. An arm that ends in `break` leaves the loop: if B is after the
  loop, control still reaches B and the arm's commits are between; if B is only inside the loop
  body (not after the loop), the arm never reaches B and its commits go to `commitsWithoutB`
  (N4). A `continue` arm goes to the next iteration, so it reaches B only over the back edge.
- **Traps on the way (N2, N3).** A call toward A that is a trap (a used `[TryFunction]` call, a
  checked run) catches an error raised in A's tail and control resumes after that call, in the
  region: B is still reachable, so the error row is left out (§5.2). Shape
  `if TrySend() then; L.Insert`, TrySend: A, `Commit`, `Error` → `yes`, the error left out.
- **Nested calls in one statement** follow T11: arguments left to right, then the call; an `if`
  condition before its branch. A commit in an argument's cone that comes after A's argument and
  before the call B (or before B's argument) is between.
- **Loops: the next B after A, cut only where B surely runs.** "Between A and B" means between A
  and the next run of B. A path from A is cut at a run of B **only when that B runs on every
  path that continues from that point**: B is not inside a branch, not after a `break`,
  `continue`, `exit` or error that may leave first, not inside a loop that may run zero times,
  and, when B is reached through a call, the call runs on every continuing path, its edge fires
  whenever it runs (§4.1, "every route edge fires once per pass"), and B runs on
  every path through the callee. Otherwise the path is not cut there and the search goes on.
  - S5 (`for: Send; L.Insert; Commit`): B surely runs after each A in the same iteration, so
    the path is cut there: `no`.
  - S5c (`for: Send; if Flag then L.Insert; Commit`): B may be skipped. Then the commit runs, the
    next iteration's A runs, and its B: so a commit can run between the first A and the next B
    that runs. `yes`, labelled `loop-carried`. A positional cut would wrongly say `no`.
  - S5b (`repeat L.Insert; Send; Commit until …`): the first B after A is the next iteration's,
    reached only over the loop's back edge: `yes`, `loop-carried`.
  - **Across routes:** `Send; Helper(); Commit; Log()` where `Helper` may reach B: `Helper`'s B
    is not sure to run, so the path is not cut there, and the commit before `Log`'s B makes the
    answer `yes`. That is the sound reading.
  - **Recursion:** the route search never repeats a state, so a route never shows a routine
    running again around A or B. Recursion through any routine on either route is not
    followed: it blocks with `recursion-on-route` (§4.1, revision 3.4). Revision 3's claim that
    "recursion is covered by the general rule and by the route enumeration's existing tests"
    was wrong (Fable review r3, shape S41), and so was revision 3.3's check of the prefix and M
    only (Fable review r4, shape S43).
  - The "must run" test is a new helper beside `control_flow`: it treats `break`, `continue`,
    `exit`, an error and a maybe-zero loop as ways B can be skipped. **A B inside a trap scope
    that the continuation is outside of** (`if TryLog() then; Commit`, B in TryLog) is never a
    cut point: an implicit or explicit error inside the trap skips the rest of B's routine and
    control resumes after the trap (N3). Where it cannot decide, it answers "not sure to run", so
    the path is not cut (the safe direction: a possible `yes`, never a wrong `no`); that fallback
    covers the trap case too, and a shape pins it (S5f).

### 4.3 What counts as a commit: two classes

Only a commit that ends the **caller's** transaction makes A's pending write durable. Every
commit is classed **by the route from the region statement to C**:

| Commit, as reached on the route | Class | Why (evidence) |
|---|---|---|
| a `Commit()` that runs normally, directly or through calls, plain subscribers, triggers | **1: ends the caller's transaction** | the definition of `Commit()`; T02, T15, T17 |
| a `Commit()` inside an **isolated** subscriber | **1** | T14, T16: it committed the caller's pending write |
| a `Commit()` under `[CommitBehavior(Ignore)]` on its route | none | `commit_scope` (per route) |
| a `Commit()` under `[CommitBehavior(Error)]` on its route | none; it is an **error** site | `commit_scope` (per route) |
| a checked `Codeunit.Run`'s own commit (ENG-09), and **any** `Commit()` whose route crosses a checked-run edge | **2: own transaction only** | nothing may be pending at a checked run (L28), so what commits inside it is the run's work; `pending_writes::is_run_edge`, `RunCleanCones` tell those edges apart |
| an isolated event's commit of its subscribers' writes (with nothing pending) | **2** | T07; with a write pending it commits nothing of the caller's (T09) |
| a `Commit()` inside an object a call starts (`Page.RunModal`, `Report.Run`, …) | class by the route, as above, with `commit_scope`'s "may apply" reading | labelled `through-object-run` |

Class-1 commits decide `yes`. Class-2 commits never make A's pending write durable; they are
listed as `ownTransactionCommits`, labelled.

**The class depends on which side of the run edge A is (N1).** The table above is for a run or an
isolated event that A is **outside** of. When A's own route crosses a checked-run edge (or an
isolated event raised with nothing pending) **below the meet routine**, A runs inside that run's
transaction, and the run's implicit commit at its successful return is exactly what makes A
durable. So such an edge contributes a **class-1** commit for A, at the run's successful return,
as part of A's tail (§4.1), labelled `run-commit-of-A`. Shape:
`if Codeunit.Run(Sender) then; if Bad then Error(…); L.Insert`, with A inside `Sender` → `yes`,
with the `Error` in `errorsAfterCommit` (S30; the deep-named DO-2 twin, §8).

Some calls between A and B are **errors when a write is pending**. They are listed in the error
lists (§5.1), each with its label, never as commits:
- a checked `Codeunit.Run` where A's write (or any write) may be pending: L28, measured.
  Label `run-error-rolls-back-a`: BC raises the error at the run, so A's pending write is rolled
  back with everything else, and the log loss does not happen through that path (S8). It is never
  the dangerous kind.
- `Page.RunModal` where a write may be pending: T18, measured without a test handler. Label
  `runmodal-pending-write`.
- `Report.RunModal` / `Report.Run` of a report **with a request page**, where a write may be
  pending: not measured. It is listed as a possible error labelled `unmeasured: request page with
  a pending write`. A processing-only report without a request page is neither a commit nor an
  error (T05).

Out of scope: the commit at the end of the transaction, outside the root. The root's callers
decide it. The answer says "the root's callers are not followed"; the follow-up task covers it
(§10.3).

## 5. The answer

Three verdicts for the between question, as every prove question has, plus four lists that keep
the facts around it (principle 17: keep every fact, as a labelled class).

### 5.1 `yes`: a route where a class-1 commit can run between A and B

Each evidence row holds:
- `after`: A's site and one route from the root to A.
- `commit`: C's site, its class, its kind (`commit`, through a callee, an event, an object run,
  an isolated subscriber), and its route **taken from the root's state graph and kept only if it
  passes through the region statement**. So the root's literal arguments hold on it, and a commit
  the literals rule out is not listed (a commit routed from the callee alone would be, and would
  break the `may-commit` contract of §5.5). The route is the existing `witness {hops, terminal}`.
- `before`: B's site and one route to B.
- `guards`: the conditions on the way, as `prove_slice` shows them (selected context, not every
  needed condition).
- `labels`: `loop-carried`, `through-callee`, `through-event`, `through-object-run`,
  `isolated-subscriber`, `run-commit-of-A`.
- `errorsAfterCommit`: the error sites that may run **after C and before B** (§5.2 says which
  traps leave a row out). **This is the dangerous case** (S7): the commit made A durable, and
  this error stops the code before B.

`--routes N` gives up to N distinct commit routes per commit site, as for `--site`.

### 5.2 Error rows and trap scope

`TrapVerdict::Caught` says an error does not escape the **root**. That is the wrong scope here.
What matters is whether control still reaches B:
- A trap **below the region statement** that holds the error (a checked run, a used
  `[TryFunction]` or an isolated event entered from inside the region, on the error's route)
  catches the error and control goes on toward B. Such an error is left out, with the trap stated.
- A trap **at or above the meet routine** (on the shared prefix: root → checked run or
  `[TryFunction]` → … → M) catches the error only after M is left. A was committed, B never runs,
  and the error is swallowed: **the worst case**. The row stays, labelled
  `swallowed-above-meet`, with the trap's site. In B's head the same holds for a trap above the
  routine that holds B's head.
- A trap that fails with a write pending (`TrapVerdict::UnlessWritePending`: an isolated event, a
  checked run with a write pending) does not catch: the row stays. **Whether a write is pending is
  read at the commit C, not at the root (N8):** right after a class-1 commit nothing is pending
  until the next write on the route, so a trap after C and before any later write does catch.
  Where the walk cannot tell (a write in a callee that may or may not run), it treats the write as
  possibly pending and keeps the row, labelled `trap-may-fail-pending` (an over-listing, never a
  dropped row).

The lists:
- `errorsAfterCommit` (per class-1 commit, §5.1).
- `errorsBetween`: every error site that may run after A returns and before B, commit or not.
  Same walk, no kind detection of A: it is always listed (revision 3 drops revision 2's HTTP/FILE
  gating, which rested on an open question). When A is an effect that is durable without any
  commit (an external send), these are the rows that lose the log.

### 5.3 `no`: proved, no class-1 commit can run between A and B

Every path from A to the next B (§4.2) was followed, no class-1 commit is in any region, and
every obligation is clear. A `no` carries:
- `relation: "b-never-after-a"` when B never runs after A at all (S9);
  `"not-in-one-build"` when A and B sit in alternative declarations of one routine (two rows of
  one routine under `#if`, engine limits B2) and every route pair needs both: they never run in
  one build; `"a-dead-under-literals"` / `"b-dead-under-literals"` (§3). When no route was found
  but the route search stopped at its budget, the relation is `"route-search-capped"` and the
  answer is `unknown` (blocker `route-cap`): the engine does not know the site is dead.
- `commitsWithoutB`: class-1 commits after A on paths that end before B, with how they end:
  `exit`, an error, a **return** (the routine runs off its end, so B, deeper or later in a caller,
  does not run on that path), or a `break` that leaves a loop when B is only inside that loop
  (not after it; §4.2, N4). A commit then an error
  that never reaches B is the same risk as S7. These rows appear with every verdict.
- `errorsBetween` and `ownTransactionCommits`, as always.

**What a path that ends before B leaves out** (revision 3.3, Fable review r3, for cost): a
statement on such a path is read for its own records only; its callees are not walked. Such a
statement never decides the verdict or a blocker. But the output lists less:
- `commitsWithoutB` lists only commits written in the statement itself, not a commit in a callee
  on such a path;
- an error raised inside a callee on such a path (for example a cleanup routine called in an arm
  that then exits) is in neither `errorsBetween` nor any commit's `errorsAfterCommit`.

Walking those callees again is on the leftovers card (item 9), with the cost it adds.

### 5.4 Commits during A and during B

When A is a call or an operation whose own cone commits, those commits run *during* A, before or
after the effect inside it. They are not "between" (§4's definition). They are listed as
`commitsInsideA`, each with its class and route, and the verdict stays what the region decides.
The human output says how many there are and names the deeper site to give as `--after` to order
them (the first call on their route below A). Symmetrically, `commitsInsideB` lists **every**
class-1 commit in B's own cone (an `OnInsert` trigger or an `OnBeforeInsert` subscriber that
commits, when B is `Insert(true)`; a commit anywhere in B's callee, when B is a call), and the
output says "name a deeper `--before` to order them", as for A (N6). Revision 2 answered those
with a silent `no`.

Why a list and not a `yes` (review finding 5): on DO, 6 of 9 and 2 of 3 commits sit behind A.
Counting them as `yes` would make the answer nearly always `yes`, and it would contradict the
definition. Each `commitsInsideA` row carries its own `errorsAfterCommit`, so the dangerous case
inside A is still shown.

**Row order in the human output:** first every class-1 commit (between, inside A or inside B)
with at least one error after it, between rows before inside rows; then the other between rows;
then `commitsInsideA` and `commitsInsideB`; then `commitsWithoutB`; then `errorsBetween`; then
`ownTransactionCommits`. The JSON keeps each list in its own field.

### 5.5 `unknown`: something could not be followed

The blockers say what. Unknown is not no (principle 5):
- a call site in the region that no edge leaves, or a dynamic `Codeunit.Run(Id)`;
- an interface call in the region with an implementation that may not be loaded (waived only
  under `--closed-world`, as prove does today);
- event subscribers beyond the loaded ones (the existing `loaded-subscribers` assumption: the
  answer carries the label, as `may-commit` does);
- **a routine on a route or in the region without a control-flow tree**: a dependency loaded from
  symbols only (its rows come from the ABI with no tree, `abi_rows.rs` ~233), or a routine whose
  tree does not hold the site the route leaves at. `pending_writes` falls back to "every site"
  there; this question answers `unknown` and names the routine and its app;
- a route or state cap fired (the pairs examined are stated);
- a construct outside §2.4's measured shapes whose commit or error behaviour the answer depends
  on (named, never guessed).

### 5.6 Consistency with `may-commit` and `throws-error`

- `may-commit` is `no` ⇒ `commit-between` is `no`, with the same obligations. This is the cheap
  first step.
- Every commit `commit-between` lists (between, inside A or B, without B, own transaction) is a
  `may-commit` evidence site of the root, or sits in the cone of one. A contract test checks it on
  the fixture and on CDO. It holds because the routes come from the root's state graph (§5.1).
- Every error row is a `throws-error` site of the root, **or** an error `throws-error` leaves out
  as caught by a trap at or above the meet routine (labelled `swallowed-above-meet`, §5.2). A
  contract test checks it.

## 6. Cost

- The region walk is linear in the size of each routine on A's and B's routes, per route pair.
- Commits and their classes come from the root's state graph and the cone the prove plan builds.
  The class and `[CommitBehavior]` are per route, so a commit may need more than one route: the
  search keeps, per commit site, the best route of each class. `RunCleanCones` is the existing
  answer for "the cone without what runs inside checked runs"; it is built once per question.
- Error sites come from the same terminal records `throws-error` reads; trap scope is decided per
  error route against the meet routine.
- The question needs a level-3 read (the control-flow trees alive), as the `--site` slice does
  today. It declares that in its `ViewNeeds` (the `declared-needs` skill): trees, site guards and
  rules, nothing new in the default build.
- Expected cost on CDO: one prove run plus the walks; no new resident memory. The implementation
  measures it (`cdo-before-after`, instructions) before landing.

## 7. Consumers, and the trees the design reads

- **CLI:** the form in §3. JSON is the prove document with a new `commitBetween` block.
- **MCP:** `alsem_prove` already turns its arguments into the CLI's argv. It gains `after` and
  `before` arguments. Answers stay byte-equal to the CLI (existing contract tests).
- **LSP:** `al-call-hierarchy/prove` passes argv through the engine child, so it gets the question
  without new code. A code action is not part of this task (principle 13).

**Do dependency routines keep their control-flow trees?** The design walks dependency routines
(A's tail in the eDocument case). Read from the code:
- `BuildProfile::ON_DEMAND` (the resident engine's `--low-mem`) is "`FULL`'s facts, with the
  dependency **syntax** trees parsed per build instead of kept in the tier" (`profile.rs`). The
  control-flow tree a model row holds (`ModelRoutine.statement_tree`, a `PCFNNode` skeleton) is a
  fact projected from the syntax tree (`workspace.rs` `project_ir`), so both profiles give it.
- In the demand-driven cross-app model, `append_dependency_rows` projects each source-bearing
  dependency file's rows from its tree (parsing it on demand under `OnDemand`) for the demanded
  population, and the row cache keeps the same rows. Routines on a route from the root are in
  that population (the cone follows every resolved call). A symbol-only dependency's rows have no
  tree (`abi_rows.rs` ~233): `unknown`, §5.5.
- This is read, not measured. The implementation adds a test that a dependency routine on a route
  has a tree in both the default and the `--low-mem` session modes
  (`program::session::workspace::tests::{default_mode,low_mem}` style), with a discrimination
  proof.

## 8. Tests and discrimination proofs

The expected answers are written down before the code runs (principle 9). The fixture moves into
`tests/cli/` as one test file with the AL inline, in the style of `cli_commit_sites_up_chain.rs`.
§2.1's shapes S1–S11 (S8 corrected), and:

| Shape | Code | Expected |
|---|---|---|
| S5b | `repeat L.Insert; Send; Commit until …` | `yes`, `loop-carried` |
| S5c | `for: Send; if Flag then L.Insert; Commit` | `yes`, `loop-carried` |
| S5d | `Send; Helper (may insert the log); Commit; Log()` | `yes` (no cut at a maybe-B) |
| S5f | `Send; if TryLog() then; Commit; Log()`, B inside `TryLog` (a used `[TryFunction]`) and B again in `Log` | `yes` (no cut at a B inside a trap scope, N3) |
| S12 | `Send; if X then begin Commit; Error('x') end; L.Insert` | `no`, one `commitsWithoutB` row ending by an error |
| S13 | `Send; Codeunit.Run(Id); L.Insert` (dynamic id) | `unknown`, naming the site |
| S13b | `Send; IFace.Run(); L.Insert` (an interface with an implementation that may not be loaded) | `unknown`; `no` under `--closed-world` when the loaded implementations do not commit |
| S14 | a helper with `[CommitBehavior(Ignore)]` that commits, between | `no`; with `(Error)`: `no`, one error row |
| S15 | A inside a callee with a commit after it in the same callee | `yes` (A's tail) |
| S16 | B inside a callee with a commit before it there | `yes` (B's head) |
| S17 | A and B reached by two routes with different meet routines; only the **second** pair has a commit | `yes` |
| S18 | `Send; OnIsolated(); L.Insert`, the isolated subscriber runs `Commit()` | `yes`, `isolated-subscriber` (class 1, T14) |
| S19 | `SendHttp (no write); OnIsolated(); L.Insert`, the isolated subscriber inserts | `no`; one `ownTransactionCommits` row (class 2, T07) |
| S20 | `Send; if Codeunit.Run(W) then; L.Insert`, W runs `Commit()` | `no`; W's commit is class 2; one `run-error-rolls-back-a` row |
| S21 | `Send; Page.RunModal(P); L.Insert` | `no`; one `runmodal-pending-write` error row |
| S22 | `Send; Rec.Insert(true); L.Insert`, `OnInsert` commits | `yes` through the trigger edge |
| S23 | `Send; L.Insert(true)`, the log table's `OnInsert` commits before its write | `no`, one `commitsInsideB` row |
| S24 | `Send; Commit; if not TryCheck() then; L.Insert`, the error is inside the used `[TryFunction]` | `yes`; the error is left out (trap below the region) |
| S25 | root: `if Codeunit.Run(Worker) then`; Worker: `Send; Commit; Check (errors); L.Insert` | `yes`; the error row stays, `swallowed-above-meet` |
| S26 | `Log2(Send(), Helper())`, Helper commits; A = `Send` by column, B = `Log2` | `yes`; with the arguments swapped, `no` (T11 order) |
| S27 | a line with two candidates, no column | refused, the candidates listed with columns |
| S28 | A in one `#if` arm of a routine, B in the other arm's row | `no`, `not-in-one-build` |
| S29 | A's route crosses a symbol-only dependency routine | `unknown`, naming it |
| DO-1 twin | `Deliver(): SetStatus; Commit; Transmit(); if Ok then Commit;` then back in the caller `if Bad then Error(…); UpdateCounter(); L.Insert` | A = `Transmit` call: `yes` (A's tail), the `Error` and `UpdateCounter`'s implicit errors in `errorsAfterCommit` |
| DO-2 twin | `PostMessage(): Commit; if Codeunit.Run(Worker) then;` then in the caller `Rec.Modify(); L.Insert` | A = the `PostMessage` call: `no`; 2 `commitsInsideA` rows (class 1, class 2); the class-1 row carries its own `errorsAfterCommit` (the `Modify`'s implicit error) and leads the output (§5.4, N7); the `Modify` also in `errorsBetween`. A = the deep site inside `Worker`: `yes` through `run-commit-of-A`, the `Modify` in `errorsAfterCommit` (N1) |
| S30 | `if Codeunit.Run(Sender) then; if Bad then Error('x'); L.Insert`, A inside `Sender` | `yes` (`run-commit-of-A`), the `Error` in `errorsAfterCommit` (N1) |
| S31 | `if TrySend() then; L.Insert`, TrySend (used `[TryFunction]`): A, `Commit`, `Error('x')` | `yes`; the `Error` is left out (trap on A's route below M, N2) |

The DO twins are public fixtures of the two acceptance shapes (the DO runs used a private copy
that is not pinned). The cap case cannot be reached by a small fixture; it is pinned by a unit
test that runs the route enumeration with a small edge budget (the budget is already a parameter
of `literal_routes`' search) and checks that the answer is `unknown` with the cap named.

One discrimination proof per correctness guard (principle 7, the repo's scope rule). Each break
is made with Edit, run with the narrowest test filter, and reverted:

| Break | The test that must fail |
|---|---|
| keep sibling arms in the region | S11 turns `yes` |
| cut at every B, not only a B that surely runs | S5c turns `no` |
| drop the cut | S5 turns `yes` |
| drop the back edge from the region | S5b turns `no` |
| drop the callee cone lookup | S3 turns `no` |
| drop event edges from the region | S6 turns `no` |
| drop operation-sited edges (triggers) from the region | S22 turns `no` |
| treat an always-leaving arm as falling through | S10 and S12 turn `yes` |
| drop the unresolved-site blocker | S13 turns `no` |
| drop the open-interface blocker | S13b turns `no` without `--closed-world` |
| skip `[CommitBehavior]` | S14 (`Ignore`) turns `yes` |
| class every commit as class 1 (ignore run edges) | S20 turns `yes` |
| class an isolated subscriber's `Commit()` as class 2 | S18 turns `no` |
| class an isolated event's own commit as class 1 | S19 turns `yes` |
| drop the L28 error row | S8 and S20 lose their `run-error-rolls-back-a` row |
| drop the `RunModal` error row | S21 loses its row |
| drop the error-after-commit list | S7 loses its error row |
| decide trap scope by `TrapVerdict::Caught` (root scope) | S25 loses its `swallowed-above-meet` row |
| ignore traps below the region | S24 gains the trapped error |
| drop A's tail or B's head | S15 or S16 turns `no` |
| check only the first route pair | S17 turns `no` |
| treat a cap as complete | the small-budget unit test turns `no` |
| fold `commitsInsideA` into the verdict | DO-2 twin turns `yes` |
| class a run edge on A's own route as class 2 (ignore A's side) | S30 turns `no` |
| ignore a trap on A's route below M | S31 gains the trapped error |
| cut at a B inside a trap scope | S5f turns `no` |
| drop `commitsInsideB` | S23 loses its row |
| order nested calls right to left | S26 swaps its answers |
| accept an ambiguous line | S27 answers instead of refusing |
| ignore `#if` alternatives | S28 turns `yes` |
| fall back to "every site" for a routine without a tree | S29 turns `yes` or `no` |
| route a commit from its callee alone (lose the root's literals) | a literal-dead commit appears; the §5.6 contract test fails |

Invariants over real output (CDO, in the gate's `output` phase): the two contracts of §5.6.

## 9. CDO check

The plan (the DO places of §2.5 are step 3):
1. **Population first** (principle 4). Before the answer kinds are fixed, count on CDO: workspace
   routines with a record write B and an earlier call site A in the same routine, where some call
   between A and B has a cone that may commit (direct, through a callee or a subscriber). The
   textual census in §2.3 says the direct shape is 0, so this count needs the engine. It is a
   small `aldump` census mode or a test-side scan; counts only.
2. **Reproduce the shape.** The print routine in §2.3 is the `no` case with B in a callee. The
   mail send and the eDocument send in the same code are candidates for the commit-inside and
   A's-tail cases. The private note lists their locations. Each answer is triaged by hand against
   the source.
3. **The DO cases.** The two places in §2.5 (engine findings that match the owner's description,
   not confirmed by the owner) are the acceptance cases, with the answers the DO twins in §8 pin.
   If either does not answer so, that blocks landing. The selectors and lines are in the private
   note, never in the repo. The CHANGELOG entry calls them "two places the engine finds in the DO
   workspace that match the owner's description; not confirmed by the owner", never "the owner's
   cases".
4. **No movement elsewhere.** `cdo-before-after` must show no change in `analyze` findings or
   fingerprints. This question adds output, it changes none.

## 10. Decisions and what is still open

Decided by the tools lane (the owner may overrule):
1. **Site naming: file and line, with an optional column, only.** Principle 13.
2. **Loops: the next B after A**, cut only at a B that surely runs (§4.2); a commit reached over
   the back edge is labelled `loop-carried`. Principles 1 and 6.
3. **Errors after B, before B's transaction commits: a follow-up task.** Card:
   `/work/.al-sem/tools-notes/ENG-165-followup.md`. Principles 3 and 35.
4. **Evaluation order inside one statement: measured** (T11). Principle 4.
5. **Runtime facts: measured** (§2.4, two rounds). Principles 4 and 6.
6. **The L28 error at a checked run has its own label**, `run-error-rolls-back-a`. Principles 6
   and 17.
7. (Revision 3) **Commits inside A or B are their own lists**, not a `yes` (§5.4). Principles 1
   and 17.
8. (Revision 3) **Commits are classed by route** (§4.3); only class 1 decides `yes`. Principles 1
   and 4.
9. (Revision 3) **Trap scope is relative to the region** (§5.2). Principles 1 and 5.

Still open:
- The Windows confirmation of §2.4, above all `Page.RunModal` and a report with a request page in
  an interactive client (owner, `kraken coord ask --kind other`, when the implementation records
  the facts in `docs/engine-limits.md`).

## 11. Review r2 (Fable): where each finding is answered

| Finding | Answer |
|---|---|
| B1 trap scope | §5.2 (trap relative to the region; `swallowed-above-meet`); §5.6 contract weakened as asked; S24, S25 and two proofs |
| B2 S8 stale; class by route | §2.1 S8 corrected to `no` + `run-error-rolls-back-a`; §4.3 classes by the route's edges; probed: a `Commit()` in an isolated subscriber is class 1 (T14, T16); S18–S20 and four proofs |
| B3 cut soundness | §4.2: cut only at a B that surely runs; across routes and recursion stated; S5c, S5d (S5e dropped, N5), S5f; proof "cut at every B → S5c no" |
| B4 RunModal | probed without a handler: an error with a pending write (T18 vs T19); §4.3 lists it as an error row; a request-page report is a possible error labelled unmeasured |
| B5 inside-A | §5.4: `commitsInsideA` / `commitsInsideB` lists, verdict unchanged; row order fixed; S23, DO-2 twin and proofs |
| S6 operation edges | §4.1 region includes trigger and table-event edges; S22, S23; proof |
| S7 no tree, `#if`, symbol-only | §5.5 `unknown` for no tree / site not in tree / symbol-only; §5.3 `not-in-one-build`; S28, S29; proofs |
| S8 literal bindings, per-route class, dead sites | §5.1 routes from the root's state graph through the region statement; §6 per-route class and `RunCleanCones`; §3 dead under literals |
| S9 returns, break | §5.3 `commitsWithoutB` includes returns and a `break` out of a loop that alone holds B; §4.2 agrees (N4) |
| Confirmation round (N1–N8) | N1 §4.3 (A's side of the run edge, S30, DO-2 twin deep); N2 §4.2 traps on the way (S31); N3 §4.2 must-run and trap scopes (S5f); N4 §4.2/§5.3; N5 S5e dropped; N6 §5.4; N7 §8 DO-2 twin; N8 §5.2 |
| S10 site naming | §3: ambiguous tails refused; candidate defined; column base and unit; parser for both forms with tests |
| S11 missing proofs, DO twins, S5 comment | §8 table and proofs extended; DO twins added; the fixture's S5 comment fixed (outside the repo) |
| scope: HTTP/FILE gating | dropped; `errorsBetween` is always listed (§5.2) |
| trees under `--low-mem` and demand | §7: read from the code, both keep `statement_tree`; a test with a proof is planned |

## 12. Principles applied

- 1 and 5: `no` only when every path was followed; caps, unresolved calls, missing trees and
  unmeasured constructs give `unknown` with the reason; the cut is made only where it is sound.
- 4: §2 measured what the commands answer before designing; §2.4 measured the runtime facts on BC
  (two rounds, the second because review showed the first's limits); §2.5 checked the design
  against the DO workspace (engine findings, not owner-confirmed); §9 measures the population
  first.
- 6 and 17: `commitsInsideA`, `commitsInsideB`, `commitsWithoutB`, `errorsBetween`,
  `ownTransactionCommits` and every label keep facts as labelled classes.
- 7 and 9: expected answers written first (S8 corrected); one discrimination proof per guard.
- 13 and 14: reuse `pending_writes`' walk and run-edge helpers, the cone's commit facts,
  `commit_scope`, `literal_routes` and the existing witness format; no new site language, no LSP
  UI.
