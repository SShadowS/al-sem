# Callers through dependency code: design

Date: 2026-11-06. Status: design only, for owner review. Base: `master` at `5c05092b`.
Inputs: the prototype `proto/dep-callers` (`5cca3e07`) and its report
(`target/tmp/dep-callers-measurement.md`); the caller-relations work on master
(`reverse::CodeCalls`, `object_run_reaches`, `query touches` up's `up.relation`, query schema
3.4.0); the d9 statement-order fix on master and the literal gap it recorded.

## 0. Summary of decisions

| # | Question | Decision |
|---|---|---|
| 1 | One graph or two relations | **Two.** The solve graph stays as it is. A new, separate, labelled relation, **through-dependency callers**, holds pairs X -> W (workspace routine X reaches workspace routine W only through dependency code). It is callers-only: it never feeds a summary, the SCC, the solver or `DbEffectQuery`. Rule: *upward* questions (who reaches W, which transaction W is in) may read it; *downward* questions (what X does) read the cone, which already follows dependency code. |
| 2 | Response engines | **An explicit, versioned catalog of configured-dispatch sites**, not a structural rule. v1 holds one entry: Base App `Workflow Response Handling::ExecuteResponse`. A path through a catalogued site is not a code call: the pair is counted in its own tier (`onlyConfigured`), never listed as a caller. A structural detector is kept as a discovery probe for whoever extends the catalog, not as a rule. |
| 3 | Literal pruning through dependency code | **Not in v1. Phase 3.** The existing literal search binds nothing across an event, has no Code/Text/Label domain and does not fold a constant-returning function, so applying it as built would not remove the noise measured. The three extensions are named and estimated (§5). |
| 4 | Evidence | Every through-dependency caller and every finding that uses one carries the **full dependency witness**: X's call site, each dependency routine, each event (by name) and the edge kind of every hop, ending at W. Never a bare "X calls W". |
| 5 | Detectors | **No detector reads the new relation in v1.** d9/d8/d50 (transaction spans, an upward walk) are the only detectors whose question it fits; they come in phase 4, opt-in first, after the literal work and a triage sample. d1 and d54 ask downward questions and are redirected to the cone, as separate work. |
| 6 | Cost, refresh, export | Under 1 MB kept; the step is 0.1-0.3 s on an idle machine (0.8-2.5 s measured on a machine at load 47-135). Rebuilt with the relation on every revision; the dependency-side closure is cached per package generation. `alsem graph` needs no change in v1 (it already keeps dependency nodes on workspace-to-workspace paths); an additive `configured` edge attribute is phase 2. |
| 7 | Honesty | Every output that lists callers names its relation and tier. `up.relation` keeps `"code-calls"`; a new `up.throughDependency` block is counted apart, with its own `relation` string and the configured-only count. |
| 8 | Tests | Three fixtures (event callback through a dependency, a configured-dispatch hub, a literal through a return and an event `var` parameter); an agreement test against R0's reverse walk; invariants that the solve graph, summaries and every finding are unchanged by the relation's presence; discrimination proofs for each. |
| 9 | v1 | The relation + the catalog (with R0 agreement) + `query touches` up + explain's callers line + the probe. Detectors untouched, goldens unchanged except the new fixtures and the query/explain goldens that gain the new block. |

## 1. Terms

- **Solve graph**: the Primary payload of `ExecRelation` (`PrimaryGraph`): the calls workspace
  routines make, plus event dispatch from any publisher. The db-effect solver, `pd_solve`,
  `DbEffectQuery`, the SCC and most detectors read it.
- **Cone**: the Cone payload of `ExecRelation` (`Vec<TypedEdge>`): the workspace half's typed edges
  and then the dependency half's. `prove`, digest and the capability cone read it. It **already
  follows dependency code downward**, including through the workflow engine.
