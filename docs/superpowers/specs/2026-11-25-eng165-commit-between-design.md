# ENG-165: can a COMMIT run between site A and site B?

Status: revision 2, spec only (no code). For review by Fable and sol. Task card:
`/coord/tasks/ENG-165/task.md`.

Revision 2: §10's questions 1, 2, 3 and 6 are decided (by the tools lane, principles named in
§10). Questions 4 and 5 are answered by measurement (§2.4: alc and the BC 28 Linux sidecar),
which changes §4.2 and §4.3. The owner's DO workspace was searched (§2.5, counts only).

## 1. The question

The owner investigated one codeunit's mail and eDocument bookkeeping on DO (2026-11-20). The
useful question was an ordered one:

> Between this delivery (a send) and its log entry (an `Insert`), can a `COMMIT` run?

Answering it by hand found two places where a sent mail or eDocument can lose its log entry.
The send happens and a commit makes it durable. Then an error stops the code before the
`Insert`. The send is not undone, and the log row is never written.

al-sem answers "does this routine commit?" and "where are its commits?". It does not answer
"does a commit run *after A and before B*?". This spec adds that question.

Words used below:
- **site**: one statement in the source: a call or a record operation, named by file and line.
- **route**: a call path from the root routine to a site, in the format `prove --site --routes`
  already prints (`witness: {hops, terminal}`).
- **cone**: everything a routine may do, its callees and event subscribers included.
- **trap**: something that catches an error: a checked `Codeunit.Run`, a `[TryFunction]` whose
  result is used, an isolated event.

## 2. What exists today (measured)

Principle 4: measured on a fixture and on CDO with master `c6f61f04`, `cdo` profile build.

### 2.1 The fixture

An alc-clean fixture (alc 18.0.41, no warnings), kept outside the repo at
`/work/.al-sem/engine-notes/eng165/fx/`. `Send()` inserts into `CB Outbox` (A is the call to
it). B is `L.Insert()` on `CB Mail Log`. One routine per shape:

