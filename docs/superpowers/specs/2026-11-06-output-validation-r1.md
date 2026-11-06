# Output validation, round 1: what the MCP answers should look like (design spec)

**Status:** design only, revision 4 (after three review rounds by Fable and astra; both accepted
revision 3 "with changes", targeted corrections only). No code lands with this file.
**Date:** 2026-11-06. **Based on:** master `6e577c24`, binary al-sem 2.1.1.
**Reviews:** of revision 1, `target/tmp/output-validation/round1/review/spec-review-{fable,astra}.md`;
of revision 2, `…/spec-r2-review-{fable,astra}.md`; of revision 3, `…/spec-r3-review-{fable,astra}.md`.
What each item changed: "Revision 4", "Revision 3" and "Revision 2" below.
**Numbers in sketches:** taken from the round-1 calls (named where used) or marked
illustrative when they cannot be computed before the fix branches land. Round-2 numeric
expectations are derived after those branches merge (§12.2), not from this file.
**Input:** 30 MCP calls recorded on the CDO baseline (`bc3ccb18`), each with the task an agent
had; a fact check of every claim against the AL source; two independent reviews of the outputs
against one rubric (Fable and astra). All of it is under
`target/tmp/output-validation/round1/` (local, not committed): `review/review-bundle.md` (the
calls), `bundle.json` (full answers), `factcheck/`, `review/review-{fable,astra}.md`.
**Register:** every finding, merged across reviewers and calls, is in Appendix A (ids F1–F85;
F73–F85 added in revision 2). This spec covers the design changes and the new correctness bugs.
Findings another branch fixes are in the register as "handled elsewhere".
**Out of scope:** the fact check's 13 wrong claims. They are being fixed on
`fix/field-writers-implicit`, `fix/explain-callers-counts`, `fix/db-event-subscribers` and
`fix/reach-and-test-selection`. Where a change here builds on one of those fixes, it says so.

**Stays consistent with:** the MCP output plan (`docs/superpowers/plans/2026-11-04-mcp-output-fixes.md`,
owner decisions 1–5, consumer notes §4.1), the wave-2 design (`…/specs/2026-11-04-mcp-output-wave2-design.md`,
rules §3.6–§3.8), the agent-tasks design (`…/specs/2026-11-05-agent-tasks-design.md`, §5 "existing
effects", §12 surface and versions), and the call-graph export contract
(`…/specs/2026-10-13-callgraph-export-design.md`). Nothing here changes `alsem graph` JSONL.

Words used here:
- **hub** (or **open site**): a call whose set of targets the engine cannot close — an
  unresolved dispatch, such as `Codeunit.Run(<variable>)`. The graph stands for it with one node
  that may lead anywhere. A call with several known targets is not a hub; when known targets and
  an unresolved remainder coexist (an interface under open world), both are shown. A path over a
  hub is a possibility, not a path the code is known to take.
- **tier**: where a path goes, from the word table (§8.1): `local`, `via-dependency`,
  `via-configuration`, `via-hub`; the weakest hop decides.
- **boilerplate**: the coverage / assumptions / limits / scope lines every card repeats.
- **envelope**: the JSON the MCP tool returns around the answer text (`exitCode`, `summary`, …).

**The contract that does not move:** the MCP answer stays the CLI's stdout byte for byte. Every
change below is a change to the CLI output, or to the envelope, made on purpose with its goldens
regenerated and read.

---

## Revision 4: what each review item changed

Both reviewers accepted revision 3 with targeted corrections. Every item is applied; none is
declined.

| Item | Change made |
|---|---|
| Fable A, astra (§3.2 entry counts) | the kind line now covers all nine recorded c12 buckets (sum 19,433), mapped to the new kinds — codeunit OnRun 464 + 21 background starts = 485 before deduplication, install triggers 3, upgrade triggers 45 (inflated, as §3.1 notes). Because the legacy buckets overlap (the same `OnAction` is under page actions and page triggers, astra), 19,433 is stated as a sum of buckets, and the disjoint per-kind counts and their total `<e>` are symbolic until classification; `k1 + … + k8 = e`, `a + b = e` |
| astra (relation totals) | §3.2: an entry with both a code and a navigation witness counts once, under `code` |
| astra, Fable B (code-callers line) | §3.2: `reached by code callers: no — no code caller found in the analyzed graph (every candidate decided)`, an absence verdict from analysis, not a claim from the routine kind |
| Fable B (negative-certainty scope) | §8.1: the row governs absence verdicts only; zero counts and empty lists (`established witnesses: none`, `limits: none`, `calls: none`) keep plain words |
| astra (F61 JSON during V2) | §5.2, §0.1: F61 lands whole in V2 for query effects — the human clause and the JSON `result.scope.families` (additive, query 3.6.0); the shared `result.scope` for the other tools stays V4 |
| astra (§11.2 row 1; §8.2) | row 1 now says only the round-trip-tested display forms are pasteable (caption or not) and `selector` is the one guaranteed form; §8.2's c05 line is `writes beyond dependency calls: no (every candidate decided; …)` |
| Fable i | §3.2: the fold line names its trigger and measure (`<p>%` of entries, or a hub covering `<q>%` of graph nodes) |
| Fable ii | §6.1: one label, `re-analysis under an assumption:`, which carries the complete `{tool, arguments}` call at `--surface=mcp`; `conditional alternative:` removed |
| Fable iii | §8.1 context row: an `install trigger` always runs in context `install` (and upgrade likewise), so those counts coincide and are not extra entries |
| Fable iv | test status uses `analysis partial`, the completeness row's word (§8.1, §5.2) |
| Fable v | §11.2 row 1 (above); §4.7's Verify line says `confirmed writers: 1` |
| Fable vi | §4.2: `references that may write, not confirmed (kinds: …): none observed (search below)` |
| Fable vii | §9.3 field mode: `none observed (searched: …; not searched: …)` from the shared helper |

---

## Revision 3: what each review item changed

Both reviewers accepted revision 2 with changes. Fable's must-changes are numbered 1–4 and its
should-changes e–p (its §5 and §3); astra's are its "remaining must-changes" list, named by
topic. Every item is applied; declines are marked. The main class was consistency: every sketch
now uses the §8.1 word table exactly, and every sketch number either adds up from a named call
or is marked illustrative.

### R3.1 Must-change items

| Item | Change made |
|---|---|
| Fable 1 (hub form in §3.1, §3.2) | §3.1 uses `[hub Codeunit.Run of a variable at …]`; §3.2 uses a real c12 hub, `[hub Page.Run of a variable at …]`. §8.1 states where a hub's variable name goes (`[hub <kind> <name>? at <file>:<line>]`) |
| astra: declared-but-never-run variables are not hubs | `page variable (declared, never run)` removed from the hub list and from every sketch: it is the false `declaredRunnable` edge `fix/reach-and-test-selection` removes, not an uncertain route. §8.1 says so |
| astra: known targets plus an unresolved remainder | the hub definition (Words) and §8.1's hub row: both print (`interface dispatch: 2 known implementations + open remainder`) |
| Fable 2, astra (§3.2 counts; fan-out population; "not specific") | §3.2 sketch rebuilt from c12's recorded numbers (ancestors 27,718 = local 0 + via-dependency 23,230 + via-hub 4,488; entries by kind from the same answer); the code/navigation split is marked illustrative (it needs the relation axis and the fix branch). The single-hub trigger is redefined on a stated population (the hub's forward cone over graph nodes, as `test_select` already measures `overHalfOfGraph`); the folding words are `broad navigation reach; list folded`, never "not specific" |
| Fable 3, astra (entry kinds vs context) | §8.1: entry kinds are routine kinds only (closed list); **execution context** is its own row with its own closed list and counting rule (contexts overlap and are never summed). §3.1, §4.4, §5.1 sketches use `codeunit OnRun [context: job queue]` and `by context: …` |
| astra: relation describes the whole witness | §8.1: `relation` is `UI navigation` when any edge of the witness is navigation, else `code` |
| astra: configuration evidence status | §8.1: a `via-configuration` witness is a candidate (the binding lives in data the analysis has not read), counted under `candidates over configuration`, never established |
| Fable 4 (negative certainty leaks) | §4.1, §4.2, §6.2 use `no (every candidate decided)` and `none observed (searched: …; not searched: …)`; no "no observed" survives in a sketch. This replaces wave-2 §3.7's absence wording; §8.1 says so |
| astra: feasible summary overflow | §5.2: the summary is bounded (8 KiB); mandatory content that does not fit goes to a **complete compact result index** file beside the answer (every requested identity, verdict and selected test), named in the summary; transport truncation (`summaryTruncated`) is reported apart from `selectionComplete` |
| astra: non-result states; structured per-family scope | §5.2: analysis `complete` / `partial` / `not analyzed` (with reasons); test status uses the §8.1 completeness words including `analysis incomplete`, `not in projection`, `not loaded`, `not requested`; per-family scope is a structured object (`depth`, `inventory`, `reasons`, `blockers`) |
| astra: c06 reused c02's counts | §5.1 sketch uses c06's own recorded section (8 witnesses, 7 from page triggers; external callers 2; publication unknown 1, unread publication file 1, background target not resolved 2) |
| astra: dependency-owned touch sites | §9.3: `dependency sites` line apart from workspace sites; the transitive relation is qualified |
| astra: confirmed writers vs reference candidates | §4.2: confirmed writers (assignment, `Validate`, `ModifyAll`) counted; `RecordRef.Field(n)` and `TransferFields` listed apart as references or copy candidates, never as writers |
| astra: "temporary records are not writes" | replaced everywhere by "writes to temporary records do not directly persist those records" (§5.3, and the sentence the spec quotes from the scope line) |
| astra: company context omitted | §9.1, §10: a write in a cone that calls `ChangeCompany` prints `target company: not established by this analysis (ChangeCompany at <file>:<line>)` |
| astra: pasteability only for tested forms | §2.1: only `selector` is guaranteed; human displays are pasteable for the forms the round-trip test covers (listed), not for dependency displays with numbers, data-item members or app labels. Fable j (the caption contradiction) resolved by the same rewrite |
| astra: stale consumer recipes | §7.2: ConTinAI `effects.py` reads `origin.line` (was `origin.anchor.line`); §7.3: bc-dev-mcp joins each root's references and annotations with `payload.effects` |
| astra: assumption-changing follow-ups | §6.1: ordinary follow-ups keep the call's assumptions; a call that changes one is printed as `conditional alternative:` |
| astra: F74 and F61 alongside V2 | §0.1: F74 and the output part of F61 move to V2 |
| Fable: scope the `run-time` decline | §8.1 and §11.2 row 5: the decline is for table, temp and condition targets; for call edges `honest dynamic` (`HonestDynamic`, `edge.rs`) is a proven run-time fact and keeps its word |

### R3.2 Should-change items (Fable e–p)

