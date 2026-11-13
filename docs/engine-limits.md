# Engine limits (from output validation, rounds 1–4, and the limit probes)

Written 2026-11-09, at master `58d0c94e`.

- **L1–L14** were found by output validation, rounds 1–4.
- **L15–L26** were suspected and then checked by probes on 2026-11-09. Their old ids, P1–P12,
  are kept in brackets.
- **L27–L34** are further limits the probes noticed along the way.
- **L35–L36** are limits the wave-2 B5 review found (2026-11-10).
- **B1–B8** are wrong answers or a crash that the probes found. **B9** is a wrong answer the
  wave-2 review probes found, and **B10** one that ENG-06 found while fixing B9. A wrong answer is worse than a limit: limits are stated in answers,
  these are not. They come first in the order.

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
- **new:** noticed while probing something else; not probed on its own.
- **fixed (remainder stated):** the main fault is fixed, and the row's "in detail" section lists
  what is left. Where what is left still gives a wrong definite answer the answer does not
  mention, the list says "wrong answer left". By this document's own rule those are B-class
  (see above); they stay with their limit until someone opens a B row for them.

## 1. Wrong answers and a crash (B1–B10)

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
| B1 | **Fixed in wave 0** (TLS-44 audit, 2026-11-13: the second pass pins with `pin_needs`, called from `src/bin/alsem.rs:4616`). Was: **`alsem_prove` with a filter (`match`, likely also `site`, `op`, `table`) crashed the resident engine** (signal 6). The resident revision is built for explain (`ViewNeeds::EXPLAIN`, `src/engine/resident/session.rs:52`, no site guards). `ProveJob::demands` asks for site guards when a filter is given (`prove.rs:3791-3801`). The function that would widen the needs, `pin_needs` (`session.rs:279`), has no production caller. Panic: `workspace.rs:907 'site guards not built'`. The one-shot CLI answers correctly. | server/ (MCP, 1-codeunit fixture, single-app and cross-app) | S |
| B2 | **`#if` arms were run one after another.** An `exit` or `Error` in one arm cut off the code after the group in every configuration, and a routine declared in two arms cited one arm's line for the other's write. **Fixed in wave 2.** What was fixed and what is left: [B2 in detail](#b2-in-detail). | values/probe-preproc.txt | M (step 1 of L22) |
| B3 | **Fixed in wave 0** (see above; the forward half is B7). Was: **`callers-intercept-errors` answered a wrong `no` for a subscriber of an isolated event.** Isolated dispatch catches the error, but it is not treated as a trap. On CDO, `OnCompanyOpenCompleted`'s subscriber gets `unknown` where the answer is `yes`. | errors/iso.out, cdo-iso-intercept.out | S (part of L7) |
| B4 | **`[CommitBehavior(Error)]` is ignored.** A `Commit()` under it raises an error, but `throws-error` says `no (every candidate decided)` and `commits-on-success-path` says `yes`. Under `Ignore`, the commit is still reported as a success-path commit. **Fixed in wave 2** (`fix/w2-b4-commit-behavior`, CHANGELOG "engine limits wave 2, B4/L16"). | locks/p2-pure-error.txt | M (L16) |
| B5 | **Fixed in wave 2** (`fix/w2-b5-flowfield-reads`): a `CalcFields`, a filter on a FlowField and a read after `SetAutoCalcFields` now read the `CalcFormula`'s table (source or symbols); an undecided one is `unknown`, never `no`. CDO: `Balance` vs `Detailed Cust. Ledg. Entry` is now `down: yes`. ~~`query touches --direction down` says a definite `no` for a table read only through a FlowField's `CalcFormula`.~~ Round 2 (`fix/w2r2-b5`): a bare `SetAutoCalcFields` no longer gives a wrong `yes` on another record; the implicit `Rec` is shared with page/table/report extensions; a qualified field argument and `FieldRef.CalcField()` no longer give a wrong `no`. Round 3: the FieldRef rebinding and agreement guards are pinned by tests, and a FieldRef read's digest/prove evidence cites its line. Round 4 (final wave-2 review): a FieldRef FILTER whose bindings disagree or cannot be read no longer answers `no`; it reads what every binding reads (a may-read) and is `unknown` when a binding's table is not known or the bindings are made outside the routine (parameter, global, a local passed to a call). What is left is in L19, L35 and L36. | declarations/out | M (L19) |
| B6 | **`prove writes-table` says a definite `no` for a table written only by an XmlPort's automatic insert.** **Fixed in wave 2** (`fix/w2-b6-dataset-ops`): the platform's dataset operations are record operations of the running call, with their reason. Round 2 (`fix/w2r2-b6`): a call to the object's own `Print`/`Execute`/`SaveAs*`/`Open` procedure binds by AL's overload rule (alc-probed), not as a run; a run's per-record triggers are conditional, never `unconditional on success`; `alsem graph` follows an import into its tables' trigger and events; see L20 for what is left. | declarations/out | M (L20) |
| B7 | **An error behind a trap is reported as escaping.** A trap is a checked `Codeunit.Run`, a `[TryFunction]` whose result is used, or an isolated event. `throws-error --site/--match` answers `yes`, and the condition reads `unconditional on success`, which describes the callee and not the route. **Fixed in wave 2** (`fix/w2-b7-traps`, CHANGELOG): an error a definite trap catches on every route is no longer counted by prove (with or without a filter), explain or digest; an isolated event's error stays `yes` and states its condition; a route trapped on some paths says which. A checked run with a write that may be pending is no definite trap for an error that has a row in the answer (L28; a commit that `[CommitBehavior(Error)]` turns into an error inside a checked run has no row, see [L16 in detail](#l16-in-detail)), and a TryFunction traps only where its value is tested. Round 2 (`fix/w2r2-b7`): a `Commit()` that `[CommitBehavior(Ignore)]` or `(Error)` stops no longer ends the write before a checked run, and explain and digest state the callers' assumption a caught ruling rests on, as prove does. Left: see L23 and L28. | errors/p9.out, cdo-sendqueue-site.out | M (L23) |
| B8 | **Small wrong output.** Each item is S. |  | S together |
| B9 | **Fixed by ENG-06** (`alsem/eng-06-b9-fieldref-validate`, 2026-11-11). ~~**`FieldRef.Validate` does not raise a table's validate events.** `FR := RR.Field(2); FR.Validate(1);` under a fully readable `[EventSubscriber(ObjectType::Table, Database::"AD T", 'OnAfterValidateEvent', 'Amt', …)] H: Commit();` answers a definite `may-commit: no`; `R.Validate(Amt, 1)` answers `yes`.~~ The cause: `FieldRef.Validate` resolved as a plain built-in call, with no trigger or table-event edge; the resolver's "any validate trigger" bound was read only by the call-graph export. Now the resolver follows the FieldRef to its `RecRef.Field(<number>|'<name>'|Rec.FieldNo(F))` (written in place, or a local FieldRef bound only that way) and the RecordRef to its one literal table (`recordref_table`), and the call is a `Validate` record operation of that field: the field's `OnValidate` and the table's validate-event subscribers of that field, as for `Rec.Validate`. `Field(B.FieldNo(F))` is `F`'s number in `B`'s table. A FieldRef whose table or field is not provable (a parameter, a global, passed to a call, `FieldIndex`, a computed number, bindings that name different fields, a field number the table does not have or declares twice under two names) is an open operation: `unknown` (`record-table-unresolved`) when a loaded table has validate code (an `OnValidate`, an extension's `OnBefore/OnAfterValidate`, or a readable validate-event subscriber); with none loaded it stays `no` within the loaded apps the answer states (B10). **Left, older than B9:** the trigger fan-out of every record operation (`resolver::resolve_implicit_trigger`, used by `Rec.Validate`/`Rec.Insert(true)`/… and the decided FieldRef alike) looks up only `On<Op>`, so a table extension's `OnBefore<Op>`/`OnAfter<Op>` triggers (a field's `OnBeforeValidate`/`OnAfterValidate`, `OnAfterInsert`, …) get no edge: a definite answer through them can be a wrong `no`. Filed as ENG-15. A RecordRef bound by `GetTable(<record of a known table>)` counts as that table, as `recordref_table` reads it everywhere. A FieldRef or RecordRef received from a caller is any table: `unknown` when loaded code may run (as above), else `no` within the loaded apps, with that stated; linking it to the caller's binding is L19 (2). | wave 2 review probes `t-read`, `t-val-read` (`DoFieldRefValidate`); ENG-06 probe `tests/cli/cli_b9_fieldref_validate.rs` | M |
| B10 | **Fixed by ENG-06** (found and fixed there, 2026-11-11). ~~**A record operation that can fire a table trigger, on a record whose table is not known, is a definite `no`.** `RR.Modify(true)` with `RR` a `var RR: RecordRef` parameter answers `may-commit: no (every candidate decided)`, and so does every caller.~~ Any table's `OnModify` and modify-event subscribers may run there. The resolver left such an operation (`Insert`, `Modify`, `ModifyAll`, `Delete`, `DeleteAll`, `Rename`, `Validate` on a table that does not resolve) with no route, and the model made no edge and no gap for it. Now it is an open edge when code may run there (`SiteFacts::record_table_unresolved`, the `record-table-unresolved` uncertainty): `prove` answers `unknown` and names it, and so does `query effects`' scope. Code may run there when the record is of a named table the program does not hold and the operation fires its trigger (that table's code is not seen), or, for a RecordRef that may hold any table, when a loaded table has a trigger of that family or a readable subscriber to its table events, or a loaded table's code is not loaded (symbols only). With nothing loaded to run, the answer stays `no` within the loaded apps and its assumptions say so ("the triggers of a record operation on a table not known, of the loaded tables only"); tables of apps not loaded that a RecordRef opens by number are outside that world, as their subscribers are (owner answer q-20261111T044238-cead8f76). The same operation on a local RecordRef opened on a literal table follows that table, as before; a local RecordRef whose provable bindings name no one loaded table (a table not loaded, or two tables) counts as a table not loaded. **Left, older than B10:** (1) a trigger-firing operation whose receiver is not a plain variable (`RRs[1].Modify(true)`, `GetRef().Modify(true)`, a record array element, L36) resolves as a built-in call with no trigger edge and no gap, known table or not; (2) a table loaded from symbols only has no trigger nodes, so its triggers are unseen but an operation on it reads as running none. Both can still give a wrong `no`; (1) is folded into ENG-12 (with L36), (2) is filed as ENG-16. **(1) fixed by ENG-12** (2026-11-12): an array element is a record operation (L36); a record operation on a function's returned record of a known table (`GetRef().Modify(true)`, `GetRef().FindFirst()`; none on CDO) is a record operation of its own: the resolver records its table (the function's declared return type), triggers (by the site's RunTrigger rule) and table events (`SiteFacts::call_result_record`), and the model builds the operation at the call site (id `{call site}/callresult`) with its trigger and table-event edges, as ENG-06 does for `FieldRef.Validate(…)`; reads, triggers and table-event subscribers all follow. A RecordRef result is an honest open dispatch (runtime-typed, `DynamicOpen`; on CDO 3 sites, all in dependency code: `CreationFactory.GeteDocumentHeader().Modify()`, whose `GeteDocumentHeader(): RecordRef` returns whatever record `SetEDocumentHeader` was given); a record of a table not loaded keeps one unresolved route. An object global through `this` (`this.X.FindSet()`, `this."X"`; 13 sites in CDO's dependencies) is a record operation on the global (`record_ops::receiver_variable`, keyed `this.x` in the receiver sets only for a record global no parameter, local or return value of the routine declares, `extract::this_receivers`); its receiver text drops `this.`, so `this.G` and `G` are one record. A `this.X` a same-named declaration shadows is typed by the resolver from the globals and takes the call-result path (review round 3: it had picked the local). Left: a call-result operation's temp state is unknown (the function may return a temporary record); a shadowed `this.X` of a record global is a call-result operation, so its temp state is unknown too. The "without load" detectors (d11, d21) see a call-result operation by its receiver text (`GetRef()`); on the ENG-12 fixture they raise nothing for it (measured: no finding with or without a skip), not measured on CDO (no known-table call result there). | ENG-06 probe (`DoRecRefModifyUnknown`), `tests/cli/cli_b9_fieldref_validate.rs` | S |

The B8 items, all **fixed in wave 0** except the two still open above (the query `scope:` line,
`effect_query_cli.rs:5611/5618`, and `.gitignore`); the UpdLock half of the `locks: none` item is
L15 (TLS-44 audit, 2026-11-13):
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

### B2 in detail

CHANGELOG sections: "engine limits wave 2, B2", "B2 per-arm ids", "B2 final review" and its
follow-ups 1 to 5.

**What was wrong.** The lowerer spliced the arms of a statement `#if` into one list. An `exit` or
`Error` inside one arm then cut off the code after the group in every configuration, so prove
answered a wrong `no (every candidate decided)` and a wrong `unconditional`. A routine declared
in two arms cited the second arm's line for the first arm's write.

**Fixed: statement `#if` arms** (`fix/w2-b2-preproc-arms`).
- A statement `#if` chain is an IR `PreprocGroup`. Its arms are alternatives: a `case` in the
  statement tree, with an implicit empty arm when there is no `#else`.
- `control_flow::may_exit` is the one rule for "can this statement return early". Both statement
  walkers apply it to every statement (an `if`, a `case` or group, a loop, a bare block, a `with`
  body):
  - in `control_context`, the code after such a statement is `conditional`. This also fixes the
    older plain-AL shapes `if a then begin if b then exit; end; Insert()` and the bare block;
  - in `operation_order`, no later operation dominates the success return. The ordering facts had
    their own one-arm-deep rule, so an arm that may exit gave a false `COMMIT_DOMINATES_RETURN` /
    `WRITE_COMMITTED_BEFORE_RETURN`, and, through a call, a false `exactly_once_on_success` in
    `ordering_inter`.

**Fixed: an attribute or property only some builds have.**
- The lowerer marks it: `AttributeIr::conditional`, `ObjectProperty::conditional`.
- These readers read a marked one as "may". (The properties not read this way yet are under
  "Left" below.)
  - `[CommitBehavior]`: `commit_scope::declared`, `return_summary`, the ordering grades.
  - `[TryFunction]` as a trap.
  - An XmlPort's `Direction`; `TableType = Temporary`; `SourceTableTemporary`; `UseTemporary`.
  - The page permissions `Editable`, `InsertAllowed`, `ModifyAllowed`, `DeleteAllowed`: the
    UI-surface block and the model fields d39 and d64 read.
  - An event's isolation: an event isolated in some builds only is no trap, and
    `prove throws-error` says so.
- `InherentCommitBehavior` per arm takes the strictest value for `commit_scope`. The return
  summary reads a conditional one as `conditional`, never as a definite `ignore` or `error`.
- A commit or commit-error route under a declaration that only may apply is `conditional`.
- Third review: a routine declared in two arms is one row per arm. An attribute that one arm's row
  carries and another's does not is conditional on every row, so an `#if`-only `[CommitBehavior]`
  or `[TryFunction]` on one arm no longer gives a definite `no`.

**Fixed: one id declared in several places** (two arms of a member list, two whole-object arms,
or two files; final review).
- The third review compared rows inside ONE object declaration only (the lowerer). So a
  `[TryFunction]` in one of two whole-object arms, or in one of two files, still trapped in every
  build: a wrong `throws-error: no`, because `LiteralReach::new` and `callers_context` read the
  id's first row.
- The rule now lives in the model. `workspace::join_alternative_declarations` reads every row of
  an id (any object declaration, any file) as one alternative build:
  - An attribute not every row carries is marked `another_declaration_lacks` on the rows that
    have it. That is a "may" for a reader of the routine from outside (`has_definite_attribute`)
    and definite for that row's own body (`find_own_definite_attribute`). So a declaration's own
    `[CommitBehavior(Ignore)]` ignores its own `Commit` in every build that has it, and
    `may-commit` is `no` when only that declaration commits.
  - A `top-level` site is `conditional`. Before the join this was counted per file, so two files
    were missed.
  - The rows of two files are numbered apart, so every site id names one site. Each file's site
    numbers sit in a block picked from a hash of its own source unit (`file_site_blocks`). A
    file's ids do not move when another file's content, name or sort order changes, except when
    two files' hashes pick one block.
  - A table is temporary only when every declaration is (a table is read by id from anywhere).
- Object rows are NOT joined. A routine reads its own declaration's object row (#279:
  `SingleInstance`, page permissions, `SourceTableTemporary`, `InherentCommitBehavior`), and that
  row holds in exactly the builds in which the routine exists. The readers that ask about an
  object from outside merge its declarations themselves: `commit_scope`, the program graph's
  `dedup_objects`, and the return summaries per id.
- The return summaries (`compute_return_summaries`) merge the rows of an id. A value the rows
  disagree on is `unknown` or `conditional`; before, it was the last row's.
- The program graph merges the copies of an object or routine from any file:
  - an XmlPort's dataset (`DatasetNode::merge`: a direction either allows, every item, an item
    property only where they agree);
  - the event facts (`EventFacts::of_arms`, which kept the FIRST file's subscriptions, so a
    subscriber declared only in the second file was lost in every build).
- The commit-scope graph (`CbGraph`) gives each row of a routine whose rows declare different
  behaviors its own node (`CommitScopes::arms`). A commit runs under the declaration of the row
  that holds it, or of the row whose call site a route leaves through.
- The snapshot clears `dominates_success_return` on every row of a collided id, frames or not. A
  two-arm callee's commit gave its caller a root `COMMIT_DOMINATES_RETURN`, valid for refutation.
- A caller's digest names each reached site by its own operation id. An inherited effect
  enumerated per site took the representative fact's id (`…/op1` at two lines; plain AL with two
  commits in one callee had the same defect).

