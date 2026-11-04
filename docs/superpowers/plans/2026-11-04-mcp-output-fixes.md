# Fixing what the MCP tools return (plan, revised)

**Status:** agreed plan, 2026-11-04. Drafted by Claude, reviewed and revised by Fable against
master `8daaf140`; owner decisions recorded below.

**Owner decisions (2026-11-04):**
1. **JSON line base: 1-based everywhere** on the engine-query surface (explain, prove,
   digest, query, fingerprint, snapshots), with `lineBase: 1` stated in every document and
   in the MCP envelope. `alsem graph` and `alsem analyze` are 1-based already.
2. **The ConTinAI citation consumer** (it resolves `dep:<guid>:<path>` anchors through
   `aldump --dump-dependency-source`): its assumed line base is not recorded. The owner
   checks it and tells its maintainers before wave 1's line-base change is merged.
3. **One copy of the answer:** the MCP default format for `prove` becomes `human` (one
   compact copy); JSON stays available on request.
4. **CDO routines for item T:** `IssueReminder` (the reviewed one), `RunReportID` (a hub,
   already used by the stage-5 and W measurements), and one small plain routine chosen in
   wave 0.
5. **The major digest schema change in wave 2 is accepted**, with `--detail` keeping the
   full form for one release.
**Input:** a hands-on run of every `alsem_*` MCP tool on the CDO baseline (routine
`IssueReminder`, Codeunit "CDO Issue Document") and the review of those outputs at
`target/tmp/mcp-output-review-fable.md` (local, not committed). The review's finding
numbers (B1–B6, P1–P6, D1–D7, Q1–Q6, T1–T3, S1–S2, E1–E7, X1–X10) are used below so each
one can be traced to an item; the first draft's own B1–B10/M1–M3 numbering is kept in
brackets where it differs.
**Contract kept:** the MCP answer stays the CLI's output byte for byte. Every change below
is a change to the CLI output itself (or to the MCP envelope around it), made on purpose,
with the goldens regenerated and reviewed. Engine facts do not change unless an item says
so — and three items do (3a, 2d, Q4); each says what it adds and runs the detector
goldens to show nothing else moved.

## 0. The problem in one paragraph

The engine's facts are mostly right and sometimes deep, but the outputs fail an agent. For
one routine: `explain` is 80 KB / 558 lines (162 writes, 77 of them with a blank table
name, 57 with no location at all, 18 exact duplicates; 288 gaps, 272 of them inside
dependency code); `digest` is 5.6 MB and its "short" MCP summary is 270,781 characters,
which Claude Code moves out of the conversation into a file; `query effects` lists 652
lines of which 294 are temporary-record operations and only 27 are physical writes; `query
touches` reports 204 witnesses for "CDO File" of which 199 are temporary, then repeats all
204 under `up`. Every human output prints 0-based line numbers as `file:line`, so an
agent's `Read` lands one statement early. The three tools disagree on whether
`IssueReminder` writes "Issued Reminder Header" (`prove` yes with a path, `explain` hides
it in a blank-named group, `query effects` omits it) — and that disagreement has a
structural cause (§2, M3), not a rendering one.

## 1. Why the tests did not catch it

- **Goldens pin what the engine prints, not whether it is good.** A golden is minted from
  the engine's own output and byte-compared. A blank table name or an off-by-one line is
  locked in as "correct" unless someone reads the golden and objects.
- **The fixtures are too small to show the shapes.** The 34 `tests/cli-explain-goldens/`
  files contain zero blank-name lines because blank names come from *dependency boundary
  groups*, which exist only in a cross-app model whose dependency ships bodies. Every
  explain/query golden fixture is single-app; the `.alpackages` fixtures under
  `tests/r0-corpus/` are symbol-only. Size problems (a 5.6 MB digest) cannot appear on a
  20-routine fixture at all.
- **The MCP tests check faithfulness, not quality.** `tests/cli/mcp_server.rs` asserts the
  MCP answer is byte-identical to the CLI's, which it is. The one summary assertion checks
  `rootsResolved == 1`, never the summary's size.
- **The CDO gate checks numbers, not rendering.** It pins resolution counts, hashes and
  memory; no CDO test renders a card, a query or a prove and looks at it. There is no
  CDO-gated member in the `cli` umbrella at all.