| Item | Disposition |
|---|---|
| e | applied: §7.1 and §3.3 use `via-dependency` / `via-configuration`; `Codeunit "Workflow Response Handling" (1521)::ExecuteResponse` in the §2.2 form |
| f | applied: §5.2's third header line and §5.3's one-liner are merged into one line (`analysis complete · assumptions: … · limits: … · scope: …`), `more:` on its own line |
| g | applied: F16 prints `caller-dependent (parameter 3, ToPurchaseHeader)`, the §8.1 form, already in V1 |
| h | applied: §5.2 test status uses the §8.1 words |
| i | applied: §2.4's `selector forms:` line lists the member forms |
| j | applied (with astra's pasteability item) |
| k | applied: `pending writes: yes` |
| l | applied: milestone markers on §5.1 (V2), §5.2 (the `tableAnswers` part is V2), §9.3 field mode (V2), §9.8 (F73 V6, F78 V4) |
| m | applied: §7.6 lists `via-configuration` |
| n | applied: §8.1 gains a **blocker kinds** row, the closed list from `callers_context.rs` plus prove's effect-question blockers |
| o | applied: the c02 sketch names the three largest kinds that stay and prints `and 4 more kinds stay (detail: true lists them)` (c02 has eight blocker kinds, not nine; external callers is the one removed); the rule caps at three named kinds |
| p | applied: §8.3 states the exception (a correctness fix that removes rows a reader should never have counted is a minor bump; a rename is not) |

No item is declined in revision 3.

---

## Revision 2: what each review item changed

"Fable n" and "astra n" point at the must-change lists (§6 of each review); "should" items and
section comments are named by their topic. Every must-change is applied. One should-change is
declined in part (marked). Where the two reviewers conflict, the decision and its reason are in
§11.2.

### R2.1 Must-change items

| Item | Change made |
|---|---|
| Fable 1 (display is not a selector) | §2.1: the selector grammar gains the member forms `Object."Field"::Trigger` and `Object.action(Name)::OnAction` (caption optional on input, ignored); JSON gains `selector` beside `display`; a round-trip test (every emitted selector resolves to its own stable id). Merged with astra's "display ≠ selector, round-trip tests" (§11.2 row 1) |
| Fable 2 (knob only when sole blocker deletes c02's line) | §4.4 rewritten: every input that removes a blocker kind is named, with what it removes and what stays. Merged with astra's objection to "would decide it" (§11.2 row 2) |
| Fable 3 (reach sketch mixes units; single-hub fan-out; name the hub) | §3.2: one unit per line; folding also fires on one hub's fan-out; the hub is named |
| Fable 4, astra (bare `complete` in the header) | §5.2, §5.3: `analysis: complete` / `analysis: partial (…)`; never bare |
| Fable 5 (word table gaps) | §8.1 rewritten: tier precedence, configured dispatch, a closed hub-kind list, resolution words, negative-certainty row, witness row, entry kinds, "entry" banned alone |
| Fable 6 (F42 `tableAnswers`, F57 into V2) | §0.1: both moved to V2 |
| Fable 7, astra (F15 also the temporary arm, all op families) | §1 F15: both non-physical classes split by op family (reads, locks, validations, writes); read and lock totals count every class. Already passed to the V1 fix branch |
| astra: Appendix A omissions, priorities, dispositions; restore F21 | Appendix A: new rows F73–F79 (astra's missing items) and F80–F85 (Fable's six low items); F20, F63 raised to medium, F22, F50 to high; the seven "already fixed" rows get their own class **handled elsewhere** (an integration and round-2 obligation, §12.2), not "rejected"; F21 reversed (§4.4); F61 split into an accepted disclosure (design) and a deferred engine capability (§10) |
| astra: tier, relationship, configured dispatch, projection defined apart | §8.1: `tier` (with precedence), `relation` (code / UI navigation), configured dispatch as its own tier, `projection: approximate` kept as its own flag |
| astra: c02 all-callers claim; "would decide it" | §3.1 answer line scoped to non-interactive routes and "established by the static model"; §4.4 "re-analysis under an assumption" |
| astra: one identity and verdict per overload; per-routine test status | §5.2: digest summary keeps one row per stable id (shared counts factored, never folded); each row has its `tableAnswers` and test status (F77); §2.1 signatures on every occurrence of an overloaded name |
| astra: standalone machine-readable scope per operation family, deep writes, test selection | §5.2 JSON `result.scope` block in every document (F76); query's per-family scope (F61) |
| astra: access kind, temp state, target resolution, reachability independent; lock/read totals | §1 F15; §8.1 resolution and temp rows; §9.2 lock scope |
| astra: lossless dedup | §7.2, §7.3, §7.4, §7.5: shared intrinsic facts plus per-root/per-row annotations; a reconstruction test per shape (§12.1) |
| astra: field references vs numeric candidates; filter result; declared keys | §9.3: separate inventories, `filters:` line always; the "keys are not searched" claim was wrong (`field_touches.rs` reads `declared_keys`) and is corrected in §9.3, §10, F60 |
| astra: c17 other-branches relation, identities, population counts | §7.1 |
| astra: run target first; superset label; scope, cap, fallback first | §9.5 (tests), §5.2 (impact and digest summaries); F63 moved to V2 |
| astra: complete MCP follow-ups; no false gates; empty analysis vs unresolved input | §6.1 `{tool, arguments}`; §6.2 gates; §1 F24 and §6.3 `input-not-mapped` |
| astra: summary truncation, JSON compatibility, three schema majors, semantic gates | §5.2 cap rules; §8.3 migration rule; §0.1 and §7: three majors (query 4, digest 4, tests 2); §12.1 semantic tests |

### R2.2 Should-change items and section comments

| Item | Disposition |
|---|---|
| Fable §6.1: one MCP-surface fixture golden per tool | applied, §6.1 |
| Fable §3.1: answer line names the actual hub kind | applied, §3.1 |
| Fable §4.1: order when both classes exist | applied, §4.1 |
| Fable §2.4: name found only in a dependency | applied, §2.4 |
| Fable: six low items | applied, F80–F85 in §5.3, §7.5, §9.1, §9.5 |
| Fable §12: dated fix-branch gating table | applied, §12.2 |
| Fable §2.1: quoting rule; overload rule on list rows too | applied, §2.1 (the rule is now "every occurrence of an overloaded name") |
| Fable §5.2: explain's `answer:` line | applied, §5.2 (the card's summary line) |
| Fable §8.1 #4: one pair `not resolved` / `run-time` on every axis | **declined in part**: `not resolved` is adopted on every axis; `run-time` is not, because the engine cannot show a target is a run-time value (astra: some RecordRefs resolve statically). §11.2 row 5 |
| astra: captions in headers and legends, not on every hop | applied, §2.1 |
| astra: near matches thresholded; complete selectors in c26 | applied, §2.4 |
| astra: c06 "8 witnesses", own state changes, every blocker kind | applied, §5.1 (F75) |
| astra: c20 company context | applied as a design row (F75) with an [engine] check: whether the model records `ChangeCompany` on a record variable is not verified (§10) |
| astra: tests group tier distribution, navigation wording, execution before navigation | applied, §3.3; F13's "ranking ignores tier" narrowed: `select.rs:457` already sorts by (tier, hops); the defect is the rendering and the missing relation axis |
| astra: c02 editing implication; c29 uncaught path under `detail` | applied, F79 in §4.4 |
| astra: field-writer definition location, categories, assignment vs persistence | applied, §4.2 |
| astra: filter carried into summary and JSON | applied, §4.3 |
| astra: §4.1 wording ("may modify persistent … when `Rec` is non-temporary") | applied, §4.1 |
| astra: "no interception was found in analyzed callers" | applied, §4.4 |
| astra: freshness compact answer keeps strength, revision, limits, `not checked`; derive input counts | applied, §4.5, §5.4, §9.6 |
| astra: diff mode says whether the diff matched and what was actually computed | applied, §4.7 |
| astra: three-line header for human output only; JSON structured | applied, §5.2 |
| astra: keep the full assumption sentence and answer-specific limits beside the answer | applied, §5.3 |
| astra: `singleApp` on an analysis request; may-commit offered when unknown; drop the pending-writes claim; `--` invariant scope | applied, §6.1, §6.2 |
| astra: query operation location kept apart from routine anchor; `via` default | applied, §7.2 |
| astra: impact detail as an evidence budget | applied, §7.6 |
| astra: caller rules "no equivalent check recognized in 41 analyzed callers" | applied, §7.7, §9.1 |
| astra: c29 overlap rule before "72 entries" | applied, §8.2 |
| astra: F55 short route, F56 qualifications, F57 lock scope, `Get` in key evidence, BOM only | applied, §9.1–§9.4 |
| astra: move completeness subject, candidates, lock qualification, exact selection, F55/F56/F48 earlier | applied, §0.1 (V2 gains F13, F22, F42-part, F48, F50, F55, F56, F57, F63) |
| astra: defer the percentage heuristic | applied as astra asked: the share only folds lists, it never sets a label (§3.2) |
| astra: `unknown` allowed wherever a verdict exists | applied, §8.1 |

---

## 0. Summary

The engine's facts in round 1 were mostly right (13 wrong claims in 30 calls, all on fix
branches). The outputs failed agents in four ways, and both reviewers found the same four:

1. **The same thing has several names.** Overloads print as one name (four identical
   `CreateSMTPLogEntries` rows); a table's field triggers and a page's actions print as bare
   `OnValidate` / `OnAction`; dependency routines print as `Codeunit 448::handlerequest`.
2. **Possibilities look like facts.** A path over a hub is printed as evidence (c02); a very
   broad reach is listed with no tier and no hub named (c12, c20); a conditional write
   is led by "no write" (c01, c23); a filtered "no" hides the sites that do match (c27).
3. **The answer is not first, and not in the summary.** c06's COMMIT hazard is the third-to-last
   line; c15's asked-for CDO File verdict is missing from the envelope summary; c17's new
   information is on line 957 of 961.
4. **Size comes from repetition, not content.** query effects JSON 1 MB for 671 rows; touches
   `callers` 320 KB with one witness repeated 952 times; digest 1.18 MB for 20 overlapping cones.

Plus seven new correctness bugs (§1), each verified in the code.

### 0.1 Milestones

Each milestone can land on its own. The goldens of later ones are rebased on earlier ones; that
is regeneration, not rework.

| # | Milestone | Findings | Size | Depends on |
|---|---|---|---|---|
| V0 | **Word table** (decision only): §8.1 and the migration rule §8.3, approved and dated before any renderer uses them | F49, F51, F52 | — | — |
| V1 | **Correctness bugs** | F5, F9, F15, F16, F24, F25, F39 | S, 7 small commits | — |
| V2 | **What misleads**: verdict and tier wording, silent lines, the evidence a hazard needs | F10, F12, F13, F14, F17, F18, F19, F20, F21, F22, F23, F27, F29, F42 (`tableAnswers` only), F48, F50, F55, F56, F57, F61 (query effects: human scope clause and JSON `families`), F63, F74, F75, F79 | L | V0; the fix-branch gates of §12.2 |
| V3 | **Identities** and the selector grammar | F1–F4, F6–F8 | M | — |
| V4 | **Result header, summaries, follow-ups** | F28, F30–F37, F42 (the rest, incl. the result index), F43, F76 (incl. `result.scope` for the other tools), F77, F78, F81, F85 | M | V0, V2, V3 |
| V5 | **Size and dedup** (three schema majors: query 4.0.0, digest 4.0.0, tests 2) | F38, F40, F41, F44–F47, F83, F84 | L | V3 (rows refer to routine ids), V4 (the `result` contract) |
| V6 | **Counting units and per-tool items** | F53, F54, F59, F60, F62, F64–F67, F72, F73, F80, F82 | M | V0 |

Order: V1 first (small, and two of its bugs feed V2 text). V0 is a one-page owner decision,
signed off here before V2 starts: **V0 approved: 2026-11-06, owner, as written.** V2 and V3 next, in
either order. V4 after V2 and V3; V6 after V0. V5 last: three schema majors with consumer
notices, and nothing in it misleads.

---

## 1. Correctness bugs (V1)

Each is its own commit with a test that pins the use (the scenario, not the helper) and one
discrimination proof (break the fix, watch the test fail, revert).

| id | Bug | Fix | Test (discrimination break) |
|---|---|---|---|
| F15 | query effects files a `Get` under "caller-dependent writes": c11 prints `caller-dependent writes 2 on 1 table` then `Purchase Header: Get 1, Modify 1`. `effect_query_cli.rs:2991` `bucket()` returns `callerDependent` before it reads `op_family`; the `Temporary` arm returns before it too (Fable), so a temporary `Get` reads as "temporary records" work | Split **both** non-physical classes by op family as the physical class is split: writes, reads, locks, validations and other families each keep their class as a note (`caller-dependent`, `temporary`). The read, lock and validate totals count every class (a temporary or caller-dependent lock is a lock). JSON `writeTables` loses the read rows; a minor query bump (3.6.0), since a row moves bucket. Passed to the V1 fix branch | fixture routine with, on one caller-dependent parameter, a `Get`, a `LockTable` and a `Modify`, and on a temporary record a `Get`: writes count 1, reads 2 (one caller-dependent, one temporary), locks 1 (break: return the class for every op again) |
| F16 | One answer, two bases: `(caller-dependent: parameter 3)` (1-based, `temp_note`) and `temp parameter-dependent(2)` (raw 0-based, `temp_state_str` in the `--all` rows) | Both human forms print the §8.1 temp-state words: `caller-dependent (parameter 3, ToPurchaseHeader)` (1-based, with the parameter name), so the golden moves once. JSON keeps `parameterIndex` (0-based) and adds `parameterPosition` (1-based) and `parameterName` | the c11 shape in the fixture: both lines name position 3 (break: print the raw index) |
| F24 | impact exits 2 when the change has no routine (c13), although the accounting is complete. `impact/render.rs:715` | Exit 0 with `outcome: "no-routine-changed"` (see F36) **only when the accounting is complete** (no unmapped line, no file not mapped, no diff line that fails to match the working tree). When no routine resolved because input did not map, the outcome is `input-not-mapped` and the exit stays 2 (no answer) | `cli_impact.rs`: a header-only diff exits 0 with `no-routine-changed`; a diff whose lines do not match the tree exits 2 with `input-not-mapped` (break: restore the `roots.is_empty()` rule; break 2: drop the completeness condition) |
| F25 | impact without a base says `— signature changed` of a routine the diff adds (c20). `impact/jobs.rs:268-274` sets the status from "a changed line falls on the signature" | Without a base, the status says what the diff shows: `new in this diff (every line added)` when all of the routine's lines are `+` lines, else `signature lines changed in the diff`. "Signature changed" is kept for a base comparison (`delta.pairing == signature`) | the c20 patch shape: an added routine prints `new in this diff` (break: drop the all-added check) |
| F5 | prove's callers' context answers print `dep:<guid>:path` in human text (c02); the rule is `dep:<app name>:path` (plan 2g, wave-2 §3.8 rule 5). `callers_document` (`prove.rs` ~L2981) never calls `dependencies.readable_locations`, which the effects path calls (`prove.rs:2711`) | Call it in `callers_document`. JSON keeps the guid (the ConTinAI citation reader needs it) | the hygiene test `human_outputs_are_clean_and_within_budget` gains a callers' context answer and asserts no `dep:<36-char guid>:` in human text (break: remove the call) |
| F9 | tests paths print two overloads alike: `Codeunit CDO E-Mail::Send → Codeunit CDO E-Mail::Send` (c28). Agent-tasks §12.1 requires an overload handle in every path; `test_select/render.rs` has none | Print the handle (`Send/1` → `Send/2`) through the same `RollupCx::handle` rule impact uses; after V3, the signature form of §2.1 | a fixture test whose path passes two same-name overloads: the two hops differ (break: print the bare display) |
| F39 | The touches `callers` input says "without their witnesses" (`tools.rs:485`, `:460`); every caller line carries one (c17) | V1 corrects the description to what is printed ("each with its first touch"). V5 (§7.1) changes both together to "each caller once; its first touch only when it differs from the shared one" | `descriptions_name_exactly_the_inputs_the_schemas_take` gains a check that the `callers` text matches the output's row shape on the fixture (break: revert the text) |

---

## 2. Identities and selectors (V3)

### 2.1 One display form for a routine (F1, F2, F3)

**Current** (quotes):
- c15 summary: four rows, each `"routine": "Codeunit CDO Log Management::CreateSMTPLogEntries"`.
- c01: `--- Table CDO E-Mail Job::OnValidate ---` (the table has 12 `OnValidate` triggers).
- c05: `Page CDO E-Mail Templates::OnAction`; c12: `PageExtension CDOSalesInvoiceExt::OnAction/0#1`;
  c05 again: `page action ExportTemplateAction on Page CDO E-Mail Templates (caption "Export Template")`.

**New:** one canonical display, used in every header, list row, path hop and JSON `display`:

```
Table "CDO E-Mail Job"."Table No."::OnValidate
Page "CDO E-Mail Templates".action(ExportTemplateAction "Export Template")::OnAction
PageExtension CDOSalesInvoiceExt.action(CDOOpenEMail "Open Email (Draft)")::OnAction
Codeunit "CDO Log Management"::CreateSMTPLogEntries(Record "CDO E-Mail Template Line", RecordRef, Variant, Text, Text, Text, Text, Boolean, Boolean)
```