**Fixed: a subscriber whose declarations subscribe differently** (final review follow-ups 1 to 5).
- The event-fact union says what the routine subscribes to in some build, not which declaration's
  body a subscription runs. So such a subscriber is split into one routine NODE per declaration,
  in the program graph itself: `program::split_decls`, decided once in `build::split_declarations`
  on the parsed subscription sets, before the dedup.
- Each declaration node has its own id (the `sig_fp` mixed with a hash of the declaration's source
  unit and its ordinal in the file), its own subscriptions, its own table-event and out edges, and
  its own source lines. The model takes each row's ids from its node
  (`workspace::adopt_declaration_ids`: stable id `{stable}~{8 hex}`, site ids).
- Name lookup shows one declaration per routine (`resolve::reads::RoutineRun`), so overload logic
  is untouched. A direct call reaches every declaration (`split_decls::expand_call_routes`), a
  dispatch only the declaration that subscribes.
- The callers' walker, the sited graph (`alsem tests`, `alsem impact`), `alsem graph`, the LSP and
  the model read the one node set, so they agree by construction. That includes the model's
  walkers: the cone, the commit-scope graph, the L4 summary solver, the digest's witness search,
  the detectors.
- A selector naming a split subscriber by name or by the routine's base id means every
  declaration; `~hex` names one.