| Routine | Shape | The right answer to "commit between A and B?" | `prove may-commit` today |
|---|---|---|---|
| S1Straight | `Send; Commit; L.Insert` | yes | yes (unconditional) |
| S2NoCommitBetween | `Send; L.Insert; Commit` | **no** | yes (unconditional) |
| S3CalleeCommits | `Send; Helper (commits); L.Insert` | yes, through a callee | yes, path `→ Helper` |
| S4Branch | `Send; if Flag then Commit; L.Insert` | yes, when `Flag` | yes, guard `Flag` |
| S5Loop | `for: Send; L.Insert; Commit` | **no** (the commit comes after this send's own log) | yes, guard "loop body" |
| S6Event | `Send; OnAfterSend; L.Insert`, a subscriber commits | yes, through a subscriber | yes, path `→ [event] HandleAfterSend` |
| S7CommitThenError | `Send; Commit; if Amount < 0 then Error; L.Insert` | yes, **and an error may run after the commit, before B** | yes (unconditional) |
| S8CheckedRun | `Send; if Codeunit.Run(W) then; L.Insert` | yes, the run's implicit commit | yes, "checked Codeunit.Run" (conditional) |
| S9Reversed | `L.Insert; Send; Commit` | **no** (B never runs after A) | yes (unconditional) |
| S10CommitThenExit | `Send; if Flag then begin Commit; exit end; L.Insert` | **no** path A→commit→B, but a commit after A on a path where B never runs | yes, guard `Flag` |
| S11ExclusiveArms | `if Flag then Send else Commit; L.Insert` | **no** (the commit is in the other arm) | yes, guard `not Flag` |

So `may-commit` says `yes` in all eleven. Five of those (S2, S5, S9, S10, S11) are a "no" for
the ordered question. `may-commit` cannot tell them apart, because it has no notion of order.

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
  loop around it. `program::body::control_flow` (`branch_termination`, `terminates`) says which
  branches always leave by `exit` or by an error. `ordering.rs` has the same ideas over
  `orderId`/`frameId` (`may_co_execute`, `ordered_before`, `dom`).
- **Commits in callees.** The cone keeps a COMMIT fact per root. ENG-09 makes a checked
  `Codeunit.Run` a commit of its own (`commit_scope::is_run_commit`, resource `checked-run`).
  `commit_scope` applies `[CommitBehavior]` (`Ignore` removes a commit, `Error` turns it into
  an error) and `InherentCommitBehavior`.
- **Routes.** `literal_routes.rs` enumerates distinct routes from a start state to a site, with
  literal arguments followed, caps and a complete/not-complete flag. `prove_slice.rs` prints
  them with guards. This is the evidence format to reuse.
- **Errors and traps.** `terminal_facts.rs` (`TerminalSite`) holds the explicit and implicit
  error sites. `literal_reach` decides traps (`TrapAt`, `TrapVerdict`, engine limits B7).
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
(only a status check runs between A and B). Locations are in the private note
`/work/.al-sem/engine-notes/eng165/cdo-shapes-PRIVATE.md`.

### 2.4 Runtime facts, measured (revision 2)

alc 18.0.41 compiles every shape below without a warning. The runtime facts were measured on the
**BC 28 Linux sidecar** (`http://bc`, instance `BC`, platform 28.0.54946.56477), 2026-11-25,
with a test codeunit that raises its results through `Error('MEASURED …')`. Pending-write
oracle (ENG-09): a checked `Codeunit.Run` inside a `[TryFunction]` fails if and only if a write
is pending. Probe source and results: `/work/.al-sem/engine-notes/eng165/bcprobe/` (the app was
unpublished afterwards).

| Test | Shape | Result |
|---|---|---|
| T01 / T02 | `Insert`; `Insert; Commit` (controls) | pending / none pending |
| T03 | caller `Insert`, then `Page.RunModal` (the test's modal handler presses OK) | no error; still pending |
| T04 | `Page.RunModal` of a page whose `OnOpenPage` inserts | still pending |
| T05 | caller `Insert`, then `Report.RunModal` (processing-only, no request page) | no error; still pending |
| T06 | a report that inserts in `OnPreReport` | still pending |
| T07 | nothing pending, isolated event, the subscriber inserts | none pending; the row is kept (committed) |
| T08 | nothing pending, isolated subscriber inserts then errors | the caller goes on; the row is gone (rolled back) |
| T09 | caller `Insert`, then an isolated event whose subscriber inserts | no error; still pending |
| T10 | nothing pending, plain event, the subscriber inserts (control) | pending |
| T13 | caller `Insert`, then an isolated subscriber inserts and errors | the error is **not** trapped: it stops the caller |
| T11 | evaluation order | `Log(Send())`: Send, Log. `Log2(Send(), Send2())`: Send, Send2, Log2. `if Send() then Log`: Send, Log |

Scope of these results: one shape each; pages under the test framework's modal handler, a report
without a request page, not an interactive client. Not decided: in T09, whether the isolated
subscriber's own row commits apart from the caller's pending write (the answer does not depend
on it: the caller's write stays pending either way).

**Windows rule (repo CLAUDE.md).** A fact measured on BC 28 on Linux is confirmed on a Windows
Cronus before it is recorded as a platform fact. Until then the rules in §4.2 and §4.3 rest on
the Linux measurement and the spec says so; the engine-limits entry the implementation writes
carries the label "BC 28 Linux; Windows confirmation pending" and the probe app's location, so
the owner can rerun it.

### 2.5 The owner's DO workspace (revision 2; counts only)

A clean DO checkout (`b3b4459`, 2026-10-05) was searched. Its source is private: no names or
text here. The owner's session was on a later revision; this one holds the same code shapes.
- Textual census (§2.3's script): 5,665 routines, 56 with a direct `Commit()`, 0 with send →
  commit → insert in one routine. A second census for "a delivery call, then a log call, in one
  routine" found 8 routines (CDO: 8), 7 with other calls between the two (CDO: 5), **0 with a
  direct `Commit()` between** (CDO: 0).
- Engine runs (copy of the workspace with the CDO baseline's dependency packages, same
  versions; in the CDO slot). On the eDocument dispatch routine of the codeunit that matches the
  owner's description, `may-commit` answers `yes` with 9 distinct commit sites. Placed by the
  first call on their path in that routine: 6 lie behind the call that sends the document, 3
  behind the calls that finalize and log the delivery, **0** behind the calls between those two.
- **The first place, found.** The send goes down into a dependency app.
  There the routine sets the document's status, commits, makes the delivery call, and **on
  success commits again**, then returns. Back in the workspace routine, two explicit error calls
  (and the implicit errors of the record operations between) can run before the call that
  writes the log entry. That is the owner's shape: sent, committed, then an error before the
  log `Insert`.
- **The second place, found** (by reading the source; the engine run is the next bullet).
  The mail send goes through System Application's `Email.Send`, which runs `Commit()` and then a
  checked `Codeunit.Run` of its dispatcher (its own transaction). Back in the workspace, a
  handled-field update (a record `Modify`) runs between the send and the log call; its errors can
  stop the code before the log entry.
- Engine run on the mail routine: `may-commit` answers `yes` with 3 commit rows. Two sit behind
  the call that sends (both at the `Email.Send` crossing: the explicit commit and the checked-run
  commit, `resourceId: checked-run`). One sits behind the log call itself. **0** sit behind the
  record update between them. So both commits that make the send durable run *during* A when A
  is the workspace's send call, and the danger is the errors of the update between A and B.

**Would the design find them?** Yes, both, with these rules (added in revision 2):
- A site may sit in dependency source, named by the path the evidence prints (`dep:<app>:<path>`
  or its tail). The eDocument case names A as the delivery call inside the dependency: the
  commit after it lies in **A's tail** (§4.1), and the error calls lie in the meet routine's
  middle, so `yes` with two `errorsAfterCommit` rows.
- When A is named as a **call** whose own cone commits (the workspace's call to the send
  routine, or to `Email.Send`), those commits run *during* A, before or after the effect inside
  it. They are reported as `yes`, labelled `inside-A`, ranked after the plain rows, and the
  output says which deeper site to name to order them. A may-question's `yes` is a static
  candidate, and the label says what kind (principle 6). The mail case answers this way: the
  `Commit()` and the dispatcher's run-commit are inside A, and the record `Modify`'s errors are
  `errorsAfterCommit`.
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
- A site is named as `--site` already names one: a source file or its tail, and a 1-based line
  (`SiteFilter`). Add an optional column for a line that holds two statements.
- A site may be **any** call site or record operation (not only a fact of the question's kind),
  in the root routine or in any routine the root reaches, workspace or dependency source (named
  by its `dep:<app>:<path>` evidence path or a tail of it).
- A line that holds no call or operation, or more than one with no column given, is refused.
  The message names the candidates with their columns (principle 5: never guess a site).
- A or B that the root cannot reach (guards ignored) is refused with the reason. An answer about
  a site that never runs would be empty, not "no".

**Rejected:** an operation selector such as `insert:"Mail Log"` or `call:Send`. One routine can
hold several, and the owner's question was about one specific send and one specific log line.
File and line is what an investigation has in hand. Principle 13: no second way to name a site
until a use needs it (decision 10.1).

**Rejected:** `commit-between:<A>..<B>` as one question string. Paths hold `:` already, and a
site is a filter like `--site`, not part of the question's name.

## 4. What "between" means

An execution of the root **runs A, and later runs B, and runs a commit C after A and before B**.

### 4.1 The meet routine

A and B may sit in different routines. Take a route to A and a route to B (each from
`literal_routes`). They share a prefix. The **meet routine** M is the last routine on that shared
prefix. In M, A's route leaves at call site `a` (or `a` is A itself, when A is in M), and B's
route leaves at `b` (or `b` is B).

The **between region** has three parts:
1. **A's tail.** In each routine on A's route below M: the statements that may run after the call
   toward A (or after A itself), up to the routine's end. Example: a commit later in `Send`
   itself, after the outbox insert.
2. **M's middle.** In M: the statements that may run after `a` and before `b`.
3. **B's head.** In each routine on B's route below M: the statements that may run before the
   call toward B (or before B itself). Example: a commit at the start of the log routine.

A commit C is between A and B when it is **in** the region: a direct `Commit()` there, or a
commit in the cone of a call or an event raised there.

Several routes to A or to B give several meet routines. Every pair is checked, up to the route
cap. A cap that fires leaves the answer `unknown`, never `no` (principle 5).

### 4.2 Order inside one routine

"May run after X and before Y" is computed on the routine's control-flow tree, as
`pending_writes` computes "may run before". The rules, each pinned by a fixture:
- **Blocks.** Statements after X and before Y in the same block are in. So are the conditions
  on the way down to Y.
- **Branches.** Only the branch that holds X (going up) and the branch that holds Y (going down)
  count. A sibling arm of either is out (S11). An `if` or `case` **between** X and Y counts
  with all its arms (S4).
- **Branches that leave.** An arm between X and Y that always ends by `exit` or by an error
  (`branch_termination`) never reaches Y. Its commits are not between A and B (S10). They go to
  the second list (§5.2).
- **Loops: the next B after A.** B means **the first run of B after A**, so a path is cut at B's
  first run. In `for: Send; L.Insert; Commit` (S5) the commit comes after this iteration's B, so
  the answer is `no`. In `repeat L.Insert; Send; Commit until …` the first B after A is the next
  iteration's, so the answer is `yes`, labelled `loop-carried`. A loop around both A and B
  counts its whole body only through the back edge, and only up to B's first run.
- **A and B in the same statement.** The arguments run first, left to right, then the call; an
  `if` condition runs before its branch. Measured (§2.4, T11): `Log(Send())` runs `Send` then
  `Log`; `Log2(Send(), Send2())` runs `Send`, `Send2`, `Log2`; `if Send() then Log(…)` runs
  `Send` then `Log`. So a call in an argument or a condition is ordered before the statement's
  own call. The walk orders a statement's nested calls that way: an argument's callee cone is
  "before" the call that takes it. A site named by a column picks the nested call. (BC 28 on
  Linux; see §2.4 for the Windows rule.)

### 4.3 What counts as a commit

| Kind | Source | In the answer |
|---|---|---|
| `Commit()` | direct, or in a callee's or subscriber's cone | `commit`, with `[CommitBehavior]` applied (`Ignore`: not a commit; `Error`: an **error** site, listed under §5.2's errors) |
| checked `Codeunit.Run` | ENG-09's `run-commit` | `run-commit`: it commits the run's own transaction. If a write may be pending at the run (A wrote, as `Send` does), BC raises an error at the run instead (L28, measured). Then it is listed as an error, not a commit |
| a commit inside an object a call starts (`Page.RunModal`, `Report.Run`, …) | the existing cone, with `commit_scope`'s "may apply" reading | `commit`, labelled as reached through an object run |
| `Page.RunModal` / `Report.RunModal` itself | measured (§2.4, T03-T06) | **not a commit and not an error.** The caller's pending write stays pending, and so does a write the page or report makes itself. Only a `Commit()` inside the object counts (row above) |
| an isolated event (`[IntegrationEvent(…, true)]`) | measured (§2.4, T07-T09, T13) | with **no** write pending at the raise: each subscriber's writes are committed and its error is trapped (rolled back, the caller goes on). With a write pending: the caller's write stays pending, and a subscriber's error is **not** trapped. So it never commits the caller's work: `isolated-commit`, own transaction only (below) |
| the commit at the end of the transaction | outside the root | out of this question. The root's callers decide it. The answer says "the root's callers are not followed"; the follow-up task covers it (§10.3) |

**Two classes of commit.** Only a commit that ends the **caller's** transaction makes A's pending
write durable: a `Commit()` that runs normally. A checked `Codeunit.Run` (ENG-09) and an isolated
event commit only their own transaction; a write pending at them stays pending (L28: the run
raises an error; T09: the event leaves it pending). So `yes` rests on the first class only. The
second class is listed as `ownTransactionCommits`, labelled: it matters when A is durable without
a commit (an external send, an `HttpClient.Send`, a file write), because then any later commit is
not what makes A durable, and the risk is an error after A in any case. An `errorsAfterA` list
(errors after A, before B, with no commit needed) is shown when A's kind is such a non-
transactional effect: one whose cone holds HTTP or FILE, as the digest's
`IO_BEFORE_ESCAPING_ERROR` already reads them. Whether a mail send through System Application's
`Email.Send` reaches HTTP in the cone (its connectors are interface implementations) is checked
on CDO during implementation, not assumed.

## 5. The answer

Three answers, as every prove question has.

### 5.1 `yes`: a route where a commit can run

Each evidence row holds:
- `after`: A's site and one route from the root to A.
- `commit`: C's site, its kind (`commit`, `run-commit`, through an object run, through an event),
  and its route **from the point in the region where it starts**. The route is the existing
  `witness {hops, terminal}`, so the commit in S6 reads `OnAfterSend → [event] HandleAfterSend`.
- `before`: B's site and one route to B.
- `guards`: the conditions on the way, as `prove_slice` shows them (selected context, not every
  needed condition).
- `labels`: `loop-carried`, `through-callee`, `through-event`, `through-object-run`,
  `inside-A` (§2.5: a commit in A's own cone when A is a call).
- `errorsAfterCommit`: the error sites that may run **after C and before B**, each with its
  route and guards, and whether a trap catches it. **This is the dangerous case** (S7): the
  commit made A durable, and this error stops the code before B. An error a definite trap
  catches on every route is left out, as prove does for `throws-error` (B7). The traps are
  stated.

The human output leads with the dangerous rows: commits with at least one error after them.

`--routes N` gives up to N distinct commit routes per commit site, as for `--site`.

### 5.2 `no`: proved, no commit can run between A and B

Every path from A to the next B was followed, no commit is in the region, and every obligation
is clear. `no` keeps two facts that matter to the owner's real risk (principle 17: keep every
fact, as a labelled class):
- `relation: "b-never-after-a"` when B never runs after A at all (S9). The answer is still `no`,
  and it says why.
- `commitsWithoutB`: commits after A on paths that leave before B, with how they leave (`exit` or
  an error) and their routes (S10). A commit then an error that never reaches B is the same
  risk as S7. These rows appear with every answer kind, not only with `no`.

### 5.3 `unknown`: something could not be followed

The blockers say what. Unknown is not no (principle 5):
- a call site in the region that no edge leaves, or a dynamic `Codeunit.Run(Id)`;
- an interface call in the region with an implementation that may not be loaded (waived only
  under `--closed-world`, as prove does today);
- event subscribers beyond the loaded ones (the existing `loaded-subscribers` assumption: the
  answer carries the label, as `may-commit` does);
- a route or state cap fired;
- a runtime fact not measured: a construct outside §2.4's shapes whose commit behaviour the
  answer depends on (named, never guessed).

### 5.4 Consistency with `may-commit` and `throws-error`

- `may-commit` is `no` ⇒ `commit-between` is `no`, with the same obligations. This is the cheap
  first step.
- `commit-between` is `yes` ⇒ every commit it lists is a `may-commit` evidence site, or sits in
  the cone of one. A contract test checks this on the fixture and on CDO.
- Every row in `errorsAfterCommit` is a `throws-error` site of the same root. A contract test
  checks this too.

## 6. Cost

- The region walk is linear in the size of each routine on A's and B's routes.
- Commits in callees come from the cone the prove plan already builds: one lookup per call in the
  region. A commit's route inside a callee uses `literal_routes` from that callee only.
- Error sites come from the same terminal records `throws-error` reads.
- The question needs a level-3 read (the control-flow trees alive), as the `--site` slice does
  today. It declares that in its `ViewNeeds` (the `declared-needs` skill): trees, site guards and
  rules, nothing new in the default build.
- Expected cost on CDO: one prove run plus the walk; no new resident memory. The implementation
  measures it (`cdo-before-after`, instructions) before landing.

## 7. Consumers

- **CLI:** the form in §3. JSON is the prove document with a new `commitBetween` block.
- **MCP:** `alsem_prove` already turns its arguments into the CLI's argv. It gains `after` and
  `before` arguments. Answers stay byte-equal to the CLI (existing contract tests).
- **LSP:** `al-call-hierarchy/prove` passes argv through the engine child, so it gets the question
  without new code. A code action ("is there a commit between here and …") is not part of this
  task (principle 13).

## 8. Tests and discrimination proofs

The expected answers in §2.1's table are written down before the code runs (principle 9). The
fixture moves into `tests/cli/` as one test file with the AL inline, in the style of
`cli_commit_sites_up_chain.rs`. More shapes:
- **S5b**: `repeat L.Insert; Send; Commit until …` → `yes`, `loop-carried`.
- **S12**: `Send; if X then begin Commit; Error('x') end; L.Insert` → `no`, with one
  `commitsWithoutB` row that leaves by an error.
- **S13**: an unresolved call between A and B (a dynamic `Codeunit.Run(Id)`) → `unknown`, naming
  the site.
- **S14**: a helper with `[CommitBehavior(Ignore)]` between → `no`. With `(Error)` → `no` commit,
  one error row.
- **S15**: A inside a callee with a commit after it in the same callee (A's tail) → `yes`.
- **S16**: B inside a callee with a commit before it there (B's head) → `yes`.
- **S17**: A and B reached by two routes with different meet routines, one with a commit → `yes`.

One discrimination proof per correctness guard (principle 7, the repo's scope rule). Each break
is made with Edit, run with the narrowest test filter, and reverted:

| Break | The test that must fail |
|---|---|
| keep sibling arms in the region | S11 turns `yes` |
| drop the "next B after A" cut | S5 turns `yes` |
| drop the callee cone lookup | S3 turns `no` |
| drop event edges from the region | S6 turns `no` |
| treat an always-leaving arm as falling through | S10 and S12 turn `yes` |
| drop the unresolved-site blocker | S13 turns `no` |
| skip `[CommitBehavior]` | S14 (`Ignore`) turns `yes` |
| drop the error-after-commit list | S7 loses its error row |
| drop A's tail or B's head | S15 or S16 turns `no` |

Invariants over real output (CDO, in the gate's `output` phase): every `commit-between` commit
is a `may-commit` site or in the cone of one, and every `errorsAfterCommit` row is a
`throws-error` site (§5.4).

## 9. CDO check

The plan (the DO places of §2.5 are step 3):
1. **Population first** (principle 4). Before the answer kinds are fixed, count on CDO: workspace
   routines with a record write B and an earlier call site A in the same routine, where some call
   between A and B has a cone that may commit (direct, through a callee or a subscriber). The
   textual census in §2.3 says the direct shape is 0, so this count needs the engine. It is a
   small `aldump` census mode or a test-side scan; counts only.
2. **Reproduce the shape.** The print routine in §2.3 is the `no` case with B in a callee. The
   mail send and the eDocument send in the same code are candidates for the `yes` case through
   callees and subscribers. The private note lists their locations. Each answer is triaged by hand
   against the source.
3. **The DO cases.** The two places in §2.5 are the acceptance cases. The eDocument case, A named
   as the delivery call inside the dependency, must answer `yes` (A's tail) with the two explicit
   error calls among its `errorsAfterCommit` rows. The mail case, A named as the call to the send,
   must answer `yes`, `inside-A`, with the record `Modify`'s errors among its rows. If either does
   not, that blocks landing. The selectors and lines are in the private note, never in the repo;
   the owner confirms they are the two he found (one line in coord, numbers only).
4. **No movement elsewhere.** `cdo-before-after` must show no change in `analyze` findings or
   fingerprints. This question adds output, it changes none.

## 10. Decisions (revision 2) and what is still open

Decided by the tools lane (the owner may overrule):
1. **Site naming: file and line, with an optional column, only.** No operation selector until a
   use needs one. Principle 13.
2. **Loops: the next B after A**, and a commit reached only through the next iteration is
   labelled `loop-carried`. It pairs a send with its own log; "any later B" would say `yes` for
   S5, where every send already has its log. Principles 1 and 6.
3. **Errors after B, before B's transaction commits: a follow-up task**, not a recorded limit.
   Card: `/work/.al-sem/tools-notes/ENG-165-followup.md`. Principles 3 and 35.
4. **Evaluation order inside one statement: measured** (§2.4, T11; alc accepts every shape).
   Arguments left to right, then the call; a condition before its branch. §4.2 uses it.
   Principle 4.
5. **Runtime facts: measured** (§2.4). `Page.RunModal` and `Report.RunModal` neither commit nor
   raise an error with a write pending. An isolated event commits its subscribers' writes and
   traps their errors only when no write is pending; with one pending it commits nothing of the
   caller's and traps nothing. §4.3 uses these. They rest on BC 28 on Linux until a Windows Cronus
   confirms them (§2.4). Principles 4 and 6.
6. **The L28 error at a checked run gets its own label**, `run-error-rolls-back-a`: BC raises
   the error at the run, so A's pending write is rolled back with everything else, and the log
   loss does not happen through that path. It is listed among the errors, never as the dangerous
   kind. Principles 6 and 17.

Decided by the tools lane in revision 2, for the reviewers to check: commits inside A's own cone,
when A is a call, answer `yes` labelled `inside-A` (§2.5), not `unknown`. A may-question's `yes`
is a labelled static candidate, and both DO places are of this shape when A is named at the
workspace level. Principles 5 and 6.

Still open:
- The Windows confirmation of §2.4 (owner, `kraken coord ask --kind other`, when the
  implementation records the facts in `docs/engine-limits.md`).
- Whether System Application's `Email.Send` reaches HTTP in the cone (decides `errorsAfterA` for
  the mail case, §4.3); checked on CDO during implementation.

## 11. Principles applied

- 1 and 5: `no` only when every path was followed; caps, unresolved calls and undecided runtime
  facts give `unknown` with the reason.
- 4: §2 measured what the commands answer before designing; §2.4 measured the runtime facts on
  BC instead of guessing them; §2.5 checked the design against the owner's own workspace; §9
  measures the population first.
- 6 and 17: `commitsWithoutB`, `b-never-after-a` and `loop-carried` keep facts as labelled
  classes instead of dropping or hiding them.
- 7 and 9: expected answers written first; one discrimination proof per guard.
- 13 and 14: reuse `pending_writes`' order walk, the cone's commit facts, `commit_scope`,
  `literal_routes` and the existing witness format; no new site language, no LSP UI.