Rules:
- **Overloads:** the parameter list prints on **every occurrence** of a name that is overloaded in
  its object — header, list row, path hop, summary row — and never on a name that is not (the rest
  stay short). This replaces wave-2 §3.8 rule 6 (hop neighbours only), which leaves a lone
  `SendMail` hop and c17's list rows ambiguous. Parameter modifiers (`var`, `temporary`) and full
  types are kept.
- **Members:** the member (field, action, data item) prints for every trigger that has one. The
  caption prints in headers and legends only, not on every path hop (astra: noise).
- **Quoting:** an object, field or action name is quoted when AL would need quotes (a space or a
  reserved character), and only then: `Codeunit "CDO Log Management"`, `PageExtension CDOSalesInvoiceExt`.
- **Apps:** two loaded objects with one name keep today's app disambiguation (`routine_rollup.rs`).
- **JSON:** gains `signature` (the parameter types), `member` (`{kind, name, caption}`) and
  `selector` (below) beside `display` and `stableId`. The `/0#1` handle stays in JSON as `handle` only.

**Display and selector** (Fable 1, astra). Today `selector_index.rs` accepts `Owner::Routine` and
`Owner::Routine(<types>)` only, so a trigger display could not be pasted into the next call. V3
extends the grammar with the member forms `Object."Field"::Trigger` and
`Object.action(Name)::OnAction` (a caption after the name is accepted and ignored on input), and
every JSON row carries `selector`: the shortest form that resolves to that one stable id.

**What is guaranteed** (astra): only `selector` is guaranteed to resolve. A human display is
pasteable for the forms the round-trip test covers, and only those: a workspace
`Object::Routine`, with a parameter list, `Object."Field"::Trigger` and
`Object.action(Name)::OnAction` with or without its caption. Other displays — a dependency
object with its number in brackets (`Codeunit "Job Queue Dispatcher" (448)::HandleRequest`),
data-item members, an app label — are not promised to parse; the description says to use
`selector`. Test: every `selector` emitted in the fixture's and CDO's goldens, and every display
of a covered form, resolves to its own stable id (break: drop the member forms from the grammar).

Where the data is: workspace routines have the member in the model; graph nodes carry `member`,
`objectName`, `memberName` and `arity` (call-graph spec, node table). No engine change.

**Files:** one formatter in `src/engine/l5/routine_rollup.rs` (next to `handle`), read by
`explain.rs`, `digest_cli.rs`/`digest_v3.rs` (summary rows), `effect_query_cli.rs`,
`field_touches.rs`, `prove.rs` and `callers_context.rs`, `test_select/render.rs`, `impact/render.rs`.

### 2.2 Dependency routines by name (F4)

**Current** (c02, c12): `Codeunit 448::handlerequest`, `Table 110::applies-to doc. no.::onlookup`,
`Codeunit 6225383::executerunner`. **New:** `Codeunit "Job Queue Dispatcher" (448)::HandleRequest`,
`Table "Sales Invoice Header" (110)."Applies-to Doc. No."::OnLookup` — the declared case, the name
first, the number in brackets. Read from the graph node's `objectName`/`memberName`/`member`, not
from its id. Same files as §2.1.

### 2.3 The resolved target in prove (F6)

**New:** prove JSON `payload.result.table: {id, name, app}` for `writes-table`, and
`payload.result.event` for `publishes-event`; the human answer line names the table as resolved:
`writes-table:CDO E-Mail Job (table 6175283, this app)`. Prove 2.5.0 (additive). Note: a
misspelled table already fails (`unknown table '…'`, `prove.rs:2574`); the echo is for a number
or a name that two apps share.

### 2.4 Selector failures that help the next call (F7, F8)

**Current** c03: `explain: no routine of the program has the id or name 'SendMailTwice'` (exit 2).
c26: one 1,000-character line with both overloads.

**New** c03:
```
explain: no routine in the loaded program is named 'SendMailTwice' (exit 2: no answer)
  nearest names: Codeunit "CDO Send Mail Management"::SendMail (6 overloads); Codeunit "CDO E-Mail"::Send (…)
  selector forms: Object::Routine, Object::Routine(<parameter types>), Object."Field"::Trigger,
    Object.action(Name)::OnAction, or a stable id
```
**New** c26:
```
explain: 'Codeunit "CDO E-Mail Template Management"::PrintDocuments' names 2 routines (exit 2: no answer)
  1. PrintDocuments(<9 parameter types>)           :1117
  2. PrintDocuments(<the same 9>, Boolean)          :1122  (+ Boolean)
  both: routines: ["<complete selector 1>", "<complete selector 2>"]
```
The sketch abbreviates; the real output prints both parameter lists in full and the `both:`
value holds two complete selectors (or the stable ids), ready to send. Nearest names come from
the selector index (edit distance on the routine name, then same object), at most 5, and only
names within a distance threshold (set on the fixture and CDO; "no close match" when none
passes): five distant names are worse than none. When the name exists only in a dependency app,
the answer says so (`found only in Base Application: Codeunit "X"::Y; selectors are scoped to the
workspace app`) instead of listing unrelated workspace names. The ambiguity stays an error
(astra's recommendation; see §11). Files: `selector_index.rs`, the selector error paths of
explain, prove, digest, query.

**Schema:** explain `al-sem.explain/4` gets additive `signature`/`member`; digest 3.1.0 (summary
rows gain `signature`, `stableId`); query 3.6.0; tests and impact keep version 1 (additive).
**Goldens:** every `--test cli` family with a routine name: `cli-explain-goldens`, `cli-query-goldens`,
`cli-b-goldens` (prove), `cli-output-goldens` (fixture, `impact/`, `cdo/`), the tests and impact
inline goldens. **Consumers:** ConTinAI and bc-dev-mcp key on `stableId`, not on `display` (plan
§4.1); no change. Tell al-lsp-for-agents' owner that display strings change (nothing parses them
today). **Verify (round 2):** c15 and c25 summaries show distinct signatures; c01 and c23 headers
name the field; c05, c06, c12, c17 name actions; c02, c12 paths name dependency objects; c03
lists `SendMail`; c26 offers the two-selector value.

---

## 3. Tiers on witnesses (V2)

### 3.1 prove: possibilities are labelled where they are printed (F10)

**Current** c02:
```
  evidence: 47 witnesses (nearest first)
    [non-interactive-entry] Codeunit 448 is started in a background session or task (at dep:…:282) at dep:…:282 (14 hops, onlyOpen)
  non-interactive entries: background-start 9, install-codeunit 3, upgrade-codeunit 35
```
**New:**
```
  answer:   unknown — no non-interactive route without a hub was established in the analyzed graph;
            47 candidates pass a hub ([hub Codeunit.Run of a variable at OnpremSessionHandler.Codeunit.al:43],
            [hub Codeunit.Run of a variable at JobQueueStartCodeunit.Codeunit.al:32],
            [hub record trigger (Modify) at CDODataUpgrade.Codeunit.al:128])
  established witnesses: none
  candidates over a hub: 47; by context (contexts may overlap): background session or task 9, install 3, upgrade 35
    [via-hub] codeunit OnRun [context: background session or task (job queue)]: Codeunit "Job Queue Dispatcher" (448)::OnRun → … →
      [hub Codeunit.Run of a variable at JobQueueStartCodeunit.Codeunit.al:32] → Codeunit "CDO Auto-Send Runner"::OnRun → …
```
(Counts from c02 as recorded. `upgrade 35` is inflated by the helper-as-entry bug on
`fix/explain-callers-counts`; round 2 takes the number after that fix.)
Rules: the answer line is about the question's own population (here non-interactive routes, not
all callers: c17 shows SetFiles has many code callers) and names the hub kinds actually on the
candidate paths, at most three sites, never a generic list (Fable). "Established" means
established by the static model, not observed. An answer with only hub-mediated witnesses
prints `established witnesses: none` before any candidate; every witness line starts with its
tier; counts of candidates are labelled as candidates and, when they are by context, say that
contexts may overlap. The JSON already carries the tier per witness; it gains
`establishedWitnesses` and `candidateWitnesses` counts (prove 2.5.0). **Files:** `callers_context.rs`
`format_human`, `prove.rs` evidence rendering.

### 3.2 A broad reach is split by tier, kind and relation, and folded (F12)

**Current** c12 (and c20, per routine and again for the whole change):
`reached from: 6,043 page actions (first: Page 21 …), 10,094 page triggers …, 27,718 ancestors`.
**New:**
```
  reached by code callers: no — no code caller found in the analyzed graph (every candidate decided)
  reached beyond it: 27,718 ancestors — by tier: local 0, via-dependency 23,230, via-hub 4,488
    entries among them: <e> distinct, by kind: page actions <k1>, page triggers <k2>, table triggers <k3>,
      event subscribers <k4>, report triggers <k5>, codeunit OnRun <k6>, install triggers <k7>, upgrade triggers <k8>
    by context (contexts may overlap; not summed): background session or task <c1>, install <c2>, upgrade <c3>
    by relation (each entry once): code <a>, UI navigation <b> (a + b = e)
    broad navigation reach (<p>% of the program's entries; list folded) — detail: true lists one representative per kind and tier
    hubs on these paths: [hub Page.Run of a variable at dep:Base Application:<Page 6302's file>:<line>], … (detail: true lists them)
```
The ancestor line is c12's as recorded (`factcheck/c12-impact.json`, `up.ancestors`). The entry
counts are not: c12 recorded nine **legacy** buckets — page-action 6,043, trigger-page 10,094,
trigger-table 1,454, event-subscriber 775, report-trigger 534, onrun-codeunit 464,
background-start 21, install-codeunit 3, upgrade-codeunit 45, which add up to 19,433 — and they
overlap (astra: the same `OnAction` is counted under page actions and under page triggers), so
19,433 is a sum of buckets, not a count of distinct entries. Mapped to the new kinds, they become
page actions, page triggers, table triggers, event subscribers, report triggers, codeunit OnRun
(the 464 plus the 21 background starts, which are `OnRun` entries: 485 before deduplication),
install triggers (3) and upgrade triggers (45, inflated by the helper-as-entry bug on
`fix/explain-callers-counts`, as §3.1 notes for c02). The disjoint counts `<k1>`–`<k8>` and their
total `<e>` are computed after classification, so they stay symbolic; `k1 + … + k8 = e`. Contexts
come from the same classification (`<c1>` the background starts) and are counted on their own:
`<c2>` and `<c3>` count every entry that runs in the install or upgrade context, which includes
the install and upgrade triggers but also any other kind that runs there (for example an event
subscriber raised during installation), so `c2 ≥ k7` and `c3 ≥ k8`, never `c2 = k7` by definition
(astra, revision 4 review). The relation split needs the
relation axis, so it stays symbolic too; **an entry with both a code witness and a navigation
witness counts once, under `code`** (the stronger relation, as tests rank code first), so
`a + b = e`. `<p>` is the reach share of §3.2's first folding trigger; when the hub fan-out
trigger fired instead, the line reads `broad reach over a hub ([hub …] covers <q>% of graph
nodes; list folded)`. All numbers are pre-fix: they include the false `declaredRunnable` edges
`fix/reach-and-test-selection` removes. Round-2 expectations come from the post-fix run (§12.2).

Rules:
- **Labels never depend on size** (astra). Every reach is split by relation (`code`, `UI
  navigation`) and by tier, whatever its population. Code reach (direct and transitive callers,
  publishers) always prints first, with its nearest witness, even when the other lines fold.
- **One unit per line** (Fable). A line counts ancestors, entries by kind, or entries by context,
  never two of them; kinds are disjoint and sum to the entry total; contexts overlap and are not
  summed (§8.1). A share has numerator and denominator of the same population.
