# Callers through dependency code: design

Date: 2026-11-06. Status: **revision 2**, the owner's decisions on r1 and both reviews applied
(§14 maps every review finding to what changed). Base: `master` at `5c05092b`.
Inputs: the prototype `proto/dep-callers` (`5cca3e07`) and its report
(`target/tmp/dep-callers-measurement.md`); the caller-relations work on master
(`reverse::CodeCalls`, `object_run_reaches`, `query touches` up's `up.relation`, query schema
3.4.0); the d9 statement-order fix on master and the literal gap it recorded; the r1 reviews
(`target/tmp/dep-callers-review-fable.md`, `target/tmp/dep-callers-review-astra.md`, both
"approve with changes").

## 0. Summary of decisions

| # | Question | Decision |
|---|---|---|
| 1 | One graph or two relations | **Two relations, one existing walk.** The solve graph stays as it is. The second relation, **callers through dependency code**, is not a new kept structure: it is R0's existing reverse walk over the sited graph, restricted to code-call sites (`reverse::CodeCalls`), run per root with a visitor that records each state's parent and marks the states whose path passed dependency code (§3.2). It composes transitive callers natively, across any number of crossings between workspace and dependency code. It never feeds a summary, the SCC, the solver or `DbEffectQuery`. *Upward* questions may read it; *downward* questions read the cone. A kept all-pairs structure is a phase-4 option only. |
| 2 | Response engines | **An explicit, engine-owned, versioned catalog of configured-dispatch sites**, applied as a **site condition** (`configuredDispatch` in the sited graph's conditions mask), not a `Boundary` variant. v1 holds one entry: Base App `Workflow Response Handling::ExecuteResponse`, its `case WorkflowResponse."Function Name"` arm labels, arm bodies and `else` body. Matched on app id, full routine signature and the normalized scrutinee; a missing or ambiguous shape never falls back to the whole body and is reported. **The catalog labels; it never hides**: a caller reachable only through a configured site is listed, with its configured hop, ranked after the others, and never turns any answer into "no". A structural detector is kept as a discovery probe. |
| 3 | Literal pruning through dependency code | **Not in v1.** Passing values across events is its own milestone, before transaction spans, with a conservative may-write rule for `var` event parameters (§5). |
| 4 | Evidence | Every listed caller carries the **full witness** from the same walk that found it: the caller, every dependency routine, every event by name, each hop's kind and site, ending at the **queried routine**. Never a bare "X calls W". |
| 5 | Detectors | **No detector reads the new relation in v1**, and `alsem analyze` does not build the sited graph at all. Transaction spans (d9/d8/d50) are phase 4, as a transaction-aware traversal of their own (§7). d54 through the cone is the next small, separate design; d1 is not planned. |
| 6 | Cost, refresh, export | No new kept structure: the walk reads the sited graph (on CDO ~1 s and 45 MiB to build, an artifact a resident revision already keeps and rebuilds), and keeps O(states) per root only while it runs. `alsem graph` gains the condition name `configuredDispatch` on the catalogued edges (additive; it already exports `conditions`). |
| 7 | Honesty | One definition of **dependency code** by app identity (§1), shared with the solve graph's split. The list is labelled "potential callers; path feasibility not checked". Zero listed callers is never a whole-program absence. `up.relation` keeps `"code-calls"`; `up.throughDependency` has its own relation string, with `notApplicable` (single-app), `notComputed` (the graph did not build) and `limited` (a walk budget hit) stated apart. |
| 8 | Tests | Fixtures: a workspace -> dependency -> workspace callback through an event with **both** a source-folder dependency and a package (`.app`) dependency; a configured-dispatch hub; transitive composition across workspace hops. Invariants: R0's workspace-only code-call callers equal the solve graph's (by the shared predicate), the list is disjoint from `up.transitiveCallers`, every witness is a path of code-call sites ending at the root, a code-tier caller has no configured hop. One discrimination proof per guard. |
| 9 | v1 | Three commits: the condition with its catalog and the probe; the walk and `query touches` up's block (query schema 3.5.0); explain's callers line under `--callers-context`. No detector, summary, solver, prove verdict or other export change. |

## 1. Terms

- **Solve graph**: the Primary payload of `ExecRelation` (`PrimaryGraph`): the calls workspace
  routines make, plus event dispatch from any publisher. The db-effect solver, `pd_solve`,
  `DbEffectQuery`, the SCC and most detectors read it. A dependency routine with source is a
  fixed leaf there, holding its own direct effects; the graph's transitivity stops at it.
- **Cone**: the Cone payload of `ExecRelation` (`Vec<TypedEdge>`): the workspace half's typed edges
  and then the dependency half's. `prove`, digest and the capability cone read it. It **already
  follows dependency code downward**, including through the workflow engine.
- **Sited graph**: the may-execute graph with one site per call (`impact_graph::sited`), built from
  every body the program parsed, dependency bodies included (`ProgramBody::edge_view`), not from
  the demanded model. R0 walks it backwards.
- **Code-call relation**: what the solve graph follows upward (calls, object runs and the triggers
  a run runs, record triggers, event dispatch; no user-started page trigger, no open site).
  `reverse::is_code_call_site` names it over the sited graph; `reverse::CodeCalls` is its visitor
  adapter. On workspace code R0 over code-call sites equals the solve graph's callers exactly
  (`WaitForMailLimit` 964 = 964, `caller_agreement_tests`).
- **Dependency code**: a node of the sited graph whose **app is not the workspace (primary) app**:
  `SitedGraph::node_app(n) != primary_app` (`dep_callers::is_dependency_node`). This is the line
  `ExecRelation` draws (`Role::Dependency` = any app other than the primary app). It is **not**
  R0's `Tier::Local`, which is a trust-tier line (an app loaded from local source is `Local`
  there, on purpose: a test universe's production app). This feature never reads `Tier`; its
  visitor computes the bit itself (§3.2), so a source-folder dependency and a package dependency
  are both dependency code.
- **Caller through dependency code** of W: a workspace routine X that is **not** a transitive
  caller of W by code calls of workspace code (`up.transitiveCallers`), but from which some path
  of code-call sites reaches W **and passes at least one dependency-code node**. The path may
  cross between workspace and dependency code any number of times, so the callers compose
  transitively: `WaitForMailLimit`'s two real callers enter the workspace at
  `OnBeforeSendCustomer`, six workspace hops before the root, and are still its callers. The
  first hop from workspace into dependency code is **any** code-call edge: a call, an object
  run, a record operation firing a dependency table trigger, or event dispatch to a dependency
  subscriber.
- **Configured dispatch**: a site whose set of targets is chosen at runtime by configuration data
  (rows a user or an admin sets up), not by the code or the caller's arguments, but whose arms the
  resolver sees as ordinary static calls. Base App's workflow response dispatch is the case on CDO.
  Configured is a **label** on a real code path, not a statement that the path cannot run.
- **Witness**: one concrete path that shows a claim, with every hop, taken from the same walk
  that made the claim.

## 2. What was measured

Workspace: the pinned CDO baseline `/home/sshadows/Projects/DO-cdo-baseline/Cloud` (`bc3ccb18`),
passed as an argument only. Binary: `proto/dep-callers` plus measurement-only cuts (branch
`proto/dep-callers-measure`, worktree `.claude/worktrees/depmeasure`, uncommitted; target dir
`target-depmeasure`, release-fast). Scratch: `target-depmeasure/scratch/`. Machine load average
47-135 during the runs (other agents), so times are noisy.

The measurement build added two env switches to the prototype's `dependency_shortcuts`:
`ALSEM_PROTO_DEP_CALLERS_CUT_ROUTINES="Obj::Routine,…"` (the named dependency routine keeps its node
but loses its successors) and `ALSEM_PROTO_DEP_CALLERS_CASE_K=<k>` (a structural rule: in a
dependency routine, a `case` with at least k non-else branches that each make a resolved call, to at
least k distinct callees; that case's call edges are dropped). The dump (`…_DUMP`) was made to honour
the cuts, so a witness never goes through a cut routine.

### 2.1 Reproduction

`alsem analyze --deterministic --scope primary`, switch off vs on: d1 +20 / 457 changed, d54 +1,
d9 8 changed, nothing removed. 628 shortcuts, 66 callers, 51 targets. Same as the report.

### 2.2 Candidate hub rules

| Variant | shortcuts | callers / targets | d1 added / changed | d54 added | d9 changed |
|---|---:|---:|---:|---:|---:|
| none (raw prototype) | 628 | 66 / 51 | 20 / 457 | 1 | 8 |
| cut objects `Workflow Management`, `Workflow Response Handling` | 166 | 66 / 51 | 2 / 2 | 1 | 8 |
| **cut one routine: `Workflow Response Handling::ExecuteResponse`** | **166** | 66 / 51 | 2 / 2 | 1 | 8 |
| structural, k = 30 (matches 1 case: `ExecuteResponse`) | 166 | 66 / 51 | (same) | | |
| structural, k = 20 (3 cases) | 166 | | | | |
| structural, k = 12 (9 cases) | 166 | | | | |

- Cutting the one routine gives exactly the object cut's result: the same 166 shortcuts (the 148
  distinct name pairs are set-equal) and byte-identical `analyze` output. So the whole hub is the
  response dispatch; the rest of the workflow engine adds nothing.
- 462 of the 628 shortcuts exist only through it (the report's "487 with the shortest witness
  through it" counts 25 that have another path).
- **All reviewed true positives were retained**: the three findings the report judged true (d1
  `LoadFiles` x2, d54 `ShowMigrationNotification`) stay, and the two true callers of
  `WaitForMailLimit` (the "Send" action -> `Document Sending Profile::Send` -> `OnBeforeSend` ->
  `CDO eDocs Sending Profile Subs::OnBeforeSendCustomer`) stay. (The cut was a measurement
  device; r2 does not cut, it labels, §4.)
- The structural rule removes nothing more on CDO, but what it matches besides `ExecuteResponse` is
  ordinary code dispatch on a parameter or a type field, which a structural rule must never treat as
  configuration:

| k = 12 match | arms | scrutinee | configured? |
|---|---:|---|---|
| `Workflow Response Handling::ExecuteResponse` | 35 | `WorkflowResponse."Function Name"`, a row read by `Get` | **yes** |
| `Workflow Response Handling::AddResponsePredecessors` | 25 | parameter `ResponseFunctionName` | no (setup code) |
| `Workflow Event Handling::AddEventPredecessors` | 19 | parameter `EventFunctionName` | no |
| `Default Dimension::UpdateGlobalDimCode` | 20 | parameter `TableID` | no |
| `Inventory Posting To G/L::SetAccNo` | 17 | `InvtPostBuf."Account Type"` (a buffer parameter) | no |
| `Reservation Management::SetReservSource` | 15 | the source record's table | no |
| `CSC Activation Wizard (Cloud)::EnableControls` | 15 | page step | no |
| `FA Insert G/L Account::GetGLAccNoFromFAPostingGroup` | 12 | parameter `FAPostingType` | no |
| `Page Management::GetConditionalCardPageID` | 12 | `RecRef.Number` | no |

### 2.3 What is left after the hub cut (166 shortcuts)

By witness length: 60 of at most 6 hops, 52 of 7-12, 54 of more than 12 (max 48). None passes
the workflow engine. Grouped by what the witness shows:

| Group | Example (dumped witness) | Judgement |
|---|---|---|
| Posting wrappers into Base App posting, back into CDO subscribers (`CDO Sales-/Purch.-/Service-Post and Handle::PostAndHandle`, 39 shortcuts) | `Sales-Post (Yes/No)::OnRun -> Code -> RunSalesPost -[codeunit-run]-> Sales-Post::OnRun -> RunWithCheck -> FinalizePosting -> OnAfterFinalizePosting -[event]-> CDO Subscribers::OnAfterFinalizePostingSalesPost` (7 hops) | **real, the main value** |
| Continia Core callbacks (Feature Management, Continia Hub, App Information, Assisted/Manual Setup registration, ~35) | `CSC Feature Management::EnableFeature -> OnAfterEnableFeature -[event]-> CDO Core Event Handler::OnAfterEnableFeature` (2 hops) | **real** |
| Base App document actions (`Document Sending Profile::Send`, `Reminder-Issue`, page post actions) | `Sales Invoice Header::SendRecords -> Document Sending Profile::SendCustomerRecords -> Send -> OnBeforeSend -[event]-> CDO eDocs Sending Profile Subs::OnBeforeSendCustomer` (4 hops) | **real** |
| Record-trigger cascades (28 through `Sales-/Purch.-Calc. Discount`, 27 through the Reservation page's `OnGetQtyPerUOMFromSourceRecRef`) | `CDO File::CalcSalesDisc -> Sales-Calc. Discount … -[implicit-trigger]-> Sales Line::OnDelete -> Catalog Item Management::DelNonStockSales … Item::OnDelete -> RunCostAdjustment … Gen. Jnl.-Post Line::PostVend -> OnBeforeVendLedgEntryInsert -[event]-> CDO Subscribers::OnBeforeVendLedgEntryInsert` (29 hops) | **noise**: each hop is a code path, but the chain needs data states that do not occur together. The Reservation hop is guarded in the subscriber by `if MatchThisTable(SourceRecRef.Number)` (`Service Line-Reserve.Codeunit.al:498`), which the literal search could decide only with event binding and a predicate helper (§5). |
| Dialogs inside posting (customer from template opens the Customer Card; CRM re-runs an assisted setup) | `Release Sales Document::Code -> Sales Header::CheckCustomerCreated … Contact::ShowResultForCustomer -[page-run]-> Customer Card …` (19 hops) | possible but rare |

Lesson: even without the hub the long tail (more than 12 hops) is mostly noise. This is why v1 keeps
the relation out of every detector and shows it only as a labelled caller list with witnesses,
where a reader judges each path.

### 2.4 d9's RunESealServiceSetup case, corrected

The report's hand-check row 10 blamed statement order (the call after the Commit). The d9 fix on
master showed the growth (5 -> 18 routines) comes from new **callers** of `RunESealServiceSetup`:
`EnableeDocsFeature` -> `CSC Feature Management.EnableFeature` (dependency) -> `OnAfterEnableFeature`
-> `CDO Core Event Handler::OnAfterEnableFeature` -> `OnAfterEnableCDOFeature`'s `case FeatureCode
of … ESealFeatureCodeLbl: RunESealServiceSetup`. The feature code passed is
`GetCDNeDocsFeatureCode()` (`exit(CDNEDocsFeatureCodeLbl)`), so the eSeal arm cannot run from there.
This is the literal gap of §5, and it is why transaction spans must not read the relation before
the event-binding milestone (§5.3).

### 2.5 Not measured here

Time, memory, prove: the report's numbers stand (no measurable cost; 0 of 464 prove verdicts move;
the north star does not read `ExecRelation`). The prototype's own step took 0.8-2.5 s in these
runs on the loaded machine (the report: 130-270 ms on a less loaded one). r2 does not build that
step at all (§3.2); v1's cost is the sited graph and one walk per question, measured with the
implementation (§8).

### 2.6 Two caller counts

`aldump --caller-diff` reports `solveLocal 964`; `query touches` up on the same root reports
966 of 967 transitive callers. Both read the solve graph. `up.transitiveCallers` counts **every**
ancestor with a summary row, and the solve graph's ancestors include dependency routines: a
dependency publisher whose dispatch reaches a workspace subscriber is a solve-graph node (event
dispatch from any publisher is in the Primary payload). `solveLocal` counts only workspace
routines with a stable id and a body. The through-dependency block sits beside
`up.transitiveCallers` and is disjoint from it by construction (§3.2); the implementation
re-measures the split of 967 on CDO and records it (§8).

## 3. Decision 1: relation design

### 3.1 Why not one graph

The prototype adds X -> W edges to `primary.edges_by_from`. `DbEffectQuery::ancestors`, the
reverse call graph, the SCC, the solver **and the detectors' own traversals** read that map, so
every edge also acts downward. That is what moved d1 (457 changed findings, 10
severity/confidence changes), and it is wrong in kind, not only noisy:

- d1 and d54 walk `ctx.graph` themselves (`d1_graph.rs`, `d54.rs`), each with a deliberate event
  policy (d1 does not follow event dispatch; d54 stops at publishers). A synthetic shortcut edge
  changes that traversal and hides the event crossing inside it, so the detector follows events
  it was written not to follow. Summary inheritance is the second effect, not the first.
- The cone already holds X's effects through dependency code, with typed edges, the dependency
  routines' own effects and the literal search over them. A via-dependency edge copies a subset of
  that into the solve summary with less fidelity: parameter binding is `Unknown` along it and no
  literal pruning applies.
- The solve graph's transitivity is restricted by design: a dependency routine with source is a
  fixed leaf holding its own direct effects (`effect_query.rs`; the U-scoped query is
  "informative" only because of that). Folding the relation in silently changes what every
  summary and every detector traversal means.

No consumer measured needs the downward half. So it is not built.

### 3.2 The relation: R0's code-call walk, per root

v1 builds no new structure. The relation of root W is **R0's reverse walk from W over the sited
graph, wrapped in `reverse::CodeCalls`**, with one new visitor
(`src/program/impact_graph/dep_callers.rs`). `aldump --caller-diff --code-calls` already runs
this walk and printed the 86 `wsNonLocal` callers with witnesses; v1 makes it a product.

The visitor:

- **Marks dependency code with its own consumer bits.** `ConeVisitor::step` returns the state's
  `ext` bits (`reverse.rs`, "the consumer's own bits"). The visitor sets
  `DEP` when the state's node is dependency code (§1: `node_app != primary_app`) and keeps it
  along the path, and `CONFIGURED` when the site carries the `configuredDispatch` condition (§4).
  R0's `Tier` is left as it is for its other consumers; this relation does not read it.
- **Records every state's parent**: per state, the state it was reached from and the site, at
  its shortest hop count (the walk hands each state once). O(states) per root, dropped after the
  answer is rendered. A path is rebuilt from the caller's state back to the root.
- **Classifies each workspace routine** (a routine node of the primary app with a stable id; the
  `#if` arms of one stable id are one routine) by the best state that reached it:
  - reached by a state without `DEP` -> an ordinary code-call caller (the solve graph's;
    not listed here);
  - else reached by a `DEP` state without `CONFIGURED` -> a **caller through dependency code**,
    tier `code`;
  - else (every state that reached it has `CONFIGURED`) -> a caller through dependency code, tier
    **`configured`**: listed, with its configured hop, after the `code` tier.
  A code path always beats a configured one, and a workspace-only path beats both, so a caller is
  counted once, in its best class. Within a class the shortest path is the witness.
- **Disjointness** with `up.transitiveCallers` is enforced, not assumed: `query touches` removes
  any routine that is also a solve-graph ancestor of W (by stable id) before it lists. On
  workspace code the two definitions agree exactly (invariant 1, §10.2), so nothing should be
  removed; a removal would mean the agreement broke, and the agreement test fails first.
- **Budget**: the walk's state budget and cancellation apply (`WalkOptions`). A walk that hit
  one says so (`limited`), and its list is then a lower bound, never complete.

Why a walk and not a kept structure: v1's two consumers (`query touches` up, explain's callers
line) are per-root questions, and the walk composes transitive callers natively (the r1
structure's first-workspace-endpoint pairs did not, review astra 1 / fable 4). The sited graph is
already an artifact of a resident revision (`ArtifactKey::Sited`), rebuilt per revision, so v1
needs no refresh rule, no cache key and no agreement test against a second implementation.

**Demand is not involved.** The sited graph is built from every parsed body's edges, not from the
demanded model, so the walk sees all dependency code the program parsed. (r1 cited the 86 = 86
agreement as proof that demand suffices; both sides read the same program, so it proves nothing
about demand. Where the model is involved, for names in the output, demand's own forward rule
covers it: every dependency routine on a path from a workspace routine is forward-reachable from
that routine, and demand follows event flow from a reached publisher into its subscribers and on
from them, `resolve::demand`.)

**Declared need**: the jobs that render the block declare `ViewNeeds::sited` (the existing
M0 path; `query touches` up with `--from`, cross-app; explain with `--callers-context`), and the
engine host's pin rule (`alsem.rs`' `reads_sited`) names them. Single-app declares nothing and
answers `notApplicable: "single-app"`.

A **kept all-pairs structure** stays a phase-4 option, decided when transaction spans need it and
measured then (the one real argument for it is `alsem analyze`, which builds no sited graph
today).

### 3.3 Which consumer reads which relation

| Consumer | Question direction | Reads today | v1 | Later |
|---|---|---|---|---|
| `query touches` up (`--from`, every case) | up | solve graph (`code-calls`) | solve graph **plus** a separate `throughDependency` block from the walk | |
| `query touches --field --from` up | up | R0 **may-execute** walk, tiered (`field_scope`, no `CodeCalls`) | unchanged | |
| explain card, callers line | up | `model.calls` direct callers | with `--callers-context` (the sited graph is built): the through-dependency count and the nearest callers | |
| explain `--callers-context` section, prove's callers' questions | up | R0 sited graph (already follows dependency code) | unchanged | the configured label on routes (Q2) |
| `alsem tests`, impact B1/B2 up | up | R0 may-execute walk (already follows dependency code) | unchanged (they do not read conditions) | rank below code-call tests by the exported condition (Q3) |
| transaction spans (d9, d8, d50, digest's tx context) | up (callers of the Commit routine) | solve graph reverse walk | unchanged | phase 4: a transaction-aware traversal (§7) |
| `DbEffectQuery::ancestors`, solver, `pd_solve`, SCC | down / structure | solve graph | **never** | never |
| d1 (loop -> op) | down | solve graph | unchanged | not planned |
| d54 (TryFunction -> publisher) | down | solve graph, no event edges | unchanged | next, its own design: traverse dependency calls, stop at the dependency publisher, carry dependency hops in the evidence |
| prove, digest, capability cone | down | cone (already through dependency code) | unchanged | label configured routes, never cut (Q2) |
| `alsem graph` export | both | sited graph | the `configuredDispatch` condition on catalogued edges (additive) | |

The rule behind the table: **the relation answers "who reaches W"; it never answers "what does X
do".** A consumer asking the second question reads the cone.

## 4. Decision 2: configured dispatch

### 4.1 Why the workflow engine is different

Most configuration-driven dispatch in AL already runs objects by a runtime id, which the engine
treats as open or honest-dynamic and never as a static edge:

| Mechanism | Shape | Already open? |
|---|---|---|
| Job Queue (`Job Queue Start Codeunit`) | `Codeunit.Run(Rec."Object ID to Run", Rec)` | yes, runtime id |
| Report Selections, Guided Experience, notifications' actions | `Report.Run(Sel."Report ID")`, run by object id / codeunit id + method name | yes, runtime id |
| Posting preview (`Gen. Jnl.-Post Preview::Preview`) | `OnRunPreview` event; each subscriber checks the `Subscriber` variant | not config: an event fan-out discriminated by an argument (literal territory, §5) |
| Approvals (`Approvals Mgmt.`) | raises workflow events; its own `case RecRef.Number` | the events enter the workflow engine; the `case` is G33's domain |
| **Workflow responses** | `case WorkflowResponse."Function Name" of DoNothingCode(): …; PostDocumentCode(): PostDocument(Variant); …` (35 arms) plus an `OnExecuteWorkflowResponse` event in the `else` | **no: every arm is a static call** |

The workflow engine is an interpreter: which response runs after which event is a program written
in table rows (`Workflow Step Instance`), so any workflow event "may" post any document. The
resolver is right that each arm is a code path; what is false is the composition of an arbitrary
event with an arbitrary response.

### 4.2 Rule: an explicit, engine-owned catalog

`src/program/impact_graph/configured_dispatch.rs`, one entry per site, **engine-owned in v1**:

```rust
pub const CATALOG_VERSION: &str = "1";
pub struct ConfiguredDispatch {
    pub name: &'static str,        // "Workflow Response Handling::ExecuteResponse"
    pub app_id: &'static str,      // "437dbf0e-84ff-417a-965d-ed2bb9650972" (Base Application), any version
    pub app_name: &'static str,    // "Base Application" (reported, not matched)
    pub object: (ObjectKind, &'static str),     // (Codeunit, "Workflow Response Handling")
    pub routine: &'static str,     // "ExecuteResponse"
    /// Full signature, normalized: (var?, type text folded, quotes and runs of spaces removed).
    pub params: &'static [(bool, &'static str)],
    // [(true, "variant"), (false, "record workflow step instance"), (false, "variant")]
    /// The case's scrutinee as a folded, unquoted member path.
    pub scrutinee: &'static [&'static str],   // ["workflowresponse", "function name"]
    pub why: &'static str,         // "responses are chosen by Workflow Step Instance rows"
}
```

- **Stable identity.** The app is matched by its id (Base Application's guid does not change
  between versions); the object by kind and folded name; the routine by folded name **and** its
  full signature (each parameter's `var` and normalized type text), not an arity. The matched
  app's name, publisher and version are reported with the match.
- **Scope: one `case`.** Inside the matched routine, the one `case` statement whose scrutinee is
  the entry's member path, compared folded and unquoted (`WorkflowResponse."Function Name"`
  matches `workflowresponse."function name"`). The configured region is that case's **arm labels,
  arm bodies and `else` body** (the labels are calls too: `DoNothingCode()`, `PostDocumentCode()`,
  constant-returning; the `else` raises `OnExecuteWorkflowResponse`, the custom-response event,
  just as configured). The routine's other calls (`OnBeforeExecuteResponse`, telemetry) stay
  unlabelled. Verified against Base App 28.1.49838.50268 (`ExecuteResponse(var Variant: Variant;
  ResponseWorkflowStepInstance: Record "Workflow Step Instance"; xVariant: Variant)`, case at
  line 381, `else` event at 453; review fable 9). The entry keys on routine + scrutinee because the
  same scrutinee text appears in `GetDescription` (line 1148), which is not dispatch.
- **No fallback.** A matched routine with no such case, with more than one, or whose case shares
  a source line with code outside it (the region is decided by lines, so a shared line would be
  ambiguous) is a **mismatch**: nothing is labelled, never the whole body, and the mismatch is a
  diagnostic. So is an app present with the object or routine missing, or a signature that
  differs. An app that is not loaded at all is `notLoaded`, not a mismatch.
- **Reported everywhere it applies.** Every output that can carry the label carries the
  catalog version and each entry's status (`matched` with the app version, `notLoaded`, or
  `mismatch` with the reason): `query touches` up's block, the probe, explain's line when it
  shows a configured count. A Base App rename or reshape is therefore loud in normal output, not
  only in a probe.
- **User extension later** (`.al-sem.json` `configuredDispatch`, same entry shape): entries
  classify, they never suppress. Not in v1; it needs only the loader, since the version, the
  status and the probe exist.
- Why not a structural rule: at k = 12 it marks 8 cases that are ordinary dispatch on a parameter
  or a type field (§2.2). Its threshold is arbitrary (an ISV dispatcher with 10 arms escapes k = 12;
  Base App's `Default Dimension` dispatch is caught). A rule that reads the scrutinee's provenance
  ("a field of a record filled by a database read in the same routine") would match
  `ExecuteResponse` but also `GetDescription` and every `case SalesLine.Type of` after a
  `FindSet`, which is transaction data, not configuration. The difference is one of meaning; a
  human decides it.
- The structural rule stays as a **discovery probe**, `aldump --configured-dispatch-candidates <ws>
  [--min-arms k]` (default 12): every `case` in dependency code with at least k arms that make a
  call, with its routine, scrutinee text, arm count and distinct callees, and whether the catalog
  labels it; first the catalog's version and each entry's status. Whoever extends the catalog runs
  it first. (r1's "pairs a cut would remove" column is dropped: r2 does not cut, and the column
  would need a walk per candidate.)

### 4.3 What a configured site means

- **A site condition, not a boundary.** The projection sets condition `configuredDispatch` in the
  site's conditions mask (`sited::CONDITIONS`) for every site whose caller is a matched routine
  and whose anchor's line lies in the configured region. The site's `Boundary` (`Sync`, `Run`,
  `EventDispatch`, …), `via` and confidence are unchanged: a configured arm is a real call in the
  caller's transaction, and `callers_context`'s per-boundary steps keep working.
- **R0's walks are unchanged.** `is_code_call_site` still keeps the site (it is a resolved arm),
  and the may-execute walks (`alsem tests`, impact, `--field --from`, `--callers-context`) do not
  read conditions. Only the through-dependency visitor reads it, to set its `CONFIGURED` bit.
- **The catalog labels, never hides.** A caller reachable only through a configured site is
  listed (tier `configured`), with the configured hop on its witness (entry name, arm label,
  response routine), ranked after every `code` caller, and counted apart. The human text calls
  these "configuration-dependent potential callers (including custom responses)", so a developer
  who wrote an `OnExecuteWorkflowResponse` subscriber does not read a missing caller as a bug.
  Nothing in v1 turns a configured path into "no".
- **The export carries the name** (`alsem graph` already writes each edge's `conditions`): the
  catalogued edges gain `configuredDispatch`. The export's collapse rule unions conditions of
  equal-confidence sites on one `(caller, callee, kind)`, so an edge carries the name when any of
  its sites is configured. bc-dev-mcp may rank a test found only through such edges lower; it
  never excludes one for it (Q3).
- **Approval workflows** are configured by rows too, but Microsoft ships the templates and most
  tenants enable them. That is why the tier is listed with its arms, not counted away (review
  fable 8).

## 5. Decision 3: literal arguments through dependency code

### 5.1 What the literal search can do today

`engine::l5::literal_reach` (G23/G33/G34): literal Booleans on guard parameters, the table a record
argument holds (`case RecRef.Number of Database::"T"`, `if R.Number <> Database::"T" then exit`),
RecordRef/Variant flow. It binds parameters across direct, variable-typed and interface calls only;
**events, triggers and object runs bind nothing.**

### 5.2 What the measured noise needs

| Case | Needed | Present? |
|---|---|---|
| CDN feature code (§2.4): `EnableFeature(…, GetCDNeDocsFeatureCode())` -> `OnAfterEnableFeature(var AppCode, var FeatureCode)` -> `OnAfterEnableCDOFeature(var FeatureCode)` -> `case FeatureCode of ESealFeatureCodeLbl: …` | (a) a Code/Text/Label/enum value domain with `case X of <Label>` guards; (b) folding a function whose body is `exit(<literal or Label>)`; (c) event binding: publisher argument -> subscriber parameter, through two events; (d) a `var` parameter is sound only if no subscriber of that event assigns it (`var_assignments`) | none of (a)-(d) |
| Reservation fan-out: `Page Reservation::UpdateReservFrom` -> `OnGetQtyPerUOMFromSourceRecRef(SourceRecRef, …)` -> `Service Line-Reserve` subscriber `if MatchThisTable(SourceRecRef.Number) then` | (c) event binding of a RecordRef; (e) a predicate helper (`MatchThisTable(n): exit(n = Database::"Service Line")`) evaluated as a guard | G34 has the table domain; (c), (e) missing |
| Approvals `PreventDeletingRecordWithOpenApprovalEntry` from a Requisition Line `OnDelete` (in hub witnesses) | trigger binding of `Rec` | G33 handles it in the cone search; not in the relation's walk |
| Workflow engine | none would help: the event code reaches `SetRange("Function Name", FunctionName)`, a database filter; the response is chosen by rows | the catalog (§4) is the only answer |

So applying today's literal search to the relation's walk would remove almost none of the measured
noise. Closing the gap is the extension, not the application.

Known limits that no literal work removes, stated so nobody files them as bugs: Base App's
`OnBefore…(…, IsHandled)` pattern (a path through the code after `if IsHandled then exit` is
feasible only if no subscriber handles it, which is undecidable statically); interface dispatch
inside dependency code to a workspace implementation is conditionally resolved (the witness hop
says `interface`; the enum value is not shown in v1).

### 5.3 Decision: passing values across events is its own milestone

Settled (Q6): **event argument binding is an independent milestone, shared with prove, scheduled
before transaction spans** (phase 4). Its contract, revised after review astra 4:

- **The conservative rule for `var` parameters.** A direct `var_assignments` check is not enough:
  a subscriber can rewrite a `var` argument through a call (`Subscriber1(var Code):
  Rewrite(Code)`), and an unknown subscriber, a nested event or a builtin can too. A literal
  bound at the publisher reaches a subscriber's `var` parameter only when **no subscriber of that
  event, and nothing any subscriber calls, may write it**: a may-write / escape analysis over every
  bound subscriber and its callees, where an unknown body, an unbound or manual subscription, an
  opaque builtin taking the argument by reference, or passing it on as `var` degrades to
  *unknown* (never to the literal). The milestone starts with **by-value** event parameters and
  `var` parameters proven immutable by that rule, and adds adversarial mutation fixtures (the
  two-subscriber case above; a rewrite through a helper; a rewrite in a nested event) before any
  pruning is enabled.
- **Over-pruning test** (review fable 12): CDO's `OnAfterEnableFeature` subscriber guards
  `if AppCode <> GetProductCode() then exit` on a `var` parameter. With binding, the rule above
  must look at every subscriber of `OnAfterEnableFeature` for a write to `AppCode`, or it would
  kill the TRUE arms (`CDNEDocsFeatureCodeLbl`). The fixture carries this shape.
- **The value domain** (Code/Text/Label/enum) states AL's rules before it compares anything: Code
  values are upper-cased and trimmed on assignment, a longer text is truncated or errors by
  target length, a `Label` may be localized (a `Locked = true` label is a constant, another one is
  not), and enum identity is the enum's value, not its caption. Constant-returning routines
  (`exit(<literal or Label>)`) become a body fact under a declared need.
- **No monotonicity promise.** r1 said prove verdicts "can only move toward `no`". A finer
  binding state can hit the state cap where the coarser one did not and lose precision, so the
  milestone measures prove verdict moves on CDO in both directions and explains each.
- (a)+(b) and (e) (predicate helpers) follow, each with a fixture and a CDO measure.
- Then, and only then, the through-dependency walk may apply the search; a caller whose every
  witness is literal-infeasible moves to a third count, `literalDead`, with the guard that kills
  it. Over the cap it stays a caller (never dropped silently). The search must run over the
  whole witness, workspace hops included: the literal fixture's `Other()` sits in the workspace
  subscriber, not in the dependency segment (review astra 1).
- Expected effect on CDO (estimate, to verify): the eSeal arm leaves `RunESealServiceSetup`'s
  callers (d9's 5 -> 18 growth goes away); the 27 Reservation paths go if `MatchThisTable` is
  decided. The Calc. Discount cascades (28) need record-state reasoning (which lines exist) and
  stay.

## 6. Decision 4: evidence

- The witness is the walk's own path: the caller's best state, then each recorded parent back to
  the root, so it comes from the same graph and the same walk as the claim (the measurement's
  first dump did not, and showed workflow paths the cut graph no longer had). It ends at the
  **queried routine** W, never at the first workspace routine it enters (r1's `entersAt`); the
  entry point is still shown (`entersAt`: the first workspace routine after the last dependency
  hop).
- Per hop: the routine it goes to; its kind (`call`, `interface`, `run` — an object run, the
  object node and its lifecycle step merged into one hop to the trigger the run runs —,
  `recordTrigger`, `event` with the publisher by name); the site's anchor where it has one (unit
  and 1-based line; dependency units name their app); and, on a configured hop, the catalog entry
  and the arm label. Routine names come from the model (cross-app, it holds every dependency
  routine on a path from a workspace routine, §3.2); a node without a model row prints the sited
  graph's own display.
- Hop count is the rendered hop count. Callers are sorted by tier (`code` first), then hops, then
  name.
- Rendering: JSON always has every caller and every hop. Human output folds a witness to its
  first 3 and last 3 hops (`… N more …`), and a list folds callers whose path is over 12 hops into
  a count ("N more with paths over 12 hops — --all lists them"). The bound is presentation, never
  a cut (Q4).
- A configured caller prints the configured hop: the catalog entry and the arm it went through.
- When a detector later uses the relation (phase 4), the finding's `evidence_path` holds a step per
  dependency hop (`EvidenceStep` already carries `routine_id` and `source_anchor`; dependency
  routines have ids in the cross-app model), with the note `through dependency code` on the first
  and the event name on each dispatch. The report's row 5 (an evidence line saying
  `Codeunit.Run(Sales-Calc. Discount)` "calls" a purchase-line subscriber) is the failure this
  forbids.

## 7. Decision 5: detector impact and triage plan

### 7.1 Expected moves

| Detector | Raw prototype (CDO, primary) | With the catalog | Design for it |
|---|---|---|---|
| d1-db-op-in-loop | +20, 457 changed | +2 (`LoadFiles` x2, info, temporary records), 2 changed | not a caller question; no change in v1; not planned (2 low-value true positives against 18 noise; its question needs loop-body semantics the cone does not carry) |
| d54-publish-in-tryfunction-cone | +1 (true) | +1 (true) | downward; next, its own small design (Q5): traverse dependency calls, stop at the dependency publisher, render dependency hops in the evidence |
| d9-transaction-span-summary | 8 changed | 8 changed (catalog does not touch them) | the one real caller question; phase 4 after the event-binding milestone, as a transaction-aware traversal |

v1's rows are all "0": no detector reads the walk, and `alsem analyze` does not build the sited
graph (its plan declares no `sited`), so its output cannot move. CDO acceptance checks it
byte-for-byte at both scopes.
| d8, d50 (spans) | 0 | 0 | with d9 |
| all others | 0 | 0 | |

### 7.2 Triage before any detector default changes (CLAUDE.md rule)

For phase 4 (transaction spans):

1. Build with the relation feeding spans behind an opt-in flag (`analyze --span-callers
   through-dependency`, name to taste); default off.
2. Run on CDO at `--scope primary` and `--scope all`; list every d9/d8/d50 finding that moves.
3. Triage every moved finding against source (the `triage-findings` skill): is each added routine
   really in the transaction that ends at that Commit? Record TP/FP with the witness.
4. More than 30% FP on the sample: stays opt-in, and the root cause is fixed (likely a missing
   literal case or a Commit on the dependency path) rather than shipped.
5. Measured base for comparison: today 8 of 8 changed d9 findings come with the relation; the one
   checked (RunESealServiceSetup) is a false positive of the literal gap. Phase 4 starts only after
   the event-binding milestone removes it (and that one success is not taken as transaction
   soundness).
6. Detector-sensitive fixtures come with the first detector that reads the relation (review
   astra 8): a transaction seed whose span gains a through-dependency member, and the unchanged
   cases next to it (an upstream loop for d1, a TryFunction-to-publisher path for d54), asserting
   those findings do not move. In v1 no detector can read it, so there is nothing for such a
   fixture to discriminate.

**Phase 4 is a transaction-aware traversal of its own, not a flag on a witness** (r1's
`commit_on_path: No | Maybe` is withdrawn, review astra 5). A boolean read off one displayed
witness cannot decide membership:

```text
X: WriteA(); D();
D: Commit(); W();
W: Commit();
```

For W's Commit, X's `WriteA` is not pending. Treating "a Commit on the path" like a Commit inside
X (include X, stop at its callers) would wrongly keep X's prefix. And one witness is not every
route: a side call before the next witness hop may commit, a checked `Codeunit.Run` is a boundary
without an explicit Commit, one path may commit while another does not, and the shortest path
need not be the transaction-preserving one. Phase 4's design must:

- walk with **site order** (which statements of each routine on the path run before the call that
  continues it: the d9 statement-order fix's facts, extended into dependency bodies);
- track nested effects and the execution boundaries that end a transaction (Commit, a checked
  run, a background start);
- consider every route, distinguishing **proven separation** (every route commits before W),
  **possible continuity** (some route does not) and **unknown continuity** (a route through
  unknown code);
- compute pending writes and coverage over the same qualified routes as membership.

It can use R0 per seed with consumer bits, since spans are already a per-seed backward walk
(`transaction_spans.rs`), or the kept structure if the cost on `analyze` says so (§3.2).

## 8. Decision 6: cost, memory, refresh, export

- **Kept memory: none new.** The walk keeps its states and parents only while it answers
  (O(states) for one root). What it reads is the sited graph, which a one-shot `query touches`
  up now builds (on CDO ~1 s and 45 MiB, measured for M0) and a resident revision keeps as
  `ArtifactKey::Sited` once a question reads it. The catalog's matches add a few dozen condition
  bits and one small per-site arm table to it. The resident budget is not changed. `--low-mem`
  needs nothing special: the sited graph build already re-parses dependency trees on demand.
- **Time**: the sited graph build plus one walk per root. Measured with the implementation on
  CDO and recorded in the CHANGELOG: one-shot `query touches` up wall time and peak RSS with and
  without the block, the walk's own time and state count.
- **Refresh**: none to design. The sited graph is rebuilt per revision by the existing artifact
  rule, and the walk runs per question. r1's per-change-type table and its closure cache are
  withdrawn, together with the cache-key gap review astra 6 found (a body edit can demand a new
  dependency routine without moving the package generation).
- **Export** (`alsem graph`, read by bc-dev-mcp): the `configuredDispatch` condition appears in
  the `conditions` of the catalogued edges (§4.3). Additive under `al-sem.callgraph` v1's rule
  (new values are additive; a consumer ignores unknown names), documented in the callgraph spec.
  Nothing else in the export changes. Ranking by it is bc-dev-mcp's decision, made with its
  parser change.

## 9. Decision 7: honesty

- `query touches` up keeps `up.relation: "code-calls"` and its counts exactly as today. The new block
  is apart, named, and its own relation string says what it follows and what it leaves out.
- **Potential, not proven.** The block says `feasibility: "notChecked"` and the human line says
  "potential callers … path feasibility not checked": a witness proves graph reachability, not a
  realizable execution (the record-trigger cascades of §2.3 are real code paths that need data
  states that do not occur together).
- **Zero is not absence.** The block covers code-call paths only; open sites, hubs and what a user
  starts on a page are outside it (`alsem tests` and impact follow them). The `follows` string
  says so, and the human line for zero says "none by code calls through dependency code".
- Every count comes with the configured count, the catalog version and each entry's status, so
  "0 callers through dependency code" is never read as "nothing reaches it through Base App".
- Four states, never a false 0: `notApplicable: "single-app"`; `notComputed` with the reason when
  the sited graph did not build; `limited` (`stateBudget` or `cancelled`) when the walk stopped,
  the list then a lower bound; otherwise the counts. An undeclared read panics (the existing
  `ViewCx::sited` rule).
- The human `callers:` line changes from "…nor a path through dependency code" to name the second
  list: "paths through dependency code are listed apart below".

## 10. Decision 8: tests, invariants, fixtures

### 10.1 Fixtures

New `tests/fixtures/dependency-callers/`: a workspace `ws/` and a dependency `dep/`. The
dependency carries Base Application's app id and name, so the **production** catalog entry is the
one under test (no test-only injection to drift from it). The same `dep/` source is loaded two
ways: as a **source folder** (`ws/al-sem.dependencies.json` -> `../dep`, trust tier local source)
and as a **package** (`.app` with embedded source, minted at test time into a copy's
`.alpackages`). Every expectation below holds in both, which is what pins the one definition of
dependency code (§1): R0's `Tier::Local` would call the source-folder dependency local.

| Part | Shape | Pins |
|---|---|---|
| `callback` + transitive composition | ws `Caller.Start` calls dep `Engine.Run` -> dep publisher `OnAfterRun` -> ws subscriber `Sub.HandleAfterRun` -> ws `Mid.Step` -> ws `Target.Target` (writes table `DC Log`); ws `Direct.CallTarget` calls `Target` | `Caller.Start` is a `code`-tier caller of `Target` (it enters at `Sub`, two workspace hops before the root), its witness names `OnAfterRun` as an event hop and ends at `Target`; `Direct.CallTarget`, `Mid.Step` and `Sub.HandleAfterRun` are ordinary code-call callers and not in the block |
| first hop by record trigger | ws `Toucher.Touch` inserts a dependency table whose `OnInsert` calls `Engine.Run` | `Toucher.Touch` is a `code`-tier caller; its first hop is `recordTrigger` |
| `configured` hub | dep codeunit `Workflow Response Handling`, `ExecuteResponse(var Variant: Variant; ResponseWorkflowStepInstance: Record "Workflow Step Instance"; xVariant: Variant)` with `WorkflowResponse.Get(…); case WorkflowResponse."Function Name" of PostCode(): PostIt(Variant); … else OnExecuteWorkflowResponse(…)`; `PostIt` raises a publisher a ws subscriber handles, which calls `Target`; ws `Approver.Approve` reaches `ExecuteResponse` | `Approver.Approve` is listed, tier `configured`, after every `code` caller, with the configured hop (entry, arm label `PostCode()`); the catalog status is `matched`; the export's `ExecuteResponse -> PostIt` edge carries `configuredDispatch`; the probe lists the case as catalogued |
| catalog mismatch | the same hub with the signature changed (a copy of the fixture) | status `mismatch` with the reason; nothing labelled (no whole-body fallback); `Approver.Approve` becomes a `code`-tier caller |

The literal fixture (§5) belongs to the event-binding milestone and is not part of v1.

### 10.2 Invariants (each can fail)

1. **Agreement by the shared predicate.** On the fixture (both dependency forms): the workspace
   routines the walk reaches by a state without `DEP` equal the solve graph's transitive callers of
   the root that are workspace routines; and the block's list is disjoint from
   `up.transitiveCallers`. Break: compute `DEP` from `Tier` (R0's trust-tier line) instead of app
   identity -> the source-folder form loses `Caller.Start` from the block.
2. **Composition.** `Caller.Start` is listed for `Target`, not only for `Sub`. Break: stop the
   visitor at the first workspace state after a dependency state (r1's segment relation) -> it
   disappears.
3. **The first hop is any code-call edge.** `Toucher.Touch` is listed. Break: let the visitor
   accept only `call` sites into dependency code -> it disappears.
4. **The catalog labels, never hides.** `Approver.Approve` is listed with tier `configured`; a
   `code`-tier caller's witness has no configured hop; a `configured` caller's witness has at least
   one. Break: skip the `CONFIGURED` bit -> `Approver.Approve` reads `code`; or drop configured
   states -> it disappears.
5. **Witness validity.** Every witness is a chain of code-call sites of the sited graph: hop i's
   site goes from hop i's routine toward hop i+1's (an object run merged with its lifecycle step),
   the first routine is the caller, the last is the root. Checked over every listed caller of the
   fixture and, under `cdo-gate`, `WaitForMailLimit`'s.
6. **Catalog matches loudly.** On the fixture and on CDO the v1 entry matches exactly one routine
   (`matched`, with the app version); the mismatch copy reports `mismatch` and labels nothing.
7. **Nothing else moves.** `alsem analyze` byte-identical at both scopes on CDO; prove answers
   unchanged on a sample; north-star stats unchanged; every golden family other than the query
   (up), explain (`--callers-context`) and graph-export goldens byte-equal.

Each discrimination proof is made with Edit, run with the narrowest command, reverted with Edit,
both outcomes recorded (CLAUDE.md).

### 10.3 Goldens

v1 moves only: the `query touches` up goldens (`tests/cli-query-goldens/`: every up answer gains
the block and the new `callers:` wording), explain goldens that use `--callers-context` (if any),
and the graph-export goldens if a fixture has a catalogued site (none should). Every other family
must stay byte-equal; `scripts/check-goldens --regen` then `scripts/check-goldens` confirms it.

## 11. Mock-ups

The shapes are the proposal; the implementation's goldens hold the exact text. The counts 966,
2 and 84 and the 4-hop dependency witness are measured (§2, the report); line numbers (shown as
0) are illustrative.

### 11.1 `query touches` up, human (CDO, `WaitForMailLimit`)

```
--- Codeunit CDO Send Mail Management::WaitForMailLimit vs CDO E-Mail Log (…) ---
  scope: per operation, one routine into dependency code; …
  callers: by code calls of workspace code (calls, object runs and the triggers a run runs, record
    triggers, event dispatch); not what a user starts on a page a caller opens (alsem tests and
    impact follow it); paths through dependency code are listed apart below
  up: 966 transitive callers inherit this touch through this routine: …
  through dependency code (potential callers by code calls, path feasibility not checked): 2 more
    workspace callers; 84 more only through configured dispatch (Workflow Response
    Handling::ExecuteResponse: PostDocumentCode() 80, …) — --callers lists them
    +11 Page CDO UnhandledPostedSalesInv.::SendCustomAction -> Table Sales Invoice
      Header::SendRecords -> Table Document Sending Profile::SendCustomerRecords -> … 5 more …
      -> … -> WaitForMailLimit
    +11 Page CDO Unhandled Posted Cr. Memo::SendCustomAction -> …
```

### 11.2 `query touches` up, JSON (additive, query schema 3.5.0)

```json
"up": {
  "scoped": true,
  "relation": "code-calls",
  "transitiveCallers": 966,
  "...": "unchanged fields",
  "throughDependency": {
    "relation": "code-calls-through-dependency",
    "engine": "reverse-walk",
    "follows": "code calls (calls, object runs and the triggers a run runs, record triggers, event dispatch) that pass dependency code, any number of times, ending at this routine; not open sites, hubs or what a user starts on a page",
    "feasibility": "notChecked",
    "callers": 2,
    "configured": 84,
    "catalog": {"version": "1", "entries": [{"name": "Workflow Response Handling::ExecuteResponse", "status": "matched", "app": "Base Application", "appVersion": "28.1.49838.50268"}]},
    "limited": null,
    "list": [{
      "routine": {"display": "SendCustomAction", "objectDisplay": "Page CDO UnhandledPostedSalesInv.", "...": ""},
      "tier": "code",
      "hops": 11,
      "crossedEvent": true,
      "entersAt": "Codeunit CDO eDocs Sending Profile Subs::OnBeforeSendCustomer",
      "witness": [
        {"kind": "call", "to": "Table Sales Invoice Header::SendRecords", "at": {"unit": "…/CDOUnhandledPostedSalesInv.Page.al", "line": 0}},
        {"kind": "call", "to": "Table Document Sending Profile::SendCustomerRecords", "at": {"unit": "Base Application::…/DocumentSendingProfile.Table.al", "line": 0}},
        {"kind": "call", "to": "Table Document Sending Profile::Send", "at": {"…": ""}},
        {"kind": "call", "to": "Table Document Sending Profile::OnBeforeSend", "at": {"…": ""}},
        {"kind": "event", "event": "Table Document Sending Profile::OnBeforeSend", "to": "Codeunit CDO eDocs Sending Profile Subs::OnBeforeSendCustomer"},
        {"kind": "call", "to": "…"},
        {"kind": "call", "to": "Codeunit CDO Send Mail Management::WaitForMailLimit", "at": {"…": ""}}
      ]
    }, {
      "routine": {"…": ""},
      "tier": "configured",
      "configuredVia": [{"entry": "Workflow Response Handling::ExecuteResponse", "arm": "PostDocumentCode()"}],
      "witness": ["… a hop with \"configured\": {\"entry\": …, \"arm\": …} …"]
    }]
  }
}
```

Single-app: `"throughDependency": {"relation": "code-calls-through-dependency", "notApplicable":
"single-app"}`.

### 11.3 explain card, with `--callers-context`

```
callers: 12 — …
  through dependency code: 2 more by code calls (nearest: Page CDO
  UnhandledPostedSalesInv.::SendCustomAction, 11 hops, via Table Document Sending Profile::Send
  -[event OnBeforeSend]->); 84 only through configured dispatch (catalog 1: matched)
```

Without `--callers-context` the card does not build the sited graph and the line is unchanged.
Why: M0's lazy rule (a resident engine builds the sited graph only once a question reads it,
~1 s and 45 MiB on CDO per revision), and an answer must not depend on whether a resident
artifact happens to exist (MCP answers equal the CLI's stdout byte for byte). `--callers-context`
already declares the graph, so the count costs one walk there.

### 11.4 Probe

```
$ aldump --configured-dispatch-candidates <ws> --min-arms 12
catalog 1: Workflow Response Handling::ExecuteResponse matched (Base Application 28.1.49838.50268)
candidates (dependency code, a case with >= 12 call-making arms):
  Codeunit Workflow Response Handling::ExecuteResponse  arms 35  callees 35  scrutinee WorkflowResponse."Function Name"  [catalogued]
  Codeunit Workflow Response Handling::AddResponsePredecessors  arms 25  callees 63  scrutinee ResponseFunctionName
  Table Default Dimension::UpdateGlobalDimCode  arms 20  callees 20  scrutinee TableID
  …
```

## 12. Decision 9: commit order and v1

v1, one branch, three commits:

1. **The condition and the catalog.** `configuredDispatch` in `sited::CONDITIONS`; the catalog
   (`impact_graph/configured_dispatch.rs`, version, entry, matching, statuses) read from the
   source facts and applied by the projection (no boundary, via or confidence change); the
   per-site arm table and the catalog report on the sited graph; `aldump
   --configured-dispatch-candidates`; the callgraph spec's note on the new condition name; unit
   tests of the matcher (match, signature mismatch, missing case, two cases, shared line) and of
   the projection (the arm sites carry it, the header's sites do not).
2. **The walk and `query touches` up.** `impact_graph/dep_callers.rs` (the predicate, the
   visitor, the classification, the witness); `query touches` up with `--from` declares the sited
   graph (cross-app) and renders `up.throughDependency` (query schema 3.5.0) and the human lines;
   the `reads_sited` pin rule; the fixture in both dependency forms with invariants 1-6 and their
   discrimination proofs; goldens.
3. **explain.** With `--callers-context`, the card's callers line gains the count, the configured
   count and the nearest callers; JSON `throughDependency` per card; CHANGELOG; the CDO
   acceptance and cost measurements.

Then, in this order (each its own design or milestone): **d54 through the cone** (small,
independent, a true positive in hand); **passing values across events** (§5.3, shared with
prove); **transaction spans** (§7, a transaction-aware traversal, opt-in, triage before default).
The export already carries the condition; bc-dev-mcp's ranking by it is its own change.

**v1 ships**: a labelled, witnessed list of callers through dependency code in `query touches` up
and explain, with the workflow engine's response dispatch labelled as configured, and the label
in the export. No detector, summary, solver, prove answer or other export change.

## 13. Open questions: settled by the owner (2026-11-06)

- **Q1. Catalog over structure.** Settled: an explicit catalog, **engine-owned in v1**, matched on
  stable app identity, full routine signature and the normalized scrutinee, with its version
  recorded; user-extensible later, where entries classify and never suppress (§4.2).
- **Q2. The cone and prove.** Settled: **no cut** in the cone or prove. Configured paths are
  labelled and measured separately; a configured path never turns an answer into "no". (A later
  step may add the label to prove's routes; it is not v1.)
- **Q3. Test selection.** Settled: never excluded for catalog membership. A test found only through
  a configured site may be ranked lower by the exported `configuredDispatch` condition; that
  ranking is bc-dev-mcp's change.
- **Q4. The long tail.** Settled: keep every caller, sorted by hops; fold paths over 12 hops only in
  human output, with `--all` listing them (§6).
- **Q5. d54 and d1.** Settled: d54 next, as its own small design (not in this work); d1 not now.
- **Q6. Event binding.** Settled: its own milestone before transaction spans, with the
  conservative `var` rule (§5.3); phase 4's Commit question is redesigned as a transaction-aware
  traversal (§7).

## 14. Changes from r1

Every finding of both reviews (F = fable, A = astra), and what r2 does with it.

| Finding | Severity | r2 | Where |
|---|---|---|---|
| F1 v1 duplicates R0's `CodeCalls` walk; build on R0 | High | **Accepted.** No `ThroughDependency` structure in `ExecRelation`; per-root R0 walk with a parent-recording visitor; kept structure is a phase-4 option | §0, §3.2, §12 |
| F2 two definitions of "dependency code"; fixtures fall between | High | **Accepted.** One predicate by app identity (`node_app != primary_app`), the solve graph's line; computed by the visitor, not read from `Tier`; fixtures load the dependency both as source folder and as package | §1, §3.2, §10.1, invariant 1 |
| F3 `--field --from` up reads the may-execute walk, not `CodeCalls` | Medium | **Accepted.** Row fixed; unchanged in v1 | §3.3 |
| F4 the block's count is an undefined composed closure | Medium | **Accepted.** Defined: listed iff not in `up.transitiveCallers` and reached by a path through dependency code; disjointness enforced; the walk composes natively | §1, §3.2 |
| F5 catalog as a site condition, not a `Boundary` | Medium | **Accepted** (owner decision): `configuredDispatch` in the conditions mask; boundary, via, confidence and `is_code_call_site` unchanged | §4.3 |
| F6 "demand does not grow" cites a circular proof | Medium | **Accepted.** The walk reads the sited graph, built from every parsed body, so demand is not involved; demand's own forward rule cited where the model names routines | §3.2 |
| F7 first hop is any code-call edge | Medium | **Accepted.** Calls, object runs, record triggers, event dispatch; fixture part "first hop by record trigger" | §1, §10.1, invariant 3 |
| F8 `onlyConfigured` as a bare count hides callers and arms | Medium | **Accepted** (owner decision "labels, never hides"): configured callers listed with entry and arm, ranked lower; human prints the count and arms by frequency | §4.3, §6, §11 |
| F9 `CaseOn` scope = arm labels, arm bodies, else body; key on routine + scrutinee | Medium | **Accepted.** Scope stated; `GetDescription` named as the reason for the key | §4.2 |
| F10 `ExecRelation::new_cross_app` does not exist | Low | **Accepted.** The real site is `build_cross_app_base` -> `ExecRelation::new`; moot in r2 (no `ExecRelation` change) | §3.2 |
| F11 964 vs 966 counts mixed | Low | **Accepted.** Explained (ancestors with a row include dependency publishers; `solveLocal` is workspace routines with a body); re-measured on CDO with the implementation | §2.6, §8 |
| F12 §2.4 verified; phase 3 must not over-prune the `AppCode` guard | Low | **Accepted.** The over-pruning test is part of the event-binding milestone | §5.3 |
| F13 d54 through the cone has two parts | Low | **Accepted.** Both parts named in the consumer table and §7.1 | §3.3, §7.1 |
| F, catalog identity in every output | — | **Accepted.** Version and per-entry status in every output that applies it | §4.2 |
| F, Q1-Q6 recommendations | — | Matched by the owner's decisions | §13 |
| A1 first-workspace-endpoint relation does not give transitive callers | High | **Accepted.** The walk composes across any number of crossings; witness ends at the queried routine; precedence (workspace path > code path > configured) and dedup by stable id stated; fixture "transitive composition" | §1, §3.2, §6, invariant 2 |
| A1, the literal search over the segment cannot prune `Other()` | High | **Accepted.** The search must run over the whole witness | §5.3 |
| A2 `Boundary::Configured` does not implement the stated R0 behaviour | High | **Accepted.** No boundary; orthogonal condition; R0's walks and tiers unchanged; the `--field` question settled as "unchanged" | §4.3, §3.3 |
| A3 the catalog is a classification, not proof a caller is unreal | High | **Accepted.** "Potential callers, path feasibility not checked"; configured = configuration-dependent potential callers, listed and inspectable; zero is not absence; "all reviewed true positives were retained" | §2.2, §4.3, §9 |
| A4 direct `var_assignments` is insufficient for event binding | High | **Accepted.** Conservative may-write/escape rule over every subscriber and its callees, unknown degrades to unknown; by-value first; adversarial fixtures; value-domain rules; no monotonicity promise | §5.3 |
| A5 `commit_on_path: No \| Maybe` cannot decide membership | High | **Accepted.** Withdrawn; phase 4 is a transaction-aware traversal with site order, boundaries, every route and three continuity outcomes | §7.2 |
| A6 closure cache key omits demand changes; `SolveIx` not stable | Medium | **Accepted**, and moot: no cache and no kept structure in v1 | §8 |
| A7 construction and ownership need an integration plan | Medium | **Accepted.** The plan is the existing sited-graph path: `ViewNeeds::sited` on scoped upward requests only, `ViewCx::sited`, the `reads_sited` pin rule; states `notApplicable` / `notComputed` / `limited` | §3.2, §9 |
| A8 d1 moves are not summary inheritance; "workspace-only summaries" too strong; detector-sensitive fixtures | Medium | **Accepted in part.** Explanation corrected (d1/d54 traverse `ctx.graph` with their own event policies; restricted transitivity, not workspace-only). Detector-sensitive fixtures move to the first detector that reads the relation: in v1 `analyze` builds no sited graph, so a fixture there would discriminate nothing; v1 checks `analyze` byte-identical on CDO instead | §3.1, §7.1, §7.2 |
| A, measure retained storage, not only the prototype step | — | **Accepted.** v1 measures the sited graph build and the walk on a one-shot query (wall, RSS) | §8 |
| A, the probe's "pairs removed" column should not block shipping | — | **Accepted.** Column dropped | §4.2 |
| A, Q1-Q6 recommendations | — | Matched by the owner's decisions | §13 |