- `alsem graph` carries each declaration's lines: a node per split declaration, and on an unsplit
  routine with several declarations the additive `otherDeclarations` field.
- Example of the old wrong answers: `#if A [EventSubscriber(…OnX…)] H: Commit #else
  [EventSubscriber(…OnY…)] H #endif` made raising `OnY` a definite `may-commit: yes` (since #103
  in one file), a `[CommitBehavior(Error)]` on the `OnX` declaration made raising `OnY`
  `throws-error: yes`, and `query touches` listed `OnY` for the `OnX` declaration's write.

**Fixed: the line a two-arm routine cites** (`fix/w2-b2-arm-ids`, owner decision 2026-11-09,
CHANGELOG "B2 per-arm ids").
- Each `#if` arm's declaration of one routine id numbers its site ids (`op`, `cs`, `loop`, `u`,
  `rule`) after the earlier arms' in the same file (`ir_walk::SiteIdBase`).
- A reader that takes a routine's row by id and then looks a SITE up in it reads the row (the arm)
  that holds the site (`projection::ArmRows`, `DetectorContext::routine_for_site`,
  `LiteralReach::row_at`). A reader that walks the whole routine reads every arm's row (the
  transaction span's pending writes, d1's and d2's terminals, d46's commits, d48's IO, impact's
  own keys). The readers: d1, d2, d8, d9 (its count and its location), d32, d35, d39, d43, d46,
  d48, the transaction spans, the summary solver's temp substitution, the cone's guard frames, the
  literal search (traps, guard frames, route sites), prove's witness and route, the callers'
  context, impact's keys and effect evidence. The integration with round 2 routes the readers
  round 2 added the same way (CHANGELOG, integration entry).