- **Folding** (lists replaced by the count lines, back with `detail: true`) fires on either of two
  measures, each with its own stated population (astra: revision 2's "one hub reaches 90% while
  the total is 45%" compared two different things without saying so):
  1. **reach share**: entries reached over navigation or hubs, divided by the program's entries
     (both counted as entries; start at 50%, measured on CDO);
  2. **hub fan-out**: a hub on the paths whose forward cone covers more than half of the graph's
     nodes (cone nodes / graph nodes, the measure `test_select` already computes as
     `overHalfOfGraph`). A hub with a huge forward cone makes any reach through it broad, whatever
     the reach share.
- **Folding is presentation only.** Its words name the trigger that fired and its measure:
  `broad navigation reach (<p>% of the program's entries; list folded)` or `broad reach over a hub
  ([hub …] covers <q>% of graph nodes; list folded)` (Fable i); it never says "not specific" or otherwise changes what a
  reach means (astra: a widely used routine can truly reach most of the program).
- **The hub is named** on the line, in the §8.1 form with its kind from the closed list: it is what
  an agent would fix or ignore.

The fix branch already prints the tier on `first:` items (F11, handled elsewhere); this builds
on it. **Files:** `impact/render.rs` (`reached_clause`, the whole-change block), `impact/up.rs`
for the counts per relation and the share. [engine] note: `fix/reach-and-test-selection`
narrows `declaredRunnable`; the share is measured after it lands.

### 3.3 Tests: tier per group, local first (F13)

**Current** c28: groups list `68960: 18 tests, 7 hops via Page CDO Email Editor → OnAction → …`
with no tier; c07: `run first — local path: none; nearest test: … (4 hops, through dependency code)`.
**New:** every group and the nearest line carry the tier and the relation. A codeunit group whose
tests differ in tier shows the distribution (`18 tests: local 0, via-dependency 18`) and labels
its representative path's tier. A witness that needs UI navigation says `this witness requires UI
navigation; invocation of the action is not established` (astra: the test's own body is not
enough to claim "no action is invoked"; helpers and handlers matter). c07's line becomes `no
local test; nearest [via-dependency]: … (not a focused test)`. JSON: `runFirst[].tier`
exists; add `relation` and `group.tiers`.

Ranking: `select.rs:457` already sorts tests by (tier, hops, codeunit, name), so revision 1's
"ranking ignores tier" was wrong (astra). What is missing is the relation: a `local` path that
needs UI navigation ranks ahead of a `via-dependency` code path today. The rank key becomes
(relation: code before navigation, tier, hops, …). **Files:** `test_select/render.rs`,
`test_select/select.rs` (rank key).

**Verify (round 2):** c02 prints `established witnesses: none` and names the hub sites on its
candidate paths; c12 and c20 split reach by tier, kind, context and relation, code callers first
with the nearest witness, broad reach folded with its hubs named; c28 groups show tier distributions and
code paths lead navigation-only ones; c07 says "not a focused test".

---

## 4. Negative and conditional verdict wording (V2)

### 4.1 A conditional write is a write risk, not a "no" (F14)

**Current** c01:
```
  summary — writes: no observed write that may persist; caller-dependent: 1 table
```
**New** (explain, digest human, query effects):
```
  summary — writes: may modify persistent CDO E-Mail Job records (Modify at :24) when Rec is not temporary;
            otherwise it changes a temporary record; no other write (every candidate decided)
```
(c01's analysis is complete, so the absence uses the top level of the §8.1 negative-certainty
row; under a partial analysis it reads `no other write: none observed (searched: …; not
searched: …)`.)
Rules:
- When a card has caller-dependent writes and no may-persist writes, the summary leads with the
  caller-dependent ones and states the condition in astra's words above ("may modify persistent …
  when … is not temporary; otherwise it changes a temporary record"): "writes … unless temporary"
  reads as unconditional, and a database write is not yet a committed change.
- When the routine is a trigger of the table it writes (`Rec` is the record being validated,
  inserted or modified), the condition names `Rec` and adds `(Rec is temporary only when its
  caller's record is)`.
- **Both classes** (Fable): may-persist writes first, then the conditional ones **on the same
  summary line** (`writes: CDO Log (Insert); may modify persistent CDO E-Mail Job when Rec is not
  temporary`), never only under a later heading.
- Path conditions qualify the line as they do today. An absence uses the §8.1 negative-certainty
  words: `no (every candidate decided)` under a complete analysis, `none observed (searched: …;
  not searched: …)` under a partial one.

This changes wave-2 §3.8 rule 2 (order), which put may-persist writes first unconditionally, and
wave-2 §3.7's absence wording (`no observed …`), which the §8.1 row replaces. The JSON is
unchanged.
**Files:** `routine_rollup.rs` summary line, `effect_query_cli.rs` summary parts.

### 4.2 Field writers: a count with its search (F17)

**Current** c10: `field: CDO Setup.Document Folder (caption "Document Folder"): 0 writer(s)`.
**New:**
```
field: CDO Setup."Document Folder" (field 60, caption "Document Folder", defined at CDOSetup.Table.al:89)
  confirmed writers: 1
    Table "CDO Setup"."Document Folder"::OnValidate — assignment at CDOSetup.Table.al:97 (changes the record in memory; persisted by whoever modifies Rec)
  references that may write, not confirmed (kinds: RecordRef.Field by number, TransferFields copies): none observed (search below)
  writer kinds searched: assignment, Validate, ModifyAll
  searched: 5,479 workspace routine bodies and the dependency bodies loaded; not searched: page and report columns, CalcFormula, TableRelation; dynamic field numbers: 183 sites not matched
  reads, filters and validations: alsem_query_touches with table and field
```
(Counts from c10 and c22's recorded search lines; the writer row is the post-fix answer of
`fix/field-writers-implicit`.) With no confirmed writer: `confirmed writers: none observed
(searched: …; not searched: …)`, never a bare `0`. Confirmed writers and reference candidates are
counted apart (astra): a `RecordRef.Field(n)` establishes a reference to the field, not a write,
and a `TransferFields` copy is a candidate. A writer row says whether it is an assignment, a
`Validate` or a `ModifyAll` (astra): an assignment changes the field in memory, and only a later
`Modify`/`Insert` persists it. The kinds are those `field_touches.rs` classifies as `write` and
`validate` (confirmed) and `by-number` / `copy-candidate` (candidates); the implementing commit
takes the exact list from that code. This uses
the same searched/not-searched sentence as touches field mode (`field_touches.rs`), from one
helper. The count is fixed on `fix/field-writers-implicit`; this lands after it. **Files:**
`explain.rs` field mode, `field_writers.rs`.

### 4.3 A filtered negative says what it filtered, and what else matched (F18)

**Current** c27: `answer: no — no reachable site matches … other sites: 3 reachable`.
**New:**
```
  answer:   no — no FieldError site is reachable (every candidate decided)
  other reachable error sites: 3 — TestField at CDOMergeTableField.Table.al:64; TestField at :65; Get (implicit error) at :526
  to see their routes: op: "TestField", or no op
```
Up to 5 other sites with op kind and location; the rest as a count. The other sites are
labelled as outside the filter. The filter is part of the verdict in every form: the human
answer line, the envelope summary (`filter: {op: "FieldError"}`) and the JSON result
(`result.filter`, `result.otherSites[]`), so a script that reads only `answer: "no"` also reads
what it is "no" about (astra). Prove 2.5.0. **Files:** `prove_slice.rs`, `prove.rs`.

### 4.4 An unknown names what it does not establish, and which assumption changes it (F19, F20, F21, F79)

**Current** c29: `unknown — no witness found, but the cone is not complete … assumptions: none
(apps that depend on this app are not loaded; their calls block a no)`.
c19: `… --closed-world assumes the loaded interface implementations`.
**New** (lines after the answer, only on `unknown`):
```
  plain reading: no interception was found in the analyzed callers; unknown because 2 routines are public
  not established: that no caller turns this error into false; and nothing here says which writes roll back
  re-analysis under an assumption: assumeNoDependents: true — assumes no other app calls these 2 routines;
    removes every blocker of this answer (external callers 2)
```
c02 (eight blocker kinds, as recorded):
```
  re-analysis under an assumption: assumeNoDependents: true — assumes no other app calls the public routines;
    removes external callers 438; stay: over a hub 9,156, order not resolved 6,710, hidden callers 15,
    and 4 more kinds (detail: true lists them)
  not established: that a user is present; do not assume an interactive session in code added here
```
c19: `re-analysis under an assumption: closedWorld: true — only if the deployment has no other
implementations of these interfaces; it changes the assumption, not what runs; removes every
blocker of this answer (interface dispatch (open world) 3)`, and `not established: that it
never commits`. Blocker kinds print in the §8.1 blocker-kinds words; at most three kinds that
stay are named, largest first, the rest counted.

Rules:
- **Every input that removes a blocker kind is named**, with what it removes and what stays
  (Fable 2: the revision-1 rule "only when it is the sole blocker kind" printed nothing for c02,
  the call that motivated it). `removes every blocker of this answer` is printed only when it
  does; no line says the input "decides" the answer (astra: an assumption is not evidence, and the
  re-run's answer is not computed here).
- **`not established:`** states the conclusion the answer does not support, in the question's
  terms: `that it never commits` (may-commit), `that no caller turns this error into false`
  (callers-intercept-errors, with the rollback clause), `that a user is present`
  (callers-may-run-noninteractive, with the editing implication astra asked for, F79). This is
  astra's F21 point without the banned word: the owner rule (agent-tasks §5) bans "safe" and
  "unsafe", not a statement of what is not established. §11.2 row 3.
- The plain reading says "in the analyzed callers", never an unrestricted "no caller".
- With `detail: true`, `callers-intercept-errors` prints one representative uncaught route
  (F79, c29).

**Files:** `prove.rs` human, `callers_context.rs`.

### 4.5 Status says how hard it checked (F22)

**Current** c09 and c24 print the same headline: `FRESH: no input changed since generation 1
(checked just now: 582 files, 11 packages).` — c09 hashed every file, c24 compared size and time.
**New:** `… FRESH: no input changed since generation 1 (every input hashed just now: 594 inputs)`
vs `… FRESH: no change seen since generation 1 (size and modification time, just now; verify: true
hashes every input)`. This edits the agent-tasks §2.3 headline table; `check.strength` is added
(`hashed` / `size-and-time`). The "594 inputs" count is the sum of the categories actually
checked (§9.6), not a fixed formula. With no engine for the universe asked: `not checked`, as
today. `statusVersion` stays 2 (additive). **Files:** `src/mcp/freshness.rs`.

### 4.6 Touches table mode says "transitively" and shows sites (F23)

**Current** c30: `1802 routines touch it` and 12 routine headers from one codeunit.
**New:** `1,802 workspace routines touch it transitively (through workspace or dependency
sites); <s> workspace sites in <r> routines and <d> dependency sites touch it directly`, then
the sites (§9.3). 1,802 is c30's count; the site counts are not in the round-1 answer and are
derived when §9.3 is built. A listed line is a site, never a routine header. **Files:** `effect_query_cli.rs` touches table mode.

### 4.7 A change mode says what it computed (F27)

**New** first line of a digest or impact answer driven by a `diff`: it states the computation
actually performed, not the inputs given (astra), and whether the diff matched the analyzed
source:
- no base: `current effects of the 2 routines this diff touches, in the working tree (the diff's
  lines match it: yes); no before/after computed — pass base for that`;
- diff lines that do not match: `… (the diff's lines match the working tree: no, 3 hunks at
  uncertain positions; routines placed by name)`;
- base given, delta skipped (`delta: false`) or the base build failed: `base <ref> given; before/after
  not computed (delta: false | base build failed: <reason>)`.

With a computed base, impact already prints its comparison tuple. **Files:** `digest_cli.rs`
human header, `impact/render.rs`.

**Schema:** prove 2.5.0 (additive); explain, digest, query text only. **Goldens:**
`cli-b-goldens` (prove), `cli-explain-goldens`, `cli-query-goldens`, `cli-output-goldens` incl.
`cdo/`, impact and tests goldens. **Consumers:** ConTinAI `verify.py` reads `result.answer`,
`evidence`, `blockedBy`, `assumptions` — unchanged. **Verify (round 2):** c01 and c23 lead with
the conditional write; c10 prints `confirmed writers: 1` with its search; c27 lists the 2 TestField
sites and the Get, and its summary carries the filter; c02, c19 and c29 name their assumption
inputs with what each removes and print `not established:`; c24's headline says "size and
modification time"; c30 says "transitively" and shows sites; c20's first line says no
before/after and that its diff matched.

---

## 5. Result first, and one result header (V4)

### 5.1 The answer leads (F28, F29) — F29 and F75 land in V2

**c06** (explain `callersContext`) **new** first section after the header:
```
  own state changes: sets 2 codeunit globals; no record operation
  for adding a COMMIT, an error or UI here (callers' context):
    pending writes: yes — 8 witnesses of an uncommitted write when this runs (7 from page triggers); first [local]:
      Page "CDO Sales Invoice Actions"::OnAfterGetCurrRecord → <write> at …:NN, then SetReportSelectionUsage at …:276
    intercepted errors: unknown (blockers: external callers 2)
      re-analysis under an assumption: assumeNoDependents: true — removes every blocker of this answer
    non-interactive: unknown (blockers: external callers 2, background target not resolved 2,
      publication data not loaded 1, publication file not read 1)
```
The counts are c06's as recorded (`pending writes: yes — 8 witnesses (trigger-page 7)`;
`intercepted errors: … external-callers 2`; `non-interactive: … external-callers 2,
publication-unknown 1, unread-publication-file 1, unresolved-background-target 2`). The first
witness's write and line are illustrative: the round-1 answer printed no witness, and the
caller counts move on `fix/explain-callers-counts`. The witness comes from the pending-writes walk
the section already runs; explain prints its first witness (tier, source of the write, order of
write and call) instead of only the count. The count says "witnesses" unless distinct callers
were counted (astra). The non-interactive line names every blocker kind by count, in the §8.1
blocker-kinds words, not the one that is easiest to explain. `own state
changes` (F75) lists what the routine itself changes: globals, record operations, or "none";
when its cone calls `ChangeCompany`, it adds `target company: not established by this analysis
(ChangeCompany at <file>:<line>)` (§9.1). **c17**: the `via-dependency` / `via-configuration`
callers block moves above the caller list. **c21/c24**: covered by §5.2 (summary first).

### 5.2 One result header on every tool, and a summary always (F30, F42, F43, F61, F76, F77) — `tableAnswers` and F61's per-family scope land in V2

**Human output** starts with the same four lines (astra: human only; a JSON document never gains
a text preamble). The third line is the only place analysis, assumptions, limits and scope print
(Fable f: revision 2 had two spellings on one answer):
```
<tool> <resolved inputs, canonical names>   [universe, revision generation]
answer: <the one-line result>
analysis complete · assumptions: <full labels> · limits: <none | kinds with counts> · scope: <one clause>
more: <MCP inputs>
```
`analysis` is `analysis complete`, `analysis partial (<reasons>)` or `not analyzed (<reason>)`
— the §8.1 completeness words.
`answer:` is the verdict for prove, the selection line for tests, the accounting outcome for
impact, the headline for status, and the card's summary line for explain and digest (Fable:
explain has no yes/no). Variants: an unresolved input prints `answer: no answer — <why>` and the
input as given, not "resolved inputs"; status prints its headline(s) only.

**JSON documents** (F76) carry the same facts as structured fields in every standalone
document, readable without the MCP envelope: `payload.result.scope` with `analysis`
(`complete` / `partial` / `notAnalyzed`, and `reasons`), `assumptions` (the full labels), `limits`,
and `families` where the tool reports operation families (F61: query effects `reads`, `writes`,
`locks`, `validate`, and the deep-write aggregate `beyondDependencyCalls`). Each family is a
structured object, not a sentence (astra):
```json
"locks": {"depth": "oneDependencyRoutine", "inventory": "notExhaustive",
          "reasons": ["dependency code deeper than one routine is not inventoried"],
          "blockers": {"callNotResolved": 0, "interfaceDispatchOpenWorld": 0}}
```
`depth` is `operation` / `oneDependencyRoutine` / `wholeCone`; `inventory` is `exhaustive` /
`notExhaustive`. Tests and impact add `selection` (`complete`, the cap asked, returned, total,
fallback).

**F61 lands whole in V2 for query effects** (astra: c08 asked for JSON, so a human-only interim
would leave the call that raised it unfixed). V2 adds, for query effects only, both the human
scope clause (`reads, locks: one dependency routine deep, not exhaustive beyond; writes beyond
dependency calls: whole cone`) and the JSON `payload.result.scope.families` object above
(additive, query 3.6.0). The shared `result.scope` block for the other tools stays in V4, where
the rest of the header contract lands.

**Envelope `summary`** (≤ 8 KiB, as today) gains, per tool: explain — each card's header and
summary line; prove — answer, filter, established/candidate counts, the assumption inputs and
what each removes; impact — accounting, outcome, first five routines, the tests line with run
target, scope and fallback; tests — `selectedTests` (≤ `maxTests`), totals, truncation,
`selectionComplete`, run target, fallback. Digest summary rows (F42, F77): **one row per stable
id** — four c15 rows, twenty c25 rows, each with its signature (astra: folding overloads into one
row would erase the differences the agent asked about). Facts shared by several rows are
factored into one `shared` block that the rows reference, so the rows stay small. Each row has
`tableAnswers` for every `writesTables` entry (`{table: {id, name}, answer, blockers,
firstBlocker}`) and its test status in the §8.1 words — `tests with a modeled path` (with the
nearest test), `no modeled path`, `analysis partial`, `not in projection`, `not loaded`, `not
requested` — never collapsing one into another (astra: `analysis partial` and `not in
projection` are not "no path"); the test scope and fallback print once. query effects summary (F43) gains `writes.byTable` (top
10 with op counts, the total table count, a truncation flag, and caller-dependent and
unresolved-table counts apart).

**Overflow policy** (astra: revision 2's rule could not hold when the mandatory content alone
exceeds 8 KiB). The summary stays bounded at 8 KiB and always holds the headline, the analysis
line, the fallback and the complete input accounting. Everything the caller asked for by name —
each requested root's identity and verdicts (`tableAnswers`, the prove answer and filter), each
selected test up to `maxTests` — also goes, always, into a **complete compact result index**: a
JSON file written next to the answer in the server's output folder
(`<answer>.index.json`, schema `al-sem.result-index/1`, one row per requested identity with its
verdicts and status, one row per selected test). The summary carries as many index rows as fit,
then `index: {path, rows, sha256}` and `summaryTruncated: {rowsListed, rowsTotal, dropped: [...]}`.
Nothing requested is ever only in the full document, and the selection is never silently cut:
`summaryTruncated` is about the transport; `selectionComplete` is about the selection, and they
are reported apart. `countsOnly` is replaced by these fields. The index is written by the same
renderer that writes the summary (`DigestRunResult` and its siblings already return the summary,
plan 1b), so it costs one more file write, not a new pass.

The JSON writer sorts keys, so "first" in a document can only mean "in the summary"; that is why
the summary carries the selection, not a reordered document.

### 5.3 Boilerplate: keep the facts, drop the prose (F31, F32)

Wave-2 §3.7 says coverage, assumptions and limits are always visible, never folded. That stays,
and the assumption keeps its full sentence (astra: "loaded program" alone hides the exclusion).
What changes: they print as the header's third line (§5.2), `analysis complete · assumptions:
loaded program; unloaded implementations and subscribers are not enumerated · limits: none ·
scope: …`. The static scope prose that is the same for every answer (writes to temporary records
do not directly persist those records, line base, which input lists everything) moves to the
tool description, where it already is; the description takes this wording instead of today's
"temporary records are not writes", which contradicts F15's independent access kind (astra). A scope fact that limits **this** answer stays beside it (astra): dependency depth when the
answer reached dependency code, an effect family not reported (`no lock facts`), literal-route
treatment when rows were ruled out. Empty sections (`extension points: none`, `events: 0 … 0
more`, `rules: none of its own; inherited: none`, `commits: none` repeated after the summary)
are not printed when the summary line already says it. A section about code the agent did not
ask about prints as a count, not a line (F81: c05's `telemetry at 1 place: TELEMETRY beyond
Codeunit Base64 Convert::ToBase64(InStream)` becomes part of the summary's kind counts). Touches
field mode drops its `note:` that repeats the `searched:` clause (F85). The dependencies line is one form everywhere: `dependencies: 11 apps`
in the header clause; the full list only with `all`/`detail`; the envelope's `universe` block has
them (finishing wave-2 Q6, which `finish_human` applied only where a `scope:` line exists).

### 5.4 Status answers in one copy (F33)

The text block becomes the headline(s), `check` (fresh, strength, complete, checked revision
and time, inputs by category including a test engine's production-source inputs, residual:
unsaved buffers and the same-size/same-time limit) and `refresh` per engine, about 500 bytes;
`not checked` with its reason when no engine matches; `structuredContent` keeps the full JSON (operator
data under `details`, as agent-tasks §2.3 set). This follows owner decision 3 (one copy of the
answer). **Files:** `src/mcp/server.rs` status, `freshness.rs`.

**Schema:** digest 3.1.0 (summary and `result.scope`, additive); query 3.6.0, prove 2.5.0,
explain `/4`, tests and impact version 1 gain `result.scope` additively; envelope `summary` for
explain/prove/impact/tests is new (MCP side, pinned by `mcp_server.rs`). **Goldens:** every human family (the header); the
envelope tests in `tests/cli/mcp_*.rs`; `cli-output-goldens/cdo/*.digest-summary.json`.
**Consumers:** bc-dev-mcp reads digest documents, not the MCP summary; no change. **Verify:**
c06 leads with pending writes and names a witness; c15's summary carries the CDO File answer on
each of its four rows; c25's summary has 20 rows, each with a signature and a test status, or
says what it cut; c21's envelope lists the selected tests and the run target; c16's JSON has
`result.scope`; c09's text is under 600 bytes; c01's and c11's dependency list is a count.

---

## 6. MCP-shaped follow-ups (V4)

### 6.1 Name MCP inputs, not CLI flags (F34, F37)

**Current:** `--all lists them` (c04), `--closed-world assumes …` (c19), `alsem prove <routine>
writes-table:<table>` (c11), `alsem diff <base> <new> gives them` (c13), `every one with detail or
format json` (c13), `--tier dependency lists them` (c07), `The next command builds it` (c14).
**New:** the MCP front end appends a hidden `--surface=mcp` to the argv it builds. With it, a
follow-up is a complete call, not a flag fragment (astra): `{tool, arguments}` with the resolved
selector, the question or filter and the scope inputs already filled, printed as
`alsem_prove {"routine": "<selector>", "question": "writes-table:CDO File"}` and carried in JSON
as `followUps: [{tool, arguments, why}]`. A follow-up keeps the assumptions of the call it came
from (astra). A call that changes one (`closedWorld`, `assumeNoDependents`) is never an ordinary
follow-up: it is §4.4's `re-analysis under an assumption:` line, the one label for it, which at
`--surface=mcp` carries the complete call — `re-analysis under an assumption: alsem_prove {…,
"closedWorld": true} — only if <the assumption holds>; removes …` — with `kind: "conditional"`
in JSON (Fable ii: revision 3 had two labels for one line). An in-place input change prints as
the input (`all: true`, `tier: "dependency"`). The no-engine status (c14) names an analysis call
that builds the engine (`alsem_explain {"routines": […], "singleApp": true}`), not "any tool"
(a second status call builds nothing). A command with no MCP tool (`alsem diff`) is named as
"not available over MCP" or dropped. The byte contract holds: the MCP answer equals the CLI's
stdout for the same argv, and the contract tests pass the flag. One helper owns the mapping
(`src/engine/follow_up.rs`, a table flag → MCP input), so no renderer spells a flag.

Tests: one fixture golden per tool rendered with `--surface=mcp` (Fable: a byte compare catches a
misspelled input, which a regex does not); every `followUps[].arguments` validates against the
tool's input schema in `tools.rs`; the invariant "no CLI flag in renderer-owned follow-up text"
covers `next:`/`more:` lines and fold notes only (astra: source excerpts, diffs and `---`
headings legitimately contain `--`). Break: drop the flag from the front end.

### 6.2 `next:` only when it applies (F35)

`next:` lines are built from the card, and a suggestion is dropped only when its answer is
already on the card (astra: an unknown or incomplete cone is exactly when a question helps):
`writes-table` for a write the card shows without a witness, or when writes are `none observed`
(a partial analysis); `may-commit` when commits are `yes` without a witness or `none observed`,
not when the card says `no (every candidate decided)`; the callers' context questions when the
card has callers. c06 (complete analysis, writes and commits `no (every candidate decided)`) gets
neither. c01's "alsem prove names
the caller" is dropped: no prove question names which callers pass a non-temporary record
(revision 1 suggested `callers-have-pending-writes` for it; it does not answer that).

### 6.3 What a non-zero exit means (F36)

The envelope gains `outcome`: `answered`, `no-routine-changed` (complete accounting only, §1
F24), `input-not-mapped`, `not-found`, `ambiguous`, `usage-error`. Exit 0 for `answered` and
`no-routine-changed`, 2 for the rest (no answer). The descriptions say so once in the server
instructions.

**Goldens:** the CLI goldens do not move (the default stays CLI-worded); new `--surface=mcp`
fixture goldens, one per tool, in the `cli-output-goldens` family; the MCP contract tests and
`output_invariants_w3::descriptions_*` gain the flag cases. **Verify:** c04, c13, c19, c11, c14
name MCP calls with complete arguments; c06 has no writes-table or may-commit suggestion; c13's
envelope says `outcome: no-routine-changed`.

---

## 7. Size and dedup of large answers (V5)

V5 restores the MCP default inline cap of 40 KiB that V3 raised to 48 KiB on 2026-11-06
(the one routine display grew RunReportID's CDO explain card to 45,058 bytes; §7.5 and §7.7's
path dedup bring it back under).

### 7.1 Touches `callers: true` (F38)

**Current** c17: 961 lines, 320 KB; every caller line repeats `12 touches (12 on persistent
records, 0 …); first [CalcFields] read at …:740 in Table CDO E-Mail Log::GetEMailFromStorage, through …`.
**New:**
```
  up: all 952 transitive callers touch CDO E-Mail Log through other branches (not through SetFiles itself);
      shared first touch: [CalcFields] at CDOEMailLog.Table.al:740 (GetEMailFromStorage)
  also, not in the 952: 2 workspace callers [via-dependency]; 81 [via-configuration] candidates
      (Codeunit "Workflow Response Handling" (1521)::ExecuteResponse); the three populations are disjoint
  callers (952 routines in N objects), by object, nearest hops first:
    Codeunit "CDO E-Mail Template Management" (9): CreateDocumentsForRecord :… (+1 hop), CreateDocuments(<list A>) :… (+2 hops), CreateDocuments(<list B>) :… (+3 hops), …
```
Every caller stays, each with its identity (§2.1), definition location, hop distance and touch
classification (the task asked for all of them; the 952 rows are 952 distinct routines, and Fable
concedes the point). The "through other branches" relation stays. The three populations' counts
say whether they overlap (here: disjoint). The witness prints once when shared, and per row only
when it differs; the `callers` description says exactly that (F39). Target: under 60 KB on c17.
JSON: `up.callers[]` rows refer to a `witnesses` table by id. Query 4.0.0 together with §7.2.

### 7.2 query effects JSON (F40, F43)

**Current** c08: 1,005,860 bytes for 671 rows; `firstHop` and `origin.routine` (display, object,
stable id, anchor) inlined per row; `via: "inherited"` in all 671. **New:** digest 3.0's shape:
`payload.routines` (`r:<n>` → display, signature, object, definition anchor, stableId) once;
rows carry `firstHop: "r:12"` and `origin: {routine: "r:40", file, line, column}` — the
operation's own location stays on the row, apart from the routine's definition anchor (astra).
`via` moves to a document default (`payload.defaults.via: "inherited"`) and a row carries it
only when it differs. Target: under 200 KB on c08. **Schema:** query 4.0.0 (fields change shape). **Consumer:**
ConTinAI `effects.py` reads each row's `origin.anchor.line` (plan §4.1): in 4.0.0 the
operation's line is `origin.line` on the same row (no join needed); the routine table is only
for the routine's identity and definition anchor. Tell its maintainers before merge; keep `--schema 3` for one release, as digest did
(owner decision 5).

### 7.3 Digest with many roots (F41)

**Current** c25: 1,182,595 bytes for 20 roots of one file, six `SendMail` overloads with ~190
effects each. **New:** the intrinsic facts of an effect (site, operation, table, effect type)
are shared: `payload.effects` holds each once. What can differ by root stays per root (astra):
each root lists its effects as references with annotations — literal liveness (live / ruled
out), temp state in that root's frame, path or crossing, conditionality, and ordering or
refutation evidence (`validForRefutation`). A root's view is the shared facts plus its own
annotations, and is the same as today's per-root entry. Test: a fixture site live from one root
and ruled out from another keeps both answers after sharing; and every root's reconstructed
entry equals the 3.x entry for the same input (break: drop the per-root liveness annotation).
`compact: true` stays one line, but the default (indented) form is what the tool documents for
file readers. Target: under 300 KB on c25. **Schema:** digest 4.0.0, `schema: 3` kept one release.
**Consumer:** bc-dev-mcp reads digest 2.x and is pinned to 1.7.1 (`satisfiesPin` refuses another
major), so it is not exposed until its pin moves; its readers (`risk.ts`, `alsem-writes.ts`) then
read each root's effect references and annotations and join them with `payload.effects` for the
intrinsic facts (a root's liveness and temp state come from its annotation, never from the
shared row). ConTinAI does not read digest.

### 7.4 Tests JSON (F44)

`hidden` becomes `{"<class>": {"reason": "…", "nodes": [...]}}` (reason once per class); a node
whose reason, channel or `selectionRelevant` differs from its class's keeps them on the node
(astra: grouping must lose nothing). `nearestTestedCaller` omitted when null. `al-sem.tests` version 2 (a field changes shape). No
outside reader yet (bc-dev-mcp adopts it later, agent-tasks Q10).

### 7.5 prove witness paths (F45)

Witnesses that share a tail print it once under a stable segment id: the first witness prints
`… → [segment S1: 7 workspace hops] → SetFiles` with the hops listed once under `segments:`, and
later witnesses print `… → [S1] → SetFiles` (astra: "see first witness" breaks when the order
changes). Hub sites, guard annotations and tiers stay on the hop that has them. A location
prints once per line (today `(at X) at X`); a `through` clause prints only when it names a
routine other than the `in` one (F84, c23's `in …::SetMergePDF, through …::SetMergePDF`). Blocker
kinds print as counts on one line, with the ones that carry an action (an unrecognized guard, a
missing `roots.config.json`) listed first. The traversal statistic `coverage: the reverse cone,
42651 states over 10091 nodes` (c02, c29) leaves the human text and stays in JSON (F83). Target:
c02 under 3 KB.

### 7.6 impact detail (F46)

With `detail: true`, a routine gets an evidence budget instead of ten items per list (astra: ten
per category is still a census): its direct callers and publishers with call sites, then one
representative per relation and tier (`code` × `local` / `via-dependency` / `via-configuration`
/ `via-hub`, and `UI navigation`), each on its own line, about 15 lines per routine; the counts per kind stay on one line. The
whole-change block is omitted when the change has one routine; "no base given" prints once in
the header (§4.7). Target: c20 under 4 KB.

### 7.7 explain caller rules (F47)

`caller rule (checked by 2: …; no equivalent check recognized in 41 analyzed callers: 5 pages,
14 page extensions — detail: true lists them)`; ties in "most actions" print as `6 objects with
5 each`.

**Schema majors in V5: three** (astra): query 4.0.0 (§7.1, §7.2), digest 4.0.0 (§7.3) and
`al-sem.tests` version 2 (§7.4). Each keeps the previous shape behind `schema: <previous>` for
one release (owner decision 5's pattern), and the word renames of §8 that touch these documents
ship in the same major (§8.3), so no reader sees two breaking changes.

**Goldens:** `cli-query-goldens` (JSON), `cli-output-goldens` digest JSON and `cdo/` summaries,
the digest size ratchets in `output_invariants*.rs`, tests JSON goldens. The ratchets are lowered
to the new targets in the same commits. The byte targets are ratchets, never a reason to drop a
requested identity or an annotation a decision needs (astra). **Verify:** c17 < 60 KB with 952
caller identities; c08 < 200 KB; c25 < 300 KB with every root reconstructable; c21 `hidden` once
per class; c02 < 3 KB; c20 < 4 KB.

---

## 8. Vocabulary and counting units (V0 decides; every later milestone uses it)

### 8.1 The word table (F49–F52)

One spelling per idea, used in human text and JSON values alike. Each row is a separate axis; a
path or a fact carries one value per axis (astra: tier alone cannot say "code or navigation").

| Axis | Words | Replaces / notes |
|---|---|---|
| **tier** of a path (where it goes) | `local` (no dependency hop, no configuration hop, no hub), `via-dependency` (a dependency hop, no configuration hop, no hub), `via-configuration` (a dispatch whose targets are decided by data the engine can name, such as workflow responses or report selections), `via-hub` (an unresolved dispatch hop) | replaces `onlyOpen`, `only-open`, `only over open sites`, `through an open site (a hub)`, `throughDependency`, `through-dependency`, `tier configured`. **Precedence** (Fable): a path's tier is its weakest hop, `via-hub` > `via-configuration` > `via-dependency` > `local`. Configured dispatch is its own tier, not a hub (both reviewers: a configured target can be decided by reading data; a hub cannot). Printed `[<tier>]` at the start of a witness or a population (`[via-hub] codeunit OnRun …`, `2 workspace callers [via-dependency]`) |
| **result tier** (what an answer found) | `established witnesses: N` / `candidates over a hub: N` / `candidates over configuration: N` | keeps the fact that **only** hub-mediated witnesses were found, which a per-witness tier loses (astra). JSON `establishedWitnesses`, `candidateWitnesses` (with `byTier`). "Established" = by the static model, never observed. A `via-configuration` witness is always a **candidate** (astra): the binding lives in data (workflow responses, report selections, `roots.config.json` entries) the analysis has not read, so naming the data does not establish that it is configured. A `local` or `via-dependency` witness with no hub and no configuration hop is established |
| **relation** of a witness (how it runs, over its whole length) | `code` (every edge is a call, run, trigger or event) / `UI navigation` (any edge is a page the user opens from another page) | new axis; the weakest edge decides, so a test that starts with a code call and later needs a user to open a page is `UI navigation` (astra). Impact and tests rank code before navigation (§3.3). Counted per **entry** (impact, tests, digest totals), an entry with both a `code` and a `UI navigation` witness counts once, under `code` (§3.2; Fable, revision 4 review) |
| **projection** | `approximate` flag beside the tier | replaces `localApproximate`, which must not silently become `local` (astra): the test universe's source projection qualifies the path |
| **hub kinds** (closed list, printed `[hub <kind> <name>? at <file>:<line>]`, the variable or receiver name optional after the kind) | `Codeunit.Run of a variable`, `Page.Run of a variable`, `Report.Run of a variable`, `background start of a variable`, `record trigger (<operation>)`, `interface dispatch (open world)` | replaces `h:run:codeunit`, `h:record:modify`, `HUB:run:report`. A hub is an **unresolved** dispatch. Known targets print as a list, not as a hub; when known targets and an unresolved remainder coexist (an interface with loaded implementations under open world), both print: `interface dispatch (open world): 2 known implementations (…) + [hub interface dispatch (open world) at …]` (astra). Configured dispatch is a tier, not a hub kind. A page variable declared and never run is **not** a hub: it is the false `declaredRunnable` edge `fix/reach-and-test-selection` removes (astra), and no output names it as a route. The implementing commit checks the list against `impact_graph` hub classes (`any`/`name`/`run`/`record`/`callbacks`) and adds any it misses |
| **entry kinds** (what routine starts a run; closed list; an entry has exactly one) | `page action`, `page trigger`, `report trigger` (request page, data-item triggers), `codeunit OnRun`, `table trigger`, `event subscriber`, `install trigger`, `upgrade trigger`, `test procedure`, `public procedure` (callable by other apps) | replaces `page-action`, `trigger-page`, `page-event`, `page triggers`, `trigger-table`, `report-trigger`, `onrun-codeunit`, `install-codeunit`, `upgrade-codeunit`. Kinds are disjoint: counts by kind sum to the entry total. Only the trigger routines of an install or upgrade codeunit are `install trigger` / `upgrade trigger`, not its helpers (fact check c02 #7, on `fix/explain-callers-counts`) |
| **execution context** (how the run was started; closed list; printed `[context: …]` after the kind) | `UI session`, `background session or task` (with `(job queue)` when started by the Job Queue Dispatcher or a `roots.config.json` job-queue entry), `API page`, `web service`, `install`, `upgrade`, `test run`, `context not established` | new axis (Fable 3, astra): replaces `background-start` and the context words revision 2 mixed into the entry list. An entry can have several contexts, so counts by context **overlap and are never summed**; a line of context counts says so. An `install trigger` entry always runs in context `install` and an `upgrade trigger` entry in context `upgrade`, so those context counts equal the kind counts and are not extra entries (Fable iii). Example: `codeunit OnRun [context: background session or task (job queue)]` |
| **blocker kinds** (why an answer is `unknown`; closed list) | callers' context: `external callers`, `hidden callers`, `over a hub`, `order not resolved`, `publication data not loaded`, `publication file not read`, `guard not recognized`, `background target not resolved`, `effects incomplete`, `TryFunction caller`, `limit`; existing-effects questions: `call not resolved`, `interface dispatch (open world)`, `conditional effect exists`, `analysis gap`, `cone truncated`, `witness truncated`, `coverage incomplete` | replaces the kebab-case keys in human text (`external-callers`, `only-open`, `order-unknown`, `publication-unknown`, `unread-publication-file`, `unrecognized-guard`, `unresolved-background-target`, `effects-incomplete`, `tryfunction-caller`, `unresolved-callsite`, `open-world-dispatch`, `non-unconditional-effect-exists`, …). JSON keeps its keys (§8.3). Taken from `callers_context.rs` (`block(` calls) and `prove.rs`; the implementing commit adds any kind it finds missing |
| **dependency boundary** | `dependency call` (the crossing), `dependency routine entered` (the target) | replaces "entry point" in explain/digest/query. "entry" alone always means an execution entry (Fable) |
| **verdict** | `yes` / `no` / `unknown`, wherever a verdict exists: prove, digest `tableAnswers`, touches `down`, explain's `writesTables` | astra: "prove only" contradicted the spec's own `tableAnswers` |
| **negative certainty** (Fable) | `no (every candidate decided)` — proven absent under the stated assumptions (a complete analysis); `none observed (searched: …; not searched: …)` — nothing found by a partial analysis or an index search, with its search; `unknown (blocked by: …)` | governs an **absence verdict** — the answer to something an analysis searched for (writes, commits, writers, callers, consumers, sites), in card summaries too. It does not govern a zero **count** or an empty **list** (`established witnesses: none`, `limits: none`, `calls: none`, `own state changes: … no record operation`), which keep their plain words (Fable B). `0 writer(s)`, `no observed write`, `no indexed consumer observed` and wave-2 §3.7's `no observed …` (and its `none` when it answered a search) map to one of the three |
| **target resolution** | `table: <name>` / `table: not resolved (<receiver kind>: <reason>)` | replaces `isUnknownBucket`, `unknown` as a table, "decided at run time". A RecordRef does not prove the table varies at run time (astra: some RecordRefs resolve statically), so the reason says what failed, e.g. `RecordRef opened from a variable` |
| **temp state** | `temporary` / `not temporary` / `caller-dependent (parameter 3, <name>)` / `not resolved` | replaces `known(true)`, `known(false)`, `parameter-dependent(i)`, `temp state unknown`; independent of access kind and reachability |
| **condition** of an effect | `unconditional on success`, `in a loop`, `conditional`, `condition not resolved` | replaces the bare `(unknown)` after an error site (c05) |
| **completeness** | always with a subject: `analysis complete` / `analysis partial (<reasons>)` / `not analyzed (<reason>)`, `selection complete|incomplete`, `freshness check complete`; and the non-results `not analyzed`, `not requested`, `not loaded`, `not applicable`, which are not synonyms for partial or empty (astra) | replaces bare `complete`, `coverage: complete`, `testCoverage` status words |
| **test status** of a routine | `tests with a modeled path` (static; changed-line execution is not established), `no modeled path`, `analysis partial` (the completeness row's word), `not in projection`, `not loaded`, `not requested` | replaces `reached`, `no-known-path`, `analysis-incomplete`, `not-in-projection`, `not-loaded`, and "coverage" for static reach |

Fable's proposal to pair `not resolved` with `run-time` on every axis is adopted for `not
resolved` and declined for `run-time` **for table, temp-state and condition targets**: for those
the engine infers "run time" from the receiver kind and does not prove it (§11.2 row 5). For
**call edges** a proven run-time fact exists — the resolver's `HonestDynamic`
(`src/program/resolve/edge.rs`, `DispatchShape::DynamicOpen`), "provably runtime-typed" in
CLAUDE.md's taxonomy — and it keeps its word, `honest dynamic`, wherever call resolution is
reported (Fable).

### 8.2 Count units (F53, F54)

Every count names its unit and, where two counts could be compared, how they relate: c05
`writes beyond dependency calls: no (every candidate decided; 1 dependency call writes only
temporary records, which does not directly persist them)` — an absence verdict, so the §8.1
negative-certainty words; c15
`commits: 1 site (2 more under literal arguments that cannot run)`; c08 summary `671 operations on
81 tables one routine deep; whole cone beyond 14 dependency calls: 397 tables (writes only)`;
c29 `uncaught at: page actions 30, page triggers 42 (distinct entries; an entry is counted
once, in its own kind)` — a total prints only when the kinds are disjoint by construction, and
the unit (entries, paths, states) is named (astra); c17 `+3 hops`; c22 `consumers: 3 sites, 1
candidate`. Operations, sites, tables, routines and entries are counted apart, and a category
sum is never printed as a total of distinct things. Touches tags get a one-line
legend under the first tagged row (`implicit-access: a bare field name inside the table's own
code`). Wave-2 §3.6 is the rule set for the units; this adds "print the unit".

### 8.3 JSON migration rule (astra)

Replacing a value inside an existing field is a breaking change, not an additive one. One
exception (Fable p): a correctness fix that removes rows a reader should never have counted
(F15's read rows under `writeTables`) is a minor bump; a rename or a changed meaning is not. So: a
renamed value goes into a **new field** beside the old one (`tier: "onlyOpen"` stays, `pathTier:
"via-hub"` is added); the old field stays for one release, then leaves in the next major. For
query, digest and tests the old fields leave in their V5 majors (§7), so readers see one break;
for prove, explain and impact the new fields are a minor bump (prove 2.6.0) and the old ones
leave in their next major. The vocabulary invariant (no replaced word) covers renderer-owned
human text and the new fields only, not legacy fields during the window or quoted source.

**Goldens:** every human family. **Verify:** round 2's bundle has none of the replaced words in
renderer-owned text (a grep over the 30 answers, also kept as an invariant on the fixture: break
= put one old word back).

---

## 9. Per-tool items (V6)

### 9.1 explain (F55, F56, F67, F48, F72, F80) — F48, F55, F56 land in V2

- **Calls first** (F55): `calls: Codeunit "CDO E-Mail Template Import/Export"::ExportTemplates
  (:181)`, the direct callees in source order, at most 5 (impact's routine block too), and, for
  each effect the card leads with, its short route from the routine (`UI: DownloadFromStream at
  CDOEMailTemplateImportExport.Codeunit.al:24, via ExportTemplates → ExportAsXMLDialog`): a
  callee list alone does not explain how the action reaches its effect (astra).
- **Tables named when few** (F56): `writes: CDO Setup (Modify at :62; the routine assigns <its 3
  fields>)` when 3 tables or fewer; the field list follows wave-2 2d (the routine's assignments,
  on INSERT/MODIFY/VALIDATE only) and is labelled as the routine's assignments, not as what each
  `Modify` writes; receiver and control-flow qualifications stay (astra). Impact's `may write 1
  table` becomes the same.
- **Company context** (F75, astra): when the routine's cone calls `ChangeCompany` (a catalogued
  builtin, so the resolved edge is there), each write line it may affect adds `target company:
  not established by this analysis (ChangeCompany at <file>:<line>)`. c20's subscriber calls
  `CDOSetup.ChangeCompany(NewCompanyName)` at `CDOSubscribers.Codeunit.al:55`, so its CDO Setup
  write prints that line. The analysis tracks no company per record (§10), so the fact is never
  inferred and never silently omitted.
- **Caller rules** (F67): `caller rule: rejects when not VariantRec.IsRecord; checked by 2
  callers; no equivalent check recognized in 41 analyzed callers` ("skipped" reads like a bypass,
  "not checked" like a proof of absence).
- **showSource** (F48): the excerpt shows the executable line (the call or the operation), not
  the declaration; the root's own excerpt starts at its first statement.
- **Own events vs reached events** (F72): `events raised (17, downstream)` and `publishes: none
  of its own` on one line. **Event names** (F80): the distinct event names print when they come
  from 5 publishers or fewer (`Codeunit "CDO Events": OnBeforeExportTemplate, OnAfter…`), else
  `detail: true lists them`.

### 9.2 query effects (F57) — V2

The summary always prints `locks:` and `reads:` (with `none observed` when zero), since a lock
question (c11) must not depend on a missing line meaning zero. Each carries its scope (astra):
`locks: none observed (one dependency routine deep; deeper dependency locks are not
inventoried)`. The counts include every temp class and unresolved-table operations (§1 F15).

### 9.3 query touches, table mode (F59, F60, F74) — field mode (F74) lands in V2

**New** (c30):
```
--- touches of "CDO E-Mail Template Header" (table 6175283) ---
  workspace sites: <s> in <r> routines — by op: Get <g>, FindSet …, SetRange …, Modify …, Insert …, SetCurrentKey <k>
  dependency sites: <d> in <q> dependency routines (one dependency routine deep), by op: …
  by object: Codeunit "CDO E-Mail Template Management" <n1> sites, Table "CDO Document Template Line" <n2>, …
  transitive: 1,802 workspace routines reach one of the sites above (workspace or dependency)
  key-relevant (not exhaustive): Get <g> (primary key), SetCurrentKey <k>, SetRange/SetFilter on key fields …
    [Get] at CDODocumentTemplateLine.Table.al:1162 in Table "CDO Document Template Line"::…
```
1,802 and the `Get` at `:1162` are c30's (the fact check's sample); the site counts are not in the
round-1 answer and stay symbolic until §9.3 is built; the by-op counts add up to the site total.
A **workspace site** is an operation on this table in a workspace routine's own body; a
**dependency site** is one in a dependency routine the workspace reaches (astra: a workspace
routine can touch a table only through a dependency-owned operation, so the transitive count
must not claim every consumer reaches a workspace site). The full routine list stays available
with `all: true`. c18's unresolved-table bucket gets the same shape, each site with `table: not
resolved (<receiver kind>: <reason>)`, and the warning that an unresolved write can be any table. Key-relevant uses (not
exhaustive, and labelled so): `Get` sites (they use the primary key), SetCurrentKey sites (from
the operation index), and SetRange/SetFilter on key fields, which needs the field-use index
joined with the table's declared keys — field mode already reads them (`field_touches.rs`
`declared_keys`, advertised in `tools.rs`). If the join costs more than a second on CDO, ship
`Get` and SetCurrentKey only and mark the rest [engine].

**Field mode** (F60, F74, astra; V2 — it qualifies the c22 answer F17's wording relies on):
confirmed named references (`read`, `write`, `validate`, `filter`, …) and numeric candidates
(`RecordRef.Field(8)` on an unresolved table) print as two inventories, with the note that a numeric reference does not break on a rename (it may on a
renumbering). `filters:` always prints, with `none observed (searched: …; not searched: …)`
from the shared helper §4.2 names when empty (Fable vii). The verdict says `not a complete rename inventory: page and report columns, CalcFormula
and TableRelation are not searched (the compiler finds them); keys holding the field are listed
above`. Revision 1 wrongly said keys are not searched: the table's declared keys are read.
Searching the other declaration-level consumers is [engine] (§10).

### 9.4 impact (F62)

`object headers: 25 files, line 1 only — a byte-order mark removed (no declaration changed)` when
the old and new lines differ only by a leading BOM, or by whitespace outside quoted names and
values (astra: whitespace inside a quoted identifier or literal is a real change); otherwise the
changed property names. The accounting has both line texts.

### 9.5 tests (F63, F64, F82) — F63 lands in V2

The answer **starts** with what to run (astra), in the test app's terms:
`run first (app "Continia Document Output 365 - Test"): codeunits 68960|68934|… — a superset:
whole codeunits, N tests, for the 369 selected procedures; then the whole suite (selection
incomplete: <why>)`. When the selection is exact, the procedures are listed instead and the line
says `exact`. The same run line, scope and fallback appear in impact's tests line and digest's
test status (§5.2). `test scope:` prints one clause (`1 test app, 1,844 tests; 4 apps skipped:
alsem_status lists why`); the symbol-download paragraph prints once as `missing test-library
symbols: 7 apps (Library Assert, …) — add them to Test/.alpackages` (agent-tasks §13.3 Q3's
specific next action, shortened). `production bytes: the same in the production and test
universes` (c07, c28) becomes `test universe: the same production source as the main engine`
or is dropped when nothing differs (F82).

### 9.6 status (F65)

`inputs` lists every category the check hashed or compared, from the categories themselves
(astra: not a fixed `appJson: 1`): on c09 `alFiles 582, packages 11, appJson 1`, which adds up to
the 594 hashed.

### 9.7 prove JSON (F66)

The prove description and schema note say `deterministic` concerns `generatedAt` only, never the
answer.

**Goldens:** `cli-explain-goldens`, `cli-query-goldens`, impact and tests goldens,
`cli-output-goldens/cdo/`. **Verify:** c05 names the callee, the route to its UI effect and the
17 event names; c20 names CDO Setup and the assigned fields; c11 prints `locks: none observed`
with its scope; c30 and c18 show ops by kind and direct sites; c22 separates the numeric
candidate and prints `filters:`; c13 says "byte-order mark"; c07 and c28 start with what to run;
c09's inputs add up.

### 9.8 Other per-answer items (F73 in V6, F78 in V4)

- **Guarantees and refutation** (F73, c15, astra): a digest guarantee whose evidence is
  `validForRefutation: false` prints as a description (`ordering: commit after insert,
  descriptive only`), never beside refutable facts without that label; the JSON keeps the flag
  next to each guarantee and the summary counts the two apart.
- **Location roles** (F78, astra): every printed location says what it is — `defined at` (a
  routine's declaration), `called at` (a call site), `at` (an operation) — and JSON location
  objects carry `role: definition | callSite | operation`. c18 and c30 printed definition lines
  where operations were asked for; this makes the difference visible everywhere.

---

## 10. Engine notes

Kept apart, per the rubric. None blocks a milestone above.

- **Declaration-level field consumers** (F60, c22): page and report columns, CalcFormula,
  TableRelation are not indexed (declared keys are). A rename inventory needs them. Output-only
  stopgap: §9.3.
- **Key-relevant uses in table mode** (F59): possible from existing indexes; measure the join (§9.3).
- **Company context of a write** (F75, c20): checked for revision 3 — `ChangeCompany` appears in
  the engine only as a catalogued builtin (`resolve/global_builtins.rs`, `member_catalog.rs`); no
  fact records which company a record variable targets. So the output prints `target company:
  not established by this analysis` beside the `ChangeCompany` site (§9.1); naming the company is
  the engine capability, deferred.
- **Deep reads and locks in query effects** (F61, c08): deferred engine capability (plan §5 Q4
  keeps per-operation detail one dependency routine deep). The output part, saying so per
  operation family, is accepted (§5.2).
- **DB-event subscribers** (c01, c11): on `fix/db-event-subscribers`.
- **`declaredRunnable` and page navigation** (c12, c20): the first on `fix/reach-and-test-selection`;
  navigation stays by design and is labelled (§3.2).
---

## 11. Where the reviewers disagree

### 11.1 Round-1 reviews (of the outputs)

| Topic | Fable | astra | Recommendation |
|---|---|---|---|
| c17 caller list | dedupe by routine, ~40 lines | keep every caller, share the witness | **astra**: the task asked for every caller, and Fable's duplicates are distinct overloads and actions (952 rows = 952 callers, checked). Share the witness, group by object (§7.1). **Settled**: Fable conceded in the spec review |
| Boilerplate | one short `scope:` line or the envelope | scope and completeness beside the answer, standardized | **both, within wave-2 §3.7**: the three facts stay visible as one line; static prose goes to the description (§5.3) |
| c13 outcome | exit 0 | a distinct outcome code | **exit 0 plus `outcome`** (§1 F24, §6.3): agents treat non-zero as failure; the outcome tells cases apart |
| c19 closed world | name `closedWorld: true` | warn it can hide the problem | **both**: name it as a re-analysis under an assumption and say it changes the assumption, not what runs (§4.4; wording settled in §11.2 row 2) |
| c12 whole-program reach | omit, "not specific" | a separate optional section | **counts by tier, kind, context and relation, lists folded**; lists only with `detail` (§3.2). Revision 3 drops the words "not specific" (folding is presentation only) |
| c26 ambiguity | one card per overload when ≤3 | a copyable request for both | **astra**: a card the agent did not ask for may be the wrong one; the error stays exit 2 with a ready value (§2.4) |
| c19 wording | — | "does not establish safety" | revision 1 rejected it; **revision 2 accepts the substance** without the banned word: `not established: that it never commits` (§4.4, §11.2 row 3) |
| c02, c07 | "partly" (layout makes candidates look like evidence) | "yes" | no output decision depends on the verdict; §3.1 and §3.3 apply either way |
| c18 | "partly" | "no" (no operation is identified) | astra's reading: §9.3 makes the answer name operations. Fable agrees in the spec review |

### 11.2 Spec reviews (of revision 1): conflicts and decisions

| # | Topic | Fable | astra | Decision and reason |
|---|---|---|---|---|
| 1 | Display vs selector | make the display a selector (extend the grammar with member forms), or print the stable id | keep `display` and a guaranteed `selector` apart; round-trip tests; captions off the hops | **Both, combined** (§2.1): extend the grammar (Fable's preferred option) so human displays of the round-trip-tested forms are pasteable, caption or not (§2.1 lists them; other displays are not promised), and emit `selector` in JSON, the one guaranteed form, with a round-trip test (astra). The two are compatible; neither reviewer's reason argues against the other's mechanism |
| 2 | Naming an assumption input on `unknown` | `would narrow it:` (removes some blockers) and `would decide it:` (removes all) | no "would decide it": an assumption is not evidence; "conditional re-analysis" | **astra's label, Fable's content** (§4.4): `re-analysis under an assumption:` with what it removes and what stays, and `removes every blocker of this answer` when true. "Decide" is dropped because the re-run's answer is not computed here and could still be unknown or yes; Fable's need — tell narrowing from full removal — is met by the removes/stays clause |
| 3 | F21 ("does not establish …") | agrees with revision 1's rejection (the owner rule) | reverse it: "transaction preservation is not established" uses no banned word | **astra** (§4.4): the owner rule bans the words "safe"/"unsafe", not a statement of what an answer does not show, and Fable's agreement was with keeping the banned word out, which still holds. `not established:` lines in the question's terms. Owner may overrule if the rule was meant more widely; flagged in the report |
| 4 | Overload signatures | also on list rows, not only path hops | on every occurrence of an overloaded name | **astra's rule** (§2.1), which includes Fable's: every occurrence, short names when not overloaded |
| 5 | Unknown words | one pair `not resolved` / `run-time` on table, temp and condition | a RecordRef does not prove a run-time table; use `not resolved` with receiver and reason | **astra** (§8.1): `not resolved (<receiver>: <reason>)` on every axis; `run-time` is not used for table, temp-state and condition targets, because for those the engine infers it from the receiver kind and does not prove it. Revision 3 scopes this (Fable, r2 review): for call edges `HonestDynamic` is a proven run-time fact and `honest dynamic` keeps its word. Fable's goal (the same words on every axis, saying why) is kept through the reason |
| 6 | Folding a broad reach | fire on the total share or on one hub's fan-out | the share may fold, never label; split by relation regardless of size | **Both** (§3.2): Fable's triggers decide folding only; astra's rule that labels and the split never depend on size. Revision 3 gives each trigger its own stated population (reach share over entries; hub fan-out over graph nodes), after astra showed revision 2 compared two different measures |
| 7 | Digest summary rows | (accepted revision 1's folded "distinct routine" rows) | one row per stable id, never folded | **astra** (§5.2): the c15 task compares overloads, so folding loses the asked-for difference; sharing common facts keeps the size down. Fable did not argue for folding |
| 8 | Impact `detail` | (accepted ten items per list) | an evidence budget | **astra** (§7.6): a census with shorter lines is still a census |
| 9 | Moving work earlier | F42 `tableAnswers`, F57 into V2 | also F13, completeness subject, candidates, lock scope, exact selection, F55/F56/F48 | **both** (§0.1): V2 grows to everything that misleads or leaves a hazard without its evidence. V2 is now the largest milestone; it can split into V2a (wording, F10–F29) and V2b (evidence lines, F48/F55/F56/F57/F63) if the implementer wants two branches |

No conflict is left open. Row 3 depends on how the owner reads the agent-tasks §5 rule; the
decision follows its words.

---

## 12. How it is verified

### 12.1 Tests

- **Semantic gates** (astra), beyond goldens and byte counts, each with a discrimination proof:
  - every emitted `selector` resolves to its own stable id (§2.1);
  - shared evidence reconstructs each root's and each row's original meaning: a digest root's
    entry, a query row's operation location, a prove witness's hops through its segment (§7);
  - read, lock and validate totals include every temp class and unresolved-table operations (§1 F15);
  - overflow: on a fixture whose requested rows exceed 8 KiB, the summary stays ≤ 8 KiB with the
    headline, analysis line, fallback, accounting, `index` and `summaryTruncated`, and the result
    index holds every requested identity, verdict and selected test (§5.2; break: write the index
    without the rows the summary dropped);
  - `unknown` stays `unknown` unless new evidence or an explicitly changed assumption is given
    (no renderer turns it into a softer "no");
  - zero resolved roots from input that did not map is `input-not-mapped`, never
    `no-routine-changed` (§1 F24).
- **Per change:** the test and discrimination proof named in its section, run with the narrowest
  command (`cargo test --test cli <member>::`). Correctness guards (§1, the tier labels of §3, the
  outcome of §6.3) each get one proof; wording, display and size do not (owner decision
  2026-11-04).
- **Per milestone:** the task gate; `scripts/check-goldens --regen` with every moved family read
  and named in the commit; `check-goldens --verify-coverage`; `scripts/cdo-gate` at the branch
  gate (the CDO rendering goldens in `cli-output-goldens/cdo/` move in every milestone); a
  CHANGELOG entry.
- **Round 2:** rebuild and copy `~/.al-sem/bin/alsem` (it does not update itself), re-run the
  same 30 calls with `target/tmp/output-validation/round1/record.py` and `plan.json`, and check
  each call against the "Verify" lines above. The "Verify" lines state shapes, not numbers:
  round-2 numeric expectations (counts, sizes past the ratchets) are derived from a run after
  the four fix branches of §12.2 have merged, because those branches move c02, c06, c12, c20,
  c21 and c25's numbers. The same two reviewers read round 2 against the same rubric; a finding
  that comes back is reopened in the register.

### 12.2 Fix-branch gates (check and date before the dependent commit)

| Branch | Findings it handles (handled elsewhere) | Spec work that waits for it | Merged (date, commit) |
|---|---|---|---|
| `fix/field-writers-implicit` | c10, c01 (F71) | §4.2 F17 | 2026-11-06, `57ad5025` (via `integrate/outval-r1-fixes`) |
| `fix/explain-callers-counts` | c06 ×2, c02 ×2 (F68, F69) | §5.1 F29 counts, §8.1 install/upgrade entries | 2026-11-06, `41184f73` |
| `fix/db-event-subscribers` | c11 (F58) | the F58 stopgap limit line is not needed: this branch landed before V2 | 2026-11-06, `57ad5025` (via `integrate/outval-r1-fixes`) |
| `fix/reach-and-test-selection` | c12 ×3, c20, c21, c25 (F11, F26, F70) | §3.2 F12 (the share is measured after it), §3.3 F13, §5.2 digest test status | 2026-11-06, `57ad5025` (via `integrate/outval-r1-fixes`) |

All four gates are closed. The V1 bugs (`fix/output-v1`) landed in the same integration merge.

Each handled-elsewhere finding is verified in round 2 on the call that raised it, like every
other finding.

---

## Appendix A. Findings register (round 1)

Columns: class = correctness bug / design change / handled elsewhere (another branch fixes it;
an integration and round-2 obligation, §12.2); priority: high = can mislead an agent, medium =
missing or inconsistent, low = noise or size. "Raised by": fable, astra or both (in revision 2,
"spec review" marks an item a reviewer added when reviewing revision 1).

Revision 2 changes: F20 and F63 raised to medium, F22 and F50 to high (astra: each can mislead a
decision); F21 reversed to a design change; F61 split (its output part is a design change, its
engine part deferred, §10); the seven "already fixed" rows moved from "rejected" to "handled
elsewhere"; F73–F79 (astra's missing items) and F80–F85 (Fable's six low items) added. No
finding is rejected any more.

### A.1 Counts

| Class | High | Medium | Low | Total |
|---|---|---|---|---|
| correctness bug | 2 | 3 | 2 | 7 |
| design change | 12 | 35 | 24 | 71 |
| handled elsewhere | — | — | — | 7 |
| **all** | | | | **85** |

### A.2 Correctness bugs

| id | Summary | Calls | Raised by | Priority |
|---|---|---|---|---|
| F5 | prove callers' context human text prints `dep:<guid>:`; `callers_document` skips `readable_locations` | c02 | both | medium |
| F9 | tests paths print two overloads alike; agent-tasks §12.1 requires handles | c28 | fable | low |
| F15 | query effects counts a `Get` under "caller-dependent writes" (`bucket()`, `effect_query_cli.rs:2991`) | c11 | both | high |
| F16 | parameter 3 (1-based) and `parameter-dependent(2)` (0-based) in one answer | c11 | both | medium |
| F24 | impact exits 2 on a complete "0 changed routines" answer (`impact/render.rs:715`) | c13 | both | high |
| F25 | impact without a base says "signature changed" of a routine the diff adds (`impact/jobs.rs:268-274`) | c20 | both | medium |
| F39 | touches `callers` described as "without witnesses"; each row has one (`tools.rs:485`) | c17 | fable | low |

### A.3 Design changes

| id | Summary | Calls | Raised by | Priority | Spec |
|---|---|---|---|---|---|
| F1 | Overloads by bare name in summaries, JSON display, caller lists, paths | c15, c17, c21, c25, c28 | both | high | §2.1 |
| F2 | Table triggers without the field | c01, c23 | both | medium | §2.1 |
| F3 | Page action triggers: three forms, no action name | c05, c06, c12, c17 | both | medium | §2.1 |
| F4 | Dependency routines by number, lower-cased member | c02, c12, c20 | fable | medium | §2.2 |
| F6 | prove does not echo the resolved table (a misspelling already errors) | c16 | both | low | §2.3 |
| F7 | Not-found selector: no near matches or selector forms | c03 | both | medium | §2.4 |
| F8 | Ambiguous selector: one long line; no ready value for both | c26 | both | low | §2.4 |
| F10 | prove prints hub-mediated witnesses as evidence; tier at line end only | c02 | both | high | §3.1 |
| F12 | A reach about the size of the program listed with no tier, kind split or hub, twice | c12, c20 | both | high | §3.2 |
| F13 | Tests: no tier per group; nearest reads as focused; no code-vs-navigation relation in the rank (tests already sort by tier and hops, `select.rs:457`) | c07, c21, c28 | both | medium | §3.3 |
| F14 | Conditional write led by "no observed write that may persist" | c01, c23 | both | high | §4.1 |
| F17 | Field writers: bare `0 writer(s)` with no search statement | c10 | both | high | §4.2 |
| F18 | Filtered negative hides the other reachable sites | c27 | both | high | §4.3 |
| F19 | unknown does not name the deciding input or the plain reading | c02, c19, c29 | both | medium | §4.4 |
| F20 | Interception answer may read as a rollback guarantee | c29 | astra | medium | §4.4 |
| F21 | An unknown does not say which conclusion it does not establish ("transaction preservation is not established"), without the banned word (reversed in revision 2) | c19 | astra | medium | §4.4 |
| F22 | Status headline same for hashed and size/time checks | c09, c24 | astra | high | §4.5 |
| F23 | Touches table mode: no "transitively"; routine headers shown as sites | c18, c30 | both | high | §4.6 |
| F27 | Change modes do not say "current source, no before/after" | c01, c20 | astra | medium | §4.7 |
| F28 | Answer buried (c17 line 957; status; tests selection) | c17, c21, c24 | both | medium | §5.1 |
| F29 | explain callers' context: COMMIT hazard last, no witness | c06 | both | high | §5.1 |
| F30 | No shared result header; no summary for inline explain/prove/impact/tests | all | both | medium | §5.2 |
| F31 | Four boilerplate texts per card; empty sections | c01, c04, c05, c11, c22, c23 | both | medium | §5.3 |
| F32 | Dependencies line in three forms | c01, c04, c17, c18, c22, c30 | fable | low | §5.3 |
| F33 | Status text duplicates the envelope; operator data by default | c09, c14, c24 | both | low | §5.4 |
| F34 | CLI flags and non-MCP commands in MCP text | c04, c11, c13, c17, c19, c28 | both | medium | §6.1 |
| F35 | Static `next:` lines | c01, c05, c06 | fable | medium | §6.2 |
| F36 | Exit-code meaning not in the envelope | c03, c13, c26 | fable | low | §6.3 |
| F37 | No-engine status does not name `singleApp: true` | c14 | both | low | §6.1 |
| F38 | Touches callers: 320 KB of repeated witnesses (reviewers disagree, §11) | c17 | both | medium | §7.1 |
| F40 | query effects JSON 1 MB: routine identities per row, constant `via` | c08 | both | medium | §7.2 |
| F41 | Digest repeats overlapping cones per root | c15, c25 | both | medium | §7.3 |
| F42 | Digest summary: no `tableAnswers`; 9 of 20 routines with unexplained `countsOnly` | c15, c25 | both | high | §5.2, §7.3 |
| F43 | query effects summary: no writes by table | c08 | fable | medium | §5.2 |
| F44 | Tests JSON: `hidden` reason ×47; null fields; selection not in summary | c21 | both | medium | §7.4 |
| F45 | prove paths: shared tails repeated; blocker counts; `(at X) at X` | c02 | both | medium | §7.5 |
| F46 | impact detail: 3,000-character lines; whole-change census repeated | c12, c20 | both | medium | §7.6 |
| F47 | explain caller-rule line 1,900 characters | c06 | both | low | §7.7 |
| F49 | "unknown" with five meanings | c05, c08, c11, c18 | both | medium | §8.1 |
| F50 | "complete"/"coverage"/"reached" without a subject | c09, c21, c25, c28 | astra | high | §8.1 |
| F51 | "entry point" with two meanings | c05, c06, c11, c12 | astra | medium | §8.1 |
| F52 | Tier and entry-kind words spelled many ways | c02, c06, c07, c12, c20, c29 | fable | medium | §8.1 |
| F53 | Count units not stated or reconciled | c05, c08, c15, c17, c22, c29 | both | medium | §8.2 |
| F54 | Touches tags without a legend | c22 | both | low | §8.2 |
| F55 | No direct callees in explain and impact | c05, c12 | both | medium | §9.1 |
| F56 | Tables (and fields) not named when few | c20 | both | medium | §9.1 |
| F57 | query effects: no `locks:`/`reads:` line when zero | c11, c23 | both | medium | §9.2 |
| F59 | Touches table mode: no ops, no direct sites, no key-relevant uses | c18, c30 | both | high | §9.3 |
| F60 | Rename: declaration-level consumers (columns, CalcFormula, TableRelation; keys are read) not searched; say so [engine] | c22 | both | low | §9.3, §10 |
| F61 | query effects: no machine-readable scope per operation family (reads, writes, locks; deep writes); the deep read/lock inventory itself is deferred [engine] | c08, c11 | astra | medium | §5.2, §10 |
| F62 | impact: header change kind not said (a BOM) | c13 | both | low | §9.4 |
| F63 | tests: no runnable target first; exact selection vs codeunit superset not said | c07, c28 | both | medium | §9.5 |
| F64 | tests: scope and symbol paragraphs repeat | c07, c28 | both | low | §9.5 |
| F65 | status inputs omit app.json (594 = 582 + 11 + 1) | c09 | astra | low | §9.6 |
| F66 | prove `deterministic: false` unexplained | c16 | astra | low | §9.7 |
| F67 | "skipped by" reads as a guard bypass | c06 | astra | low | §9.1 |
| F48 | showSource excerpts show declarations, not the call | c04, c05 | both | low | §9.1 |
| F72 | Own events vs reached events not told apart | c05 | astra | low | §9.1 |
| F73 | Digest guarantees not told apart from evidence marked `validForRefutation: false` | c15 | astra (spec review) | medium | §9.8 |
| F74 | Touches field mode: confirmed references and numeric candidates mixed; no `filters:` result; rename sensitivity of a numeric reference not said | c22 | astra (spec review) | medium | §9.3 |
| F75 | explain/impact: the routine's own state changes (c06: two globals) and the company a write targets (c20) not stated; the company part needs an [engine] check | c06, c20 | astra (spec review) | low | §5.1, §10 |
| F76 | Standalone JSON documents lack scope, assumptions and completeness without the MCP envelope | c16 (all JSON) | astra (spec review) | medium | §5.2 |
| F77 | Digest summary rows lack per-routine test status | c25 | astra (spec review) | medium | §5.2 |
| F78 | Locations do not say their role (definition, call site, operation) | c18, c30 (all tools) | astra (spec review) | medium | §9.8 |
| F79 | c02: no editing implication ("do not assume a user is present"); c29: no representative uncaught route under `detail` | c02, c29 | astra (spec review) | low | §4.4 |
| F80 | The 17 raised events folded as `Codeunit CDO Events 17`, no names | c05 | fable (spec review) | low | §9.1 |
| F81 | `telemetry at 1 place … Base64 Convert` is noise for an export action | c05 | fable (spec review) | low | §5.3 |
| F82 | `production bytes: the same in the production and test universes` unexplained | c07, c28 | fable (spec review) | low | §9.5 |
| F83 | `coverage: the reverse cone, 42651 states over 10091 nodes` is a statistic in human text | c02, c29 | fable (spec review) | low | §7.5 |
| F84 | `in …::SetMergePDF, through …::SetMergePDF` names one routine twice | c23 | fable (spec review) | low | §7.5 |
| F85 | Touches field `note:` repeats the `searched:` clause (also in F31's text) | c22 | fable (spec review) | low | §5.3 |

### A.4 Handled elsewhere

Not design work here; each is an integration obligation (the branch merges) and a round-2 check
on the call that raised it (§12.2). Where a reviewer's presentation point goes beyond the fix,
it is kept as its own design row (named in the last column).

| id | Summary | Calls | Raised by | Branch / note |
|---|---|---|---|---|
| F11 | impact `first:` items without tier | c12, c20 | both | `fix/reach-and-test-selection` (`node_short`, `reached_clause`); the size-independent split is F12 |
| F26 | digest `testCoverage` "complete" with 0 tests | c25 | both | `fix/reach-and-test-selection` (`NoTestCode`, `test-apps-not-loaded`); astra's presentation points (test scope and per-routine status in the summary) are F50 and F77 |
| F58 | limit line "DB-event subscribers not followed" | c01, c11 | fable | `fix/db-event-subscribers` (the cone follows them); a dated stopgap in §12.2 if that branch lands after V2 |
| F68 | no bucket for page triggers among callers | c06 | fable | `fix/explain-callers-counts` |
| F69 | upgrade helpers counted as entries; guard line | c02 | fable | `fix/explain-callers-counts` |
| F70 | runFirst prefers add-in-event paths | c21 | both | `fix/reach-and-test-selection`; astra's presentation point (relation in the rank) is F13 |
| F71 | "UI and errors: none" for an added `Modify()` | c01 | fable | `fix/field-writers-implicit` |

Ids are not in theme order: they were numbered as findings were merged.