- **Code-call relation**: what the solve graph follows upward (calls, object runs and the triggers
  a run runs, record triggers, event dispatch; no user-started page trigger, no open site).
  `reverse::is_code_call_site` names it over the sited graph; `reverse::CodeCalls` is its visitor
  adapter. On local code R0 over code-call sites equals the solve graph's callers exactly
  (`WaitForMailLimit` 964 = 964).
- **Through-dependency caller** of W: a workspace routine X that calls a dependency routine D, where
  D reaches W through dependency code only (dependency routines' resolved calls and event dispatch
  from dependency publishers), without passing another workspace routine. R0 already has the word:
  `Tier::ThroughDependency`.
- **Configured dispatch**: a site whose set of targets is chosen at runtime by configuration data
  (rows a user or an admin sets up), not by the code or the caller's arguments, but whose arms the
  resolver sees as ordinary static calls. Base App's workflow response dispatch is the case on CDO.
- **Witness**: one concrete path that shows a claim, with every hop.

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
- **No true positive is lost**: all three findings the report judged true (d1 `LoadFiles` x2, d54
  `ShowMigrationNotification`) stay, and the two true callers of `WaitForMailLimit` (the "Send"
  action -> `Document Sending Profile::Send` -> `OnBeforeSend` -> `CDO eDocs Sending Profile
  Subs::OnBeforeSendCustomer`) stay.
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
phase 3.

### 2.5 Not measured here

Time, memory, prove: the report's numbers stand (no measurable cost; 0 of 464 prove verdicts move;
the north star does not read `ExecRelation`). The relation's own step took 0.8-2.5 s in these runs
on the loaded machine (the report: 130-270 ms on a less loaded one); §8 asks for an idle
re-measure.

## 3. Decision 1: relation design

### 3.1 Why not one graph

The prototype adds X -> W edges to `primary.edges_by_from`. Because `DbEffectQuery::ancestors`,
the reverse call graph, the SCC and the solver all read that map, every edge also flows downward:
X inherits W's effects in its solve summary. That downward half is what moved d1 (457 changed
findings, 10 severity/confidence changes) and is wrong in kind, not only noisy:

- The cone already holds X's effects through dependency code, with typed edges, the dependency
  routines' own effects and the literal search over them. A via-dependency edge copies a subset of
  that into the solve summary with less fidelity: parameter binding is `Unknown` along it, the
  dependency routines' own effects are skipped, and no literal pruning applies.
- The solve summary is documented as workspace-only (`effect_query.rs`: summaries are
  transitive-down over the solve graph; the U-scoped query is "informative" only because of that).
  Folding the relation in silently changes what every summary means.

No consumer measured needs the downward half. So it is not built.

### 3.2 The relation

New in `src/engine/l4/dep_callers.rs` (name to taste), owned by `ExecRelation`:

```rust
pub struct ThroughDependency {
    /// Per workspace caller X (solve index), sorted by (W, first-hop callsite).
    rows: HashMap<SolveIx, Vec<DepCaller>>,
    /// Reverse index: per target W, its through-dependency callers.
    by_target: HashMap<SolveIx, Vec<SolveIx>>,
    /// Pairs reachable only through a configured-dispatch site: counted, never listed as callers.
    only_configured: HashMap<SolveIx, Vec<ConfiguredPair>>,
    witnesses: WitnessArena,            // §6
    stats: ThroughDependencyStats,      // components, set elements, time
}
pub struct DepCaller {
    pub target: SolveIx,
    pub callsite_id: CallsiteId,        // X's call into dependency code
    pub witness: WitnessIx,             // shortest path, every hop
    pub crossed_event: bool,            // the witness passes an event dispatch
}
```