- The older rules for rows that share site ids (#160 step 11b) are still there. Their tests now
  state the shared ids by assignment (`workspace::share_collided_site_ids`).
- Per arm: a duplicated declaration is one producer per arm, an interface call is one site per
  arm (each `candidate_count` the number of implementers), and d1 seeds each arm at its own site.

**Left.**
- *What the engine does not choose.* No configuration is chosen (L22 later steps), so an arm's
  effect is `conditional`, never `unconditional`, even in the build that compiles it.
- *Not yet read as conditional.* These can still give a definite answer that holds in only some
  builds:
  - a local or global declared `temporary` in one arm only. A `VarDecl` has no arm mark; one
    declaration's temp state decides (the first for a global, which the row builder keeps once;
    the last for a local, in `record_types`' name map);
  - a `SourceTable = X temporary` marker in one arm (`node_extract`'s
    `ObjectNode::source_table_temporary`);
  - `ObsoleteState` in one arm (`obsolete_gone`; it picks which object a name binds to, a
    resolution choice);
  - `Access` in one arm (`internal`);
  - `SingleInstance` in one arm (`bool_prop`).
- *Other readers.*
  - A raiser's route through an event isolated in some builds only says `no guard on the route`
    without naming the builds that isolate it.
  - `alsem graph` does not label a subscription whose `[EventSubscriber]` is in one arm.
  - No CLI shape reaches the ordering engine's proven-errors rule through an `#if`-only
    `[TryFunction]` (a try routine's own ordering is opaque); that hook is pinned by a unit test
    only.
  - The lowerer's own per-declaration rule (`mark_attributes_some_arm_lacks`) sees one object
    declaration only. So the IR (the L2 projection's attribute JSON) marks an attribute that only
    a whole-object arm or another file lacks as definite. No model reader reads the IR's mark.
- *An `[EventSubscriber]` the engine cannot read* (0 of 3,629 on CDO and its dependencies) is a
  coverage gap, never a definite `no`:
  - `read_event_subscriber_ir` keeps its readable parts. Every publisher routine and record
    operation they allow carries the `unreadable-event-subscriber` uncertainty, so prove, explain,
    digest and query say `unknown` or `partial`.
  - A table validate event's field in no readable form is one too. A readable field narrows the
    gap to a `Validate` of that field.
  - A declaration with one counts as subscribing differently only when its readable parts rule out
    every readable subscription of its routine. It stays reachable from each readable sibling's
    event those parts allow.
  - `alsem graph` marks it `hiddenCallers: unboundSubscriber`.
  - The gap is reported at the dispatch, not followed into the body. With no readable sibling its
    effects are not seen, so a route through it is `unknown`, never a `yes`.
- *Selectors and ids of a split subscriber.*
  - `prove`, `digest`, `tests` and `impact` resolve a split subscriber named by name. The
    fingerprint query lists its declarations as candidates (`differs_in` says "the file or #if
    arm").
  - An unchanged file's routine moves between its base id and `~hex` when another file's
    subscription change starts or ends the split. This is a one-time move; always suffixing every
    routine declared twice was not worth it (CHANGELOG).
  - A routine declared in two `#if` arms or two files that subscribes alike stays one node.
    `otherDeclarations` lists its other places in `alsem graph`.
- *One id in several places, site numbers.*
  - A table's fields and keys and an object's other properties (`ObsoleteState`, `Access`) are not
    joined across declarations (a resolution choice, as above).
  - Renumbering two files' rows happens at every assembly; the row cache keeps each file's own
    numbering.
  - A file's site ids for such a routine move when a second file starts or stops declaring it (the
    block applies only then) and when its own path changes.
- *Reading arms.*
  - A ROUTINE-level location (d9's and d8's routine, a finding's routine declaration) is the arm
    that holds the finding's site; where a finding has no site it is the id's last row.
  - The literal flow program is built for a routine's first row only. Another arm's call reads no
    flow value (no literal pruning there, the conservative side).
  - `closed_world_temp` still refuses to prove a routine with several rows (fail-closed).
  - The `earlier exit if …` guard is not carried past a group, so such an `Insert` reads plain
    `conditional`.
  - Split shapes (`#if` inside an `if` header, a split call or `case` header) keep their union
    reading.
  - Collecting walkers (record-ref tables, closed enums, first-assignment) still read the arms in
    document order as one list.
  - Site guards list no `#if` arm as a guard.
- *Imprecise but safe.*
  - A `case` arm or two-arm `if` arm that always raises still makes the code after it
    `conditional` (older, conservative).
  - An attribute or property whose arms together cover every build (`#if X [A] #else [A] #endif`)
    still reads as conditional.

## 2. All limits

### Locks and transactions

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L1 | Reads, locks and validations listed one dependency routine deep. Deep reads and lock facts already ride the cone, so this is mostly output. Validations need a new cone fact. | found | M | — |
| L3 | No lock state along call paths and no lock order. Answers must list held tables, never give a "safe" verdict. Lock escalation and SQL range locks are not modeled. | found | L | L1, L15 |
| L15 (P1) | `ReadIsolation` UpdLock/RepeatableRead and `FindSet(true)` are plain reads, never locks. On CDO, `RunMigration :307-308` takes an update lock and the answer says `locks: none`. Approach: record isolation per read in the body walk, then emit a lock fact. | confirmed | M | — |
| L16 (P2) | **Fixed in wave 2 (B4).** `[CommitBehavior]` now decides every forward commit and error answer (prove, its `--site/--match` slice, digest, explain). ENG-09: a checked `Codeunit.Run` commits (item 2). What is left, numbered: [L16 in detail](#l16-in-detail). | fixed (remainder stated) | M | — |
| L27 | `callers-have-pending-writes` says `no` when a caller holds a `LockTable` but wrote nothing, so an agent about to add a lock gets no hint. | new | S | L3 |
| L28 | A checked `Codeunit.Run` inside an open write transaction is a run-time error in BC. CDO avoids it by committing first (`CDOQueueManagement:46`). Measured on the BC 28.4 Linux sidecar (ENG-09 probe): an `Insert` followed by a checked run fails at the run ("An error occurred and the transaction is stopped.", inside a `[TryFunction]`). Confirmed on Windows (BC 28.0.54946.56477, owner, q-20261112T021007-384a506a): the run raises that error itself and it is not caught by the checked form (the run does not return `false`), whether the callee succeeds or fails, and the caller's write is not committed. Probe code and results: `docs/superpowers/notes/2026-11-11-eng09-checked-run-probe.md`. **Wave 2 (B7 review):** the forward error answers read a pending write at the run. **ENG-22:** the run's own error is an error site, raised where a write may be pending at the run and not raised (with the callers' entry assumption) where none may; d50 reports it. **ENG-11** re-probed this wrong answer on master `5ebb5e01`: fixed by ENG-22 (L28 in detail). d50's coarser pending walk is ENG-24. Details and what is left: [L28 in detail](#l28-in-detail). | partial | M | L23 |

#### L16 in detail

**Fixed.** `[CommitBehavior]` reaches every forward commit and error answer through the rule the
callers' question reads (`commit_scope::declared`).
- A route that crosses a checked `Codeunit.Run` keeps a commit that runs normally or is ignored as
  a commit (Learn: the run's implicit commit is not affected by `CommitBehavior`).
- Under an `Error` that may follow into a checked run, the run traps the error and the commit may
  not happen: `may-commit yes`, never `unconditional on success`, and no commit error row. This
  speaks of a commit error raised inside a run that has started. If a write is pending at the run,
  BC raises a run-time error at the run itself and the body never runs; B4 still adds no commit
  error row there. L28 states what that does to the answer.
- Every other way of starting an object only *may* carry the scope (Learn is silent): any
  `Run`/`RunModal`, a report's `SaveAs*`/`Execute`/`Print`, an XmlPort's `Import`/`Export`, a
  query's `Open`/`SaveAs*`, keyword or variable form, and the started object's dataset operations.
  This is decided once, where the call resolves (`dataset::starts_object` →
  `SiteFacts::starts_object` → `CallEdge::starts_object`). A crossing into a dependency enters by
  the same step.
- Round 2 (`fix/w2r2-b4`) fixed three things: a run into a dependency, a report variable's `.Run()`
  and the B6 run routes gave a definite wrong `may-commit no` under `Ignore`; a "may" step below a
  plain call lost its "may" in the backward search; the `--site/--match/--op` slice did not trap a
  commit turned into an error.
- Round 3 fixed an object run *inside* dependency code (a dependency routine's `Codeunit.Run`,
  checked or not, or `R.Run()`). Reached by a plain call from an `Ignore` scope, it gave a definite
  wrong `may-commit no`: the run steps were read from the model's calls only, not from the
  dependency code's calls the cross-app tree keeps apart. The prefix at a crossing is now one
  forward walk per root instead of a whole-graph backward search per crossing routine.

**Left.**
1. `InherentCommitBehavior` is parsed (the probe was wrong: `workspace.rs` reads it) but is not on
   Microsoft Learn, so it only *may* apply: it never removes a commit, and under `Error` adds a
   possible error.
2. **Fixed (ENG-09): the implicit commit of a checked `Codeunit.Run` is a fact.** Before, a
   checked run whose codeunit has no `Commit()` answered `may-commit no`, and a run-time id
   `unknown` (re-probed on master `c498fd98`). Measured on the BC 28.4 Linux sidecar: after
   `if Codeunit.Run(W) then;`, where W inserts a row and never commits, a second checked run
   succeeds, which BC refuses while a write is pending; so the run committed. The same held under
   the caller's `[CommitBehavior(Ignore)]` and `(Error)` (no error) and for `Ok := W.Run()` on a
   codeunit variable. So `program_calls` marks each checked run (`PCallSite::checked_run`, by
   `traps::is_checked_run`: the keyword form by its shape, a run-time id included, the variable
   form when the resolver decided the call starts a codeunit, with or without an `OnRun` row), and
   the capability cone gives the site a commit fact witnessed by the call site, with the resource
   id `checked-run` (`capability_cone::RUN_COMMIT`, read by `commit_scope::is_run_commit`). The
   resource gives it its own cone entry and witness carriers: review round 1 (astra, Fable) found
   that without it a root reaching both a `Commit()` and a checked run had ONE inherited commit
   whose representative decided the `[CommitBehavior]` rule for both (`[Ignore]` over a `Commit()`
   and a deeper run gave a wrong `may-commit no`). No `[CommitBehavior]` rule applies to it (the
   digest's split, `CbGraph`'s commits, the `--site/--match` slice). A failed run commits nothing
   and the caller carries on, so it is at least `conditional`, never `unconditional on success`.
   In the ordering engine it is `COMMIT_REACHABLE` only: it commits the run's own transaction,
   never a write of the caller (one pending at the run is L28's error, one after it is not in the
   run); for the same reason no IO of the caller is "before" it (`EXTERNAL_IO_BEFORE_COMMIT`). It
   does not set the cone's `MAY_COMMIT` flag, so d34 and d35 (which ask about a `Commit()` that
   commits the caller's work) find what they found before. The explicit commit inside a run is
   still read as the run's commit too (above).
   Left: a run that always fails (its `OnRun` raises an error on every path) still answers
   `may-commit yes` (an over-statement, never a wrong `no`); `Ok := Codeunit.Run(X); if not Ok
   then Error(…)` answers `commits-on-success-path unknown` where `yes` holds (an under-statement);
   the `CbGraph` carrier exclusion has no test that reaches it (only an `#if`-arms routine where one
   arm holds only a checked run and another a `Commit()` would differ); d34 counts a `Commit()`
   only, so a checked run in a loop is no finding. ENG-22: `transaction_spans` and d50 no longer
   read a checked run as committing the caller's posting (L28): d50 reports the error BC raises at
   a run with a physical write pending.
3. `alsem fingerprint` (and `policy check`, which reads its capabilities) does not apply the rule:
   it says `commit: yes` for a root whose only commit is ignored or errors. That over-states; it
   never says a wrong `no`.
4. A commit turned into an error is trapped like any other error (B7/L23), in prove, its slice and
   the digest alike. Inside a used `[TryFunction]` it is caught (`no`). Inside an isolated event it
   escapes only if a write is pending when the event is raised. The trap is ruled over every route
   to the commit, not only the routes that run it under `Error` (sound; it may over-report).
   Inside a checked run there is no error row at all (see "Fixed" above), even with a write
   pending at the run; the run's own error is L28's.
5. `query effects` lists table effects only, so it has no commit to change.
6. A slice site that is a commit-turned-error names no reason (the unfiltered answer does).
7. Whether the scope follows into a started object (checked or not) was not checked on a BC server
   (the BC 28 sidecar).
8. A dynamic run (`Codeunit.Run(Id)`, `Report.Run(Id)` with a computed id) has no target, so a
   commit in it is unknown, as before. A checked `Codeunit.Run(Id)` commits all the same (item 2).
9. Two body-level lists of the keyword run forms stay apart from `dataset::starts_object`:
   `combined_graph::keyword_run_kind` (the typed edge of a dynamic keyword run) and
   `ir_walk::object_run_kind` (the L2 callee shape). They answer other questions and move no commit
   answer.

#### L28 in detail

**Fixed in part.**
- Wave 2 (B7 review): the forward error answers read a pending write at a checked run. A run at
  which a write may be pending (a write before it in its routine, a callee that may leave one, or a
  caller in the root's reach that calls with one pending) is no definite trap. A `Commit()` on the
  way clears it, also one earlier in the same loop iteration. A caught ruling that may rest on a
  checked run states that it assumes the root's callers have no write pending.
- Round 2 (`fix/w2r2-b7`): a `Commit()` clears only when it runs normally on every route from the
  root, by B4's `[CommitBehavior]` rule (`CbGraph::commit_surely_runs`). Under `Ignore` the write
  stays pending; under `Error` the commit raises an error and commits nothing (Learn,
  "CommitBehavior attribute"). Before, `[CommitBehavior(Ignore)] … ModifyAll; Commit(); if
  Codeunit.Run(Thrower) then;` answered a wrong `no`.
- A run that is clean only because a `Commit()` ends the write adds a second assumption: the callers
  do not call the root under `[CommitBehavior(Ignore)]` or `(Error)`. explain and digest now state
  both assumptions too (header assumptions line, the caught line, JSON
  `summary.assumesNoPendingWriteAtEntry` / `assumesNoCommitBehaviorAtEntry` and
  `coverage.assumptions`), not only prove.

- **ENG-22: the run's own error is an error site.** Each checked run (keyword, variable and dynamic
  form; `PCallSite::checked_run`) carries an implicit error at its call site (resource
  `checked-run`, its own impact key `error|error-implicit|checked-run`), labelled
  `ERROR (checked Codeunit.Run with a write pending)`. It is ruled per root
  (`LiteralReach::run_error_verdict`, in the digest's trap pass and the `--site/--match` slice):
  where a write may be pending at the run on some route it is raised, never `unconditional`
  (`escapes-if-write-pending`, kind `checked-run-pending-write`); where none may, it is not raised
  (prove prints "not raised", JSON a `caught` verdict of that kind) and the answer states the
  callers' entry assumption, plus the `[CommitBehavior]` one when a `Commit()` ends the write. A
  trap outside the run (a used TryFunction around it) still catches it. Before,
  `R.ModifyAll(V, 1); if Codeunit.Run(Codeunit::"C") then;` with a `C` that raises nothing
  answered `throws-error: no (every candidate decided)`.
- ENG-22 review (both holes also moved the existing "caught by a checked run" rulings): a run in a
  `while` or `repeat` condition counts the loop body as run before it (an earlier pass), and a
  subscriber is entered with a write pending when another subscriber of the same event may write
  (their order is unknown). The second applies to the trap search only; `transaction_spans`'s own
  pending walk (`pending_writes`) still leaves sibling subscribers out (its stated
  under-approximation).
- ENG-22 review round 1 (astra, Fable), all in the trap search: a call in a loop runs again after
  its own earlier pass, so what it left is pending at it (`Before::repeats`); a subscriber or
  trigger of a table event shares the sibling rule (subscribers of one record operation), and one
  fired by a write operation is entered with that write pending (conservative for `OnBefore…`
  events and `OnInsert`-style triggers, which run before the write); `LockTable` counts as a write
  (measured on the BC 28.4 sidecar, confirmed on Windows BC 28.0.54946.56477 by the owner,
  q-20261112T043721-7da83bae:
  `LockTable` then a read makes `IsInWriteTransaction()` true and a checked run after it fails). d50 also seeds the variable form (`Ok := C.Run()`). Round 2: a
  call in a `while`/`repeat` condition counts what an earlier pass of the whole condition ran
  (`while Step() do;`, `Step` running a checked run and then writing).
- **`if IsInWriteTransaction() then Commit();` ends the write** on both paths, in the trap search
  and the transaction spans (d50). Measured on the BC 28.4 sidecar (probe note, second probe):
  `IsInWriteTransaction()` was true exactly when a checked run failed, in all seven states tried
  (nothing, a pending insert, after a `Commit()`, a plain `Get`, `LockTable` + `FindSet`, a
  `ModifyAll` that matched no row, the guarded commit itself); confirmed on Windows BC 28.0.54946.56477 (owner,
  q-20261112T043721-7da83bae: all seven states the same). Only
  that shape: the whole condition is the platform's call (resolver-decided,
  `PCallSite::write_txn_query`, so a procedure of that name does not count), not negated, no
  `else`, and the `then` branch is one `Commit()`. CDO's `CDOLegacyeDocDispatcher:254-255` has it;
  d50's one CDO finding there was a false positive from the branch commit.
- **ENG-11 re-probe (2026-11-12, master `5ebb5e01`): the wrong answer no longer reproduces; fixed
  by ENG-22.** `prove <routine> throws-error` on four routines, each running a checked
  `Codeunit.Run` of a codeunit whose `OnRun` is empty: `R.ModifyAll(V, 1)` before
  `if Codeunit.Run(…) then;` answers `yes` (`ERROR (checked Codeunit.Run with a write pending)`
  at the run, conditional); `R.Insert()` before the variable form `Ok := C.Run()` answers `yes`
  (the Insert's error and the run's own); `R.LockTable(); if R.FindFirst() then;` before the run
  answers `yes`; `R.ModifyAll(V, 1); Commit();` before the run answers `no`, the run's own error
  "not raised (no write may be pending at the run)", with the callers' entry assumption. d50's
  pending walk was coarser than prove's: **fixed by ENG-24** (2026-11-13; the d50 item below).

**Left.**
- An error row of the callee under a run a write may be pending at keeps its old ruling
  (`escapes-if-write-pending`), though with a write pending the run raises its own error before
  the callee runs (an over-report, never a wrong `no`).
- A run's own error under a trap on some routes only keeps that trap's words, not the run's
  condition (still escaping, never `unconditional`). A run under `asserterror` at its own call site
  is not ruled as caught.
- A run's own error in dependency code beyond a crossing is part of the crossing's group row, not
  ruled per run.
- An unchecked `Codeunit.Run` with a write pending was not measured; nothing is added for it.
- The same shape with `[CommitBehavior(Error)]` on the routine, where `C` only does `Commit()`, and
  no write pending: the commit error inside the run has no row (B4/L16, item 4).
- "Not raised" is carried as a `caught` ruling of its own kind (`checked-run-pending-write`), so a
  count of caught error sites (digest `caughtErrors`, prove's caught list) includes clean runs'
  own errors; the human lines name them apart ("not raised").
- A trap outside the run that catches its error is assumed from the trap rule, not measured for
  an outer checked run (`if Codeunit.Run(Outer)` whose `OnRun` writes and then runs a checked
  `Inner`); the probe measured a TryFunction only.
- The cone carries the run's error unconditionally; only the answers that run the trap pass
  (prove, its slice, explain, digest) rule it. `alsem fingerprint`/`policy check` facts and impact's
  key list a possible error at every checked run (an over-statement; impact's label says
  "raised if a write is pending at it").
- d50 reads `transaction_spans`, whose pending walk counted only writes to a known table (a
  write whose table does not resolve, a `LockTable`, unknown code and sibling subscribers were
  left out: wrong negatives). **Fixed by ENG-24:** one rule decides "may this leave a write
  pending" for the trap search, the spans and the cone fold
  (`cone_derived::op_may_leave_write_pending`, `pending_writes::is_unknown_code_call`, which
  leaves a checked run out, a dynamic `Codeunit.Run(Id)` included), and both readers follow
  unknown code back through calls with one closure (`pending_writes::reach_back`, never through
  a checked run). A span names what has no physical table (`pending_other`: "a LockTable", "a
  write to a table that does not resolve", "unknown code at <call>", "unknown code behind
  <call>"). Review round 1 (astra) found the first walk wrong in the other direction: a record
  operation whose OnAfter table event's subscriber reaches the run had its own write left out
  (the probe had tested a subscriber that writes, not one that runs), and the other callees of
  a dispatch counted only for an event subscriber outside the span. Now every callee of one
  dispatch that may run before the one leading to the run or Commit counts, by run phase (an
  OnBefore table event's subscriber, then the table's trigger, then the write, then an OnAfter
  subscriber or an event subscriber; callees of the same phase in either order), a span member
  among them, each through its cone minus its own checked runs. Their physical tables join the
  span's pending tables and `pending_order_unknown_tables` lists the ones only they write. d50
  (opt-in) counts them, marked "(order unknown)"; the default d8 and d9 gate on and list only
  the tables whose order is known (`known_order_pending_tables`, orchestrator ruling: precision
  a default detector cannot measure is noise), so the others never make a d8/d9 finding nor push
  d9 over its two-table gate, and appear only in the text ("possibly also pending (order
  unknown)"). The trap search changed on purpose: a dynamic checked `Codeunit.Run(Id)` is no
  longer unknown code (CDO's one site, `CDO Legacy eDoc Dispatcher.RunExportEDocCodeunit`, moves
  no `prove` answer). Review round 1 (gpt-6.1-sol): a callee with no row counts as unknown code
  here too, as in the trap search (`UnknownCode::build` seeds it), and a codeunit variable's
  checked `Run()` the resolver decided (`PCallSite::checked_run`; no `OnRun` row, or a symbol-only
  dependency's) is no unknown code in either reader. Left: each reader seeds the closure
  from its own inputs (the trap search from its guarded out-edges); a `LockTable` on a temporary
  record counts as pending in both readers (an over-approximation the shared rule keeps); the
  digest's pending line and explain's pending report still name physical tables only (explain
  passes no unknown-code sites); the digest's transaction context of a Commit whose only
  pending tables are order-unknown is `span-may-have-writes-order-unknown` (neither "known" nor
  "no known" writes); a table event's phase is read from its name (`onbefore…`; any
  other name counts as after the write). On CDO (base `67d75bea`) no default detector moves; d50
  goes from 0 to 2 findings, both resting only on unknown code in dependencies (a chained
  interface call left open, ENG-64, and a telemetry logger registered at run time) and false in
  practice; it stays opt-in. A helper's writes
  inside its own checked run no longer count (the CDO landing measurement: both d50 findings on
  CDO's send-on-posting were this shape, and with the fix d50 has none there); this reaches
  workspace routines only, so a checked run inside dependency code still counts its writes (an
  over-count). The same walk serves the explicit-commit spans. A routine is walked only when its
  cone follows no edge its forward sites lack, it has a summary and one row (otherwise its whole
  cone counts), and what a walked group's cone holds that its own writes and the cones it calls
  outside the group do not explain still counts. Left: that last check subtracts by table, so a
  cone fact with no edge behind it, on a table a skipped run also writes, would still be dropped
  (no such fact is known; on CDO nothing was unexplained, 4,694 routines checked).
- A locking read without `LockTable` (`FindSet(true)`, `Find` for update,
  `ReadIsolation := IsolationLevel::UpdLock`) was not measured and does not count as a write.
- A group row beyond a dependency crossing (a run's error in dependency code) gets its terminal
  witness from the run's own terminals (review round 1); no test reaches a group row of this kind.
- Platform-side database writes before a checked run (`IsolatedStorage.Set`, Media import and
  similar) do not count as a pending write. Whether BC counts them as a write transaction is not
  established.
- An event with no loaded subscriber before a checked run counts as no write. That rests on the
  loaded-program assumption every answer states.
- A `Commit()` inside a callee before the run still ends nothing (the pending-writes walk's known
  over-approximation).
- The transaction spans and the callers' questions still read every `Commit()` as a commit (B4's to
  decide).
- An explicit commit inside a checked run is read as possibly under its caller's scope (Learn is
  silent). No current answer depends on it, since an error inside a clean checked run is caught by
  that run anyway.

### Values (literal pruning)

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L4 | **Cause corrected:** literals are carried through any number of calls. m10 fails because **platform option types** (`TelemetryScope`, `Verbosity`, `DataClassification`, `ObjectType`, …) are not a value domain (`literal_values.rs:298 domain_of`). | partial | S | L14 |
| L5 | Pruning gaps by design: globals, conversions, unresolved labels, values from data (91 of CDO's 323 `case` statements switch on a record field), and arithmetic. Each keeps both branches live. | confirmed | L | L4, L14, L21 |
| L29 | **Text and Code literals are not a value domain:** `case T of 'A':` keeps every arm live. | new | S | L14 |
| L6 | The company a write targets after `ChangeCompany` is unknown. All 12 CDO sites pass a variable, so it needs values from data. | confirmed | M | L5, L21 |
| L22 (P8) | `#if`: no configuration is chosen. Step 1 (B2, wave 2) is done: statement arms are alternatives, not flattened, and a routine declared in two arms of one file gives each arm its own site ids, read through the arm that holds each site; one id declared in two files, or in two whole-object arms, is read the same way (the model-level join numbers two files' rows apart and reads every declaration as one alternative build; see B2 for what is left). Later steps: follow the app's `preprocessorSymbols` and a file's `#define`/`#undef`, infer each dependency's symbols, state the configuration in the answer, and make routines that exist in only one arm agree with the resolver's filter (a chosen configuration lets a two-arm routine's arm read `unconditional` again in the build that compiles it). | partial | L | — |

### Errors

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L7 | Whether a subscriber's error escapes its publisher. The answer is decidable locally: it escapes unless the event is isolated (see B3). Implicit errors (`Insert` failing because the record exists) name the operation, not the receiver or the reason. | partial | S | L10 |
| L8 | Guards past an unanalyzed dependency entry. 0 CDO calls reach it, because every dependency ships its source. The answer should name the routine without a body. | confirmed | S | — |
| L23 (P9) | Traps were not applied in the forward error answers (see B7). **Fixed in wave 2** (one trap rule, `src/engine/l5/traps.rs`, read by both directions). What is left: [L23 in detail](#l23-in-detail). | fixed (remainder stated) | M | L7, L14 |
| L30 | `ErrorBehavior::Collect` is modeled only in the callers' question, not in forward `throws-error`. 5 uses in CDO, 13 in Base App. **Wave 2:** a forward `Error` row whose route passes a collecting procedure says it may be collected and raised when that procedure ends (the error is kept: Learn says a collected error is still raised unless the code clears it). **Left:** whether the `Error` is collectible (an `ErrorInfo` with `Collectible = true`) is not read, and `ClearCollectedErrors` is not followed, so the answer never drops a collected error. | partial | S | L23 |

#### L23 in detail

Population: CDO has 34 TryFunctions and 8 checked runs; Base App has 327 and 30. The fix touches the
literal reach, so measure speed when changing it.

**Left.** (Numbered as CHANGELOG and earlier reviews cite them.)
1. A row that may stand for unseen sites (a site search stopped at its depth bound) is never ruled
   caught. A dependency boundary group is ruled only by the route to its crossing: a crossing that
   is itself a trap, or an event crossing, keeps the group escaping.
   - 1b. The cone's obligations (open-world and unresolved calls) are cone-wide. So a filtered answer
     whose only matching site is caught is `unknown`, not `no`, when an unresolved call sits anywhere
     in the cone, even inside the trapped part.
2. An interface call into a TryFunction implementer is not a trap.
   - 2b. The trap-free walk ignores guards (for speed), so an error whose only trap-free route the
     literal arguments rule out stays escaping.
3. The writes of a failed checked run are rolled back, but `writes-table` is a may-answer and says
   nothing about it, which stays correct.
4. A checked run inside an open write transaction raises its own error (L28).
   - The forward error answers treat a checked run at which a write may be pending (within the root)
     as no definite trap (`escapes-if-write-pending`). A caught ruling that may rest on a checked run
     states, in prove, explain and digest alike, that it assumes the root's callers have no write
     pending (and, when a `Commit()` before the run ends the write, that they call it under no
     `[CommitBehavior(Ignore|Error)]`; see L28).
   - An effect row that stands for several error sites is judged as a whole. Example: one
     `ERROR_THROW` row whose origins are a site behind a checked run with a write pending and a site
     with no trap, as in `Report.Run(R); if Codeunit.Run(Thrower) then;` where the report raises an
     error too. It escapes on a trap-free route, so it carries no trap verdict and its condition does
     not mention the pending write at the run. The answer `yes` is right.
   - The commit answers (`may-commit`, spans) still do not model that error. (The transaction
     spans' checked-run test counts a codeunit variable's `.Run` since ENG-22:
     `transaction_spans::is_checked_codeunit_run` reads `PCallSite::checked_run`; the doc comment
     at `traps.rs:83-86` still says otherwise. TLS-44 audit, 2026-11-13.)
5. A TryFunction call traps only as the whole right-hand side of `Ok := …` or the whole condition of
   `if`/`while`/`until`, through parentheses and `not`. Learn names `OK :=` and `if-then`;
   `while`/`until` and `B := not TryIt()` are read the same way, which goes slightly beyond Learn
   and was not checked on BC. Used as an argument, in `exit(…)` or inside `a and …` it is counted as
   an ordinary call (safe; whether BC traps there is not established, no BC probe was run).
6. Not modeled:
   - errors in a try call's arguments. Learn says they are evaluated inside the try function, so
     they are caught; the answers report them as escaping, which is safe;
   - **Fixed (ENG-10):** a write inside a try call on premises with
     `DisableWriteInsideTryFunctions` on (measured on BC 28 on premises, Linux sidecar and Windows
     28.0.54946.56477, q-20261112T104134-338e9d4a and q-20261112T110129-63d6683c; probe
     `docs/superpowers/notes/2026-11-12-eng10-write-in-try-probe.md`). A database write
     (`Insert`, `Modify`, `ModifyAll`, `Delete`), `LockTable` or `Commit()` that runs inside a try
     call raises an error that no try call catches (neither the try call itself nor an outer one);
     a checked `Codeunit.Run` or `asserterror` between catches it; a temporary record's write is
     allowed. Every use of the try function's value makes it a try call (`Ok :=`, a condition,
     `exit(..)`, an argument, `a and ..`); only a call statement is ordinary; what the call's
     arguments run is inside it. Each such call (`PCallSite::try_call`: any resolved callee, any
     `#if` arm declaring `[TryFunction]`) carries the error at its call site, ruled per site:
     raised (conditional, never unconditional: "if a database write runs inside it on a server
     that refuses one") when a write, `LockTable`, `Commit()` or unknown code may run inside it
     without crossing a checked run or `asserterror`, and not caught by any try call above;
     "not raised" otherwise. A filtered slice (`--site`, `--match`, `--op`) on a refused
     operation inside a try call rules it the same way: a refused operation raises the refusal
     where some route from the root runs it inside a try call (through the call, or in its
     argument list, an argument operation's trigger or event included) with no checked run or
     `asserterror` anywhere on the route, whatever trap another route names; one that has no
     error of its own (a `Commit()`, a tested `if not R.Insert()`) is an error site there too, and
     a try call where nothing may be refused is "not raised" in the slice as well. An implicit-
     error boundary group a try call catches is kept escaping when a crossing edge runs inside
     a try call on such a route and the dependency code beyond it makes a try call of its own
     that may be refused (that refusal leaves the outer try call, T14); a write it runs
     directly is the outer try call's own refusal row, and the group's other errors stay
     caught. A routine with
     no body (a symbol-only dependency routine, a `[TryFunction]` among them) is unknown code
     inside a try call. A root that is itself a `[TryFunction]` and may run a refused operation
     states the assumption its answer rests on: its callers call it as a statement (prove's
     assumptions and `obligations.assumesStatementCallsOnly`, explain's and digest's header).
   - **Left (ENG-10):** Learn says Business Central online has no such restriction; online was
     not measured, and the rows name the setting rather than claim either way. Platform writes
     (`IsolatedStorage.Set`, Media import and similar) are not counted as refused (not measured).
     A root that is itself a try function is answered as called as a statement (a tested write
     such as `if not R.Insert()` and a body that only commits or locks answer `no`); the
     assumption is stated, not answered both ways, and digest's JSON `coverage.assumptions` labels
     do not carry it. The refusal row names the try call, not the refused write inside it (BC's
     message names the operation). An impact key for the refusal does not carry its verdict: a
     write added inside an existing try call shows as the new write's own key, not as a new
     refusal error (a new try call does show one). A refused operation written directly under
     `asserterror` inside the try function (`asserterror R.Insert();`, no call between) still
     counts as refused, an over-report: the body walk marks `asserterror` only on `Error()`
     sites (an `asserterror` around a call is followed). "Not raised"
     is carried as a caught ruling of its own kind
     (`try-write-refused`), as ENG-22's clean runs are, inside the engine; every output counts
     it apart from the caught errors (prove's "not raised" line and count, the slice's and
     explain/digest's `notRaised`, not in `caughtErrors`; ENG-22's clean checked runs are still
     counted among the caught errors). A write whose execution depends
     on data (CDO's cached access token, bug #80680) is a "may". Whether the wider value uses
     (`exit(..)`, an argument, `a and ..`) also CATCH an `Error()` was not measured: the trap rule
     still reads only `Ok :=` and whole conditions (the safe side for catching).

### Declarations and objects

| # | Limit | Status | Size | Depends on |
|---|---|---|---|---|
| L2 | A rename misses consumers in declarations: CDO has 101 `CalcFormula`, 210 `TableRelation`, and about 1,750 page field controls. One sample field had 3 code consumers and 12 in declarations. Approach: one index of field names in declarations, used only by field answers. | confirmed | M | — |
| L17 (P3) | Permission failures are not predicted. Object `Permissions` (32 CDO objects), permission sets (252 entries) and `InherentPermissions` are never read. Ship as advisory first. | confirmed | L | L19, L20, L1, L2 |
| L19 (P5) | **Code reads fixed in wave 2 (B5).** Left: (1) a page that shows a FlowField calculates it when displayed; no routine's cone holds that read, so it needs L2's page columns. The same holds for a report or query column bound to a FlowField: the dataset reads (B6) name the dataitem's table, not the FlowField's source table. (2) A record passed to another routine: a caller's `SetAutoCalcFields` and the callee's `Find*`/`Get`/`Next` are not linked (the caller can answer `no`); a caller's filter on a FlowField is placed at the filter (the caller answers `yes`) but the callee's own answer can be `no`. Needs resolved calls with record arguments, which L3 resolve lacks; 0 such `SetAutoCalcFields` cases in CDO. The same holds for a FieldRef or RecordRef received from a caller: a `CalcField` or a filter on it answers `unknown` (table not resolved; a filter since the final wave-2 review, before it added nothing). (3) Reads stay one dependency routine deep (L1). (4) A report dataitem's `CalcFields` property (a declared read) is not read. (5) Sharing of the implicit `Rec` is keyed by the base object's NAME, so two same-named base objects of different apps share (a may-read, never a lost one). The sharing also does not tell instances apart: `X.TabFetchBare()` called on an `X` that never had `SetAutoCalcFields` answers a plain `yes` because another routine set up another `Rec` (a may-read). (6) The temp state of a RecordRef bound only by `Open(Database::X)` is not taken from `Open`'s default (not temporary): a FieldRef read on it says "temp state not resolved" (safe). (7) Record array elements: see L36. **Covered after round 2:** a bare `SetAutoCalcFields` on its implicit record frame (the `with` record, else `Rec`; the first fix-up's blanket "with" fallback, which gave wrong `yes` answers, is gone); the implicit `Rec` across the routines of an object AND its extensions (page, table, report), object globals (a base object's also in its extensions); a record-qualified field argument (`X.SetFilter(X."F", …)`, `X.CalcFields(X."F")`); `FieldRef.CalcField()` and a FieldRef filter on a FlowField (`ModelRoutine::field_ref_operations`); `RecRef.SetAutoCalcFields(<number>)`. | partial | S (left) | L2, L1 |
| L20 (P6) | The platform's own dataset operations were missing: report and query dataitem reads, and XmlPort import inserts (see B6). Trigger code is analyzed. **Fixed in wave 2** (`program::resolve::dataset`). What is left: [L20 in detail](#l20-in-detail). | fixed (remainder stated) | M | — |
| L35 | `Record.GetBySystemId` (and `RecordRef.GetBySystemId`) is not a record operation: it reads its table, and a `SetAutoCalcFields` makes it read the FlowFields' tables too, but `query effects` lists no operation for it and `query touches --direction down` can answer `no (every candidate decided)` for the record's own table. Found in the wave-2 B5 review (probes `SysIdMember`, `TabBareSysId`). Adding it to the record-operation map renumbers operation ids and changes detector input, so it is its own change. **Fixed by ENG-12** (2026-11-12, probe `docs/superpowers/notes/2026-11-12-eng12-record-ops-probe.md`): `GetBySystemId` is a record read (`record_ops::record_op_type`), with the read family's arms (table facts, summaries, the implicit error of an unchecked one in statement position, d11/d21's load ops, d5/d60's whole-set break, FlowField retrieval). Left: the performance detectors' retrieval lists (d1, d3, d36, d4) do not include it (they never saw it; adding it would be new findings to triage). | fixed | S | — |
| L36 | A record ARRAY element is not a record: `Recs[1].FindFirst()`, `Recs[1].SetAutoCalcFields(..)` and `with Recs[1] do FindFirst()` produce no record operation at all, so `query effects` lists 0 operations and `query touches --direction down` answers `no (every candidate decided)` even for the record's own table. Predates wave 2; found in the wave-2 B5 review (probes `ArrMember`, `ArrWith`, `ArrWithBareFind`). Needs the record-operation map to accept an indexed receiver; that adds operations (new ids, new detector input), so it is its own change. **Fixed by ENG-12** (2026-11-12): an array of `Record`/`RecordRef` receives through its elements in both engines (`record_ops::receiver_variable`, `receiver_type_of`); the receiver text stays `Recs[1]`, the table is the element type's (model pass 2b, and the resolver types the element through the record operation's receiver expression), so `Recs[1].Modify(true)` runs the table's triggers and `RRs[1].Modify(true)` is ENG-06's open edge (`unknown`). Review round 1: a parameter, local or return value of a non-record type shadows a record global of its name in both engines (`extract::routine_non_record_names`; before, a local `X: array[2] of Codeunit W` under a global `X: Record T` made `X[1].Modify()` a record operation, a wrong `no`; the same held for a plain name); an element of a `temporary` array is temporary (pass 2b); an indexed receiver's text drops whitespace outside quotes (`record_ops::receiver_key_text`), so `Recs[ 1 ]` is the element `Recs[1]` set up. Left: detectors that match receivers by their text treat `Recs[i]` as one record whatever `i` holds, and `Recs[1]` and `Recs[2]` as two; an element of a non-temporary array has an unresolved temp state ("possibly persistent"); an array is not a `record_variables` entry. A record operation written without parentheses in expression position (`if Recs[1].FindFirst then`, and the same on any record) made no record operation: **fixed by ENG-25** (2026-11-13). The resolver's parens-less filter (`full.rs`) now keeps a site the extractor classifies as a record operation (an `ImplicitTrigger` edge), except a field read on a `Record` whose table or an extension declares a field of that name (`SiteFacts::record_op_field_named`, `table_scope_may_have_field`: `F.Count` with a field `Count`; a `RecordRef` keeps the method) and a bare name on the implicit `Rec` that a parameter, local, return value, global or (in an extension) the base object's `protected var` declares (`exit(Count)` with a parameter `Count`; inside `with` the record's members come first). The field rule is the compiler's: alc 18.0.41 binds a parens-less `F.Count`/`F.IsEmpty` to a field of that name (probe 2026-11-13). A table that does not resolve keeps the operation. Left: the field check answers "may be a field" when it cannot see the table's scope (a table outside the caller's closure, an untargeted table extension declaring the name), and then drops a real operation (rare; not seen on CDO); an extension whose base object does not resolve has no known `protected var` names, so a bare name there stays an operation (an extra read, never a lost one); an object global `Rec` of a non-record type still makes `Rec.Count` and `Rec.Count()` record operations (older than ENG-25, with or without parentheses; receiver sets, with ENG-26). On CDO the new operations sit in 199 routines (30,604 added L4 read effects, no write changed); the d56 pair it surfaced on a `SourceTableTemporary` page was a d56 false positive, fixed by ENG-27 (2026-11-13: a temporary clone target is skipped, and only the last copy before the write counts, so at most one finding per write; d14 also reports one finding per `#if`-arm claim). A base object's `protected var` record used in an extension (`Other.Get('x')` in a tableextension) is not in either engine's receiver sets, so it made no record operation either (older than ENG-12; a resolved `Record::get` call only): **fixed by ENG-26** (2026-11-13). Such a receiver, typed by the variable's own declared table (receiver Step 2c), takes ENG-12's call-result path, so the model builds its record operation with its triggers and table events; the declaration's `temporary` is its temp state; a parens-less read is one too (unless its table has a field of that name), and on a protected RecordRef or a record of a table not loaded it answers `unknown`. A symbol-only base reads its protected vars from the symbol file's `"Protected": true` variables (683 protected records in the Base Application's symbols). A same-named local or global, record or not, still hides it. On CDO nothing moves (its 15 extension uses of a Base Application protected name are all shadowed by locals). Review round 2 (astra): a `SetRange`/`SetFilter` on such a record (and on a function's returned record, ENG-12) is an operation too, so a FlowField filter reads its source table; `temporary` is the declaration's trailing modifier only (not a word of a quoted table name); a table whose `TableType = Temporary` makes the appended operation temporary (as `record_types` does for the others). Left: the variable is in no `record_variables` entry, so detectors that look for an earlier load of the receiver (d11, d21) cannot see one made in another routine; a protected array record is not modelled (an indexed use reads only the routine's own declarations); a `with` over a base object's protected record (`with Other do exit(IsEmpty)`) is not bound to it, because one file cannot see the base's variables: a bare name inside a `with` whose receiver the file does not resolve, in an extension whose base has protected vars, answers `unknown` (one unresolved call), never `no`; the cross-file fix is task ENG-63. That rule reads a bare record-operation name only (a field or variable read inside a `with` stays decided), so it also makes `unknown`, never `no`: an extension's own procedure named like a record operation (its `Init`, `Reset`) read without parentheses inside such a `with`, and a bare record-operation name inside a `with` on a non-record receiver there. | fixed | S | — |

#### L20 in detail

Population: CDO has 14 reports, 1 query and 0 XmlPorts. CHANGELOG section: "engine limits wave 2, B6".

**Fixed.**
- Report run, print, save and execute, and query open and save, read each dataitem's table (report
  extensions' too).
- An XmlPort import looks up, inserts (AutoSave) and modifies (AutoUpdate/AutoReplace) each table
  element's table. An export reads it. `Run` follows `Direction`. The insert may run `OnInsert`
  (Learn is silent) and raises the table events.
- A symbol-only or unparsed dataset gives table-less operations, so `unknown`, never `no`.
- Review fixes:
  - `Report.SaveAs*`/`Execute`/`Print` and `Query.Open`/`SaveAs*` (static and on a variable) now run
    the object's triggers like `Run` (a query's trigger is `OnBeforeOpen`).
  - A dynamic id (`Report.SaveAsPdf(Id, …)`, `Query.SaveAsCsv(Id, …)`) is a dynamic run, so
    `unknown`, never `no`.
  - `Xmlport.Run(X, false, Import)` follows a literal `Import`.
- Round 2:
  - A call on a report or query variable to `Print`/`Execute`/`SaveAs*`/`Open` binds to the object's
    own procedure or to the built-in by AL's overload rule (argument count, then the fail-closed type
    pick; `unknown` when the types do not decide).
  - The triggers a run may skip (report dataitem and XmlPort element triggers, a page's
    `OnAfterGetRecord` and the other per-record ones) are `conditional`, also through `Report.Run`,
    and explain's rules across calls do not inherit them.
  - A `Direction` written in one `#if` arm alone keeps `Both` possible (`ObjectProperty::conditional`,
    the same reading the other properties in [B2 in detail](#b2-in-detail) get; that section also
    lists the properties not read this way yet).
  - `alsem graph` has `implicitTrigger`/`tableEvent` edges from an XmlPort import to its tables'
    `OnInsert`/`OnModify` and table events.
- ENG-07 (the request page): a write in a report's request-page `OnOpenPage` answered `no` through
  `Report.Run`, `RunModal`, `Execute` and a report variable's run (re-probed on master `a8a6c70a`).
  A run of a report (or of the report a report extension extends) now follows its request page's
  triggers (`run_triggers`, which also feeds the sited graph and explain's `reachedFrom`). Which
  trigger runs was **measured on the BC 28.4 Linux sidecar; the report paths below are confirmed on
  Windows BC 28.0.54946.56477** (Cronus28, the same probe unchanged, owner 2026-11-11,
  q-20261111T144908-3d4d3d0b); the report-extension and XmlPort bullets are sidecar only:
  - Page shown (`Run`/`RunModal`): `OnInitReport`, request page `OnInit`, `OnOpenPage`, the page,
    `OnQueryClosePage`, `OnClosePage`, `OnPreReport`, the data items, `OnPostReport`.
  - `UseRequestPage` false: `OnInitReport`, `OnInit`, `OnPreReport`, … (no `OnOpenPage`/`OnClosePage`).
  - Cancelled, or `RunRequestPage`: as shown, then stops after `OnClosePage` (no `OnPreReport`).
  - `Execute` with parameters: `OnInit`, `OnOpenPage`, `OnClosePage` (no `OnQueryClosePage`),
    `OnPreReport`, … Learn's "the OnOpen and OnClose triggers on the request page will run even
    though the request page is not shown" holds for `Execute`, not for `Run(…, false)`.
  - `SaveAs*` and `Print` were not measured; they are runs like `Execute`.
  - (Sidecar only.) A report extension's request page: its `OnOpenPage` runs after the base's, its `OnClosePage`
    after the page; neither with `UseRequestPage` false. It cannot declare `OnInit` (alc AL0162).
  - (Sidecar only.) An XmlPort's request page: `OnInit` ran before `OnInitXmlPort` on `Export` to a stream and on
    `Run(X, false)` and `Run(X, true)`; no `OnOpenPage` ran (in a test session the page did not
    show, so its shown order was not seen).

  So the request page's `OnInit` is `unconditional on success`; its `OnOpenPage`, `OnQueryClosePage`,
  `OnClosePage` and record triggers (`OnAfterGetRecord`, …, for a request page with a
  `SourceTable`) are `conditional`, for a report, a report extension and an XmlPort. A control's
  `OnValidate` stays unfollowed (a user's action). The resolver's run follows triggers only, so a
  report procedure named `OnOpenPage` is not taken for the trigger (for a page, report, XmlPort and
  query run; explain's `reachedFrom` already checked it). `alsem impact` (the sited graph) now
  counts the runs as code callers of a request-page trigger; before, it said "direct code callers:
  none found" and reached it by UI navigation only. Tests: `tests/cli/cli_eng07_request_page.rs`.
- ENG-08 (an import's error): `throws-error` answered `no` for a statement `Xmlport.Import(X, IS)`,
  an `Xmlport.Run` that imports and a variable's `X.Import()` (re-probed on master `f12bf250`). What
  makes an import fail was **measured on the BC 28.4 Linux sidecar** (XML format, statement form):
  - A key that exists, or that the file holds twice: "The record in table … already exists", unless
    `AutoUpdate` or `AutoReplace` is true (then it modifies, no error).
  - A value that does not evaluate ("The value "abc" can't be evaluated into type Integer.") and
    malformed XML fail whatever the properties say.
  - `Ok := Xmlport.Import(…)` returned `false` and raised nothing.
  - A literal direction the XmlPort's `Direction` refuses fails before any trigger runs:
    `Xmlport.Run(X, false, false)` on a `Direction = Import` XmlPort gives "The XMLPort 'X' is
    designed for import only.", and `Run(X, false, true)` on a `Direction = Export` one "…
    designed for export only." The B6 dataset model let the literal win, so the first answered
    `throws-error: no` and read the table, and the second claimed the import's writes.

  Since bad input fails any import, the error belongs to the call, not to its insert: a call the
  resolver decided starts an import (`Import`, or a `Run` whose `Direction` allows one; keyword or
  variable form) is marked (`PCallSite::start_may_fail`, set in `program_calls`), and in statement
  position it is an implicit error (`ERROR_IMPLICIT`, the `Evaluate` rule, G50). A used result
  (`Ok :=`, `if`, `exit(…)`) catches it, and a run that only exports raises none. An import with
  `AutoSave = false` raises it too. An explicit import or export now runs only when `Direction`
  allows it (`dataset::import_export`); one it refuses runs nothing (no dataset operation) and is
  marked too (`dataset::direction_refused`), so in statement position it raises the error.
  Tests: `tests/cli/cli_eng08_xmlport_import_error.rs`.

**Left.**
- **`Report.RunRequestPage` is not a run: wrong answer left.** It runs the request page's `OnInit`,
  `OnOpenPage`, `OnQueryClosePage` and `OnClosePage` (measured, above) but no `OnPreReport` and no
  data item, and it resolves as a plain built-in, so a write in those triggers answers `no` through
  it. CDO calls it twice, both with a run-time id. The fix needs the triggers one start method runs
  (not the whole run's list), its keyword and variable forms, and B4's commit scope.
- Which request-page triggers one call runs is not read from the call: `RunModal(X, false)` (and a
  report with `UseRequestPage = false`) reaches `OnOpenPage`/`OnClosePage`, and `Execute` reaches
  `OnQueryClosePage`, all `conditional` (safe, imprecise).
- A request page's `SourceTable` does not type its `Rec`: a write through `Rec` in a request-page
  trigger is a table not known, so `unknown` (safe, imprecise).
- **A report procedure with a request-page trigger's name and parameters hides the trigger: wrong
  answer left.** Both get one routine id (object, name, no member, parameters), and the build keeps
  the first one declared. alc accepts the pair (a report with `procedure OnOpenPage()` and a
  request-page `trigger OnOpenPage()` compiles, checked with alc 2026-11-11). When the procedure
  comes first, a run follows neither, so a write in the trigger answers `no` (before ENG-07 the run
  followed the procedure's body, also not the trigger's). The root fix is a member for request-page
  triggers in the lowerer, or `is_trigger` in the routine id; both move ids.
- A checked import (`Ok := Xmlport.Import(X, …)`, `if Xmlport.Import(…)`) is not a trap: an error
  raised inside `X` (an `Error` in its triggers, or, since ENG-08, a statement import there) still
  reaches the caller's `throws-error`, though the probe saw `Ok :=` return `false` instead of
  raising (a false `yes`; it predates ENG-08). The trap rule (`engine::l5::traps::call_trap`)
  knows `asserterror`, `Codeunit.Run` and try methods only; which errors a checked import traps
  needs its own probe.
- An export was not measured failing, so a run that only exports raises no error fact (Learn gives
  `Export` an optional `Ok` result too, so it can fail somehow). A `Run` whose `Direction` is
  `Both`, or not known, counts as an import.
- The LSP call hierarchy still lacks the trigger and table-event edges (only the model and `alsem
  graph` have them).
- A dynamic run target stays the run's own unknown (no table is named).
- A request page the user cancels ends `Report.Run` before `OnPreReport` without an error (measured,
  above), yet `OnPreReport` and `OnPostReport` still read `unconditional on success` when a request
  page can show. Which runs can show one (`UseRequestPage`, the run's own argument) is not modelled.
- Which data items are indented is not in the model, so a top-level data item's
  `OnPreDataItem`/`OnPostDataItem` reads `conditional` (safe, imprecise).
- A call whose argument types do not decide between the built-in and a same-arity procedure is
  `unknown` even where alc decides (a Text literal binds `SaveAsWord(Text)` over
  `SaveAsWord(Code[250])`).
- An XmlPort import's insert and modify sit at the call site with its loop and condition context,
  while the platform does them once per imported record (possibly never). This matters for in-loop
  detectors and wording, not for yes/no answers.
- A property set in both arms of an `#if`/`#else` reads as "may" (safe, imprecise).
- DataItemTableView/DataItemLink filters are not modelled (every read is "may read").

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
| L14 | No speed or memory gate on CDO. It must be a branch-gate step on release-fast and tolerate a loaded machine. Cost: 2–4 min. **Fixed in wave 1:** `cdo-gate`'s `speed` phase (`scripts/cdo-speed`), A/B against the merge base on resident CDO calls, CPU and wall time (lost parallelism shows in the warm-up's wall time); timing 178–301 s; round 4's slowdown fails it at x2.1 (CHANGELOG). Linux only. | fixed | M | — |
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
waves. L35 and L36, found during wave 2, are fixed by ENG-12 (2026-11-12). B9, found then, and
B10, found with it, are fixed by ENG-06 (2026-11-11). Waves 0–2 are done; most of L28 (wave 8) was
fixed by ENG-22. Status of every item as of 2026-11-13, with code evidence:
`docs/superpowers/notes/2026-11-13-engine-limits-audit.md` (TLS-44).

## 4. Verification gaps (no limit suspected)

astra named these cases as fixture-tested only. Add one real CDO call for each to the next
sample:
- touches with `opFamily: "write"`;
- an assumption whose `subscribersConsidered` is not empty;
- a digest in human format for an ambiguous name (the `differs in:` lines);
- a test whose code path was not kept.