- **Two line conventions live in one product and nobody wrote the rule down.** The
  analyzer surface is 1-based already (`alsem analyze` JSON `projection.rs:87`, SARIF
  `format_sarif.rs:206`, HTML, PR summary, d35's message) and so is `alsem graph`
  (`meta.lineBase: 1`). The engine-query surface (explain, prove, digest, query,
  fingerprint, snapshot) is 0-based, and only prove and fingerprint say so
  (`payload.lineBase: 0`, pinned by `tests/cli/cli_ids_and_line_bases.rs`). The human
  renderers inherited the 0-based number without the declaration.
- **Nobody had looked at the outputs as an agent.** The context-server work optimised
  speed and memory under a "byte-identical to the CLI" contract; that contract made the
  output's quality somebody else's problem.

So the fix is two-fold: the bugs below, **and** a new layer of output tests (§3, item T)
that checks properties of the output — invariants, sizes, agreement between tools — on a
cross-app fixture with dependency bodies and on CDO, so these classes of defect fail a
gate next time.

## 2. Findings this plan fixes (code locations at `8daaf140`)

Rendering defects (each is a renderer reading the wrong field or not reading one):

- **B1 blank names** [B1]. `explain` prints dependency boundary groups as `writes  (DB_DELETE, …)`
  and `raises  (…)`: the group's tables live in `detail.boundary/resources`
  (`digest.rs:4274-4300`), but `explain.rs:1302-1303, 1380-1388` read only
  `resourceId/resourceDisplay`. Same cause: `extension point : 0 subscriber(s)`
  (`explain.rs:1398-1460`). 108 of the routine's 275 effects are groups.
- **B2 boundary name without its object** [B2]. `boundary_terminal` (`digest.rs:1598-1601`)
  formats `{app}.{routine}` with the bare routine name: "Base Application.OnRun" names both
  Reminder-Issue.OnRun and a `Codeunit.Run` of "Sales-Calc. Discount".
- **B3 temporary writes shown as writes** [B3]. `explain` never reads `tempState` (60 of
  275 effects are `known(true)`); `prove` already excludes them (`prove.rs:550
  writes_only_temporary`), so two tools disagree on what a write is.
- **B4 field lists per routine, not per operation** [B4] (`explain.rs:1306-1311`,
  `ix.row_fields`): a `DeleteAll` is annotated with fields. Note the data: a record
  operation carries `fieldArguments` only for operations that name fields (Validate,
  SetRange…); the fields a `Modify` writes are the routine's assignments, which IS a
  routine-level fact. So the honest fix is "fields only on INSERT/MODIFY/VALIDATE, labelled
  as the routine's assignments", not a per-op recomputation.
- **B5 exact duplicate lines** [M1]: a temporary and a non-temporary group (`|kt` keys,
  `digest.rs:4303-4309`) render to the same text.
- **B6 `(no anchor)` rows** (57 of them; **missing from the first draft**). Dependency-
  internal writes whose evidence is `sourceKind: unavailable`. The digest knows the owning
  routine and the via path; the card prints nothing an agent can act on.
- **P1 may-commit evidence proves nothing** [B6 second half]: `groupPath` is produced only
  when the question names a resource; a COMMIT has none, so the evidence is
  `excerpt: "enters Base Application.OnRun", viaPaths: [[]]`.
- **P2 `unresolvedResources: "1"` on a COMMIT** [B6 first half]: `digest.rs:4151` counts any
  fact without a resource id, so every COMMIT/UI/ERROR/HTTP/FILE group claims an
  "unresolved resource". The verdict is unaffected (`prove.rs:597` looks at DB types only).
- **P4 obligations are counts without pointers** (**missing from the first draft**):
  `unresolvedCallsites: 21, openWorldRows: 267` with no "which" and no "what would close
  them".
- **D1 digest summary embeds every unresolved call site** [B5] (`src/mcp/pool.rs:746`
  `"unresolved": e["unresolved"]`) and the summary is inserted after `bound_frame` ran
  (`pool.rs:956-957`), so it is outside the frame bound: 270,781 characters.
- **D2 the summary decides nothing** (**only half in the first draft**): it needs per-root
  counts by effect type, physical tables written (count, names, workspace vs dependency),
  commits, boundaries entered, coverage reasons — about 1 KB.
- **D3 5.6 MB, 59% of it `viaPaths`** [M2]: 553 paths × 8.8 hops × ~700 bytes (two 90-char
  ids, two displays, `edgeKind` and `kind` both, receiver type). `--max-paths` is parsed
  (`src/bin/alsem.rs:930`) and never read (`digest_cli.rs:2484, 2506`). `entryDiagnostics`
  (32 KB) and `scopedGuarantees` (17 KB, one object per effect saying the same thing) are
  not in the first draft.
- **D4 group resources cut at 10** [B7] (`digest.rs:4283 .take(10)`), the rest available
  nowhere. "Issued Reminder Header" happened to be tenth.
- **D5 unverifiable deletes beyond a boundary** (`Sales Header` through Reminder-Issue):
  same fix as P1, a path per group resource on demand.
- **D7 the "effects of a change" naming** (**missing from the first draft**): `routines`
  mode is explain-minus-rules-plus-paths; the change modes are `diff`/`files`, and the
  description should say so.
- **Q3 `via inherited` on every line** [M1] of a single-routine query carries no information.
- **Q4 `RecordRef.Next()` printed as `on unknown (target table unresolved — not a table)`**
  [M1]: 69 lines whose honest reading is "RecordRef: table decided at run time".
- **Q5 `dep:<36-char guid>:…` anchors** [2g]; **Q6** the 400-char `dependencies:` header on
  every answer, the `LockTable` line without a line number, no line-base statement
  (**header and LockTable missing from the first draft**).
- **T1 touches headline counts temp witnesses as touches**; **T2 the `up` section repeats
  all 204 down witnesses** in the "trivially" case (`effect_query_cli.rs:1183-1225`) [M1].
- **B8 0-based lines printed as `file:line`** [B8] in every human output (`explain.rs:440,
  1102`; `effect_query_cli.rs:725, 1078`; `prove.rs:1262ff`; `digest_cli.rs` human);
  `lineBase` only in prove/fingerprint JSON.
- **E2 a spilled file gets no summary except for digest** (**missing from the first
  draft**): the 80 KB explain card went to a file and the envelope said only `{path,
  bytes, sha256}`. **E3** the summary is not frame-bounded (with D1). **E6** the placeholder
  string `stdoutText: "<the next content block>"` (`server.rs:405`).
- **P3/E1 the answer delivered up to three times** [B10] (`server.rs:398-420`: content
  block + `structuredContent.stdoutText` + `structuredContent.output`).
- **S1 stale `alsem_refresh` description** [B9] (`tools.rs:344`, "there is no built
  revision to update" — there is). **S2 `alsem_status` lacks the freshness facts** an agent
  asks for (revision generation, inputs stamp, last build time/duration, RSS) (**missing
  from the first draft**). **E7/P6** descriptions describe the CLI, not the agent's decision.
- **M2 too little** [M2]: no summary line anywhere; explain's human card has no commit
  site (the JSON has `commitSites`) and no path to any effect; HTTP, FILE,
  BACKGROUND_TASK, ISOLATED_STORAGE, TELEMETRY and ERROR_* effects are not in the card at
  all (`Card::build` matches `DB_*`, `COMMIT`, `EVENT_PUBLISH`, `UI_*`); "caller rules:
  none (1 known caller(s))" and the later `reached from …` line should be one line.

Structural cause (not a rendering bug):

- **M3 the tools disagree on dependency depth** [M3], and the code says why. The L4
  db-effect store treats every dependency routine as a *fixed leaf*: its summary is its own
  direct operations only (`summary_runner.rs:1520-1560`, `leaf_summaries`), and the
  combined graph drops dependency-internal call sites as opaque
  (`combined_graph.rs:724`). So `query effects` and `query touches` (both read that store
  through `DbEffectQuery`) see exactly one routine into a dependency. The capability cone
  (`capability_cone::build_cross_app_base`, read by digest, explain and prove) follows
  every resolved call that dependency code makes. Two substrates, two depths, and the
  `effect_query.rs` module doc already says which one answers "does this write a physical
  table" better (`cone_derived`'s `writes_physical_tables_of`).

## 3. The work, in waves

### Wave 0 — decisions and the fixture (before any renderer moves)

- **0a. Decide the line base for JSON** (open question 1, answered in §5). The decision
  changes the scope of 1a and which goldens move; take it first.
- **0b. The cross-app fixture with dependency bodies** (item T's fixture, built with
  `tests/common/symbol_app.rs::write_source_app`, which several `cli` members already
  use). It must produce every shape the review saw: a workspace routine that runs a
  dependency codeunit whose callee (not the entry routine) writes two tables and commits
  (a boundary group with a commit and resources beyond the first hop), a dependency
  publisher raised beyond the boundary (an EVENT_PUBLISH group), a temporary-record
  `DeleteAll` and a physical `Insert` on the same table (B3/B5/T1), a `RecordRef.Next()`
  loop (Q4), a dynamic `Codeunit.Run(<variable>)` and an interface call (gaps and P4), an
  HTTP call and a background task (M2's missing kinds), a page action caller, and one
  `[Test]` procedure that reaches one routine. Check it in under `tests/fixtures/` and
  mint **default and detailed** goldens for explain, prove (may-commit, writes-table for
  each table), digest (routines mode, human and JSON), query effects and query touches
  from it. From here on every renderer change shows its effect on this fixture's goldens.
  New golden families: remember `check-goldens` is the source of truth for the family
  table in CLAUDE.md — add the new files to the family that owns the command (`--test cli`)
  and confirm `check-goldens --verify-coverage` sees them.

### Wave 1 — outright bugs, small diffs, each its own commit

- **1a. 1-based lines** (B8, X1). Human: every `file:line` prints `start_line + 1`, the
  same rule `format_pr_summary.rs:150` and `format_html.rs:195` already use. JSON: per the
  0a decision (§5 recommends 1-based across the whole engine-query surface, with
  `lineBase: 1` declared in every document). The MCP envelope gains `lineBase`. Land this
  **alone, before the renderer rewrite**: its golden diff must be "every `:N` became
  `:N+1` and nothing else", which a reviewer (or a 10-line script in the commit message)
  can verify; mixed into wave 2's diff it is unreviewable. Yes, wave 2 regenerates the
  same goldens again — regen is cheap (`check-goldens --regen`), review is not, so two
  clean diffs beat one muddled one. `tests/cli/cli_ids_and_line_bases.rs` moves with it.
- **1b. digest summary fixed and bounded** (D1, D2, E3). `unresolved` becomes
  `{total, polymorphic, dynamicTarget, inWorkspace, topCallees[3]}`; per root the D2
  counts; the summary is produced **before** `bound_frame` and counted in it, with a hard
  cap of its own (8 KB; past it the summary degrades to counts only, never to a spill).
  The summary becomes a renderer responsibility: `DigestRunResult` returns it, the server
  stops parsing stdout to build it (this is what makes E2 possible below).
- **1c. one copy of the answer** (P3/E1, E6). `structuredContent` carries the envelope
  **without** `stdoutText` and with `output` only when the format is JSON; the content
  blocks stay (the MCP spec asks for a text block beside `structuredContent`, and the byte
  contract reads it). Drop the placeholder key. Make `human` the MCP default for `prove`
  (its human form is six lines; add `groupPath` to it in 2f). Measure, do not assume,
  whether `structuredContent` counts toward Claude Code's result limit (the docs do not
  say): one call through Claude Code, look at what it saved.
- **1d. qualified boundary names** (B2: `Codeunit Reminder-Issue::OnRun`, the same
  `qualified()` the query renderer uses), **no `unresolvedResources` for resource-less
  types** (P2: count only `DB_*` facts; omit `resourceCount/resources` on COMMIT, UI,
  ERROR, HTTP, FILE, BACKGROUND groups), **all resources in JSON** (D4: a `resources[]`
  array, `resourceCount == resources.len()` always; the comma string stays one more minor
  version for readers, then goes). Digest schema: additive fields are a minor bump; the
  `resources` string's meaning does not change.
- **1e. summaries on every spill** (E2). `ExplainRunResult`, `QueryRunResult` and prove's
  result each return a `summary` value (explain: the header line of each card; query
  effects: counts by op and temp state; touches: the `down:`/`up:` headlines; prove: the
  answer line and obligations). The envelope carries it whenever the stdout went to a
  file, under the same 8 KB cap as 1b.
- **1f. descriptions, first pass** (S1, E5): `alsem_refresh` rewritten; every description
  states the line base, the output's typical size and when to pass `format: json`, and
  (until 3a lands) the tool's dependency depth in one sentence. The full rewrite is 3b.

### Wave 2 — rendering that serves an agent (the redesign)

2a–2c reshape the same renderers and land together; 2d–2h after, each its own commit.

- **2a. Boundary groups rendered properly** (B1, B6, X3). In `Card::build`, branch on
  `detail("boundary")`: `writes 10 table(s) in dependency code beyond Codeunit
  Reminder-Issue::OnRun: Cust. Ledger Entry, … (DB_DELETE)`; JSON gets `boundary`,
  `resourceCount`, `resources[]`; groups are skipped when collecting extension points.
  Rows with `evidence.sourceKind: unavailable` print `in dependency code:
  <Object::Routine> (no source position kept)` and fold into a per-table count — never a
  bare `(no anchor)`.
- **2b. Physical vs temporary separated in every tool** (B3, T1, Q2, X4), the way `prove`
  does it: explain's writes split into physical writes and a `temp writes: N` section;
  query effects and touches headline physical first (`down: yes — 1 physical touch (read,
  CalcFields in CDO File::CreateZip); 199 temporary-record touches; 4 unknown`); temp and
  read rows behind `--all` (query) / `--detail` (explain). **JSON keeps every effect**
  with its `tempState` — this wave changes no fact and no JSON row (see §5 Q3).
- **2c. Summary first, then groups, then detail** (M2, X5, 1.2, D6, T2, Q3, Q6). One
  header line per card/answer: physical writes by table (count, workspace vs dependency),
  commits with the first site and its boundary, events, UI, HTTP/file/background, callers
  (named — merging the "caller rules: none (1 known caller)" and "reached from" lines),
  tests, gaps split workspace/dependency. Physical tables grouped by table with operation
  counts, workspace tables first; dependency gaps collapsed to a count plus the top
  callees, workspace gaps verbatim; duplicate lines merged; `via inherited` dropped in
  single-routine mode (print the first hop instead); the `up` section in the trivial case
  is one line; the `dependencies:` header moves into the envelope's `universe` block and a
  one-line `dependencies: N apps (--detail lists them)`; the `LockTable` row gets its line.
  Target for `IssueReminder`: explain human under 150 lines; query effects human under
  100 lines by default; digest JSON under 200 KB by default. The digest human output gets
  the same grouping (D6) and diagnostics as counts by reason, op ids behind `--detail`.
- **2d. Field lists** (B4): attached only to INSERT/MODIFY/VALIDATE effects, labelled
  `fields this routine assigns`; nothing on DELETE/DELETEALL. (If per-operation fields
  are wanted later, that is a substrate addition under the `declared-needs` rules, not
  this item.)
- **2e. Missing effect kinds in the card** (M2, X8): HTTP, FILE, BACKGROUND_TASK,
  ISOLATED_STORAGE, TELEMETRY, ERROR_THROW/ERROR_IMPLICIT, each as a counted line with its
  first site; the commit site in the human card (from `commitSites`).
- **2f. Paths** (P1, D3, D5, X7). `groupPath` for every group evidence in prove, COMMIT
  included, ending at the routine that holds the matching fact (`group_ops` has it,
  `digest.rs:4160`); prove human prints it. Explain human and digest human print one path
  per physical write and per commit, displays only. Digest JSON: `--max-paths` honoured
  (default 1; the MCP tool passes 1 unless asked), hops as displays with ids in a side
  table, the duplicated `kind` dropped, `entryDiagnostics` and `scopedGuarantees` folded
  to counts unless `--detail`. This is a digest schema change that removes fields from
  the default document: a major bump (the schema contract at `digest_cli.rs:2262ff` says a
  minor bump removes nothing), with the previous shape available behind `--detail` for one
  release.
- **2g. Readable dependency locations** (Q5): the human form prints
  `dep:Base Application:src/Sales/Reminder/ReminderIssue.Codeunit.al:182`; the JSON keeps
  the guid, because `aldump --dump-dependency-source` writes dependency AL to
  `<out>/<app guid>/<virtual path>` precisely so a `dep:<guid>:<path>` citation resolves
  to a file, and an external pipeline (ConTinAI, per that test's doc) relies on it. No
  `%20` escapes in the human form. Add `--show-source` (an excerpt of the cited line ±2)
  to explain and query, reading the same text that dump command reads.
- **2h. Prove obligations with pointers** (P4): `unresolvedInWorkspace`, the top three
  unresolved callee displays, and the human hint `coverage incomplete: 21 dynamic call
  sites (16 in this workspace, see alsem_explain gaps); 267 interface calls counted
  open-world (closedWorld: true to assume loaded implementations)`.

### Wave 3 — consistency and guidance

- **3a. One dependency depth, labelled** (M3, Q1, X6). Do **not** make L4 solve the full
  cone: it would change every detector's input (every detector reads L4 db_effects), force
  a CDO re-triage, and cost cone-sized memory per routine. Instead `query effects` and
  `query touches` read `cone_derived`'s `writes_physical_tables_of` /
  `physical_table_reads_of` for everything beyond the first dependency routine and print
  those rows as `beyond <boundary>` (no per-op anchor, the boundary's path instead), keeping
  the per-operation rows at today's depth. Every output's header then states the same
  sentence: `dependency code: full cone for physical writes, grouped per entry point;
  per-operation detail one routine deep`. The query view plan must declare the cone store
  (the `declared-needs` skill); on the resident engine the cone store already exists after
  stage 5b, measure RSS before/after with `aldump --snapshot-memory`. Q4 lands here too:
  print `on <RecordRef> (table decided at run time)` as one counted line — check whether
  the record operation keeps "the receiver was a RecordRef"; if only `mayUseRecordRef`
  exists at routine level, add the per-operation bit (an engine fact addition, declared).
- **3b. Descriptions and server instructions rewritten around the agent's decisions**
  (E7, P6, D7, X10): which tool for which question, accepted selector forms (one grammar,
  both spellings accepted, stated once), line base, size expectations, what `partial`,
  `openWorld`, `unknown` and `coverage: incomplete` mean, that temp operations are listed
  (query, under `--all`) and not counted as writes anywhere, that `diff`/`files` are the
  change modes, and one suggested next step per tool.
- **3c. `alsem_status` for freshness** (S2): per engine `revision.generation`, the inputs
  stamp, last build start/duration, RSS, and whether a build is running.

### Item T — output tests that would have caught this

Built in wave 0/1, extended per wave. Two homes: a new `cli` umbrella member
(`tests/cli/output_invariants.rs`) that runs on the wave-0 fixture in every `cargo test`,
and a CDO-gated member (`tests/cli/output_cdo.rs`, gated by `crate::cdo::cdo_ws_or_enforce`
— the first CDO-gated member in the `cli` umbrella) that runs under `scripts/cdo-gate` on a
fixed routine set: `IssueReminder`, one hub (a routine with the most transitive callers,
chosen once and named in the test), and one routine with no dependency calls.

Invariants (each with the defect it would have caught and the break that proves it):

| Invariant | Catches | Discrimination break |
|---|---|---|
| No `writes`/`raises`/effect row with an empty name; every group row names its boundary with `::` | B1, B2 | restore the `resourceDisplay`-only read |
| No two identical lines in any human output | B5 | drop the `|kt` distinction in the text |
| `resourceCount == resources.len()` on every group; `unresolvedResources` only on `DB_*` groups; every group evidence in prove has a `groupPath` whose last hop holds the fact | D4, P2, P1 | restore `.take(10)`; count resource-less facts |
| **Anchor source-check**: every `file:line` in a human output (a sample of 50 on CDO) opens to a line whose text contains the printed operation verb or callee name, case-insensitive; `ws:` resolves against the workspace, `dep:` against `aldump --dump-dependency-source` output | B8 | print `start_line` again |
| Every JSON document and envelope states `lineBase`, and it equals the base the human form uses (spot-check one anchor) | E5 | remove the key |
| No `(no anchor)` text anywhere; a row without a position names its owning routine | B6 | restore `anchor_value_human`'s fallback |
| Temp split: for every table, `physical + temporary + unknown == total` in touches; explain's `writes` list has no `tempState: known(true)` row; prove `writes-table` and explain's physical list agree | B3, T1 | stop reading `tempState` |
| **Every effect kind digest reports for the routine appears in explain's card** (HTTP, FILE, …) | M2 | drop a match arm |
| **Cross-tool agreement**: for each table T in the union of tables any of the four tools names for routine R, `prove writes-table:T` is yes ⇔ T is in explain's physical writes (own or beyond-boundary) ⇔ T is in digest's effects (own or group resources) ⇔ (after 3a) T is in query effects' physical rows. On the fixture: every table; on CDO: at most 12 tables per routine (prove's cone walk is seconds each) | M3, D4, Q1 | restore `.take(10)`; skip groups in explain |
| **Size budgets**: for the fixture and the CDO routines, explain human ≤ 150 lines, query effects human ≤ 100 lines, digest JSON ≤ 200 KB, every `summary` ≤ 8 KB; the whole serialized `tools/call` result (content + `structuredContent`) ≤ 40,000 characters when the stdout went to a file. Recorded in the test as ratchets (a number may go down freely; up only by editing the test) | D1, D3, E3, E4 | re-embed `unresolved` |
| **MCP result against the client's real limit**: Claude Code saves a text result over 50,000 characters or 25,000 tokens (`MAX_MCP_OUTPUT_TOKENS`) to a file instead of showing it; the test asserts the inline-cap default keeps a routine like `IssueReminder` under both (50,000 chars is the binding one for the summary-only envelope; the 64 KiB inline cap is above it — lower the default inline cap to 48 KiB or make the test prove the envelope stays under) | E4, D1 | raise the cap |
| **Rendering goldens on CDO**: the explain human, query effects human, touches human, prove human (may-commit and one writes-table) and digest summary for the three CDO routines, byte-compared under `cdo-gate`, regenerated only on purpose with the diff in the commit | the whole class: the next blank name or duplicate shows up as a golden diff someone must read | any renderer change |
| **One copy**: the serialized `tools/call` result contains the stdout text exactly once | P3 | restore `stdoutText` in `structuredContent` |
| **Descriptions**: every tool description and `INSTRUCTIONS` contain "1-based" and a size sentence; `alsem_refresh`'s does not contain "no built revision" | S1, E7 | revert the text |

Flakiness: the engine is deterministic (`--deterministic` pins timestamps); the only
environment-dependent parts are the CDO paths, which the CDO gate already handles. The
anchor source-check reads files, so it skips a `dep:` anchor whose app is not in the dump
output (and counts the skips; more than 10% skipped is a failure). The size ratchets are
the one place a legitimate change can trip a test; that is the point, and the number is
edited in the same commit with the reason.

## 4. Order and gates

Wave 0 (decision 0a, fixture 0b and its goldens), then wave 1 (1a first and alone; 1b–1f
each its own commit), then wave 2 (2a+2b+2c together; 2d–2h after), then wave 3. Item T's
fixture lands in wave 0, its invariants with the item they pin (the table above says
which), the CDO rendering goldens at the end of wave 2 (before that they would be
re-minted every commit).

Every wave: task gate; `check-goldens --regen`, the moved families reviewed and the reason
for every moved family in the commit message; `check-goldens --verify-coverage`;
`cdo-gate` (every wave touches rendering the CDO goldens see); the MCP contract tests; a
CHANGELOG entry per wave (Changed for the renderings, Fixed for B1–B7, Added for the
tests); and a re-run of the hands-on review on CDO for the three T routines, recorded
before/after in `docs/Handoff/` or the PR. **The installed `~/.al-sem/bin/alsem` does not
update itself** (CLAUDE.md): rebuild and copy it before each hands-on re-run, or the re-run
measures the old binary.

Golden churn, by wave: 1a moves every human golden with an anchor (explain 10 files, query
≈10, prove 16, digest 20, diff 11, fingerprint 22 if they print anchors) and, under the
1-based-JSON decision, every JSON golden with a `line` (prove 16, digest 22, fingerprint
and snapshot families, the explain/query JSON files, the `split/full-snapshot` file).
Wave 2 moves the human goldens of explain/query/prove/digest again, by shape, and the
digest JSON by schema. Wave 3 moves query goldens once more. `alsem graph` JSONL moves in
no wave: bc-dev-mcp parses it and nothing here touches it. `alsem analyze` outputs (the
`cli-a` family, SARIF, policy) move in no wave either: they are already 1-based and read
no renderer this plan changes.

Risks to watch: (1) the byte-identical CLI/MCP contract — every item is a CLI change, so
the contract holds by construction, but 1b/1e add server-side fields (`summary`,
`lineBase`) that `mcp_server.rs` must pin; (2) the ConTinAI citation consumer (2g, and the
0a decision) — notify before 1a lands; (3) the LSP's `al-call-hierarchy/explain|prove|
digest|queryEffects` relay the same stdout to the editor extension (`src/lsp/engine.rs`);
nothing in this repo parses it, but an extension that does (file:line links) reads the new
base after 1a — tell the extension's owner in the same change; (4) 3a's cone store on the
query path: measure memory, and keep the per-operation depth unchanged so no L4 number
moves; (5) digest's major schema bump in 2f: any script reading `viaPaths` hop shape
breaks — search `docs/Handoff/` and bc-dev-mcp for readers first.

### 4.1 Outside consumers after waves 0 and 1 (checked 2026-11-04, merged as `769d481a`)

A read-only check of every repo on the owner's GitHub account (24 cloned; GitHub code
search missed bc-dev-mcp, so the clones were grepped) and the local ConTinAI copy. Nothing
breaks silently: the readers that care check `schemaVersion` major == 1 and switch the
feature off with a warning on 2.x. `lineBase` is in `payload.lineBase`.

- **bc-dev-mcp** (`SShadowS/bc-dev-mcp`, main `bd6543b`): pinned to al-sem 1.7.1
  (`src/core/alsem-pin.ts:9`; `satisfiesPin` in `src/core/tool-binary.ts:63` refuses another
  major), so it is not exposed until the pin moves. When it moves:
  `src/core/risk.ts:22` major 1 → 2, `:110` drop the `+ 1` (better:
  `line - (payload.lineBase ?? 0) + 1`), `:14` doc; `src/core/alsem-writes.ts:25` major → 2,
  `:167-168` drop `anchorLine + 1`, `:67` doc; `src/core/break-effects.ts:21` and
  `src/core/test-order-static.ts:22` majors → 2 (no lines read); tests use "2.0.0" as the
  unsupported example (`tests/core/{risk,break-effects,test-order-static,alsem-writes}.test.ts`)
  → 3.0.0; hand-built 0-based anchors in `tests/core/risk.test.ts:27-31` and
  `alsem-writes.test.ts:77-82`; re-record `tests/fixtures/alsem-risk/digest.json`,
  `alsem-order/*`, `alsem-prove/recorded/*`, `record-writes/recorded/*`; docs
  `scripts/e2e.md:131,503`. **Keep** `src/core/hotspot-explain.ts:171-172`'s `+ 1`: it reads
  `alsem analyze --with-evidence` evidence ranges, which are still 0-based (see below).
- **ConTinAI** (local only, `/home/sshadows/Projects/CONtinAI`): no change needed.
  `work/scripts/verify.py:216-220` and `graphs.py:32,39,42` honor `lineBase`;
  `dep:<guid>:<path>` citations now open exactly at the cited statement (the old ones were
  one line early). Suggested: `effects.py:71-72` should record `payload.lineBase` beside the
  copied `origin.anchor.line` (nothing reads that field today). Stale doc: `kb/gaps.md:35`.
- **al-lsp-for-agents** (the editor extension, the Go wrapper, the LSP packages): unaffected;
  it uses only the call-hierarchy and property requests. No repo uses the
  `al-call-hierarchy/{explain,prove,digest,…}` engine requests yet.
- **al-perf** (dormant): `src/semantic/contracts.ts:17` `EXPECTED_INVENTORY_SCHEMA_VERSION`
  "1.1.0" → "2.0.0", or its al-sem fusion turns off (`engine-runner.ts:353-360`,
  `diff-runner.ts:296-303`). It reads no lines from the inventory.
- **Gap in this plan:** `alsem analyze` findings are 1-based, but
  `analyze --with-evidence` evidence `sourceAnchor.range.startLine` is still 0-based, while
  the same-shaped `rootClassifications[].sourceAnchor` in the fingerprint inventory is now
  1-based. Aligning it must land together with removing bc-dev-mcp's
  `hotspot-explain.ts:172` `+ 1` (tracked as an issue).

## 5. Open questions, answered from the code

1. **JSON line base.** Who reads engine-query JSON lines today: the G13 contract test
   (`tests/cli/cli_ids_and_line_bases.rs`: graph `meta.lineBase 1`, prove and fingerprint
   `payload.lineBase 0`, graph `line == inventory anchor + 1`, prove `routine.anchor ==`
   the inventory's); bc-dev-mcp reads `alsem graph` JSONL only (1-based, untouched); the
   LSP relays bytes and parses nothing; the ConTinAI pipeline resolves `dep:` citations
   (which base it assumes is not recorded here — ask); the analyzer surface (SARIF, analyze
   JSON, HTML, PR summary, policy's `startLine`) is already 1-based and independent.
   `alsem diff` compares snapshots that carry anchors, so a base change is a snapshot
   schema bump; the fingerprint hash itself does not include line numbers. **Recommendation:
   move JSON to 1-based across the whole engine-query surface at once** (prove,
   fingerprint inventory, explain, query, digest, snapshot), declare `lineBase: 1`
   everywhere, bump the digest/snapshot schema versions. Reasons: 1-based is already the
   product's convention everywhere else (graph, analyze, SARIF); agents read the JSON as
   often as the human form; two bases in one product is exactly the trap this review fell
   into. The cost is one regen of every JSON golden with a line and one note to the
   ConTinAI owner. If the owner prefers the smaller change, the fallback is 1-based human
   + 0-based JSON with `lineBase: 0` stated in every document and in the envelope — but
   then the G13 "+1" rule must appear in every description, forever.
2. **Targets and the detail flag.** ~150 lines / ~200 KB are the right order for
   `IssueReminder` (the summary-and-groups form of its card is about 60 lines of substance
   plus 93 raises, which a count header makes skimmable). Full detail stays behind flags,
   never removed: digest already has `--include-reads` and `--tests`; add `--detail` to
   explain and digest, `--all` to query, honour `--max-paths`; the MCP tool inputs expose
   each. Goldens pin both forms.
3. **Who relies on temp operations appearing as writes.** No renderer consumer does.
   Detectors read `cone_derived` (which keeps `table_writes_all` including temp and
   `physical_table_writes` excluding it) and the L4 solver, never the rendered outputs;
   `prove` already excludes temp; digest JSON carries `detail.tempState` and `|kt` keys
   (`cli_digest_temp_state` pins them), and `digest diff` keys on effect ids, so dropping
   temp rows from the JSON would change diffs — which is why 2b changes the human form and
   the counts only, and keeps every JSON row; policy predicates read detector facts, not
   these outputs; fingerprint/snapshot carry the temp class as a fact. So 2b is safe as a
   rendering change and unsafe as a JSON deletion.
4. **Depth for `query effects`.** The one-hop depth is structural (fixed leaves in L4,
   §2 M3). Solving the full cone in L4 is the wrong fix (detector inputs, memory, CDO
   re-triage). The right one is 3a: physical effects beyond the boundary from
   `cone_derived` (built for exactly this, per `effect_query.rs`'s own doc), labelled, with
   per-operation rows one deep and the header saying so. Cost: the cone store on the query
   view plan (already resident after stage 5b; measure) and one more join in the renderer.
5. **What an agent still needs that the first draft missed** — all now items: summaries on
   every spilled file (1e), `(no anchor)` rows (2a), obligations with pointers (2h), the
   `dependencies:` header and the `LockTable` line (2c), `entryDiagnostics` /
   `scopedGuarantees` / duplicated hop `kind` (2f), `alsem_status` freshness (3c), the
   `diff`-is-the-change-mode wording (3b), `--show-source` (2g), the RecordRef wording
   (3a), and the fixture built before any renderer moves (0b).

## Review notes (what changed from the first draft, and why)

- **Renumbered the findings back to the review's ids** (B1–B6, P, D, Q, T, S, E) and kept
  the draft's ids in brackets. The draft's "B6" merged my P1 and P2 and dropped my B6 (the
  57 `(no anchor)` rows); the draft's M1/M2 absorbed Q3/Q4/T2 and lost Q6, E2, P4, D2's
  content, D3's non-path bulk, D7, S2. Each of those is now an item.
- **Added wave 0.** The draft built the fixture "with wave 1"; but without it the first
  renderer change has no golden showing a boundary group, and the JSON line-base decision
  changes what 1a touches. Both now come first.
- **1a lands alone and first, not after wave 2.** The question was whether to wait to avoid
  regenerating goldens twice. Regen is cheap; reviewing a diff where every line changed
  for two reasons is not. A pure "+1" diff is checkable mechanically.
- **Recommended 1-based JSON**, with the evidence that the rest of the product (graph,
  analyze, SARIF, HTML, PR summary) already is, and named the one external reader to warn
  (the ConTinAI `dep:` citation consumer, found through `cli_dump_dependency_source.rs`).
- **M3 got its root cause** (dependency routines are fixed leaves in L4; the capability cone
  is not), and 3a was rewritten from "follow the same cone or label it" to a concrete,
  bounded fix that reuses `cone_derived` and changes no detector input.
- **B4 corrected**: per-operation fields do not exist for `Modify` (the fields are the
  routine's assignments), so the fix is to attach fields to the right operation kinds and
  label them, not to recompute per op.
- **2g keeps the guid in JSON** for the citation consumer; the draft said "keep the guid in
  JSON" too, now with the reason and a `--show-source` that reuses the dump command's text.
- **Item T was made concrete**: a table of invariants, each with the defect it would have
  caught and its discrimination break; the cross-tool pairs and how tables are chosen; the
  two test homes (an always-on `cli` member and the first CDO-gated `cli` member); CDO
  rendering goldens; the size test against Claude Code's real limits (50,000 characters /
  25,000 tokens — it files the result rather than refusing it, which is why the agent "got
  nothing"); the one-copy and description checks; and a flakiness note.
- **Schema consequences stated** where the draft was silent: 1d is a minor digest bump, 2f
  a major one (fields leave the default document), the snapshot schema moves with 1a.
- **Added the operational gotchas** an executing agent would hit: the installed MCP binary
  does not update itself; `check-goldens` is the family table's source of truth; the
  `cdo-gate` is the branch gate, not a per-commit one; CHANGELOG per wave.
- **Not changed:** the waves' overall shape, the contract (MCP answer = CLI output), the
  targets (~150 lines / ~200 KB), and the rule that every invariant carries a
  discrimination proof.

### What the owner must decide

1. JSON line base: 1-based everywhere (recommended) or 0-based with `lineBase` declared.
2. Whether to warn the ConTinAI pipeline owner before 1a/2g, and which base it assumes.
3. 1c: accept two copies of a JSON answer under protocol 2025-06-18 (content block +
   `structuredContent.output`, as the MCP spec recommends) or make `human` the MCP default
   for prove and keep JSON opt-in.
4. The CDO routine set for item T beyond `IssueReminder` (which hub, which plain routine).
5. 2f's major digest schema bump and its one-release `--detail` compatibility window.