- Built in `ExecRelation::new_cross_app` from data the relation already assembles (the dependency
  half, and the dependency publishers' dispatch edges in the Primary payload). Demand does not grow
  (the report proved it: the 86 callers equal R0's `wsNonLocal` set exactly).
- Algorithm: the prototype's (Tarjan over the dependency subgraph, each component's reachable
  workspace set built callees-first and shared by pointer), plus the configured-dispatch rule
  (§4): a catalogued site's edges are left out of the code-call walk and walked in a second pass
  that only fills `only_configured`.
- **Never** written into `PrimaryGraph`. The type makes it so: `ExecRelation::primary()` is
  unchanged; the new accessor is `ExecRelation::through_dependency() -> Option<&ThroughDependency>`
  (`None` single-app, and when not declared).
- Declared need (declared-needs skill): a `Plan::own` bit `THROUGH_DEPENDENCY` on the plans that
  read it (`query_plan` for `touches` up, `explain_plan`); built only when declared; reading it
  undeclared panics in debug and tests ("declare THROUGH_DEPENDENCY"). Cross-app only: single-app
  has no dependency code, and the reader says "not applicable (single-app)", never "0 callers".

### 3.3 Which consumer reads which relation

| Consumer | Question direction | Reads today | v1 | Later |
|---|---|---|---|---|
| `query touches` up (`--callers`, the informative and trivial cases) | up | solve graph (`code-calls`) | solve graph **plus** a separate `throughDependency` block | |
| `query touches --field --from` up | up | R0 `CodeCalls`, already tiered | unchanged; configured sites become `onlyOpen`-like (§4.3) | |
| explain card, callers line | up | `model.calls` direct callers | adds a through-dependency count + first witness | |
| explain `--callers-context`, prove's callers' questions | up | R0 sited graph (already follows dependency code) | unchanged | the configured tier (§4.3) |
| `alsem tests`, impact B1/B2 up | up | R0 sited graph (already follows dependency code, `throughDependency` tier) | unchanged | ranking through configured dispatch (open question Q3) |
| transaction spans (d9, d8, d50, digest's tx context) | up (callers of the Commit routine) | solve graph reverse walk | unchanged | phase 4, opt-in: needs §5 and a Commit-on-the-dependency-path check |
| `DbEffectQuery::ancestors`, solver, `pd_solve`, SCC | down / structure | solve graph | **never** | never |
| d1 (loop -> op) | down | solve graph | unchanged | if ever, through the cone with the catalog applied, as its own project |
| d54 (TryFunction -> publisher) | down | solve graph, no event edges | unchanged | through the cone: the dependency publisher raised under the try (`CSC App Management::OnRequestAppInformation` in the measured case) is the better finding than the workspace publisher after it |
| prove, digest, capability cone | down | cone (already through dependency code) | unchanged | the catalog applied to the cone (open question Q2) |
| `alsem graph` export | both | sited graph | unchanged | additive `configured` edge attribute |

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

### 4.2 Rule: an explicit catalog

`src/program/resolve/configured_dispatch.rs`, one entry per site:

```rust
pub struct ConfiguredDispatch {
    pub app: AppMatch,           // publisher + name ("Microsoft", "Base Application"); any version
    pub object: (ObjectKind, &'static str),   // (Codeunit, "Workflow Response Handling")
    pub routine: &'static str,   // "ExecuteResponse"
    pub arity: u8,               // 3
    pub scope: DispatchScope,    // WholeBody | CaseOn { scrutinee_text: "WorkflowResponse.\"Function Name\"" }
    pub why: &'static str,       // "responses are chosen by Workflow Step Instance rows"
}
```

- v1 entry: `ExecuteResponse`, scope `CaseOn` the response's `Function Name` case including its
  `else` branch (the `OnExecuteWorkflowResponse` event is just as configured). The routine's other
  calls (telemetry, `OnBeforeExecuteResponse`) stay code calls. Measured: the routine-level cut gives
  166 shortcuts, identical to the object cut; the case-level cut must give the same, and the
  fixture pins it (§10).
- A catalog entry that matches nothing in a program is reported once in the probe and in the
  relation's stats (`catalogUnmatched`), so a Base App rename is loud.
- Why not a structural rule: at k = 12 it marks 8 cases that are ordinary dispatch on a parameter
  or a type field (§2.2). Its threshold is arbitrary (an ISV dispatcher with 10 arms escapes k = 12;
  Base App's `Default Dimension` dispatch is caught). A rule that reads the scrutinee's provenance
  ("a field of a record filled by a database read in the same routine") would match
  `ExecuteResponse` but also every `case SalesLine.Type of` after a `FindSet`, which is
  transaction data, not configuration. The difference is one of meaning; a human decides it.
- The structural rule stays as a **discovery probe**, `aldump --configured-dispatch-candidates <ws>
  [--min-arms k]`: every `case` with at least k call-making arms in dependency code, with its
  scrutinee text, arm count, distinct callees and how many through-dependency pairs a cut there
  would remove. Whoever extends the catalog runs it first.

### 4.3 What a configured site means

- In the through-dependency relation: the site's edges are not followed by the code-call walk. A
  pair (X, W) reachable only through one is recorded in `only_configured` with its witness and
  printed as a count with the catalog entry's name, never as a caller.
- In R0's code-call relation (`reverse::is_code_call_site`): the sited graph's site gets
  `Boundary::Configured`; `CodeCalls` treats it as R0 treats an open site (tier `OnlyOpen`), so the
  agreement test (§10) keeps holding: R0 code-call callers with tier `ThroughDependency` equal the
  relation's callers.
- R0's may-execute walks (`alsem tests`, impact) and the cone: **unchanged in v1.** Both are
  soundness-first consumers (a test that runs a workflow can post a document), and changing them
  moves test ranking and possibly prove verdicts. Open questions Q2/Q3.

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

### 5.3 Decision and estimate

- Phase 3 extends `literal_reach` (shared with prove, so prove gains too): (c) event argument
  binding with the `var` soundness rule (d); (a) a value domain for Code/Text/enum literals and
  Labels/TextConsts, matched against `case` arms and `if X = <lit>` guards; (b) constant-returning
  routines as a per-routine body fact (dependency routines included; it needs the body facts the
  demanded dependency bodies already have); (e) single-expression predicate helpers. Estimate:
  (c)+(d) 1 milestone (the binding table exists for calls; events add the subscriber's parameter
  map and the assignment check); (a)+(b) 1 milestone (a new lattice element, the label table, a
  body fact under a declared need); (e) half a milestone. Each with a fixture and a CDO measure of
  prove verdict moves (they can only move toward `no`/fewer routes).
- Then the relation's walk applies the search per (X, first hop) over the dependency subgraph,
  bounded by `STATE_CAP`; a pair whose every witness is literal-infeasible moves to a third count,
  `literalDead`, with the witness and the guard that kills it. Over the cap it stays a caller
  (never dropped silently).
- Expected effect on CDO (estimate, to verify): the eSeal arm leaves `RunESealServiceSetup`'s
  callers (d9's 5 -> 18 growth goes away); the 27 Reservation shortcuts go if `MatchThisTable` is
  decided. The Calc. Discount cascades (28) need record-state reasoning (which lines exist) and
  stay.

## 6. Decision 4: evidence

- The relation keeps one shortest witness per (X, W, first-hop callsite) in a `WitnessArena`:
  interned routine ids, per hop the edge kind (`call`, `codeunit-run`, `implicit-trigger`,
  `event-dispatch` with the event's publisher and name, `interface`, `page-run`) and the dependency
  call site's anchor (app, file relative to that app's source root, line). Built at relation time by
  BFS over the dependency subgraph, honouring the catalog (the measurement's first dump did not,
  and showed workflow paths that the cut graph no longer has: a witness must come from the same
  graph as the claim).
- Rendering: every consumer that prints a through-dependency caller prints the hops. Long witnesses
  are folded in human output (first 3 and last 3 hops, `… N more …`); JSON always has every hop.
- A configured-only pair prints the catalog entry it went through.
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
| d1-db-op-in-loop | +20, 457 changed | +2 (`LoadFiles` x2, info, temporary records), 2 changed | not a caller question; no change in v1. A d1 that follows dependency code would read the cone, as its own project |
| d54-publish-in-tryfunction-cone | +1 (true) | +1 (true) | downward; redirect to the cone (report the dependency publisher), own project |
| d9-transaction-span-summary | 8 changed | 8 changed (catalog does not touch them) | the one real caller question; phase 4 after §5 |
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
   phase 3 removes it.

Spans need one more rule before they may read the relation: dependency code between X's call and W
may itself Commit (`Sales-Post` commits unless suppressed). A through-dependency caller is in W's
transaction only if no Commit may run on the witness before W. The dependency routines' own
direct facts carry Commit sites; the relation records, per pair, `commit_on_path: No | Maybe`, and a
`Maybe` pair ends the span walk there, as a caller that commits does today.

## 8. Decision 6: cost, memory, refresh, export

- **Memory**: the kept structure is ~166 pairs on CDO (628 without the catalog) plus witnesses
  (median 8 hops, max 48) and the configured-only pairs (462): well under 1 MB. The resident budget
  (8 GiB cross-app, 6 GiB `--low-mem`) is not affected. `--low-mem` needs nothing special: the
  relation keeps no syntax tree.
- **Time**: the prototype's step, 130-270 ms on a lightly loaded machine; 0.8-2.5 s measured here at
  load 47-135, with witness BFS on. Phase 1 re-measures on an idle machine and records it; budget:
  under 1% of a cold `analyze`.
- **Refresh** (context server, `program::session` change types): the relation is part of
  `ExecRelation`, which every revision rebuilds (`relation_of`). Its inputs:

| Change type | What it can change | Action |
|---|---|---|
| B (body) of a workspace routine | X's calls into dependency code | rebuild X's rows only from the cached dependency closure |
| S (declaration) / F (file) | workspace subscribers to dependency publishers, workspace table/page extension triggers dependency code runs, interface implementations dependency code dispatches to | rebuild the closure's workspace endpoints, then all rows |
| P (package) | the dependency half and the catalog's matches | rebuild everything |

  v1 may rebuild the whole relation per revision (it is cheap) and cache only the dependency-side
  component closure keyed by the package generation plus a hash of the dependency->workspace edges.
  The session tests' "same model as a cold build" rule extends to the relation: a refreshed
  revision's `ThroughDependency` equals a cold build's, in both default and `--low-mem` modes.
- **Export** (`alsem graph`, read by bc-dev-mcp): no change in v1. It already keeps every dependency
  node and edge on a workspace-to-workspace path (callgraph spec §4.6), so test selection sees these
  paths today. Phase 2 (additive only, schema `al-sem.callgraph` minor bump): an edge attribute
  `configured: true` on catalogued dispatch edges and `meta.configuredDispatch` listing the catalog
  entries matched, so bc-dev-mcp may rank a test found only through one as `dynamic`. Changing that
  ranking is bc-dev-mcp's decision, made with its parser change.

## 9. Decision 7: honesty

- `query touches` up keeps `up.relation: "code-calls"` and its counts exactly as today. The new block
  is apart, named, and its own relation string says what it follows and what it leaves out.
- Every count of through-dependency callers comes with the configured-only count and the catalog
  entry names, so "0 callers through dependency code" is never read as "nothing reaches it through
  Base App".
- Single-app: the block says `notApplicable: "single-app"`.
- An undeclared read panics in debug/tests; release prints nothing rather than a false 0.
- The human `callers:` line changes from "…nor a path through dependency code" to name the second
  list: "paths through dependency code are listed apart below".

## 10. Decision 8: tests, invariants, fixtures

### 10.1 Fixtures

New `tests/fixtures/dependency-callers/` with a workspace and a source dependency
(`ws/al-sem.dependencies.json` -> `../dep`):

| Fixture part | Shape | Pins |
|---|---|---|
| `callback` | ws `Caller` calls dep `Engine.Run` -> dep publisher `OnAfterRun` -> ws subscriber `Sub` -> ws `Target` (writes table T) | `Caller` is a through-dependency caller of `Sub` and of `Target` (via `Sub`), with a 3-hop witness naming `OnAfterRun`; the solve graph, `Caller`'s solve summary and all findings are the same as without the relation |
| `configured` | dep `Dispatcher.Execute` with `Resp.Get(Step."Code"); case Resp."Code" of 'A': PostA(); 'B': PostB(); else OnUnknown(…)` where `PostB` -> dep publisher -> ws subscriber; a catalog entry for it (test catalog injection) | the pair is in `onlyConfigured`, not a caller; removing the catalog entry makes it a caller (discrimination); the probe lists the case |
| `literal` | ws `Enable` passes `GetCode()` (`exit(CodeLbl)`) to dep `FeatureMgt.Enable(var Code)` -> dep event `OnAfterEnable(var Code)` -> ws subscriber `case Code of OtherLbl: Other(); CodeLbl: Mine(); end` | phase 3: `Other` is `literalDead` for `Enable` with the guard named; before phase 3 the test is `#[ignore]`d with the reason, not deleted |

### 10.2 Invariants (each can fail)

1. **Agreement with R0.** For every workspace routine W in the fixtures (and on CDO under
   `cdo-gate`): {X : X in `ThroughDependency.by_target[W]`} equals the R0 `CodeCalls` walk's
   workspace states of tier `ThroughDependency` whose shortest path enters W from dependency code
   (the report's 86 = 86, made executable). Break: drop event dispatch from the dependency walk ->
   the fixture's `callback` caller disappears from one side.
2. **The solve graph is untouched.** `ExecRelation::primary()`, the SCC and every solve summary are
   byte-equal with the relation built and not built (hash of `edges_by_from` + summaries). Break:
   push the pairs into `edges_by_from` (the prototype) -> fails on `callback`.
3. **Findings are untouched in v1.** `analyze` output with the relation declared equals output
   without it, on the corpus and on CDO under `cdo-gate`. Break: let d1 read it -> fails.
4. **The catalog is applied.** A configured-only pair is never in `rows`. Break: skip the catalog
   check -> `configured` fails.
5. **Witness validity.** Every witness is a path in the same graph the claim was made over: each
   hop's edge exists in the dependency half (or the Primary payload's dependency dispatch), none
   goes through a catalogued site for a `rows` entry, the last hop ends at W. Break: build witnesses
   from the unfiltered adjacency (as the first measurement dump did) -> fails on `configured`.
6. **Refresh equals cold.** After each change type in the session tests, the relation equals a cold
   build's, default and `--low-mem`.
7. **Catalog matches loudly.** On CDO the v1 entry matches exactly one routine; an entry matching
   none is reported.

Each discrimination proof is made with Edit, run, reverted with Edit, both outcomes recorded
(CLAUDE.md); a scripted break asserts it applied.

### 10.3 Goldens

v1 moves only: the new fixtures' goldens, and the `query touches` up / explain goldens that gain the
new block (`tests/cli-query-goldens/`, `tests/cli-explain-goldens/`). Every other family must stay
byte-equal; `scripts/check-goldens --regen` then `scripts/check-goldens` confirms it. The CDO query
goldens (`e6b303bd`) gain the block under `cdo-gate`.

## 11. Mock-ups

The shapes are the proposal. The counts 966, 2 and 84, the 4-hop dependency witness and the probe's
rows are measured (§2, the report); the direct/transitive counts in 11.3, the "6 workspace hops",
the line numbers (shown as 0) and the d9 finding in 11.5 are illustrative.

### 11.1 `query touches` up, human (CDO, `WaitForMailLimit`, `--callers`)

```
--- Codeunit CDO Send Mail Management::WaitForMailLimit vs CDO E-Mail Log (…) ---
  scope: per operation, one routine into dependency code; …
  callers: by code calls of workspace code (calls, object runs and the triggers a run runs, record
    triggers, event dispatch); not what a user starts on a page a caller opens (alsem tests and
    impact follow it); paths through dependency code are listed apart below
  up: this routine itself touches the table; 966 transitive callers by code calls inherit it …
  through dependency code: 2 more workspace callers reach this routine only through dependency
    code (84 more only through configured dispatch: Base Application, Workflow Response
    Handling::ExecuteResponse; not listed)
    Page CDO UnhandledPostedSalesInv.::SendCustomAction
      -> Table Sales Invoice Header::SendRecords -> Document Sending Profile::SendCustomerRecords
      -> Document Sending Profile::Send -[event OnBeforeSend]-> Codeunit CDO eDocs Sending
         Profile Subs::OnBeforeSendCustomer -> … 6 workspace hops … -> WaitForMailLimit
    Page CDO Unhandled Posted Cr. Memo::SendCustomAction
      -> … (same 4 dependency hops) …
```

(84 is the report's count of `WaitForMailLimit` callers that exist only through the hub; the
fixture-backed golden will hold the exact numbers.)

### 11.2 `query touches` up, JSON (additive, query schema 3.5.0)

```json
"up": {
  "scoped": true,
  "relation": "code-calls",
  "transitiveCallers": 966,
  "...": "unchanged fields",
  "throughDependency": {
    "relation": "code-calls-through-dependency",
    "follows": "a workspace caller's call into dependency code, then dependency calls, object runs, record triggers and event dispatch from dependency publishers, to the first workspace routine",
    "callers": 2,
    "onlyConfigured": 84,
    "configuredSites": [{"app": "Microsoft/Base Application", "routine": "Codeunit Workflow Response Handling::ExecuteResponse", "why": "responses are chosen by Workflow Step Instance rows"}],
    "literalDead": null,
    "list": [{
      "routine": {"object": "Page CDO UnhandledPostedSalesInv.", "name": "SendCustomAction", "...": ""},
      "entersAt": {"routine": "Codeunit CDO eDocs Sending Profile Subs::OnBeforeSendCustomer"},
      "witness": [
        {"kind": "call", "from": "Page CDO UnhandledPostedSalesInv.::SendCustomAction", "to": "Table Sales Invoice Header::SendRecords", "at": {"unit": "ws", "file": "src/…/CDOUnhandledPostedSalesInv.Page.al", "line": 0}},
        {"kind": "call", "to": "Table Document Sending Profile::SendCustomerRecords", "at": {"unit": "dep:Base Application", "file": "src/Foundation/Reporting/DocumentSendingProfile.Table.al", "line": 0}},
        {"kind": "call", "to": "Table Document Sending Profile::Send"},
        {"kind": "event-dispatch", "event": "Table Document Sending Profile::OnBeforeSend", "to": "Codeunit CDO eDocs Sending Profile Subs::OnBeforeSendCustomer"}
      ],
      "commitOnPath": "notComputed"
    }]
  }
}
```

`literalDead` is `null` until phase 3 (then a count and a list with the deciding guard).
`commitOnPath` is `notComputed` until phase 4.

### 11.3 explain card, callers line

```
callers: 12 direct (code calls), 431 transitive; through dependency code: 2 (first: Page CDO
  UnhandledPostedSalesInv.::SendCustomAction via Document Sending Profile::Send -> OnBeforeSend);
  only through configured dispatch: 84
```

### 11.4 Probe

```
$ aldump --configured-dispatch-candidates <ws> --min-arms 12
catalog: 1 entry, 1 matched (Workflow Response Handling::ExecuteResponse)
candidates (dependency code, case with >= 12 call-making arms):
  Workflow Response Handling::ExecuteResponse   arms 35  callees 35  scrutinee WorkflowResponse."Function Name"  [catalogued]  pairs removed by a cut: 462
  Workflow Response Handling::AddResponsePredecessors  arms 25  callees 63  scrutinee ResponseFunctionName (parameter)  pairs removed: 0
  Default Dimension::UpdateGlobalDimCode   arms 20  callees 20  scrutinee TableID (parameter)  pairs removed: 0
  …
```

### 11.5 A d9 finding with a through-dependency member (phase 4 only)

```
d9 transaction ending at Commit in CDO X::Y (file:line): 4 routines, 3 tables pending
  members: … ; CDO Install::EnableImprovedEDocumentsFeature (through dependency code:
  CSC Feature Management::EnableFeature -[event OnAfterEnableFeature]-> CDO Core Event
  Handler::OnAfterEnableFeature; no Commit on the dependency path)
```

## 12. Decision 9: commit order and v1

Phase 1 (v1), one branch, commits in this order:

1. `ThroughDependency` relation in `ExecRelation::new_cross_app` (no consumer), with the witness
   arena, the stats, the declared-need bit, invariants 2, 3 and 5 and the `callback` fixture. No
   output changes; all goldens byte-equal.
2. The configured-dispatch catalog (`configured_dispatch.rs`), `Boundary::Configured` in the sited
   graph and `CodeCalls`' tier for it, the `configured` fixture, invariant 4 and 7, the agreement
   test (invariant 1) on the fixtures and under `cdo-gate`.
3. `aldump --configured-dispatch-candidates`.
4. `query touches` up's `throughDependency` block (query schema 3.5.0), the human line, goldens.
5. explain's callers line, goldens.
6. Refresh: invariant 6 in the session tests (both modes); idle-machine time and memory measure
   recorded in the CHANGELOG.
7. CHANGELOG, `docs/` notes on the two relations (the `effect_query.rs` module doc gains one
   paragraph: the U-scoped query stays solve-graph-only; through-dependency callers are listed by
   the relation, never probed against a solve summary, because their touch reaches them through
   dependency code the summary does not hold).

Phase 2: the export's additive `configured` attribute (with bc-dev-mcp). Phase 3: the literal
extensions (§5), shared with prove, each measured on CDO for prove moves. Phase 4: transaction
spans read the relation, opt-in, Commit-on-path rule, triage (§7.2); default only after triage.
d1 and d54 through the cone are separate designs.

**v1 ships**: a labelled, witnessed list of callers through dependency code in `query touches` up
and explain, with the workflow engine's response dispatch held apart as configured. No detector,
summary, prove answer or export changes.

## 13. Open questions for the owner

- **Q1. Catalog over structure.** The design rejects a structural hub rule (§4.2) and keeps an
  explicit catalog with one entry. Agree? Should the catalog be user-extensible (`.al-sem.json`
  `configuredDispatch`) for ISV interpreters, or engine-owned only?
- **Q2. The cone and prove already follow the workflow engine downward.** Applying the catalog to
  the cone would change what prove says for any routine that can reach a workflow event (it can
  only move verdicts toward `no` or `unknown`). Measure and decide separately?
- **Q3. Test selection.** R0's may-execute walks keep resolving through the workflow engine. Rank
  tests found only through a configured site as `dynamic` (phase 2 with bc-dev-mcp), or leave them
  resolved because test selection must be sound first?
- **Q4. The long tail.** After the catalog, 54 of 166 pairs have witnesses over 12 hops, mostly
  record-trigger cascades the literal work will not decide. Show them as callers (v1 as designed),
  or fold pairs over a hop bound into a count with a "long path" label?
- **Q5. d54 and d1 through the cone.** The one d54 true positive suggests d54 should report the
  dependency publisher raised under the try (it does not walk into dependency code at all today).
  Worth its own design now, or after phase 4?
- **Q6. Phase 3 priority.** Event argument binding (§5.3 (c)+(d)) benefits prove as much as this
  relation. Schedule it as its own milestone ahead of phase 4?
