# MCP output fixes, wave 2: the rendering redesign (design spec, revision 3)

**Status:** design, revision 3, 2026-11-04. W2-0a implemented on branch `mcpout/wave2` (§11); W2-0b
onward not implemented. The owner approved revision 3 without a further full review round.
**Based on:** master `367a664d` (waves 0 and 1 merged, plus the resident-store deadlock fix). The
measurements in §1 were taken on branch `mcpout/wave0-1` at `a2464efc`, whose code tree master
still has for every file this design touches. Read the wave 0/1 commits and the CHANGELOG before
implementing.
**Plan:** `docs/superpowers/plans/2026-11-04-mcp-output-fixes.md` (items 2a–2h, item T).
**Reviews folded in:** round one — astra (`target/tmp/mcp-output-wave2-review-astra.md`, "reject as
written", A1–A11) and Fable (`…-review-fable.md`, "approve with changes", F1–F16), folded into
revision 2 (§0.R2). Round two — astra (`…-review-astra-r2.md`, "approve with changes", findings 1–11
with three blockers for W2-0b/W2-1, an invariant table and answers to §9) and Fable
(`…-review-fable-r2.md`, "approve", N1–N15 and answers), folded into this revision (§0). Revisions 1
and 2: `target/tmp/mcp-output-wave2-design.md`, `…-design-r2.md`.
**Owner choices** where the reviews differ are applied; round one's in §0.R2.2, round two's in §0.2.
**Measured on:** the CDO baseline (`/home/sshadows/Projects/DO-cdo-baseline/Cloud`, `bc3ccb18`),
routines `CDO Issue Document::IssueReminder` (IR) and `CDO E-Mail Handler::RunReportID` (RR), with a
`release-fast` build of the branch (`a2464efc`). Every number is from those runs unless it is marked
"estimate" or "mock-up". How they were produced: Appendix A.

Words used in this spec (each is defined again where it is first used):
- **crossing**: one edge from workspace code into dependency code — a call site, an implicit-trigger
  operation, or an event dispatch. Today's digest "boundary group" is one crossing per effect type.
- **entry point**: a dependency routine a crossing enters (its *target*). One crossing can have
  several targets (a `Page.Run` enters `OnInit`, `OnOpenPage`, …).
- **write class**: one of `physical` ("may persist"), `callerDependent`, `temporary` (§3.2).
- **literal reach**: prove's existing pass that follows literal Boolean/table arguments and marks
  effects and group resources that cannot run (`digest_cli.rs:1445 literal_reach_pass`).
- **witness**: a path from the root to one operation, with its edges and its terminal operation.
- **rollup**: the per-routine summary every renderer reads (§3.1).

- **effect classification**: which write class one effect contribution is in (§3.2). It is
  shared by every tool.
- **question evaluation**: the answer to "does R write T" for one table T: the classification plus
  field-guarded matches, caller resolution and the obligation rules (§3.2.1). It is shared too.
- **typed edge set**: the edges that make up one crossing, each as (source routine, dependency
  target, edge kind, site identity) (§3.3).

---

## 0. Changes from revision 2

### 0.1 Every round-two finding and what changed

"Accepted" means this revision does what the finding asks; the section named says how. Owner choices
for this round are marked **R3a–R3j** (§0.2).

| Finding | Summary | Decision | Where |
|---|---|---|---|
| astra 1 (blocker) | T-W2-16 does not describe prove: field-guarded unknown-table writes give `yes`; ordinary obligations give `unknown` | Accepted (R3a). Effect classification is split from question evaluation. One read-side `evaluate_writes_table` (field-guarded matches, caller resolution and the full obligation rules) gives every tool the same table-specific answer; T-W2-16 is rewritten around it and also tests tables absent from every output | §3.2, §3.2.1, T-W2-16 |
| astra 2 (blocker) | The G1 fixture needs crossing-local pruning the literal pass does not do; a bare site id is not a unique edge | Accepted (R3a). Crossing membership is a typed edge set (source, dependency target, edge kind, site identity); root-level liveness and crossing-local liveness are separate; a crossing's resource status is `reachable` / `disproved` / `not established` from the crossing's incoming binding states; a capped search never deletes a resource; mixed workspace/dependency dispatch fixture sharing one site id | §3.3, §3.3.1, §3.4.3, W2-0a fixtures |
| astra 3 | The terminal-fact index is too lossy (temp class, producer frame, original op kind) | Accepted (R3a). Terminal records keep the source fact identity, original operation kind, resource, temp/producer information and the terminal identity; they are joined in the root's frame before any class count; zero-hop is `Found(Witness { hops: [], terminal })`; the two temp-vs-persistent fixtures | §3.4.1, §3.4.2, W2-0a fixtures |
| astra 4 | Raw effects, refined effects and the compatibility output are conflated | Accepted (R3c). Rows keep an additive reachability/contribution status; original and refined resource membership are kept apart; each count is derived from the exact list it describes; `--schema 2` is the old serialization shape over the current analysis, not frozen output | §3.1, §4.4.2, §5, T-W2-6, T-W2-7 |
| astra 5 | Query's last-row origin selection; per-routine single file; prove's `groupPath.hops` are strings; collided declarations; hash-prefix extension | Accepted (R3d). The operation index is a multimap with legacy accessors: query keeps its last-row selection, explain its first-row selection; ambiguity is exposed separately. Hops keep their own source-file provenance. Prove keeps `groupPath.hops` as strings and adds a structured `groupWitness` beside it. Hash-prefix extension repeats until unique | §3.5, §4.4.2, §4.7, T-W2-8, T-W2-24 |
| astra 6 | "open world" and `no` with incomplete coverage misdescribe prove | Accepted (R3e). The real answer is authoritative: `answer: no` with effective coverage and assumptions; `answer: unknown — no matching effect observed; coverage incomplete` when obligations block an absence proof; `yes` is never qualified. Explain/digest scope: "loaded program; unloaded implementations/subscribers are not enumerated". Analysis incompleteness is separated from informational warnings | §3.2, §3.7, §4.7 |
| astra 7 | First-hop membership does not prove "through this callee" for this effect | Accepted (R3g). `firstHop` must match the complete effect contribution after the edge's argument/temp transfer; otherwise explicit unavailable, or a hop labelled "example graph route to the operation owner". A "has summary row" probe is added | §4.5, §4.6, T-W2-11 |
| astra 8 | Temporary-only replaces temporary operations; field wording overclaims; Validate falls into "read"; recovered owners only under `--detail` | Accepted (R3f). All temporary operations/tables are reported plus a separate `temporaryOnlyTables`; caller-dependent exclusion is per contribution; fields say "fields this routine assigns on this table; not operation-specific"; explicit buckets for Validate and unclassified kinds; recovered no-path owners and locations in the default output | §3.6, §4.1, §4.5, T-W2-4, T-W2-19, T-W2-20 |
| astra 9 | Per-search caps do not bound the cache; the fallback needs an outcome contract | Accepted (R3h). Aggregate byte/state limits, deterministic eviction, cancellation checks; outcomes `completed` / `capped` / `unavailable`; the fallback's crossing membership is verified, not assumed; dual-cap and run-order tests; `LiteralReach` kept per revision on the resident engine; cold/warm/RSS/multi-root measured before W2-1 lands | §3.4.3, §3.1 |
| astra 10 | Invariant corrections; T-W2-23 conflicts with the wire representation | Accepted (R3i). Each row amended as astra's table asks; T-W2-23 has two budgets: `structuredContent` (the client-consumed limit) and total wire size; T-W2-12's source-check parser must handle `dep:Base Application:…` anchors with spaces | §8.3 |
| astra 11 | The issue's reproduction join and its `alsem diff` impact are wrong | Accepted. The issue text reproduces ownership from internal `origin_operation_ids`; the demonstrated impact is ambiguous digest row identity and unsafe external joins, not `alsem diff` (whose capability key is op, resource kind and resource id) | §10, issue file |
| astra invariant table | Per-row assessment of T-W2-1..24 | Accepted row by row | §8.3 |
| astra answers q1–q6 | Budgets 1,000,000 / 480,000 B; narrowed default paths; +1 s warm as a review trigger; `/3` accepted; schema-2 end date at release prep; prove wording | Accepted (R3j, R3e) | §9 |
| Fable N1 | Event-dispatch edges carry no id in `LiteralReach` | Accepted (R3b). `event_id` is carried on event-dispatch adjacency (W2-0a, done) | §3.4.3, §11 |
| Fable N2 | One product BFS and a suffix cache do not compose | Accepted (R3b). Prefix phase = today's root tree; suffix tree cached per `(target, bindings)`; cap per suffix tree; fallback per crossing | §3.4.3 |
| Fable N3 | An `ev:` crossing is one per event, not per publish site | Accepted (R3b). `sites` is plural; an event crossing with several dependency subscribers is multi-target | §3.3 |
| Fable N4 | The prefix's tie rule | Accepted. Shortest `via_paths[0]` over the crossing's rows, ties by row order | §3.3 |
| Fable N5 | Operation count depends on an uncapped search | Accepted. Fallback "at least" plus `limits: path-search-capped`; the scope line says what reach counts | §3.6 |
| Fable N6 | Leg (c) needs group `unresolvedResources` | Accepted. Group unresolved resources are counted in the rollup's unresolved-table writes | §3.2.1, T-W2-16 |
| Fable N7 | `--schema 2` byte-identity is a prediction | Accepted (R3c). Re-minted with the reason named if a fixture moves; the 1b summary line in the CHANGELOG says literal-dead resources are excluded | §5 |
| Fable N8 | Define "trivial" scoped guarantee with every field | Accepted | §4.4.2 |
| Fable N9 | Explain cost split; per-revision `LiteralReach` | Accepted (R3h) | §3.1 |
| Fable N10 | Cache `firstHop` membership per direct callee | Accepted; fixture where the first sorted callee lacks the operation | §4.5, T-W2-11 |
| Fable N11 | `groupPath` hop change is not additive | Accepted (R3d). `groupPath` unchanged; `groupWitness` beside it | §4.7 |
| Fable N12 | Which path a digest group row carries | Accepted. One representative terminal witness per may-persist group row; the prefix lives once on the crossing | §4.4.2 |
| Fable N13 | Dangling "§3.9" references | Fixed (they pointed at §4.4.2 and §3.8 rule 6) | §0.R2.1, §4.2 |
| Fable N14 | The factId fix's home | Accepted. `build_canonical_key` falls back to the first origin operation id when a non-group row has no terminal id; put in the issue | §10, issue file |
| Fable N15 | CDO facts verified; add the `CDO Log Attachment` case | Accepted (T-W2-16 CDO case) | T-W2-16 |
| Fable answers q1–q6 | Budgets; per-group-row witness; +1.5 s; schema-2 end rule; prove wording | Owner took the budgets, the per-group-row witness, astra's review trigger (+1 s), the combined end-date rule (R3j) and astra's prove wording (R3e) | §9 |

### 0.2 Owner choices applied (round two)

- **R3a:** astra's three blockers. Effect classification separate from question evaluation, with a
  shared `evaluate_writes_table`; typed crossing edge sets with crossing-local liveness and
  `reachable` / `disproved` / `not established` status; lossless terminal records joined in the
  root's frame; the fixtures she names.
- **R3b:** Fable's N1 (event id on event-dispatch adjacency), N2 (prefix phase + cached suffix tree
  per `(target, bindings)`, cap per suffix tree, fallback per crossing), N3 (plural sites), N5, N7–N14.
- **R3c:** astra's raw-vs-refined contract. Rows keep an additive reachability/contribution status;
  counts come from the exact list they describe; `--schema 2` = old serialization shape over the
  current analysis, stated as such.
- **R3d:** query keeps its legacy last-row origin selection; ambiguity is exposed separately. Prove
  keeps `groupPath.hops` as strings and adds a structured witness field beside it. Hop source-file
  provenance kept. Hash-prefix extension repeats until unique.
- **R3e:** prove wording per astra (§3.7, §4.7); explain/digest scope line "loaded program;
  unloaded implementations/subscribers are not enumerated"; incompleteness apart from warnings.
- **R3f:** temporary summary with all temporary operations/tables plus `temporaryOnlyTables`;
  contribution-level caller-dependent exclusion; field wording; Validate and unclassified buckets;
  recovered no-path owners/locations by default.
- **R3g:** first-hop by complete effect contribution, else unavailable or labelled; "has summary
  row" probe.
- **R3h:** cache limits, eviction, cancellation, outcomes; `LiteralReach` per revision on the
  resident engine; measure cold/warm/RSS/multi-root before W2-1 lands; ~+1 s warm per root is a
  review trigger, not a CI assertion.
- **R3i:** T-W2-23's two budgets; T-W2-12's parser handles `dep:<name with spaces>:` anchors.
- **R3j:** the open questions are settled (§9).

---

## 0.R2 Changes from revision 1 (kept for history)

### 0.R2.1 Every round-one finding and what changed

"Accepted" means the design now does what the finding asks (the section says how). "Rebutted"
gives the evidence. Owner choices are marked **O1–O4** and **OS** (size) in §0.R2.2.

| Finding | Summary | Decision | Where |
|---|---|---|---|
| A1 (blocker) | "physical = not known(true)" is not prove's whole rule; T-W2-16's biconditional cannot hold; explain has no L4 temp-state join | Accepted (O1). One three-class classifier in `rollup.rs`, prove calls it; the literal-reach pass runs for every reader; explain declares the L4 temp-state join it lacks; agreement is exact for the shared classes and prove's question-specific refinements are listed | §3.1, §3.2, T-W2-16 |
| A2 (blocker) | G1 is real; restarting at the entry point drops bindings; groups are keyed by crossing, not target | Accepted. Crossing records (identity, kind, targets, site, prefix) on `DigestEffectResult`; G1 fixed by one search from the root that must traverse that crossing's edge, so bindings and edge identity survive; entry points are a presentation over crossings; adversarial fixture | §3.3, §3.4.3, W2-0a/0b |
| A3 | COMMIT groups have no `group_ops`; a prefix to an entry point is no evidence for every table listed; path and anchor chosen apart | Accepted. Read-side terminal-fact index (every direct fact, resource-less ones included); witness bundle with terminal; explicit `unavailable` outcomes; path and anchor from one witness. The per-table promise for tables beyond an entry point is narrowed and put to the owner | §3.4, §9 q2 |
| A4 | The default drops assumptions, non-call limits and scoped guarantees; "no" reads like a proof | Accepted (O3). Always-visible `coverage:` + `assumptions:` + `limits:` lines; typed limitations; "no observed COMMIT" unless completeness holds; non-trivial `scopedGuarantees` kept | §3.7, §4.4.2 |
| A5 | `payload.routines` keyed by display; `path` shape vs T-W2-8; `pathCount`; producer truncation hidden; hop line without file; table ids dropped | Accepted. Keys are hashed stable ids; `paths: [{hops, terminal}]` for every cap; `pathsProducerTruncated` and `pathsOmitted` kept apart; a hop's line is in its calling routine's file, named in the routine table; table resources keep `{id, display}` | §4.4.2 |
| A6 | Count units; representatives are not an inventory; one COMMIT group ≠ one site; ownership vs execution; unions; event units; LockTable; "RecordRef" overclaims; fields vs original op kinds | Accepted. Units and dedup keys defined (§3.6); operations counted from the terminal-fact index, not from `origin_operation_ids`; "commit-bearing entry points"; owner vs where axes; unions; LockTable its own class; "table not resolved"; fields by original op kind with ModifyAll/Rename/Validate fixtures. Measured: r1's "6,284 events beyond" was the wrong unit (the union is 7,360) | §3.6, §4.5 |
| A7 | Touches folding must change the computation; temporary touches matter (schema, triggers); `firstHop` must be a real association | Accepted (O4). Branch before witness expansion; membership probes; per-caller aggregation; "any record touch" vs persistent / possibly persistent; `firstHop` checked by summary membership or `null` with a reason | §4.5, §4.6 |
| A8 | Owner recovery needs a shared index with fallbacks; "unavailable" is not "dependency" | Accepted. One operation index (internal op id → owner routine, anchor, original kind) with `missing` / `ambiguous` outcomes; workspace vs dependency by the owner's app; recovered location shown | §3.5 |
| A9 | `--detail = old schema` traps; prove reuses `effect_to_value`; flag precedence | Accepted. `--schema 2` (frozen renderer) and `--detail` = unfolded 3.x; prove's serializer separate from the digest-v3 projection; precedence table | §4.4, §5 |
| A10 | Make the consumer fields an executable contract; do not prescribe a bc-dev-mcp migration | Accepted. Consumer-shaped test T-W2-21; §7.3 of r1 (bc-dev-mcp "switch to `--detail`") removed: `entries[].routine` stays in 3.x | T-W2-21, §8.4 |
| A11 | Rewrite the 16 invariants; missing coverage; envelope size after escaping; digest `always_file` | Accepted. Table in §8.3 (each row says which review row it answers); T-W2-22..24 added; T-W2-23 checks the serialized result; digest's `always_file` stated | §8.3, §6 |
| A OQ1 | Digest size: pretty with an approved budget ~320 KB | Accepted in form (OS), but the measured number moved: see §4.4.3 and §9 q1 | §4.4.3 |
| A OQ2 | Fold dependency event names by default | **Overridden by O2** (keep every name in the digest file; fold in explain) | §4.4 |
| A OQ3 | `--schema 2` plus `--detail` for unfolded 3.x | Accepted (O-agreed) | §5 |
| A OQ4 | Explain `/3` only after the extension owner confirms | Accepted; answered since by the plan's §4.1 (`1ebd5103`): the extension does not use the engine requests | §4.2, §9 q4 |
| A OQ5 | File the no-path issue now with op ids and a minimized fixture | Accepted; text prepared (not filed) | §10, issue file |
| A OQ6 | G1 as a separate prerequisite commit | Accepted (owner) | W2-0b |
| F1 (high) | Prove has three classes; caller-dependent tables; mixed rows; groups are physical or temporary | Accepted (O1). Mixed-row rules stated; groups never caller-dependent | §3.2 |
| F2 (high) | No entry-point identity on the row | Accepted. Crossing record with target ids added where the group is created (`digest.rs:1599`); non-goal relaxed to "R4-F projections unchanged" | §2, §3.3 |
| F3 (high) | Explain/digest read before literal reach | Accepted (O1). The pass runs for every reader; dead resources counted, not listed as writes | §3.1 |
| F4 | G1 fix must replay the prefix bindings; per-(entry, bindings) cache; cap fallback; cost; `path_through_entry_point` | Accepted, as its own commit (owner, not "in 2f" as F4 proposed); the fallback is labelled, never presented as through the entry point | §3.4.3, W2-0b |
| F5 | Say which id the owner is and why no path exists | Accepted. Internal id (`compute_routine_id`), never `fact_subject`; three hypotheses and the probe in the issue text | §3.5, §10 |
| F6 | "workspace" means two axes; sections overlap | Accepted. Owner axis vs where axis; unions | §3.6 |
| F7 | Display-keyed side table breaks on overloads | Accepted, by a different mechanism than F7 proposed: keys are hashes of stable ids (A5), the display is data; the human form disambiguates overloads by signature | §4.4.2, §3.8 rule 6, T-W2-24 |
| F8 | Prove must keep `evidence` on group rows (verify.py) | Accepted. Prove's serializer is its own (A9); T-W2-17 | §4.7, T-W2-17 |
| F9 | T-W2-2 and T-W2-3 fail on r1's own mock-up | Accepted. Duplicate check on logical records; fold through one helper | T-W2-2, T-W2-3 |
| F10 | `ViaRank` has five values; bucket precedence | Accepted | §4.5 |
| F11 | LockTable is not a write | Accepted (also A6) | §3.6, §4.5 |
| F12 | Three digest write types vs five printed; one spelling | Accepted. Original op kinds from the operation index; one spelling (`Insert`, `DeleteAll`, …) in every human output | §3.5, §3.8 |
| F13 | Path and anchor from the same row | Accepted (also A3) | §3.4 |
| F14 | No gratuitous renames in explain `/3`; the LSP relay reader is unverified | Accepted. `fields` and `via` keep their names (narrowed meaning stated); the reader question is answered by the plan's §4.1 (none) | §4.2 |
| F15 | 1b's number changes; two summary vocabularies | Partly obsolete: on the branch 1b already counts a group's whole set (`digest_cli.rs:2505 written_tables`). Accepted: one `RollupTotals` serialized once; 1b moves from two classes to three | §3.1 |
| F16a | "followed in full" is too strong | Accepted (also A4) | §3.8 rule 3 |
| F16b | Section 9's cap 9 vs 12 | Accepted: one table cap | §3.8 rule 4 |
| F16c | Inline cap is 40 KiB on the branch | Accepted | §6 |
| F16d | COMMIT groups have no op ids | Accepted (also A3): terminal-fact index | §3.4.1 |
| F16e | A crossing's prefix is often several hops | Accepted: the prefix is the group's shortest witness, printed once per crossing | §3.3 |
| F16f | Whose file is a hop's line in | Accepted: the calling routine's file | §4.4.2 |
| F16g | T-W2-14 counts the wrong lines | Accepted | T-W2-14 |
| F16h | T-W2-9's workspace route must be shorter | Accepted | T-W2-9 |
| F16i | T-W2-6 compares like with like | Accepted | T-W2-6 |
| F16j | "callers 1" counts a page action | Accepted: `callers: none; reached from 1 page action` | §4.1 |
| F16k | Digest has no callers/tests in its summary | Accepted | §3.8 rule 2 |
| F OQ1 | (a) ~300 KB with ratchet + `--compact` | Accepted in form (OS); number re-measured | §4.4.3 |
| F OQ2 | Keep every event name in the digest; fold in explain | Accepted (O2) | §4.2, §4.4 |
| F OQ3 | `--schema 2`; `--detail` = unfolded 3.x | Accepted | §5 |
| F OQ4 | `/3` acceptable with no renames and one extension check | Accepted | §4.2, §9 q4 |
| F OQ5 | File the no-path issue | Accepted (prepared) | §10 |
| F OQ6 | G1 inside 2f, own commit | Owner chose a prerequisite commit before the path work (W2-0b) | §8.1 |
| F invariants table | Per-row corrections | Accepted; merged with A11's table | §8.3 |
| F missing (a)–(d) | prove evidence anchor; crossing target names; physical/temporary disjoint; callerDependent never physical | Accepted as T-W2-17..20 | §8.3 |

### 0.R2.2 Owner choices applied (round one)

- **O1 (agreement with prove):** (refined in revision 3 by R3a: the classifier is shared, and so is
  the table question's evaluation, §3.2.1) Fable's approach. One three-class classifier in
  `src/engine/l5/rollup.rs` (`physical` / `callerDependent` / `temporary`), which prove's
  `writes_only_temporary` / `writes_caller_dependent` call; prove's literal-reach pass runs for every
  reader (explain, digest), so "does X write T" has one answer. astra's honest categories are folded
  in: the physical class carries its reason (known persistent, or temp state unknown); the
  caller-dependent class is "possibly persistent: depends on the caller". Agreement tests are exact
  for the shared classes; prove's question-specific refinements are listed.
- **O2 (event names):** every event name stays in the digest file (`resourceCount == listed count`
  unconditionally there); explain's card and explain JSON fold them to a count.
- **O3 (safety qualifications):** astra's. Always-visible coverage/assumptions line; typed non-call
  limitations; "no observed COMMIT" when completeness is not established; non-trivial
  `scopedGuarantees` kept.
- **O4 (temporary touches):** astra's. "Any record touch" shown apart from persistent / possibly
  persistent; never dropped.
- **OS (size):** accept ~300–320 KB pretty with a ratcheted per-fixture budget and an opt-in
  `--compact`. **Measured since: not reachable together with O2 and the identity fields both reviews
  require** — IR is 469 KB pretty with no event names and 981 KB with every name (§4.4.3). This needs
  a new owner number (§9 q1). The design keeps the ratchet mechanism and proposes the numbers.
- Both reviews agree, adopted: crossing records; G1 as its own prerequisite commit; terminal-fact
  index; explicit unavailable outcomes; stable-id keys; `paths: [{hops, terminal}]`; table ids kept;
  defined units; LockTable class; "table not resolved"; fields against original op kinds; touches
  computation; real `firstHop`; operation index; `--schema 2`; prove's serializer separate; the
  invariants rewritten; the no-path issue prepared (§10).

---

## 1. What the measurement found (branch `a2464efc`)

Facts r1 did not have or had wrong. Each is from the runs in Appendix A.

1. **Sizes after wave 1.** Digest JSON IR 7,339,040 B (r1: 5.6 MB on master), RR 4,952,155 B (r1:
   3.2 MB): 1d lists every resource. Explain human IR 80,769 B / 558 lines (unchanged), RR 194,357 B;
   explain JSON IR 310,692 B, RR 760,024 B; digest human IR 95,870 B, RR 74,945 B; query effects
   JSON IR 670,121 B, human 126,430 B; touches IR vs `CDO File` (down only) JSON 193,498 B.
2. **Groups are crossings, and some crossings have several targets.** IR: 275 effects, 167 own, 108
   group rows over **31 crossings** with **26 distinct entry-point displays**. Five entry points are
   entered at two sites (`CSC Core::IsAppActiveOrAskToActivate`, `Reporting Triggers::GetPrinterName`,
   `CTS-CDN Network Doc. Mgt.::CreateEDocumentLog`, `::SendNetworkDocument`,
   `Document Attachment::ExportToStream`). One crossing has three targets: a `Page.Run` at
   `CDOSendMailManagement.Codeunit.al:242` enters `Page Email Account Wizard::OnInit, ::OnOpenPage,
   ::OnQueryClosePage` (the joined display comes from `digest.rs:382-394 boundary_displays`). The two
   sites of `CSC Core::IsAppActiveOrAskToActivate` write different table sets (16 and 3; the 3 are a
   subset). RR: 32 crossings, 28 displays, the same three-target crossing.
3. **Write classes on CDO.** IR (own rows, distinct tables): 11 known persistent, 40 temp state
   unknown (37 of them are the no-path rows, whose L4 join is empty), 16 temporary, **0
   caller-dependent**. The fixture has one caller-dependent write (`Out Doc` `Modify` on IssueDoc's
   `var` parameter). IR may-persist tables, union over own rows and non-temporary groups: **413** (11
   owned by CDO, 402 by dependencies) plus 2 writes whose table is not resolved. Temporary-only tables
   (never in a may-persist row): 88 (7 in workspace code, 81 only beyond entry points). RR: 406 (11 +
   395) plus 2.
4. **Events, in one unit (distinct event id).** IR raises 7,435 distinct events: 75 in workspace
   code, 7,360 only beyond entry points (r1 printed "+6,284", a wrong unit: one group alone lists
   6,892). Group event memberships 7,366, so events barely repeat across crossings. RR: 7,393 (72 +
   7,321).
5. **The no-path rows (B6).** IR: 57 rows (30 INSERT, 24 MODIFY, 3 DELETE), 47 of class "temp state
   unknown" on 37 tables (27 Base Application, 5 System Application, 5 Continia Delivery Network) and
   10 temporary. 26 of the 37 tables are also written beyond an entry point. RR: 20 rows, 12 tables.
   **The 57 rows share 5 `factId`s** (one per effect type and temp class): a fact id hashes the
   terminal anchor, which these rows lack. So `factId` is not a unique row key, which breaks r1's
   T-W2-6 and makes digest row identity ambiguous for external joins (§10; revision 3: not an
   `alsem diff` problem, whose key is not `factId`). Their operation ids are on the
   `temp-state-incoherent` (`no-l4-row`) diagnostics, 61 for IR; their owner routines are 50
   distinct routines, none of them in query's one-routine-deep store.
6. **Scoped guarantees.** IR: 144, all trivial (`scope: owning-routine`, `interveningBoundary: none`,
   `validForRefutation: false`); the fixture has 1 non-trivial of 14 (`WRITE_PENDING_AT_EXTERNAL_IO`,
   `scope: root`, `interveningBoundary: unknown`). So "fold the trivial ones" loses nothing on CDO
   and keeps the fixture's one.
7. **Paths.** IR: 143 rows carry 3 paths, 155 have `viaPathsTruncated: true` (the producer cut some).
8. **Gaps.** IR: 288 unresolved call sites = 16 in workspace code (10 interface, 6 dynamic), 265 in
   dependency code (251 interface, 14 dynamic), and **7 with no call-site anchor** (6 interface, 1
   dynamic). r1's "272 in dependency code" lumped the 7 in.
9. **Query (L4) reports operations the cone rules out under literal arguments, inside workspace
   code.** IR query lists `Modify` on `CDO Log Attachment` 3 times (`MoveToFilesystem`,
   `MoveToAzureBlob`, `DeleteLogAndAttachmentData`); the digest has no `DB_MODIFY` on that table at
   all. Source: the routes from IR pass `OnInsert = true` (`CDOLogAttachment.Table.al:46`,
   `CDOEMailLog.Table.al:340` → `MoveEMailToFile(…, true)`, which exits at `:932` before the
   `Modify` and before `LogAttachment.MoveToFilesystem(…, false)` at `:941`). So r1's mock-up line
   "CDO Log Attachment: …, MODIFY 3", taken from query, was wrong for explain. The cone's literal
   handling and query's are different substrates; wave 3's fourth agreement leg must allow for it
   (§7).
10. **Query buckets, measured.** IR: 652 operations, all `via: inherited`. Writes that may persist:
    30 on 15 tables (27 known persistent, 3 temp state unknown). Locks: 3 on 3 tables
    (`Reminder/Fin. Charge Entry` is lock-only), plus 5 on an unresolved table. Table not resolved:
    69 operations in **30** routines (r1: 28). Reads: 258 on 71 tables. Temporary: 292 on 12 tables
    in 87 routines.
11. **G1, on the branch.** `prove "CDO E-Mail Handler::RunReportID" writes-table:"CDO E-Mail Log"`:
    `yes`, 3 evidence rows; the third is the group beyond `Codeunit Sales-Calc. Discount::OnRun` at
    `ws:src/Foundation/CDOFile.Table.al:125`; its `groupPath` (10 hops) runs only through workspace
    code (`… → CDO Log Management.CreateLogEntries → implicit-trigger → CDO E-Mail Log.OnInsert →
    MoveEMailToFile → UpdateStoredAtBasedOnFilePath`). Its hops print `Object.Routine` (dot), not
    1d's qualified `Codeunit X::Y`.
12. **Commits.** IR has no own COMMIT; its one COMMIT is the group beyond `Codeunit
    Reminder-Issue::OnRun`. RR has one own COMMIT (`CDOEMailTemplateManagement.Codeunit.al:1241`,
    guarded by IsHandled) and no group COMMIT.
13. **Digest v3 sizes** (rules of §4.4 applied by script to the real 2.1.0 documents): §4.4.3.

---

## 2. Goals and non-goals

Goals, per the plan: one summary line first; then groups; then detail behind a flag. Writes grouped
by table with operation counts; temporary-record operations counted apart; entry points named
(qualified, from 1d), with tables and events; dependency gaps as counts plus top callees; no
duplicate records; a path per physical write in workspace code and per commit; fields only where an
operation assigns them; readable dependency locations; every effect kind shown; **one answer to
"does R write T" across explain, digest and prove**; every absence statement qualified by coverage.

Non-goals: no change to what the engine *computes* as facts (3a, Q4's per-operation RecordRef bit
are wave 3). **Changed from r1:** `DigestEffectResult` gains read-side records (the crossing record,
§3.3); what the R4-F projections read does not change, so `tests/r4f-goldens/` must not move (a
test-run check in every W2 commit). No change to `alsem graph`, `alsem analyze`, fingerprint or
snapshot output.

---

## 3. Shared design

### 3.1 One rollup, one set of inputs

Wave 2 adds `src/engine/l5/rollup.rs`. Every reader builds its rollup from the **same inputs**:

1. the root's `DigestEntryFull` **after** the L4 temp-state join (`DbTempStates`,
   `digest_cli.rs:1096`) and **after** the literal-reach pass (`literal_reach_pass`,
   `digest_cli.rs:1445`), so dead effects are marked, dead group resources are removed and
   `via_paths` are pruned;
2. the crossing records (§3.3), the terminal-fact index (§3.4.1) and the operation index (§3.5);
3. the `Model` (names, routine roles).

Consequences, each a change from today:
- **Explain gains two inputs.** It builds with `ViewNeeds::EXPLAIN` and "no l4"
  (`explain.rs:40-43`). It must declare the L4 db-effect store for the temp-state join and run the
  literal-reach pass (declared-needs rules: the view plan states both). Cost to measure in W2-1
  before landing: cold `alsem explain` time and RSS on CDO for IR and RR, before/after, and the
  per-root cost of the pass (0.94 s for one CDO root today, prove's measurement at
  `digest_cli.rs:1443`). On the resident engine the L4 store already exists after any query. If the
  cost per root is above 1 s the owner decides (§9 q3); the design does not offer a cheaper
  explain that disagrees with prove.
  **Revision 3 (R3h, Fable N9, astra 9):** measure the split before W2-1 lands —
  `LiteralReach::new` (model-wide, span `prove.literal_reach`) apart from the per-root work
  (`prove.reach_effects`) — for cold CLI explain, warm resident explain, retained and peak RSS, and
  a multi-root request. On the resident engine `LiteralReach` (and the terminal-fact index it holds,
  §3.4.1) is kept **per revision**: it borrows the snapshot and the model, which the revision
  already retains, so the per-root part is what a warm explain pays. Index lifetimes must not keep
  a whole snapshot alive that the revision would otherwise release. About +1 s warm per root is a
  **review trigger**, not a CI assertion; the remedy is caching, never skipping the pass. Default
  cards never compute every dependency resource's path eagerly.
- **Raw versus refined (R3c, astra 4).** The pass refines; it does not delete. Every raw row stays,
  with an additive **contribution status** (`live`, `literal-dead`, `not established`); a group
  keeps its original resource membership and gains a refined membership (resources with status
  `reachable`, §3.3.1). Every count is derived from the exact list it describes: a refined
  `resourceCount` counts the refined list, and a document that shows the original list says so.
  A literal-dead row never serializes as `class: physical` without its status beside it.
- **Digest runs the pass too** (it runs only on prove's read today). It runs on the read's own
  copies, as now, never on memoized entries (`digest_cli.rs:1441` rule).
- The rollup serializes **one** `RollupTotals` (F15): the digest MCP summary (1b), the explain
  summary (1e), `cards[].summary` and `entries[].summary` all print that one struct. 1b's
  `physicalTablesWritten` today uses two classes (`digest_cli.rs:2560`, `tempState == known(true)`);
  it moves to the three classes. The 1b test
  `digest_summary_counts_and_names_without_the_unresolved_items` moves with it.
- `query` reads the L4 store, not the cone. It gets `QueryRollup` (§4.5) with the same vocabulary
  and the same classifier; it is a different substrate (§1 item 9), stated in its scope line.

```rust
pub struct RoutineRollup {
    pub routine: QualifiedRoutine,
    pub workspace_code: Vec<TableWrites>,          // where = workspace code (§3.6)
    pub entry_points: Vec<EntryPointView>,         // presentation over crossings (§3.3)
    pub no_path: Vec<NoPathWrite>,                 // own rows with no witness (§3.5)
    pub unresolved_table_writes: OpCounts,         // writes on a table not resolved
    pub caller_dependent: Vec<TableWrites>,        // class callerDependent (§3.2)
    pub temporary: TempCounts,                     // class temporary
    pub literal_dead: DeadCounts,                  // resources the pass removed
    pub commits: CommitView,                       // own sites + commit-bearing entry points
    pub events: EventCounts,                       // distinct event ids (§3.6)
    pub kinds: BTreeMap<KindFamily, Vec<Place>>,   // UI, HTTP, FILE, BACKGROUND_TASK, …
    pub gaps: GapCounts,                           // by where the call site is
    pub coverage: Coverage,                        // status, reasons, assumptions, limits (§3.7)
    pub totals: RollupTotals,                      // the one serialized summary
}
```

### 3.2 The write classifier (shared with prove)

One function, in `rollup.rs`:

```rust
pub enum WriteClass {
    Physical(PhysicalReason),        // "may persist"
    CallerDependent(CallerReason),   // "possibly persistent: depends on the caller"
    Temporary,                       // "known temporary"
}
pub enum PhysicalReason { KnownPersistent, TempStateUnknown, NotJoined }
pub enum CallerReason { Parameter(u32), ImplicitRec }
pub fn classify(states: &[TempState]) -> WriteClass;
```

Rules, applied to an own row's joined `tempState` list (the comma-joined
`detail.tempState`, parsed once into `TempState` values instead of by string, which prove does today
at `prove.rs:542 temp_states`):

| Joined states | Class | Matches prove today |
|---|---|---|
| any `known(false)` | `Physical(KnownPersistent)` | yes (`effect_matches`, `prove.rs:611`) |
| no `known(false)`, any `unknown` | `Physical(TempStateUnknown)` | yes |
| empty (no join: G31, `digest_cli.rs:1711`, and the no-path rows) | `Physical(NotJoined)` | yes (`writes_only_temporary` needs a non-empty list) |
| only `parameter-dependent(i)` and `known(true)`, at least one parameter | `CallerDependent` (`ImplicitRec` when every parameter state is the receiver's, `prove.rs:568`) | yes (`writes_caller_dependent`, `prove.rs:560`) |
| only `known(true)` | `Temporary` | yes (`writes_only_temporary`, `prove.rs:551`) |

Groups: a `kt` group row carries `tempState: known(true)` (`digest_cli.rs:1714`) → `Temporary`; an
`nt` group carries none → `Physical(NotJoined)` with the group's cone class as its reason text
("not known temporary in dependency code"). A group is never caller-dependent: a parameter of a
dependency routine is resolved in its caller's frame inside the cone (#69 D1). Mixed own row
examples (F1): `known(false),unknown` → physical (known persistent); `known(true),parameter-dependent(0)`
→ caller-dependent; `known(true)` alone → temporary.

Prove's `writes_only_temporary` and `writes_caller_dependent` become one-line calls of `classify`.
**Prove's verdict does not change** (a W2-1 check: every `cli-b-goldens/prove`, `cli-output-goldens`
prove file byte-stable, and the CDO prove answers in `output_cdo` unchanged). Any verdict change
that correcting crossing-local attribution causes (§3.3.1) is a separate, separately reviewed
correctness commit, never folded into the classifier refactor (astra 2).

`classify` answers one question only: **which class is this effect contribution in** (effect
classification). It does not answer "does R write T"; that is question evaluation (§3.2.1).
Revision 2 claimed the two coincide; they do not (astra 1).

#### 3.2.1 Question evaluation: `evaluate_writes_table` (shared, R3a)

Revision 2's "prove refinements that stay question-specific" were wrong in two places: a
field-guarded write on a table not resolved can make `writes-table:T` **yes**
(`evaluate_may_question` counts `field_guarded[i]` as a positive match, `prove.rs:653-675`), and an
absent match can be **unknown** because of ordinary obligations alone (unresolved calls, open-world
dispatch, analysis gaps, incomplete coverage, truncation; `prove.rs:693-748`). So the owner's choice
is to share the evaluation, not only the classifier:

```rust
pub struct TableAnswer {
    pub answer: Answer,                    // Yes | No | Unknown — prove's own enum
    pub matches: Vec<TableMatch>,          // why yes: one per evidence row
    pub blocked_by: Vec<Obligation>,       // why unknown: prove's obligations, unchanged
    pub assumptions: Vec<Assumption>,      // loaded-subscribers, closed-world
}
pub enum TableMatch {
    Named { row: RowRef, class: WriteClass },          // the row names T, physical class
    ViaCaller { row: RowRef, caller: RoutineRef },     // caller-dependent, resolved through a caller
    FieldGuarded { row: RowRef },                      // table not resolved, field guard matches T
}
pub fn evaluate_writes_table(r: &RoutineRollupInputs, table: &TableRef, world: World) -> TableAnswer;
```

It is prove's current `writes-table` logic moved, not rewritten: `evaluate_may_question`,
`field_guard`, the `viaCaller` resolution (`prove.rs:835-947`) and the obligation rules. Prove calls
it and prints its answer; explain and digest call it when they state anything table-specific
(`--table`, the cross-tool test) and print the same answer. Group rows' `unresolvedResources`
count as writes on a table not resolved (Fable N6): `writes_unknown_table` is true for them too.

Worlds (astra 6): `--closed-world` and `loaded-subscribers` do **not** change what the graph reaches
— the graph already follows every loaded implementation and subscriber. They change which
obligations are waived and which assumptions qualify the answer. Explain and digest have no world
switch; their scope line states "loaded program; unloaded implementations/subscribers are not
enumerated" (§3.7).

### 3.3 Crossings and entry points

**Crossing record**, built once per tree read beside `boundary_displays` (`digest.rs:381-395`, the
loop over every edge from a primary routine into a dependency routine) and attached to every group
row of that identity (revision 3: a **typed edge set**, astra 2; plural sites, Fable N3; as built in
W2-0a, §11):

```rust
pub struct Crossing {
    pub id: String,                    // today's BoundaryEnd.id: "cs:<callsite>", "op:<operation>", "ev:<event>"
    pub site: CrossingSite,            // Callsite(id) | Operation(id) | Event(id): the identity, typed
    pub edges: Vec<CrossingEdge>,      // the typed edge set (below), sorted
    pub targets: Vec<String>,          // the distinct dependency targets of `edges`, sorted
    pub sites: Vec<SiteAnchor>,        // the distinct source anchors of `edges` (several for `ev:`)
}
pub struct CrossingEdge {
    pub from: String,                  // the primary routine the edge leaves (stable id)
    pub to: String,                    // the dependency routine it enters (stable id)
    pub kind: &'static str,            // the snapshot edge kind ("direct-call", "event-dispatch", …)
    pub site: CrossingSite,            // the edge's own site identity
}
```

**Membership** is `(from, to, site)` in `edges`, never the bare site id. An interface dispatch, an
object run or an event can have several targets sharing one site id; when one target is a workspace
routine and another a dependency routine, only the dependency edge is in the crossing (the
workspace edge is not a crossing at all). `Crossing::contains(from, to, site)` is the one test.

**Event crossings** (Fable N3): the id is per event (`ev:<event id>`), so two workspace publish
sites of one event are one crossing with two `sites`, and every dependency subscriber is a target.
With one target the subscriber is the entry point; with several the crossing is shown as itself
(the multi-target form below). `unreachable_effects` never judges an `ev:` crossing dead (an event
edge has no guard); `site: Event(_)` makes that explicit.

**Prefix** (Fable N4, W2-0b): the shortest `via_paths[0]` over the crossing's rows, ties broken by
row order; T-W2-5 checks it ends at one of the crossing's sites.

Each group row has exactly one crossing (the group key is `B|{type}|{kt|nt}|{crossing id}`,
`digest.rs:4146`). Nothing the R4-F projections read changes.

#### 3.3.1 Crossing-local liveness (W2-0b/W2-1, astra 2)

Today's pass asks whether **any** matching direct fact is live **anywhere** in the root's reach
(`kind_dead(&reached, (kind, Some(resource)))`, `literal_reach.rs:844`). That is **root-level
liveness**: with `PostWith(true)`, `PostWith(false)` and a workspace write to the same table, the
table stays live for the `false` crossing too. Wave 2 adds **crossing-local liveness**:
- the crossing's **incoming binding states** are every `(state at the caller, callee bindings)` at
  which one of its edges is taken in the root's live tree (the prefix phase, §3.4.3);
- a crossing resource is `reachable` when the suffix search from those states reaches a live
  terminal record of it (§3.4.1, in the class of the row); `disproved` when the suffix search
  **completed** and reached none; `not established` when the search capped, was cancelled, or a
  terminal has no operation id;
- only `disproved` removes a resource from the refined membership (§3.1). A capped or incomplete
  search **never deletes** a resource; it is reported as a limit (`path-search-capped`).
Fixture: the mixed workspace/dependency dispatch sharing one site id (W2-0a), and `PostWith`
(below).

**Entry points are a presentation.** The rollup groups crossings by target routine id:
- a crossing with **one** target belongs to that entry point; an entry point entered at several
  sites lists each site, and its table set is the union over its crossings (each site keeps its own
  set under `--detail`);
- a crossing with **several** targets is shown as itself, unattributed:
  `Page.Run at <site> enters Page Email Account Wizard::OnInit, ::OnOpenPage, ::OnQueryClosePage`.
  Splitting its resources per target needs per-target attribution, which the cone does not keep;
  the design does not invent it.
- an event-dispatch crossing names the subscriber as the entry point and says `(event subscriber)`;
  an implicit-trigger crossing says `(trigger)`.

### 3.4 Witnesses and paths

#### 3.4.1 Terminal-fact index (read side)

Built once per read while the `CapabilitySnapshot` is available, inside `LiteralReach::new`
(which held the same set as a private `direct_by_kind` map; W2-0a replaced that map with this
index, §11): for every direct capability fact (`tree.capability_facts`) one **lossless terminal
record** (revision 3, astra 3):

```rust
pub struct TerminalRecord<'a> {
    pub fact: usize,                     // source fact identity: the index into capability_facts
    pub effect_type: &'static str,       // DB_INSERT, COMMIT, … (effect_type_of)
    pub resource: Option<&'a str>,       // the table / event; None for COMMIT, UI_*, HTTP, …
    pub owner: &'a str,                  // the fact's subject (stable routine id)
    pub terminal: Terminal<'a>,          // Operation(id) | Callsite(id) | None: the exact terminal
    pub op_kind: Option<&'a str>,        // the ORIGINAL operation kind: Insert, ModifyAll, Validate, …
    pub temp: TerminalTemp,              // Known(true/false) | Unknown | ParameterDependent(i) | Absent
}
// producer() == matches!(temp, ParameterDependent(_)): a raw producer, whose class at a caller
// comes from the producer solve, never from the record
```

Lookups (as built): by `(effect type, resource)` (what `kind_dead` reads) and by terminal id
(operation or call-site id); an owner lookup is added when a reader needs it. It
covers resource-less facts (COMMIT, UI_*, HTTP, FILE, …), which `group_resource_ops` cannot (it is
filled only for facts with a resource, `digest.rs:4176-4190`; F16d, A3).

**Joined in the root's frame** before any class count (astra 3): a record's `temp` is its class in
its **owner's** frame. A non-producer record keeps its class at every caller. A producer record's
class at the root comes from the producer solve (`pd_solution`'s states, the association
`enumerate_proofs` and `is_bfs_carrier` already use): a witness or count for a physical row may only
end at a record whose root-frame class is that row's class. A shortest-path search over all
`DB_INSERT` records for T must therefore never pick `TempT.Insert()` as evidence of a physical T
write. Literal reach alone does not supply caller-frame temporariness (it checks Boolean and table
guards), so the class join is a separate step, reusing the producer-proof association. Placement
("own row" vs "workspace code"): a record is "in workspace code" when its **owner** is a primary
routine **and** the witness to it does not traverse a crossing; a workspace operation reached only
after crossing into dependency code (re-entry) is placed under that crossing.

#### 3.4.2 The witness bundle and its outcomes

```rust
pub struct Witness {
    pub crossing: Option<String>,        // the crossing id it traverses (group witnesses)
    pub hops: Vec<Hop>,                  // edge kind, edge id (call site / op / event), from, to, line
    pub terminal: Terminal,              // operation id, owner routine, anchor
}
pub enum PathOutcome {
    Found(Witness),                      // zero hops (the operation is in the root) = Found(Witness { hops: [], terminal })
    OutsideEntryPoint(Witness, Reason),  // a real path that does NOT traverse the crossing (cap fallback;
                                         // its membership is checked, never assumed — §3.4.3 item 3)
    Unavailable(Reason),
}
pub enum Reason { SearchCap, Cancelled, NoOperationId, NoWitness, LiteralDead }
```

Revision 2's bare `Direct` outcome is gone (astra 3): it lost the terminal the schema and T-W2-5
need. A zero-hop witness is an ordinary `Found` with an empty hop list.

Rules:
- **Path and anchor come from one witness** (F13, A3): the representative of a table is chosen
  first (shortest witness, then earliest terminal anchor by file and line), and the line prints
  that witness's terminal anchor and that witness's hops. T-W2-5 checks that the last hop owns the
  printed anchor's operation.
- **Never invent a path.** An `Unavailable` outcome prints its reason (`no path kept: the witness
  walk found none`, `no operation id on the fact`, `search cap reached`).
- **Own rows** use the producer's `via_paths` (already pruned by the literal pass); their terminal is
  the row's own evidence anchor.
- **Resource-less group facts** (COMMIT, HTTP, …) get a terminal from the index: the search ends
  at a live state whose routine owns a direct fact of that type.
- **What the default prints** (A3; **approved**, §9 q2): one witness per physical-write table in
  workspace code; one per commit witness (own or group); for each crossing its prefix once plus
  **one** representative terminal witness, explicitly labelled as representative (to the nearest
  table the crossing writes); a crossing that carries a COMMIT gets its **own** COMMIT terminal
  witness besides. When several sites share an entry point, one site's witness never implies that
  every site reaches every listed resource (each site's set is its own). The digest JSON default
  carries one representative terminal witness **per may-persist group row** (Fable N12), so prove's
  `writes-table` and the digest cite the same witness for the same row; the crossing holds the
  prefix once. Per-table witnesses beyond a crossing (complete per-resource expansion) only under
  `--detail` (and in prove `writes-table:T`). The plan's "one path per physical write" is thereby
  narrowed for the 392 tables beyond entry points.

#### 3.4.3 G1: group witnesses must traverse their crossing (own commit, W2-0b)

Today `GroupPaths::paths` (`group_path.rs:62-89`) builds `live_tree(root)` and returns the first
state in root-BFS order that owns one of the resource's operations (`literal_reach.rs:742-755`). It
never requires the crossing, so a workspace route to the same operation wins (§1 item 11).

The fix (revision 3: two phases, Fable N2; typed membership, astra 2; event ids, Fable N1):
1. **Prefix phase.** Today's `live_tree(root)` (the pass pays for it already). For each of the
   crossing's typed edges (§3.3, matched by `(from, to, site)`; event edges by their `event_id`,
   which W2-0a now carries on `LiteralReach`'s event-dispatch adjacency) collect every
   `(state at the caller, callee bindings)` at which it is taken: `bind(caller, site, target, b)`
   for call kinds, `Bindings::default()` for object run, trigger and event (as `live_tree` does).
   **All** incoming states are kept: the shortest prefix is not necessarily one that witnesses
   every resource (astra 9).
2. **Suffix phase.** Per distinct `(target, bindings)`, one `live_tree`-shaped search starting from
   that state with those bindings, cached for the read (only the suffix is cached, never a witness
   holding another root's prefix). A witness is prefix + suffix; the shortest total wins, ties by
   prefix visit order. This is r1's restart at the entry point **with** bindings, which is sound:
   the suffix depends on nothing but `(target, bindings)`. For object-run, trigger and event
   crossings the bindings are always default, so there is one suffix tree per target per read (IR:
   at most 31). The terminal is the first suffix state whose routine owns a live terminal record
   of the resource in the row's class (`!dead_at(owner, op, b)` and the root-frame class join of
   §3.4.1); for a resource-less COMMIT witness, a live state owning a direct COMMIT record.
   Edge identity is kept: `GroupPaths.kind` is keyed by `(from, to)` today (`group_path.rs:79`),
   which cannot tell two calls of one routine apart; hops carry the edge id instead.
3. **Caps and the outcome contract (astra 9).** `STATE_CAP` applies to each suffix tree on its own
   (the prefix tree keeps the pass's existing cap). Every search returns `Completed`, `Capped` or
   `Unavailable`; `unreachable_effects`' silent "no pruning on cap" becomes a reported outcome so
   the always-visible limit (§3.7) can say it. The cache has **aggregate** limits (total states
   and an estimate of bytes across all suffix trees of one read), evicts deterministically (oldest
   insertion first, so output never depends on timing), and checks the request's cancellation
   flag between expansions. **Fallback per crossing:** when that crossing's suffix capped, the
   root tree's path to the resource (if any — the root tree can return none, then `Unavailable`)
   is checked for membership: if it does traverse one of the crossing's edges it is an ordinary
   `Found`; else `OutsideEntryPoint(w, SearchCap)`, JSON `throughEntryPoint: false, reason:
   "search-cap"`, human `(path does not pass Codeunit X::Y: search cap reached)`. It is never
   presented as evidence of "written beyond X". Tests: the dual-cap case (prefix and suffix both
   capped) and run-order independence (two orders of roots give byte-equal output). `path_live`
   replays hop guards only; it is not a terminal or class validator, so the tests check the
   terminal record and class separately.
4. **Prove JSON (Fable N11, R3d):** `groupPath` stays exactly as it is (strings `"<kind> →
   Object.Routine"`); prove gains, beside it and additively (prove 2.2.0), `crossing`,
   `throughEntryPoint` and `groupWitness: {hops: [{edge, edgeId, to, line, file}], terminal}`. The
   human `path:` line is rendered from `groupWitness`, with 1d's qualified names.
5. **Fixtures (adversarial).** W2-0a added the in-process versions (unit tests over temporary
   workspaces, `src/engine/l5/wave2_index_tests.rs`, §11), which move no golden; W2-0b adds the
   output shapes to `tests/fixtures/mcp-output/` (the `.app` re-packed with `write_source_app`;
   `fixture_app_matches_its_sources` keeps it honest) and re-mints `cli-output-goldens`:
   - `Base Poster` gains `PostWith(Flag: Boolean)` with `if Flag then L13.Insert();`;
   - the workspace calls `BasePoster.PostWith(true)` and `BasePoster.PostWith(false)` at two sites:
     two crossings to one entry point with different literal arguments. The `false` crossing's group
     must not list `Base Ledger 13` (literal-dead); the `true` crossing's witness must traverse the
     `true` site;
   - a workspace route to `Base Ledger 13` (a workspace routine writing it, called before the
     crossings) that is **shorter** in BFS order than the route through the crossing (F16h), so a
     root-started search finds it first and the break discriminates;
   - (revision 3) the **mixed dispatch**: an interface declared in the dependency, implemented by a
     dependency codeunit and by a workspace codeunit, called once from the workspace — one call-site
     id, two edges, one of them a crossing (astra 2);
   - (revision 3) one **event crossing**: a workspace routine calls a dependency publisher whose
     dependency subscriber writes a table, so T-W2-9 covers `ev:` crossings (Fable N1);
   - (revision 3) **temp vs persistent on one table**: a temporary and a persistent `Insert` on one
     table with the temporary route **shorter**, and one parameterized writer called with a
     temporary and with a persistent argument (astra 3); the physical row's witness must end at the
     persistent operation.
6. **CDO before/after** in the commit message: RR / `CDO E-Mail Log` evidence 3 (today: 10 hops, all
   workspace) and IR / `Issued Reminder Header`.

### 3.5 The operation index (owner, anchor, original kind)

One index, built from the model, shared by explain, digest, query and the no-path rows (A8):
`operation id → {owner routine (internal id), owner stable id, owner app, anchor, original op kind}`
and `call-site id → {owner routine, anchor}`. Ids have the form `{app hash}/{routine hash}/op{N}`
and `…/cs{N}` (`ir_walk.rs:1064`); the routine part is the model's **internal** routine id
(`compute_routine_id`, `scope.rs:161`), not the stable id, and never the row's `fact_subject` (which
is the root). Explain already joins operation ids to L4 rows (`explain.rs:518 op_row`); the index
replaces those private joins.

**A multimap, with the legacy selections kept (revision 3, astra 5, R3d).** Operation ids embed
the owning routine's internal id, so only collided rows (two declarations sharing one internal id)
share an id; the index keeps **every** claim, in model row order. Two existing joins pick one
claim, and each keeps its rule exactly:
- `query effects`' `origin` (`OperationJoin`, `effect_query_cli.rs`) takes the **last** claiming
  row (#160 step 11b §4.6, class P) — ConTinAI reads `origin` directly, so this never becomes null
  or changes for an ambiguous id;
- explain's `op_row` (`explain.rs`) takes the **first** claiming row.
Both were moved onto the index in W2-0a (§11) with their selections unchanged. New presentation
reads `lookup(op)`, which returns `Found(claim)`, `Ambiguous(claims)` or `Missing`, so ambiguity is
**exposed separately** and never hidden behind a pick.

Outcomes: `Found`, `Missing` (the op id is in no loaded body: symbol-only or unfetched dependency),
`Ambiguous(n)` (more than one routine claims it), `NoOperationId` (a fact without one). Workspace vs
dependency is the **owner's** app role, never the table's app and never the absence of an anchor.
When the index finds an anchor for a row whose witness has none, the **default** output shows that
owner and location (§4.1 section 13; astra 8) and still says `no path kept`.

The **original op kind** (`Insert`, `Modify`, `ModifyAll`, `Delete`, `DeleteAll`, `Rename`,
`Validate`, `LockTable`, reads) comes from the body fact / L4 op row. The digest's three write types
(`DB_INSERT | DB_MODIFY | DB_DELETE`, `prove.rs:450`) stay the digest's; human output prints the
original kind with one spelling everywhere (`DeleteAll`, never `DELETEALL`), and a fact with no op
row prints the digest type.

### 3.6 Count units and dedup keys

| Count | Unit | Dedup key |
|---|---|---|
| tables (any section, summary) | distinct tables | resource id (`<app>/table/<n>`) |
| summary "may write N tables" | union over workspace code, every crossing's non-temporary set and the no-path rows | resource id |
| "owned by this app / by dependencies" | the **table's** app (owner axis) | resource id |
| section placement ("in workspace code", "beyond entry points", "no path kept") | where the **operation** runs (where axis, by the owner routine's app) | — |
| operations per table and kind (workspace code) | distinct operation ids of terminal records the root reaches under literal arguments **and** whose root-frame class is the line's class (§3.4.1: reach and class join are two steps); when the literal search capped, the row's representative operations with "at least" and `limits: path-search-capped` (Fable N5) | operation id |
| per entry point | tables per kind ("DELETE on 7 tables"), never operations | resource id |
| commits | own: distinct COMMIT sites (operation id); groups: **commit-bearing entry points** (crossings), unless a terminal witness enumerates the sites | operation id / crossing id |
| events | distinct event ids; "in workspace code" = raised by an own row; "beyond entry points" = only in groups | event resource id |
| kind families (UI, HTTP, …) | places: own sites + crossings carrying the kind | operation id / crossing id |
| callers | routine callers (known), apart from page actions and page objects | routine id / action id |
| gaps | unresolved call sites, by where the call site is: workspace code, dependency code, or not placed (no anchor; owner from the call-site id via §3.5) | call-site id |
| temporary (revision 3, astra 8) | **all** temporary operations and the tables they touch, by where — a table also written persistently keeps its temporary work here | operation id / resource id |
| `temporaryOnlyTables` | a separate metric: tables with temporary contributions and no physical or caller-dependent one, computed from the temporary rows, not as a complement (Fable T-W2-19 note) | resource id |
| caller-dependent | **contributions** (rows) in class `callerDependent`; a table may also appear under physical when a different operation justifies it — what is forbidden is counting a caller-dependent contribution as physical (astra 8) | row / resource id |

Rules: a total is a union by identity, **never a sum** of section counts (T-W2-1). Each section header
gives its own distinct count. One table may sit in several classes when different contributions
justify each; a contribution sits in exactly one. The scope line says "counts are distinct tables
unless a line says operations; operation counts are direct operations in routines the literal search
reaches" (Fable N5: the `LiteralReach` graph omits `ObjectRunUnresolved` and has no depth bound,
unlike the cone's `maxDepth 64`). `origin_operation_ids` are the cone's representatives, one per kind per root
(`literal_reach.rs:818-821, 836-839`), so they are never counted as an inventory (A6).

LockTable is its own class: `locks N on M tables`; a table-grouped line lists `LockTable` only as an
extra kind on a table that is also written (F11). The cone has no lock fact (the digest effect types
are DB_INSERT/MODIFY/DELETE/READ and the non-DB kinds), so explain and digest have no lock line —
stated as a limit in their scope; query has one.

"RecordRef" is not printed until wave 3's receiver bit exists: an `isUnknownBucket` row is
"table not resolved (determined at run time or not resolved)" (A6, Q4).

### 3.7 Coverage, assumptions and limitations (always visible)

Every explain card, digest entry and prove answer prints three lines, never folded:
- `coverage:` status and reasons with counts, e.g. `partial — 267 interface calls open world, 21
  object runs not resolved` (from `entries[].coverage` and the unresolved counts).
- `assumptions:` prove's actual `assumptions` label (`loaded-subscribers`, `closed-world`) and its
  **effective** obligations; explain and digest print `loaded program; unloaded
  implementations/subscribers are not enumerated` (revision 3, astra 6: the graph already follows
  every loaded implementation and subscriber; a world changes which obligations are waived, not
  what is reached).
- `limits:` typed **analysis incompleteness** with counts, or `none`. Kinds (from explain's gaps,
  `explain.rs`, and the digest): `no-path-kept` (writes without a witness), `body-unavailable`,
  `symbol-only-dependency`, `unfetched-dependency`, `parse-incomplete`, `cone-truncated`
  (`unresolvedTraversal.truncated` or `coneTruncated`), `cross-call-rules-truncated`,
  `path-search-capped`, `crossing-status-not-established`.
- `notes:` (revision 3, astra 6) **informational warnings**, which never block an absence
  statement: `table-trigger-rules-skipped` (explanation only), `error-collector`,
  `text-differs-by-directive`, caption differences. Printed only when present.

Absence statements: "none" only when coverage is complete and `limits: none`; otherwise "no observed
…" (`commits: no observed COMMIT`, `HTTP: none observed`). **Prove's real answer is authoritative**
(R3e): `answer: no` stays `no`, followed by its effective coverage and assumptions; when obligations
prevent an absence proof prove already answers `unknown`, and the human line reads `answer: unknown
— no matching effect observed; coverage incomplete`; `yes` is never qualified (a witness is a
witness). Revision 2's `answer: no — none observed; coverage incomplete` is withdrawn: it described
a state in which prove answers `unknown`. Prove's JSON is unchanged here.

### 3.8 Rules every human output follows

1. **Order:** title, `summary:`, `coverage:`, `assumptions:`, `limits:`, `scope:`, sections, `next:`.
2. **Summary line**, in order: may-persist writes (tables, owner split, unresolved-table writes),
   caller-dependent, commits, events, UI, HTTP, file, background tasks, isolated storage,
   telemetry, errors, then — **explain only** — callers and tests (F16k), then gaps. Writes, commits,
   events and gaps always print (with "no observed" wording, §3.7); the other kinds print when
   non-zero. Numbers ≥ 1,000 get a thousands comma.
3. **Scope line:** "dependency code followed through every resolved call, with literal arguments,
   grouped per entry point; gaps and writes without a path counted above" (F16a, A4); "temporary
   records are not writes"; "counts are distinct tables unless a line says operations"; "lines are
   1-based"; the flag that shows everything. Query's own sentence: §4.5.
4. **Truncation** goes through one helper, `fold(items, cap, unit) -> Folded { shown, hidden, unit }`
   (F9). A folded list prints the full count with its unit before the colon and `… +N <unit>` after;
   the line names the flag. Caps: tables 12 (all when 15 or fewer) everywhere, one cap (F16b);
   fields 6; event publishers 6; captions 8; page objects 6; top callees 5.
5. **Locations:** `ws:<path>:<line>`, `dep:<app name>:<path>:<line>` (2g), 1-based, no `%20`. Two
   loaded apps with the same name print `dep:<name> (<first 8 of guid>):…`. A row without a position
   says why (`no path kept; in Codeunit X::Y at dep:…` when the operation index found the owner),
   never `(no anchor)`.
6. **Routine names:** qualified `<Type> <Object>::<Routine>`; in a path the object prints only when
   it changes. Two hops with one display but different stable ids print their signatures
   (`CreateAndSendMail(Rec)` / `CreateAndSendMail(Rec, Bool)`).
7. **Paths:** `path: <root> → <hop> → …`; tags `[run]`, `[event]`, `[trigger]`, `[interface]`; the
   rules of §3.4. `--max-paths N` prints up to N.
8. **Duplicates:** the rollup has no two records with one key (section, class, table or crossing,
   op kind). Text that happens to repeat (two tables written by one routine share a `path:`) is
   fine (F9).
9. **Order of items:** workspace code first, then by table count / operation count (descending),
   then by name; table names inside an entry-point line nearest-first (§4.1).
10. **Flags:** `--detail` (explain, digest), `--all` (query) remove every cap and fold.

---

## 4. Per output

### 4.1 explain, human

Sections:

| # | Section | Default | `--detail` adds |
|---|---|---|---|
| 1 | title line | | |
| 2 | `summary:` (§3.8 rule 2) | | |
| 3–6 | `coverage:`, `assumptions:`, `limits:`, `scope:` | | |
| 7 | `rules:` own and inherited (one line when both none) | | |
| 8 | `callers:` routine callers (qualified) with their caller rules; then page actions: one line each when 3 or fewer, else one line by caption and one by page object | | every action |
| 9 | `commits:` may / no observed; own sites with path; commit-bearing entry points with a terminal witness (§3.4) | | every commit witness up to `--max-paths` |
| 10 | `writes in workspace code (N tables, M operations):` one block per table: `<table>[ (<app> table)]: <Kind> n, … — <class note>` then `at <anchor>` and `path:` of the same witness, then the fields line (§4.1 rules) | | every operation's anchor |
| 11 | `caller-dependent writes (N tables):` "written only if the caller passes a record that is not temporary; prove names the caller" (absent when none) | | |
| 12 | `writes beyond dependency entry points (N tables through K entry points at S sites):` one line per entry point with may-persist writes: name, site(s), tables per kind, table names (cap 12), `also` other kinds, events; then the crossing's prefix and one representative witness | | every table name; per-table witnesses; per-site sets |
| 13 | `writes with no path kept (N tables, M operations):` per owner app: tables (cap 12), each with the owner routine and location the operation index recovered (revision 3, astra 8: default, as §3.5 promises; capped by the same table cap) | | every owner and location |
| 14 | `literal arguments rule out:` N tables listed by the cone but unreachable (absent when none) | | the names |
| 15 | `temporary records (not writes):` **all** temporary operations and tables, by where (workspace code: cap 7 names; beyond entry points: counts), then `temporaryOnlyTables` as its own count (revision 3, astra 8) | | every temporary row |
| 16 | `events:` distinct events raised in workspace code by publisher (cap 6), and "+N only beyond entry points" | | every raise site |
| 17 | `extension points:` events the routine itself publishes (never a group, 2a) | | |
| 18 | one line per present kind family (UI, HTTP, file, background tasks, isolated storage, telemetry, errors) | | |
| 19 | `tests:` | | |
| 20 | `gaps in workspace code (N):` one per line | | |
| 21 | `gaps in dependency code: N (P interface, D dynamic); not placed: K; top callees: …` | | every gap |
| 22 | `next:` | | |

Rules for the implementer:
- **2a:** a group row never prints with an empty name. `sourceKind: unavailable` rows go to section
  13. Groups never become extension points.
- **2b:** `Temporary` contributions only in section 15; `CallerDependent` contributions only in
  section 11; neither is counted in "may write". A table with a separate physical contribution is
  also in section 10 (revision 3: contribution-level, astra 8).
- **2d, fields (A6, B4):** by original op kind. `Insert`, `Modify`: the fields the owning routine
  assigns on that table, a routine-level fact (`ix.row_fields` unions field writes per routine and
  table — other record variables, later assignments, Validate and ModifyAll included), so the
  wording is exactly that (revision 3, astra 8): `fields CreateLogEntries assigns on this table (not
  operation-specific): E-Mail, Table No.`; when several owners write the table, one clause per
  owner. `ModifyAll` and `Validate`: the operation's own field argument (a per-op
  fact): `validates: Status (in X::Y)`. `Rename`: `renames (primary key)`. `Delete`, `DeleteAll`,
  `LockTable`: nothing. A fact with no op row prints no fields.
- **2e:** kind families: UI = UI_CONFIRM, UI_MESSAGE, UI_WINDOW_OPEN, UI_FILE_DIALOG, UI_HYPERLINK,
  UI_PROGRESS; errors = ERROR_THROW, ERROR_IMPLICIT and UI_ERROR beyond an entry point (the routine's
  own UI_ERROR checks stay under rules, as `prove reaches-ui` treats them).
- **Entry-point name order:** tables by call distance from the crossing's target to the nearest
  routine owning one of the table's live operations, then by name; one BFS per crossing over the
  same post-crossing search §3.4.3 caches.

**Mock-up: IR, the new card.** Real counts, names and sites from the branch run on CDO. Marked
values are not computable before W2 lands: `‹op counts›` (from the terminal-fact index; the numbers
shown are query's L4 operations minus those the cone rules out, §1 item 9), `‹path›` (the COMMIT's
terminal witness, from §3.4), `‹order›` (nearest-first table order, hand-picked from the source).
The workspace-code paths are today's shortest witnesses.

```text
Codeunit CDO Issue Document::IssueReminder  ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:11
  summary: may write 413 tables (11 owned by this app, 402 by dependencies) and 2 writes on a table not resolved; caller-dependent: none; commits: may (1 commit-bearing entry point); events: 7,435 (75 raised in workspace code); UI at 3 places; HTTP 2; file 2; background tasks 2; isolated storage 2; telemetry 2; errors 1; callers: none, reached from 1 page action; tests: none; gaps: 16 in workspace code, 265 in dependency code, 7 not placed
  coverage: partial — 267 interface calls open world, 21 object runs not resolved
  assumptions: loaded program; unloaded implementations/subscribers are not enumerated
  limits: no-path-kept 57 operations on 37 tables (owners named below)
  scope: dependency code followed through every resolved call, with literal arguments, grouped per entry point (26 entry points at 31 sites); temporary records are not writes; no lock facts (alsem query effects lists locks); counts are distinct tables unless a line says operations; lines are 1-based; --detail lists everything
  rules: none of its own; inherited: none
  callers: none; reached from 1 page action: "Issue and send/print" (IssueSendPrintAction) on Page CDO Reminder Actions at ws:src/DocumentTypes/Finance/Reminder/CDOReminderActions.Page.al:132
  commits: may — none in workspace code; 1 commit-bearing entry point:
    Codeunit Reminder-Issue::OnRun, entered at ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19
      path: IssueReminder → [run] Reminder-Issue::OnRun → ‹path› (terminal: the Commit's own anchor in dep:Base Application:…)
  writes in workspace code (12 tables and writes on a table not resolved, ‹op counts› operations):
    CDO E-Mail Log: Insert ‹3›, Modify ‹4› — Modify with temp state unknown
      at ws:src/Features/Logging/CDOLogManagement.Codeunit.al:392
      path: IssueReminder → CDO E-Mail Handler::DocHandle → RunReport → RunReportID → CDO Document Template Line::SendElectronicDocument → CDO E-Mail Template Management::CreateAndSendMail → CreateAndSendMail(…) → CDO Log Management::CreateSkipLogEntries → CreateLogEntries
      fields CreateLogEntries assigns: Insert E-Mail, Table No.
    CDO Log Attachment: DeleteAll ‹1›, Insert ‹1› — known persistent
      at ws:src/Features/Logging/CDOLogManagement.Codeunit.al:380
      path: IssueReminder → CDO E-Mail Handler::DocHandle → RunReport → RunReportID → CDO Document Template Line::SendElectronicDocument → CDO E-Mail Template Management::CreateAndSendMail → CreateAndSendMail(…) → CDO Log Management::CreateSkipLogEntries → CreateLogEntries
      fields MoveFilesToLogAttachment assigns: Insert Attachment, Entry No., File Name, Log Entry No.
    CDO File: Insert ‹2›, Modify ‹1› — temp state unknown (also temporary records in workspace code; section below)
      at ws:src/Features/EDocuments/CDOLegacyeDocDispatcher.Codeunit.al:319
      path: IssueReminder → CDO E-Mail Handler::DocHandle → RunReport → RunReportID → CDO Document Template Line::SendElectronicDocument → CDO E-Mail Template Management::CreateDocuments → CreateDocumentsForRecord → CDO Electronic Document Mgt.::SendElectronicDocument → CDO Legacy eDoc Dispatcher::GetFile
      fields GetFile assigns: Insert Entry No.; fields … assign: Modify File Blob, File Blob Password Protected
    CDO Template Recipient Setup: Insert ‹2› — known persistent
      at ws:src/Features/Recipients/CDORecipientMgt.Codeunit.al:73
      path: IssueReminder → CDO E-Mail Handler::DocHandle → RunReport → RunReportID → ValidateCombineGroupRecipients → GetResolvedRecipientKey → ResolveRecipients → CDO E-Mail Template Header::GetRecipients → CDO Recipient Mgt.::MoveRecipientFromHeaderToTable
      fields MoveRecipientFromHeaderToTable assigns: Insert E-Mail Address Field, E-Mail Recipients Field, E-Mail Template Code, Priority
    … 8 more tables in the same form (CDO Azure Application Setup, CDO Document Template Line, CDO E-Mail Template Header, CDO File Download Document, CDO Html Color, CDO Print document, CDO Queue Entry, Incoming Document Attachment (Base Application table))
    table not resolved (determined at run time or not resolved): Insert ‹1›, Modify ‹1› — at ws:src/Features/Email/Templates/CDODocumentTemplateLine.Table.al:1713
  writes beyond dependency entry points (392 tables through 15 entry points at 18 sites; names nearest first ‹order›):
    Codeunit Sales-Calc. Discount::OnRun at ws:src/Foundation/CDOFile.Table.al:125 — 313 tables (Insert on 210, Modify on 151, Delete on 130): Sales Header, Sales Line, … +311 tables; also file, background task, isolated storage, telemetry, UI_HYPERLINK, UI_PROGRESS; 6,892 events
      prefix: IssueReminder → CDO E-Mail Handler::DocHandle → … → CDO File::‹routine at :125›
      e.g. path: … → [run] Sales-Calc. Discount::OnRun → ‹nearest writer› (Sales Header)
    Codeunit Reminder-Issue::OnRun at ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19 — 54 tables (Insert on 32, Modify on 28, Delete on 7): Reminder Header, Reminder Line, Reminder Comment Line, Issued Reminder Header, Issued Reminder Line, Reminder/Fin. Charge Entry, Reminder Action Log, Cust. Ledger Entry, Detailed Cust. Ledg. Entry, Gen. Journal Line, VAT Entry, No. Series Line, … +42 tables; also COMMIT, errors; 299 events
      e.g. path: IssueReminder → [run] Reminder-Issue::OnRun → ‹nearest writer› (Reminder Header)
    Codeunit CTS-CDN Network Doc. Mgt.::CreateEDocument at ws:src/Features/EDocuments/CDOeDocumentsDispatcher.Codeunit.al:307 — 20 tables (Insert on 20, Delete on 1, Modify on 1): …; 87 events
    Codeunit CSC Core::IsAppActiveOrAskToActivate at 2 sites (ws:src/Features/Email/Sending/CDOEMailHandler.Codeunit.al:240: 16 tables; ws:src/Features/Email/Templates/CDOEMailTemplateManagement.Codeunit.al:480: 3 of them) — 16 tables (Delete on 14, Insert on 13, Modify on 4): CSC Subscription, …; also HTTP, telemetry, UI_CONFIRM, UI_MESSAGE, UI_WINDOW_OPEN; 37 events
    Table Incoming Document Attachment::OnInsert (trigger) at ws:src/Features/EDocuments/CDOElectronicDocumentMgt.Codeunit.al:182 — 5 tables (Delete on 3, Modify on 2): …; 8 events
    … 10 more entry points with 1–4 tables each, one line each (as above)
    11 entry points at 13 sites write only temporary records or raise only events, among them the three-target crossing Page.Run at ws:src/Features/Email/Sending/CDOSendMailManagement.Codeunit.al:242 → Page Email Account Wizard::OnInit, ::OnOpenPage, ::OnQueryClosePage (--detail lists them)
  writes with no path kept (37 tables, 47 operations; the witness walk found no path from IssueReminder — tracked in the no-path issue):
    in Base Application code (27 tables): Activity Log, Attachment, Data Exch., Deferral Posting Buffer, IC Outbox Transaction, IC Partner, Interaction Log Entry, Job Planning Line Archive, Job Queue Entry, Job Task Archive, My Notifications, Notification Entry, … +15 tables
    in System Application code (5 tables): Email Outbox, Email Rate Limit, Feature Data Update Status, Guided Experience Item, Sent Email
    in Continia Delivery Network code (5 tables): CTS-CDN Allowance/Charge, CTS-CDN eBilling Line, CTS-CDN eOrder Header, CTS-CDN eOrder Line, CTS-CDN eReminder Header
  temporary records (not writes): 7 tables only temporary in workspace code (CDO E-Mail Signature, CDO Temp Blob, CDO Template Line Attachment, CDO Template Variant Entry, Error Message, …) and 81 only beyond entry points
  events: 75 raised in workspace code, by publisher: CDO E-Mail Template Management 19, CDO File 12, CDO Contnia Online PDF Mgt. 5, CDO Payment Link Handler 4, CDO E-Mail Handler 4, CDO Universign eSeal Service 4, … +14 publishers; 7,360 more only beyond entry points
  extension points: none (the routine publishes no event itself)
  UI at 3 places: UI_FILE_DIALOG at ws:src/Features/Email/Sending/CDOEMailHandler.Codeunit.al:1288 (conditional); UI_CONFIRM, UI_MESSAGE, UI_WINDOW_OPEN beyond Codeunit CSC Core::IsAppActiveOrAskToActivate; UI_HYPERLINK, UI_PROGRESS beyond Codeunit Sales-Calc. Discount::OnRun
  HTTP 2: WebClient.Get at ws:src/Foundation/CDOHttpManagement.Codeunit.al:173; beyond Codeunit CSC Core::IsAppActiveOrAskToActivate
  file 2: PdfTempBlob.CreateOutStream at ws:src/Features/EDocuments/CDOeDocumentsDispatcher.Codeunit.al:286; beyond Codeunit Sales-Calc. Discount::OnRun
  background tasks 2: beyond Codeunit CSC Core::UploadUsageAsync; beyond Codeunit Sales-Calc. Discount::OnRun
  isolated storage 2: IsolatedStorage.Contains at ws:src/Features/ESeal/CDOeSealSetup.Table.al:220; beyond Codeunit Sales-Calc. Discount::OnRun
  telemetry 2: beyond Codeunit CSC Core::IsAppActiveOrAskToActivate; beyond Codeunit Sales-Calc. Discount::OnRun
  errors 1 place: ERROR_THROW, ERROR_IMPLICIT, UI_ERROR beyond Codeunit Reminder-Issue::OnRun (the routine's own checks are under rules)
  tests: none reach it
  gaps in workspace code (16 calls the engine cannot follow): … (the 16 lines of r1's mock-up, unchanged)
  gaps in dependency code: 265 (251 interface, 14 dynamic); not placed: 7 (owners named with --detail); top callees: CurrLineWithPrice.SetLine ×21, Codeunit.Run(<dynamic>) ×11, CurrLineWithPrice.GetTableNo ×8, CurrLineWithPrice.GetLine ×8, PriceCalculation.ApplyPrice ×7
  next: alsem prove <routine> writes-table:<table> for one table with its witness; may-commit for the commit; alsem query effects for every operation one routine deep
```

About 75 lines and 16 KB in full (estimate from the mock-up; before 558 lines, 80,769 B). The
section "literal arguments rule out" is absent: on CDO the pass is not run for explain today, so
the count is not known yet (W2-1 measures it).

**RR, header and callers** (real data):

```text
Codeunit CDO E-Mail Handler::RunReportID  ws:src/Features/Email/Sending/CDOEMailHandler.Codeunit.al:283
  summary: may write 406 tables (11 owned by this app, 395 by dependencies) and 2 writes on a table not resolved; caller-dependent: none; commits: may (1 site); events: 7,393 (72 raised in workspace code); …; callers: 2, reached from 647 page actions on 121 pages; tests: none; gaps: 16 in workspace code, 265 in dependency code, 7 not placed
  …
  callers: 2 — Codeunit CDO E-Mail Handler::RunReport, ::RunReportSelection
    caller rule (applied by ::RunReport; skipped by ::RunReportSelection): rejects when not VariantRec.IsRecord: "You can not call this function with this variable type."  [Error ws:src/Features/Email/Sending/CDOEMailHandler.Codeunit.al:238]
    reached from 647 page actions on 121 pages and page extensions, all through ::RunReport; by caption: Open Email 114, Print PDF 109, Save PDF 109, Queue Email 93, Send/Print 86, Send eDocument 20, Print/Email 20, Post and send/print 18, … +12 captions
    most actions: Page CDO Sales Invoice Actions 11, Page CDO Sales Order Actions 11, PageExtension CDOSalesOrderExt 10, PageExtension CDOSalesOrderListExt 10, PageExtension CDOSalesInvoiceExt 10, PageExtension CDOSalesInvoiceListExt 10, … +115 page objects (--detail lists every action)
  commits: may — 1 site
    COMMIT at ws:src/Features/Email/Templates/CDOEMailTemplateManagement.Codeunit.al:1241 (guarded by IsHandled)
      path: RunReportID → CDO Document Template Line::Print → CDO E-Mail Template Management::PrintDoc → PrintDocuments → PrintDocuments(…)
```

### 4.2 explain, JSON (`al-sem.explain/3`)

Major bump from the branch's `/2`: group rows change shape and lists fold. **No gratuitous renames**
(F14): `fields` keeps its name, narrowed to the §4.1 field rule; `via` keeps its name and meaning
(display hops of the representative witness). Additions:

```jsonc
{
  "schema": "al-sem.explain/3", "lineBase": 1, "mode": "routine", "selector": "…", "diagnostics": [],
  "cards": [{
    "routine": { /* unchanged */ },
    "summary": { /* RollupTotals, the one serialization (§3.1) */ },
    "coverage": {"status": "partial", "reasons": ["interface-open-world", "object-run-unresolved"],
                 "assumptions": [], "limits": [{"kind": "no-path-kept", "operations": 57, "tables": 37}]},
    "rules": { /* unchanged */ },
    "effects": {
      "writes": [
        { "op": "DB_INSERT", "opKinds": {"Insert": 3}, "table": "CDO E-Mail Log", "tableId": "f4b69b55-…/table/6175273",
          "class": "physical", "classReason": "known-persistent", "tempState": "known(false)",
          "fields": {"CreateLogEntries": ["E-Mail", "Table No."]},
          "via": ["CDO E-Mail Handler::DocHandle", "…"], "anchor": { /* the same witness's terminal */ },
          "paths": [{"hops": [/* §4.4.2 hop */], "terminal": {"operationId": "…", "file": "ws:…", "line": 392}}] },
        { "op": "DB_INSERT", "crossing": "c:3f1a…", "class": "physical", "classReason": "not-joined",
          "resourceCount": 32, "resources": [{"id": "437dbf0e-…/table/297", "display": "Issued Reminder Header"}, "…"],
          "anchor": { /* the crossing's site */ } },
        { "op": "DB_MODIFY", "table": "Activity Log", "tableId": "…", "class": "physical", "classReason": "not-joined",
          "noPathKept": true, "owner": {"display": "Codeunit … ::…", "stableId": "…", "outcome": "found", "anchor": {…}} }
      ],
      "crossings": [ {"key": "c:3f1a…", "id": "cs:…", "kind": "objectRun", "targets": ["Codeunit Reminder-Issue::OnRun"],
                      "targetIds": ["437dbf0e-…:Codeunit:393#…"], "site": {…}, "prefix": {"hops": []}} ],
      "commitSites": [ {"conditionality": "…", "crossing": "c:3f1a…", "anchor": {…}, "paths": [ … ], "pathOutcome": "found"} ],
      "events": [ /* own raise rows; group rows as {crossing, resourceCount, resourcesListed: false} */ ],
      "ui": [], "http": [], "file": [], "backgroundTasks": [], "isolatedStorage": [], "telemetry": [], "errors": []
    },
    "extensionPoints": [ /* groups excluded */ ],
    "reachedFrom": [ /* every action; "via" and "tooltip" only with --detail */ ],
    "reachedFromSummary": {"actions": 647, "objects": 121, "byCaption": [ … ]},
    "tests": [],
    "gaps": [ /* workspace gaps; dependency gaps only with --detail */ ],
    "gapsInDependencies": {"total": 265, "polymorphic": 251, "dynamicTarget": 14, "notPlaced": 7, "topCallees": [ … ]}
  }]
}
```

Group rows lose `table: ""` / `tableId: ""`. Folded by default, back with `--detail`: dependency
gaps, `reachedFrom[].via`/`tooltip`, **event-group names** (O2: explain folds them;
`resourcesListed: false` and the count), per-table witnesses beyond crossings. Every row keeps its
`tempState`. Size: estimate IR ~270 KB, RR ~580 KB; both are files in MCP with the 1e summary.
Readers: the plan's §4.1 (`1ebd5103`, a read-only check of every repo on the owner's account)
found that the editor extension (al-lsp-for-agents) uses only the call-hierarchy and property
requests, and no repo uses the `al-call-hierarchy/{explain,…}` engine requests yet. So `/3` has no
known outside reader and needs no compatibility window; W2-2's commit message repeats the check
against the then-current extension source.

### 4.3 digest, human

The explain card's effect sections (9–18, 20–21) for each root, rendered by **the same function**
(`render_effect_sections(&RoutineRollup, opts)`), plus the digest's own lines: changed-roots
diagnostics (unchanged), `coverage:` / `assumptions:` / `limits:`, and `diagnostics:` as counts by
kind and reason (`evidence-unavailable 57; temp-state-incoherent 61 (no-l4-row 61)`; op ids with
`--detail`). No rules, callers or tests (the `--tests` block stays as today). IR about 70 lines,
~14 KB (estimate; before 95,870 B).

### 4.4 digest, JSON (schema 3.0.0)

Branch: 2.1.0 (1a → 2.0.0, 1d → 2.1.0). Wave 2 removes fields from the default: **3.0.0**.
`--schema 2` renders the 2.1.0 document (§5).

#### 4.4.1 The default document

```jsonc
{
  "schemaVersion": "3.0.0", "kind": "digest", "alsemVersion": "…", "deterministic": true,
  "diagnostics": [], "generatedAt": "…",
  "payload": {
    "lineBase": 1, "changed": {…}, "changedRootsDiagnostics": [], "dependencies": {…},
    "summary": {…},
    "routines": {                                  // every routine a hop or a target names
      "r:5b0c9e31a2f4": {"stableId": "f4b69b55-…:Codeunit:6175303#9d6f…", "display": "Codeunit CDO E-Mail Handler::DocHandle",
                         "signature": "DocHandle(var Rec)", "file": "ws:src/Features/Email/Sending/CDOEMailHandler.Codeunit.al"}
    },
    "entries": [{
      "routine": { /* unchanged: anchor and anchors (bc-dev-mcp reads them) */ },
      "coverage": {"status": "partial", "reasons": [ … ], "assumptions": [], "limits": [ … ]},
      "summary": { /* RollupTotals */ },
      "crossings": {
        "c:3f1a…": {"id": "cs:…", "kind": "objectRun", "targets": ["r:…"],
                    "site": {"file": "ws:…/CDOIssueDocument.Codeunit.al", "line": 19, "column": 8},
                    "prefix": {"hops": [ … ]}}
      },
      "effects": [                                 // EVERY effect row
        { "factId": "aa70fd66e09a9913", "type": "DB_INSERT", "provenance": "transitive", "confidence": "static",
          "conditionality": "unknown", "transactionContext": "unknown", "guarantees": [],
          "detail": {"resourceDisplay": "Incoming Document Attachment", "resourceId": "437dbf0e-…/table/133", "tempState": "known(false)"},
          "class": "physical",
          "evidence": {"file": "ws:…/CDOElectronicDocumentMgt.Codeunit.al", "line": 182, "column": 12, "excerpt": "IncomingDocumentAttachment.Insert", "sourceKind": "source"},
          "paths": [{"hops": [{"edge": "variable-typed-call", "to": "r:5b0c9e31a2f4", "line": 23}, "…"],
                     "terminal": {"operationId": "…", "file": "ws:…", "line": 182}}],
          "pathsKept": 3, "pathsOmitted": 2, "pathsProducerTruncated": true },
        { "factId": "44b466b874cd339c", "type": "DB_INSERT", "crossing": "c:3f1a…", "class": "physical",
          "provenance": "transitive", "confidence": "static", "conditionality": "unknown", "transactionContext": "unknown",
          "guarantees": ["COMMIT_REACHABLE"], "detail": {"resourceCount": "32"},
          "resources": [{"id": "437dbf0e-…/table/297", "display": "Issued Reminder Header"}, "…"] },
        { "factId": "…", "type": "EVENT_PUBLISH", "crossing": "c:77d0…", "detail": {"resourceCount": "6892"},
          "eventsByPublisher": [{"publisher": "437dbf0e-…/Codeunit/101", "publisherDisplay": "Codeunit Cust. Entry-Edit",
                                 "events": ["onafterupdatecustledgerentry", "onbeforesetapplid", "…"]}, "…"] }
      ],
      "scopedGuaranteesFolded": 144,
      "unresolved": [ /* workspace call sites and not-placed ones (with owner), candidates → candidateCount */ ],
      "unresolvedInDependencies": {"total": 265, "polymorphic": 251, "dynamicTarget": 14, "topCallees": [ … 10 ]},
      "entryDiagnosticCounts": {"evidence-unavailable": 57, "temp-state-incoherent": {"no-l4-row": 61}},
      "unresolvedTraversal": {"maxDepth": 64, "truncated": false, "visitedRoutines": 20863}
    }]
  }
}
```

#### 4.4.2 The changes, each with its reason

| Change | Default | `--detail` | Why |
|---|---|---|---|
| `viaPaths` (≤ 3 paths of hop objects) → `paths: [{hops, terminal}]` | 1 path on decision effects (may-persist writes, COMMIT, HTTP, FILE, BACKGROUND_TASK, ISOLATED_STORAGE, TELEMETRY, UI_*, ERROR_*); 0 on events, reads, temporary rows | up to 3 (all kept) | D3 |
| `pathsKept`, `pathsOmitted`, `pathsProducerTruncated` | `pathsKept` = paths the producer kept; `pathsOmitted` = kept − shown (render cut); `pathsProducerTruncated` = today's `viaPathsTruncated` | same | A5: the two truncations are different facts |
| hop = `{edge, to, line}` | `to` is a `payload.routines` key; `line` is the call site's line in the **calling** routine's file (the previous hop's routine, or the root), whose `file` is in the routine table (F16f, A5) | + `edgeId` | small structured hops, no string to parse |
| `payload.routines` keys | `"r:" + first 12 hex of sha256(stable id)`; on a collision the prefix of **every** colliding key extends by 4 hex, **repeated until all keys are unique** (revision 3, astra 5); `display`, `signature`, `file` are data | same | A5, F7: overloads share a display |
| hop file provenance (revision 3, astra 5) | a stable id can have several declarations (collided declarations; digest carries plural `routine.anchors`), so `payload.routines[k].file` is the routine's **default** file and a hop carries `file` whenever its call site's source unit differs from that default | same | a hop's line is always locatable |
| group row path (Fable N12) | one representative terminal witness per may-persist group row (`paths[0]`, labelled `representative: true`); the prefix lives once on the crossing | every per-resource witness | prove and the digest cite the same witness for the same row |
| contribution status (revision 3, R3c) | `status` (`live`, `literal-dead` or `not-established`) on every row; a group's `resources` is the refined list and `resourceCount` counts it; `resourcesOriginalCount` when the refined list is shorter | same | astra 4: a count describes its own list |
| group rows: `evidence` ("enters X at site") → `crossing` key | the site, targets and prefix live once in `entries[].crossings` | same | 31 crossings carried 108 copies of the same prefix |
| group `resources` (tables) | every resource, `{id, display}` | same | D4, A5 |
| group events | every event (O2), grouped by publisher: `eventsByPublisher[{publisher, publisherDisplay, events[]}]`; the event id is `<publisher>/event/<name>`, so the list is lossless; `resourceCount` == number of names | same | O2; half the bytes of `{id, display}` (§4.4.3) |
| `detail.resources` (the first-ten comma string kept by 1d "one more minor") | dropped | dropped | 1d's stated end |
| `class` on DB rows | new | same | §3.2 |
| `scopedGuarantees` | only non-trivial entries; **trivial** = `scope == owning-routine && interveningBoundary == none && !validForRefutation && commitEffectiveness` absent (revision 3, Fable N8); `scopedGuaranteesFolded` per entry | every entry | O3, A4 |
| `entryDiagnostics` | counts by kind and reason | the list | D3 |
| `unresolved` | workspace and not-placed items without `candidates` (→ `candidateCount`); dependency items as counts + top 10 callees | full list | 270 KB |
| `entries[].summary`, `crossings`, `coverage.assumptions/limits`, `payload.routines` | new | same | D2, A2, A4 |

Kept exactly: every effect row (multiplicity included — `factId` is not unique, §1 item 5), `factId`,
`type`, `detail` with `tempState`, `provenance`, `confidence`, `conditionality`,
`transactionContext`, `guarantees`, own rows' `evidence`; `entries[].routine` (anchor and anchors);
`coverage.status/reasons`; `payload.dependencies`; `changed*`. The path cap is a **render-time** cut;
the engine still keeps up to 3 (`HARD_PATH_CAP`), so the R4-F projections do not change.

#### 4.4.3 Size, measured

Built from the real 2.1.0 documents by a script that applies §4.4.2 (sorted keys, 2-space indent,
the repo serializer's shape; the summary is a 1.5 KB placeholder):

| | 2.1.0 today | 3.0.0 core (event names left out) | 3.0.0, events by publisher (proposed) | 3.0.0, events as `{id, display}` (1d shape) |
|---|---|---|---|---|
| IR pretty | 7,339,040 B | 468,641 B (13,110 lines) | **980,618 B** (23,467 lines) | 2,041,015 B |
| IR compact | — | 266,534 B | 589,018 B | 1,352,266 B |
| RR pretty | 4,952,155 B | 422,771 B | **928,650 B** | 1,986,356 B |
| RR compact | — | 243,854 B | 562,984 B | 1,323,476 B |

Where IR's core goes (approximate, measured at top-level indent): own rows 130 KB (own events 55 KB,
own temporary rows 14 KB, paths 22 KB, evidence 21 KB), group rows 166 KB (table resources with ids
113 KB), crossings 38 KB, routine table 19 KB, workspace unresolved 8 KB, the rest 24 KB. r1's
307 KB left out table ids, used string hops and had no crossing records or terminals; both reviews
require those (A5, A2/F2). Every event name (O2) adds ~510 KB even grouped by publisher.

**The owner's 300–320 KB target is not reachable** with O2 and the identity fields. **Settled
(R3j, §9 q1):** provisional budgets whole document ≤ 1,000,000 bytes pretty for IR and RR, and the
core (the document without `eventsByPublisher`) ≤ 480,000 bytes pretty, both per routine, pinned to
the CDO input (§8.3 T-W2-15), **remeasured once the real indexes, witnesses and summaries exist**
(W2-2) and set from that measurement. Every event name is kept, grouped losslessly by publisher,
and a test reconstructs every event id exactly. The sizing script is kept; its numbers are not a
lower bound.
`--compact` (opt-in, no indentation) gives 589 / 563 KB. The MCP digest is always a file
(`always_file`, `src/mcp/tools.rs`) with the ≤ 8 KiB summary, so the size never reaches the
conversation.

### 4.5 query effects, human and JSON

Human, default (IR; numbers real from the branch run; `‹hop›` marks first hops computed by the
membership rule below, hand-checked against the source):

```text
--- effects of Codeunit CDO Issue Document::IssueReminder ---
  summary: 652 operations — writes that may persist 30 on 15 tables (27 known persistent, 3 temp state unknown); locks 3 on 3 tables; reads 258 on 71 tables; temporary records 292 on 12 tables in 87 routines; table not resolved 69 in 30 routines
  coverage: one routine into dependency code; operations ruled out by literal arguments are listed (alsem explain and prove drop them); lines are 1-based
  scope: per operation, one routine into dependency code (alsem explain and prove follow the whole cone); temporary records are not writes; dependencies: 11 apps (--all lists them)
  writes that may persist (15 tables):
    CDO E-Mail Log: Insert 3, Modify 4 (+ LockTable 1) — first Insert at ws:src/Features/Logging/CDOLogManagement.Codeunit.al:392 in Codeunit CDO Log Management::CreateLogEntries, through CDO E-Mail Handler::DocHandle
    CDO Log Attachment: Modify 3, DeleteAll 1, Delete 1, Insert 1 — first DeleteAll at ws:src/Features/Logging/CDOLogManagement.Codeunit.al:380 in Codeunit CDO Log Management::CreateLogEntries, through CDO E-Mail Handler::DocHandle
    CDO File: Insert 2, Modify 1 (temp state unknown) — first Insert at ws:src/Features/EDocuments/CDOLegacyeDocDispatcher.Codeunit.al:319 in Codeunit CDO Legacy eDoc Dispatcher::GetFile, through CDO E-Mail Handler::DocHandle
    Reminder Header (Base Application): Delete 1, Modify 1 — first Delete at dep:Base Application:src/Sales/Reminder/ReminderIssue.Codeunit.al:183 in Codeunit Reminder-Issue::OnRun, through Reminder-Issue::OnRun ‹hop›
    … 11 more tables, one line each
  locks (3 tables): CDO E-Mail Log, CDO Azure Application Setup (both also written), Reminder/Fin. Charge Entry (Base Application) at dep:Base Application:src/Sales/Reminder/ReminderIssue.Codeunit.al:‹line› in Codeunit Reminder-Issue::‹routine›; table not resolved: 5 more
  reads: 258 on 71 tables (Get 104, Next 38, CalcFields 37, FindSet 33, FindFirst 29, FindLast 11, IsEmpty 4, Count 2) — --all lists them
  temporary records: 292 operations on 12 tables in 87 routines — --all lists them
  table not resolved (determined at run time or not resolved): 69 operations in 30 routines (FindSet 18, Next 18, FindFirst 7, Find 5, LockTable 5, Modify 5, Count 3, Get 3, IsEmpty 3, FindLast 1, Insert 1) — --all lists them
  next: alsem prove <routine> writes-table:<table> follows the whole cone for one table
```

The `CDO Log Attachment: Modify 3` line is real: query lists those operations although the cone
rules them out (§1 item 9). The coverage line says so; wave 3 decides whether query should apply the
same literal pass (§7).

Rules:
- **Buckets** (exclusive, in this precedence, F10): table not resolved (`isUnknownBucket`) first;
  then the write class of the row's `tempState` (§3.2: temporary; caller-dependent; physical); then
  the op family: lock (`LockTable`), write (`Insert`, `Modify`, `ModifyAll`, `Delete`, `DeleteAll`,
  `Rename`), **validate** (`Validate`, its own bucket — it runs the field's trigger and may write,
  so it is never a read), read (`Get`, `Find*`, `Next`, `IsEmpty`, `Count*`, `CalcFields`,
  `CalcSums`, named), and **unclassified** (any other kind, listed by name; revision 3, astra 8).
  The summary's buckets sum to `effectCount` (T-W2-11).
- **`via`:** JSON `via` unchanged (ConTinAI `effects.py` filters on `via == "direct"`). Human, per
  `ViaRank` (`effect_store.rs:116-122`): `Direct` prints nothing; `Inherited` prints `through <first
  hop>`; `Dynamic` `through <first hop> (dynamic dispatch)`; `EventSubscriber` `(event subscriber)`;
  `ImplicitTrigger` `(table trigger)`.
- **First hop must be a real association** (A7; revision 3, astra 7, R3g): operation-id
  membership is not enough, because one operation can sit in several callee summaries with
  different table/temp instantiations (a parameterized writer called with a temporary and with a
  persistent record). The first hop is the first direct callee `c` of the root, in sorted order,
  whose summary holds a row that equals **this row's complete effect contribution after the
  edge's argument/temp transfer** (operation id, table, op kind and the temp state the row has at
  the root). Membership is decompressed once per direct callee into a set of contribution keys,
  not once per row (Fable N10). If no callee matches, JSON `firstHop: null, firstHopUnavailable:
  "<reason>"` and human prints no `through`; or, where a graph route to the operation's owner is
  known, human prints it labelled `example graph route to the operation owner: …` and JSON
  carries it as `exampleRoute`, never as `firstHop`.
- **Q5/2g:** readable `dep:` anchors (§3.8 rule 5). **Q6:** the `dependencies:` header moves into
  the scope line; the full list into the MCP envelope's `universe` block and `--all`; the LockTable
  row prints the operation's own anchor, never the routine's.

JSON (`query.effects`, schema **3.0.0**, shared with touches, §4.6):

```jsonc
{ "schemaVersion": "3.0.0", "kind": "query.effects", "payload": {
    "lineBase": 1, "dependencies": {…}, "routine": {…}, "effectCount": 652,
    "summary": {
      "writes": {"operations": 30, "tables": 15, "knownPersistent": 27, "tempStateUnknown": 3},
      "callerDependent": {"operations": 0, "tables": 0},
      "locks": {"operations": 3, "tables": 3},
      "reads": {"operations": 258, "tables": 71},
      "temporary": {"operations": 292, "tables": 12, "routines": 87},
      "tableNotResolved": {"operations": 69, "routines": 30},
      "byTable": [{"table": "CDO E-Mail Log", "tableId": "…", "app": "…",
                   "ops": {"Insert": 3, "Modify": 4, "LockTable": 1}, "class": "physical",
                   "first": {"operationId": "…"}, "beyond": []}],      // beyond: wave 3
      "scope": "operations-one-routine-deep"
    },
    "effects": [ /* EVERY row, fields unchanged + "firstHop" (or "firstHopUnavailable") on non-direct rows */ ]
}}
```

Every existing row field stays (`op`, `operationId`, `origin`, `tableId`, `tableName`, `tempState`,
`via`, `isUnknownBucket`). Size: IR 670 KB → ~690 KB (estimate). A file in MCP with the 1e summary.

### 4.6 query touches, human and JSON

**The computation changes, not only the output** (A7). Today `ancestors_touching`
(`effect_query.rs:315`) enumerates every ancestor's witnesses and the renderer builds rows and lines
from them; for RR vs `CDO File` that is 883 callers × 204 witnesses (r1: 257 MB JSON).
- **Trivial case** (the root touches the table: `touches_table`, `effect_query.rs:194`): branch
  before any witness expansion. For each transitive caller one **has-summary-row** probe first
  (revision 3, astra 7: the existing membership boolean cannot tell "no touch" from "no row"), then
  one membership probe; outcomes touches / does not / no row (rowless: unknown). No ancestor witness
  is built.
- **Informative case** (the root does not): per caller, stream its witnesses and keep `{first
  witness, count}`; never build the full product.
- `--all`: today's full witness lists.
- **Any record touch vs persistent** (O4): `down` counts persistent (`known(false)`), possibly
  persistent (temp state unknown; caller-dependent with the parameter), and temporary-record touches.
  Temporary touches are shown, counted and listed under `--all`: they persist nothing, but they
  depend on the table's schema and run its triggers and table-event subscribers. Operations on a
  table not resolved are counted apart ("may be this table").

Human (IR vs `CDO File`; real counts):

```text
--- Codeunit CDO Issue Document::IssueReminder vs table CDO File (workspace) ---
  scope: per operation, one routine into dependency code; lines are 1-based
  down: yes — 204 record touches: 1 known persistent, 4 possibly persistent (temp state unknown), 199 on temporary records (they persist nothing but use the table's schema and run its triggers)
    [CalcFields] read at ws:src/Foundation/CDOFile.Table.al:704 in Table CDO File::CreateZip, through CDO E-Mail Handler::DocHandle
    [CalcFields] read at ws:src/Features/ESeal/CDOUniversigneSealService.Codeunit.al:122 in Codeunit CDO Universign eSeal Service::SendSealRequest (temp state unknown), through CDO E-Mail Handler::DocHandle
    [Insert] write at ws:src/Features/EDocuments/CDOLegacyeDocDispatcher.Codeunit.al:319 in Codeunit CDO Legacy eDoc Dispatcher::GetFile (temp state unknown), through CDO E-Mail Handler::DocHandle
    [Insert] write at ws:src/Features/EDocuments/CDOeDocumentsDispatcher.Codeunit.al:57 in Codeunit CDO eDocuments Dispatcher::GetFile (temp state unknown), through CDO E-Mail Handler::DocHandle
    [Modify] write at ws:src/Features/ESeal/CDOUniversigneSealService.Codeunit.al:183 in Codeunit CDO Universign eSeal Service::ProcessSealResponse (temp state unknown), through CDO E-Mail Handler::DocHandle
    temporary records: 199 touches in 55 routines (Table CDO File::MergeAndReplacePDF 14, Codeunit CDO E-Mail Template Management::CreateDocumentsForRecord 14, ::CreateReportsForRecord 11, ::CreateDocuments 11, … +51 routines) — --all lists them
  up: 1 transitive caller (Page CDO Reminder Actions::OnAction): touches it, through this routine — not a finding; --all lists its witnesses
```

(Counts measured on the branch: of 204 witnesses, 1 `known(false)` (the `CreateZip` CalcFields),
4 `unknown`, 199 `known(true)` in 55 routines.)

RR vs `CDO File`: `up: 883 transitive callers: 883 touch it, 0 do not, 0 unknown — through this
routine; not a finding`.

JSON: `down` keeps every witness row (temp ones marked) and adds `down.counts {knownPersistent,
possiblyPersistent: {tempStateUnknown, callerDependent}, temporary, tableNotResolved, byOp}`. `up`
holds `transitiveCallers, callersTouching, callersNotTouching, callersUnknown, informative, note`;
the trivial case has **no** `witnesses` (built never); the informative case has `callers[]`
(`depth, routine, firstWitness, witnessCount`); `--all` adds `witnesses`. A removed default field:
query schema **3.0.0**. Size: RR ~195 KB (the down list, 193,498 B measured with `--direction
down`), IR ~195 KB.

### 4.7 prove, human and JSON

Human (IR real except `‹path›`):

```text
--- Codeunit CDO Issue Document::IssueReminder ---
  question: writes-table:Issued Reminder Header
  answer:   yes
  evidence: 2 effects
    [DB_INSERT] beyond Codeunit Reminder-Issue::OnRun, entered at ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19
      path: IssueReminder → [run] Reminder-Issue::OnRun → ‹path to the Insert›
    [DB_MODIFY] beyond Codeunit Reminder-Issue::OnRun, entered at ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19
      path: IssueReminder → [run] Reminder-Issue::OnRun → ‹path to the Modify›
  coverage: incomplete — 21 object runs not resolved (6 in this workspace; top: Codeunit.Run(<dynamic>) ×16), 267 interface calls open world (10 in this workspace); alsem explain lists the workspace ones under gaps
  assumptions: none (default world; --closed-world assumes the loaded interface implementations)

--- Codeunit CDO Issue Document::IssueReminder ---
  question: may-commit
  answer:   yes
  evidence: 1 effect
    [COMMIT] beyond Codeunit Reminder-Issue::OnRun, entered at ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19
      path: IssueReminder → [run] Reminder-Issue::OnRun → ‹path› (Commit at dep:Base Application:‹file›:‹line›)
  coverage: …
  assumptions: none (…)
```

A `no` answer prints `answer: no` followed by its effective coverage and assumptions. An absence
question the obligations leave undecided is prove's `unknown`, printed `answer: unknown — no
matching effect observed; coverage incomplete` with its blockers. `yes` is never qualified (R3e).
A capped G1 fallback prints `(path does not pass Codeunit X::Y: search cap reached)`.

Rules:
- **W2-0b (G1)** lands before everything else on prove (§3.4.3).
- **2f / P1:** every group evidence gets a witness, COMMIT included (terminal-fact index).
- **2h / P4:** obligations gain `unresolvedInWorkspace` (split dynamic/interface) and
  `topUnresolvedCallees` (3); the human `coverage:` line names them.

JSON: **additive only, prove 2.2.0** (W2-0b: `crossing`, `throughEntryPoint`, `groupWitness`; 2f:
`groupWitness` on COMMIT and other resource-less group evidence; 2h: the obligations fields).
Revision 3 (Fable N11, astra 5, R3d): `groupPath` keeps its `string[]` hops exactly as today; the
structured witness (`groupWitness: {hops: [{edge, edgeId, to, line, file}], terminal}`) is a new
field beside it, so the bump stays additive.
Unchanged: `evidence[].evidence`, `answer`, `blockedBy`, `assumptions`, `lineBase`, `viaPaths`, `groupPath`
(ConTinAI `verify.py` reads `payload.lineBase`, `result.answer`, `result.evidence[0].evidence.{file,
line}`, `result.blockedBy`, `result.assumptions`).

**Prove's serializer is its own** (A9, F8). Today prove builds evidence with
`digest_cli::effect_to_value` (`prove.rs:1162` region). W2-2 splits:
- `digest_v2::effect_value` — today's function, frozen; used only by `digest --schema 2`;
- `prove::evidence_value` — prove's 2.x contract; starts as a copy, takes prove's additive fields;
- `digest_v3::effect_value` — the §4.4 projection.
They may share leaf helpers (`anchor_to_value`, `hop_to_value`) only where those helpers' output is
part of all three contracts; a helper change that moves any one of them is caught by its goldens.

---

## 5. Flags and MCP inputs

| Command | Flag | MCP input | Meaning |
|---|---|---|---|
| explain | `--detail` | `detail: bool` | every list complete, no folds |
| digest | `--detail` | `detail: bool` | unfolded 3.x (every path kept, every list, every diagnostic) |
| digest | `--schema 2` | `schema: 2` | the 2.1.0 document, from the frozen renderer; one release |
| digest | `--compact` | `compact: bool` | JSON without indentation |
| explain, digest | `--max-paths N` | `maxPaths: int` | paths per item |
| query effects / touches | `--all` | `all: bool` | every row in human; `up.witnesses` in JSON |
| explain, query | `--show-source` (2g) | `showSource: bool` | the cited line ±2 under each anchor |

**Precedence (A9):**
- path cap = `--max-paths N` when given; else 3 with `--detail`; else 1. N above 3 gives at most 3
  and the document states `pathCap: 3`; `--max-paths 0` gives no paths (counts only).
- The MCP tool passes `--max-paths` only when the caller sets `maxPaths`; it has no default of its
  own, so `detail: true` alone gives the detail cap (3), never 1.
- `--schema 2` with `--detail`, `--max-paths` or `--compact`: usage error (exit 2) — the 2.1.0
  document has no folds or caps to set.
- **`--schema 2` is the old serialization shape over the current analysis** (revision 3, astra 4,
  R3c), not frozen wave-1 output. It renders the same corrected facts (the literal pass and the
  classifier run for every reader), in the 2.1.0 shape. Where the analysis corrected something —
  a literal-dead group resource, a pruned own-row path with `viaPathsTruncated` set, a row marked
  literal-dead (Fable N7) — the schema-2 document shows the correction too. The existing
  `cli-b-goldens/digest` goldens are expected to stay byte-identical (no fixture has a dead path
  today); if one moves, it is re-minted with the reason named in the commit message, never
  presented as a compatibility break. `tests/r4f-goldens/` cannot move: the pass runs on the
  CLI's copies, never in the engine. The CHANGELOG's 1b line says literal-dead resources are now
  excluded from `physicalTablesWritten`.
- The window's end (R3j, §9 q5): removed at the **latest** of one full release after wave 2,
  bc-dev-mcp's `alsem-pin` moving past 2.x, and 90 days after wave 2 ships; the CHANGELOG gets the
  resulting date at release preparation, and the flag's help names it.

---

## 6. Before and after on CDO

"After" is a mock-up measured with `wc` (explain IR, query IR, touches, prove), the §4.4.3 script
(digest JSON) or an estimate where marked. "Before" is the branch `a2464efc`.

| Output | Routine | Before | After | Plan target | Met? |
|---|---|---|---|---|---|
| explain human | IR | 558 lines, 80,769 B | ~75 lines, ~16 KB (mock-up) | ~150 lines | yes |
| explain human | RR | 194,357 B | ~85 lines, ~17 KB (estimate) | ~150 lines | yes |
| explain JSON | IR / RR | 310,692 / 760,024 B | ~270 / ~580 KB (estimate) | none | file + summary |
| digest human | IR / RR | 95,870 / 74,945 B | ~70 lines, ~14 KB each (estimate) | grouped | yes |
| digest JSON | IR | 7,339,040 B | 980,618 B pretty (core 468,641); 589,018 B compact | ~200 KB | no: §9 q1 |
| digest JSON | RR | 4,952,155 B | 928,650 B pretty (core 422,771); 562,984 B compact | ~200 KB | no: §9 q1 |
| query effects human | IR | 126,430 B | ~25 lines, ~5 KB | < 100 lines | yes |
| query effects JSON | IR / RR | 670,121 / 646,165 B | ~690 / ~665 KB (rows kept) | none | file + summary |
| touches human | IR, CDO File | 412 lines (r1) | 10 lines | one-line `up` | yes |
| touches JSON | RR, CDO File | 257,337,809 B (r1, master) | ~195 KB | — | file + summary |
| prove human | IR, RR | 5–6 lines | 9–14 lines (paths, coverage, assumptions) | — | — |

MCP against Claude Code's 50,000-character limit: the inline cap is **40 KiB** (branch 1c), and
Claude Code reads and size-checks `structuredContent` only (measured in 1c). Every human default
above is under 20 KB. The digest always goes to a file (`always_file`), whatever its size, with the
≤ 8 KiB summary. Revision 3 (astra 10, R3i): MCP sends stdout once as text content (the byte
contract) and once as `structuredContent`, so the whole result can exceed 50,000 characters while
`structuredContent` stays under the client limit. T-W2-23 therefore has **two budgets**: the
serialized `structuredContent` (what Claude Code reads and size-checks) ≤ 50,000 characters, and
the total serialized `tools/call` result ≤ its own stated wire budget; heavily escaped strings are
in the fixture, because a raw 40 KiB cap guarantees neither.

---

## 7. Wave 3 dependencies

- **3a (query depth from `cone_derived`):** `summary.byTable[].beyond`, `summary.scope` and touches'
  `down.beyond` exist now, empty; 3a fills them (3.1.0, additive); the "beyond entry points" section
  of query effects is rendered by the explain function. The cross-tool agreement's query leg is
  added then, **with the §1 item 9 difference stated**: query lists operations the cone rules out
  under literal arguments, so the leg is "every table the cone's physical class names is in query's
  writes (own + beyond); query may name more, and each extra is accounted for by a stated
  substrate difference (literal-dead in the cone is one; revision 3, astra 11: not the only one,
  so never assumed for all)" — or 3a applies the literal pass to query's rows too (to be decided
  in 3a's spec). The `CDO Log Attachment` case (IR, `DB_MODIFY` absent from the digest; prove
  `yes` only through the Insert) is pinned by its exact operation ids in `output_cdo.rs`.
- **3a (Q4):** the per-operation RecordRef bit; before it, "table not resolved" is the wording.
- **3b** rewrites descriptions around these formats.
- Nothing in explain, digest or prove depends on wave 3.

---

## 8. Commit order, goldens and tests

### 8.1 Commit order

| Commit | Content | Golden families that move |
|---|---|---|
| **W2-0a** (done, §11) | Crossing records (typed edge sets, plural sites) on `DigestEffectResult`; `event_id` on `LiteralReach`'s event-dispatch adjacency; operation multimap index (§3.5) with query's last-row and explain's first-row selections kept; lossless terminal-fact index (§3.4.1) inside `LiteralReach`; in-process adversarial fixtures. No output change | none (every golden byte-identical, no regen) |
| **W2-0b (G1)** | Prefix phase + cached suffix trees per `(target, bindings)` through the crossing's typed edges, bindings kept, edge ids on hops, aggregate cache limits, outcomes, per-crossing cap fallback with checked membership; `crossing`, `throughEntryPoint`, `groupWitness` on prove group evidence (prove 2.2.0, `groupPath` unchanged); output fixtures; CDO before/after in the message | `cli-output-goldens` (prove, + the new fixture shapes in every family), `cli-b-goldens/prove` (version) |
| **W2-0c** | Crossing-local resource status (`reachable` / `disproved` / `not established`, §3.3.1) as its own reviewable correctness commit; any prove verdict it moves is named in the message | the families whose fixtures it corrects, named |
| **W2-1** | Classifier in `rollup.rs` and the shared `evaluate_writes_table` (§3.2.1); prove calls them (verdicts byte-stable); literal pass for explain and digest with the raw/refined contract (§3.1); explain declares the L4 temp-state join; cost measured (cold/warm/RSS/multi-root, split) before it lands; 1b summary to three classes | `cli-output-goldens` / `cli-b-goldens/digest` only where the pass removes a resource (named in the message) |
| **W2-2** | 2a+2b+2c: rollup and `RollupTotals`; explain human + `/3`; digest human; digest 3.0.0 with `--schema 2`, `--compact`; serializers split (§4.7); query effects 2b/2c (3.0.0) | explain, digest, query, gate batch families (§8.2) |
| **W2-3** | Touches computation and output (§4.6) | `cli-query-goldens`, `cli-output-goldens` touches |
| W2-4 … W2-8 | 2d fields, 2e kinds, 2f witnesses (COMMIT, resource-less, representative per crossing), 2g locations + `--show-source`, 2h obligations | per item |

Each commit: `scripts/check-goldens --regen`, the moved families named with the reason, the task gate,
the discrimination proofs of its invariants; the branch gate (with `scripts/cdo-gate`) before merge.

### 8.2 Golden families

| Family | Test target | Moves because |
|---|---|---|
| `tests/cli-output-goldens/` (wave 0) | `--test cli` | every explain/digest/query/prove file; add `.detail`, `.all`, `.schema2` variants |
| `tests/cli-explain-goldens/` | `--test cli` | layout and `/3` |
| `tests/cli-query-goldens/` | `--test cli` | layout; JSON `summary`, `firstHop`, touches `up` |
| `tests/cli-b-goldens/digest/` | `--test cli` | new default human + JSON 3.0.0; today's JSON files become the `--schema 2` goldens **without being regenerated** |
| `tests/cli-b-goldens/prove/` | `--test cli` | `path:`, `coverage:`, `assumptions:` lines; JSON additive |
| `tests/gate-goldens/batch/` | `--test cli` | digest, prove, query job stdouts |

Must **not** move: `tests/r4f-goldens/` (if it moves, a render change leaked into what the engine
projects), `cli-b-goldens/{fingerprint,snapshot,diff,split,single-app,cross-app}`, `cli-a`, `cli-c*`,
`graph`. CDO rendering goldens for IR, RR and `CDO Telemetry::UpdateLastUpload` (explain human, query
effects human, touches human, prove human may-commit and one writes-table, digest summary),
byte-compared under `scripts/cdo-gate` from `tests/cli/output_cdo.rs`, minted after W2-8.

### 8.3 Invariants (item T, wave 2)

Fixture tests go in `tests/cli/output_invariants.rs`; CDO ones in `tests/cli/output_cdo.rs`. Every row
needs a discrimination proof (break with Edit, run, see it fail, revert, see it pass), recorded in the
commit message, with the break asserted to have applied. The wave-1 tests stay (anchor source-check,
line base, one copy, summaries, descriptions, boundary names).

| Id | Invariant | Review rows answered | Discrimination break |
|---|---|---|---|
| T-W2-1 | Summary numbers are identity-set unions in the §3.6 units; each equals the matching section's distinct count, `cards[].summary` / `entries[].summary` and the MCP summary; **plus** literal expected numbers on the fixture, counted by hand from its AL | A11-1, F6, F invariants 1 | compute the summary's table count as a sum of section counts |
| T-W2-2 | No two rollup records share a key (root, section, class, table id or crossing id, op kind); checked on records, not text; `path:` lines never checked | A11-2, F9 | drop the `kt`/`nt` part of the rollup key |
| T-W2-3 | Every list goes through `fold`; unit tests at cap−1, cap, cap+1 and the 12/15 rule (fixture uses a test-only cap knob, F9); on the CDO goldens every `… +N <unit>` line also carries the full count with the same unit | A11-3, F9 | drop the `+N` suffix in `fold` |
| T-W2-4 | Fields only on Insert, Modify (routine assignments, named per owner), ModifyAll and Validate (the op's field); never on Delete, DeleteAll, Rename, LockTable. Fixture adds ModifyAll, Rename, Validate, a DeleteAll on a table its routine assigns fields to, and two writing routines on one table | A11-4, A6, F invariants 4 | attach fields to every op |
| T-W2-5 | Every table in "writes in workspace code" has a witness whose last hop owns the printed anchor's operation, or an `unavailable` reason; every commit witness ends at a COMMIT operation or has a reason; a crossing's prefix ends at its site; a direct effect is a valid zero-hop path | A11-5, A3, F13 | pick the path and the anchor from different rows |
| T-W2-6 | Default 3.x and `--detail` 3.x hold the same **multiset** of rows keyed by (factId, type, tempState, resource id or crossing), multiplicity included (CDO IR: 57 rows on 5 factIds); every `known(true)` row present with its `tempState`; the `--schema 2` goldens are byte-stable un-regenerated | A11-6, F16i | filter temporary rows out of the default |
| T-W2-7 | Every DB group lists exactly the resource ids the cone gives (set equality against a recomputation on the fixture); `resourceCount ==` listed count, for events too in the digest (O2); explain's event groups carry `resourcesListed: false` and the count; resource-less groups carry neither; `unresolvedResources` only on DB groups | A11-7, F invariants 7 | `.take(10)` on table resources |
| T-W2-8 | `paths` is a list of `{hops, terminal}`; its length ≤ the effective cap (§5) for caps 0, 1, 2, 3 and 7; `pathsOmitted == pathsKept − len(paths)`; `pathsProducerTruncated` equals the engine's flag; every hop's `to` is a `payload.routines` key and the number of distinct stable ids referenced equals `len(payload.routines)` | A11-8, A5, F7 | key the routine table by display |
| T-W2-9 | Every group witness (prove `groupPath`, digest/explain `--detail` witnesses) traverses its group's crossing edge (edge id and site equal), is live under `LiteralReach::path_live`, and ends at a live operation of the resource; fixture: two crossings to `PostWith` with `true`/`false` and a **shorter** workspace route to `Base Ledger 13`; a capped fallback passes only with `throughEntryPoint: false` and a reason; CDO: RR / `CDO E-Mail Log` evidence 3 | A11-9, A2, F4, F16h | start the search at the root without the crossing condition |
| T-W2-10 | Touches: in the trivial case no ancestor witness is built (a counter in `effect_query`, read by a unit test on a synthetic bundle) and human `up` is one line, JSON `up` has no `witnesses`; `knownPersistent + possiblyPersistent + temporary == down` witnesses; `callersTouching + callersNotTouching + callersUnknown == transitiveCallers`; cases: temporary-only root, caller-frame temp rebinding, rowless caller, `--direction up`, no `--from`, `--all` | A11-10, A7, F invariants 10 | restore the per-witness `up` loop |
| T-W2-11 | Query buckets are exclusive in the §4.5 precedence and sum to `effectCount`; every `firstHop` is a direct callee whose summary contains the operation, or `null` with a reason | A11-11, A7, F10 | print the first sorted direct callee without the membership check |
| T-W2-12 | No `dep:<guid>:` in any **parsed** human anchor (so `--show-source` text may contain a guid); JSON keeps the guid; two loaded apps with one name are told apart; no `%20` | A11-12 | print the guid |
| T-W2-13 | The MCP summary is the `RollupTotals` serialization (one struct); ≤ 8 KiB; equals a hand-written expected summary on the fixture | A11-13, F15 | rebuild the 1e summary from the stdout |
| T-W2-14 | With more than 3 page actions the page-action part of `callers:` is ≤ 3 lines (routine callers and caller rules are not counted); the grouped and `--detail` forms name the same action ids (CDO RR) | A11-14, F16g | drop the grouping |
| T-W2-15 | Size ratchets, bytes **and** lines, pinned to the CDO input stamps (`workspace_git_sha`, closure stamp): explain human ≤ 150 lines, query effects human ≤ 100, touches human ≤ 20, every human line ≤ 2,000 characters, digest JSON whole and core (§4.4.3) per the owner's number | A11-15, OS | re-embed `viaPaths` |
| T-W2-16 | **Rewritten in revision 3 (astra 1, R3a).** Three legs. (1) **Classification:** explain's and digest's rollups give every live effect contribution the same `WriteClass` (equal records). (2) **One table answer:** for a table set T that includes every table named by explain, digest or prove for R **and** tables named by **none** of them (fixture: every table of both apps; CDO: the 12 named per routine plus 12 drawn from the model's tables that no output names), explain/digest's `evaluate_writes_table(R, T)` equals prove `writes-table:T`'s `answer`, `blockedBy` and `assumptions` exactly. (3) **Evidence:** every prove `yes` match is attributable to one of `Named` (a row naming T in the physical class), `ViaCaller`, or `FieldGuarded` (a table-not-resolved row whose field guard matches T); every `no`/`unknown` follows the complete obligation rules (unresolved calls, open-world dispatch, analysis gaps, coverage, truncation). Group `unresolvedResources` count as writes on a table not resolved (Fable N6). Fixture cases: literal-dead, parameter-dependent, a field-guarded unknown-table write **in a group**, an absent table with only ordinary obligations (expect `unknown`). CDO case: IR / `CDO Log Attachment` `yes` only through the Insert (Fable N15) | astra 1, A11-16, A1, F1, F3, N6, N15 | in `evaluate_writes_table` drop the `FieldGuarded` match (leg 2 fails on the fixture's group case); classify `parameter-dependent` as physical (leg 1) |
| T-W2-17 | Every prove evidence row has `evidence.file` or `evidence.sourceKind: unavailable` | F missing (a), F8 | drop `evidence` from group rows in prove |
| T-W2-18 | Every crossing's targets are routines in the model; the printed entry point equals the target's qualified display, or the multi-target form lists every target | F missing (b), F2 | print a different target's display |
| T-W2-19 | No table is both in "may write" and `temporaryOnlyTables` of one card, with `temporaryOnlyTables` computed from the temporary rows, not as a complement (Fable); the temporary section still lists a table's temporary work when the table is also in "may write" (astra 8) | F missing (c), astra 8 | put `kt` group resources in both |
| T-W2-20 | No caller-dependent **contribution** is counted as physical; a table with a separate physical contribution may be in both sections (revision 3, astra 8) | F missing (d), F1, astra 8 | count caller-dependent contributions as physical |
| T-W2-21 | Consumer contract: on fixture outputs, the fields ConTinAI `effects.py` reads (`via` with `direct` meaning own operation, `operationId`, `origin.routine.{stableId, objectDisplay, display}`, `origin.anchor.{file, line}`, `tableName`, `tableId`, `isUnknownBucket`, `tempState`, `op`) and `verify.py` reads (`payload.lineBase`, `result.answer`, `result.evidence[0].evidence.{file, line}`, `result.blockedBy`, `result.assumptions`), including a dependency anchor, an absent evidence and an assumption-bearing (`--closed-world`) answer; digest `entries[].routine.anchor` and `anchors` | A10 | rename `via` |
| T-W2-22 | Flag and schema combinations: `--schema 2` with `--detail` / `--max-paths` / `--compact` exits 2; MCP `detail: true` without `maxPaths` gives cap 3; multiple roots; `--include-reads`; `tests/r4f-goldens/` unchanged | A9, A11 coverage | let the MCP tool pass `--max-paths 1` by default |
| T-W2-23 | **Two budgets (revision 3, astra 10, R3i):** the serialized `structuredContent` of every inline answer is ≤ 50,000 characters (the client limit Claude Code checks), and the whole serialized `tools/call` result (text content + `structuredContent` + metadata, after JSON escaping) is ≤ the stated wire budget; the fixture includes heavily escaped strings; a digest always goes to a file | A11 (envelope, `always_file`), astra 10 | raise the inline cap to 64 KiB (and add a fixture answer between 40 and 64 KiB, since 1c recorded that CDO has none) |
| T-W2-24 | Same-display identities: the fixture has two overloads with one display on one path and two objects with one name in two apps; JSON keys differ; human prints signatures | A5, F7, A11 coverage | key by display |

**Revision 3 amendments to the other rows** (astra 10's table, Fable §4):

| Id | Amendment |
|---|---|
| T-W2-1..3 | The hand-counted fixture numbers test semantic totals, not one rollup against its serializers |
| T-W2-4 | Field wording per §4.1 (not operation-specific); Validate in its own bucket |
| T-W2-5 | Class-correct terminals (§3.4.1); known simple fixtures must produce real witnesses, so "unavailable everywhere" cannot pass; COMMIT terminal case "ends at a live state owning a direct COMMIT record" (Fable) |
| T-W2-6, T-W2-7 | Schema-2 compatibility tested separately (old shape over current analysis, §5); T-W2-7 compares against the **refined** set (§3.1, §3.3.1) and checks `resourceCount` counts that list |
| T-W2-8 | Count routine-table references from roots, terminals and crossing targets as well as hop targets; at cap 0 crossing targets still need entries; zero-hop witnesses (`Found` with `hops: []`) and every truncation outcome tested; hash-prefix extension until unique |
| T-W2-9 | Adds crossing-local pruning (the `false` crossing's resource is `disproved`), the mixed workspace/dependency dispatch sharing one site id, an `ev:` crossing (Fable N1), the terminal temp class, and uncapped expected-success cases: a labelled fallback never makes every failed witness acceptable; dual-cap and run-order cases |
| T-W2-10 | The counter is read on a synthetic bundle with ≥ 2 ancestors (Fable); state whether table-not-resolved candidates are outside the `down` denominator, including `--table unknown` (astra) |
| T-W2-11 | `firstHop` by complete effect contribution (§4.5); fixture where the first sorted callee lacks the operation and one where two callees hold it with different temp instantiations |
| T-W2-12 | The source-check parser and resolver read `dep:<app name>:<path>:<line>` with spaces in the app name (`dep:Base Application:…`), and the `(<guid8>)` disambiguator; it no longer stops an anchor at whitespace or assumes `dep:<guid>:` |
| T-W2-13..15 | Keep the deterministic input stamps; add a byte budget for the whole human output, not only per line |
| T-W2-17, T-W2-18 | Exact legacy evidence values; crossing edge membership checked by `(from, to, site)`; the multi-target `Page.Run` crossing on CDO (Fable) |
| T-W2-21, T-W2-22 | Collided-origin fixtures (query's last-row `origin` kept); schema-2 semantics as §5 |
| T-W2-24 | Adds identical signatures and names in different apps, and collided declarations sharing one stable id in different source files |
| T-W2-25 (new, W2-0a) | The read-side indexes: crossing membership excludes a workspace edge sharing a crossing's site id; event crossings list every publish site; event-dispatch adjacency carries the event id; the operation index keeps every claim with query's last-row and explain's first-row selections; terminal records keep temp class, producer flag and original op kind (temp-vs-persistent on one table; one parameterized writer with both arguments) — §11 lists the tests and their discrimination proofs |

Flakiness: none is timing-dependent; the searches are BFS over sorted adjacency, and cache eviction
is by insertion order. The capped G1 fallback is deterministic and accepted by T-W2-9 only when
labelled and membership-checked.

### 8.4 Compatibility

- digest JSON: `--schema 2` (old shape over current analysis) until the §5 end rule. bc-dev-mcp is pinned to al-sem 1.7.1 and refuses
  another schema major (plan §4.1), so 3.0.0 cannot be misread silently. Its `risk.ts` reads
  `entries[].routine` anchors, which 3.x keeps; when its pin moves past wave 2, the other digest and
  prove readers the plan lists (`alsem-writes.ts`, `break-effects.ts`, `test-order-static.ts`) are
  checked field by field against §4.4.2 and use `--schema 2` until updated. No migration is
  prescribed here (A10).
- explain JSON `/3`: no outside reader (§4.2).
- query JSON 3.0.0: rows unchanged; touches readers that want `up.witnesses` pass `--all`.
- prove JSON 2.2.0: additive (`groupPath` unchanged; `groupWitness` beside it).
- CHANGELOG (Changed / Fixed / Added): the layouts, schema versions, flags, the `--schema 2` window
  and its end date (inserted at release preparation, §5), G1 (Fixed), the classifier change in 1b's
  summary and that literal-dead resources are excluded from it.

---

## 9. Open questions — settled (revision 3, R3j)

1. **Digest size budget.** Provisional: whole ≤ 1,000,000 bytes and core ≤ 480,000 bytes pretty per
   routine (IR, RR), remeasured after the real indexes, witnesses and summaries exist (W2-2), plus
   opt-in `--compact`. Every event name kept, grouped losslessly by publisher (§4.4.3).
2. **Paths beyond entry points.** Approved: per crossing its prefix once and one representative
   terminal witness by default, labelled as representative, never implying that every site reaches
   every resource; a COMMIT gets its own terminal witness; full per-resource witnesses under
   `--detail`. The digest JSON default carries one per may-persist group row (Fable N12, §3.4.2).
3. **Explain's added cost.** Accepted; the remedy is caching (per-revision `LiteralReach`, seeded
   L4), never skipping the pass. ~+1 s warm per root is a review trigger, not a CI assertion; the
   cold/warm/RSS/multi-root measurement comes before W2-1 lands (§3.1).
4. **Explain JSON `/3` readers.** The plan's §4.1 audit stands; W2-2 re-checks the then-current
   extension source.
5. **`--schema 2` end date.** Removed at the latest of: one full release after wave 2, bc-dev-mcp's
   pin moving past 2.x, and 90 days after wave 2 ships; the date goes into the CHANGELOG at release
   preparation (§5).
6. **Prove human wording.** astra's: the real answer is authoritative (§3.7, §4.7).

Left for the owner after W2-0a: §11.4.

---

## 10. The no-path engine issue

Prepared, not filed: `target/tmp/issue-no-path-dependency-writes.md`. It carries representative
operation ids from IR and RR, the hypotheses (a write two dependency apps deep; re-entry from
dependency code into workspace code and back; a cone edge the witness walk cannot seed; H4, the
same seen from the DAG's side), the probe, a minimized fixture per hypothesis, and the `factId`
collision (§1 item 5). Revision 3 (astra 11, Fable N14): ownership is reproduced from each row's
internal `origin_operation_ids` (joining diagnostics back by the colliding `factId` is
many-to-many); the demonstrated impact is ambiguous digest row identity and unsafe external joins
— `alsem diff` compares capability snapshots keyed `op|resourceKind|resourceId`, not `factId`, so
no diff impact is claimed; the fix's home is `build_canonical_key` (`digest.rs:3205`), falling
back to the first origin operation id when a non-group row has no terminal id. Wave 2 renders the
rows honestly (§4.1 section 13) and does not depend on the issue.

---

## 11. W2-0a as built

Branch `mcpout/wave2`, the commit after this spec's first one. No output changes: every golden
family is byte-identical without a regen, and the CDO digest/prove/query outputs are
byte-identical (timestamps aside).

### 11.1 What each index holds

- **`Crossing`** (`src/engine/l5/digest.rs`), on every group row's `DigestEffectResult::crossing`
  (an `Arc`, shared by the rows of one identity; `None` off a group): `id`, typed `site`
  (`CrossingSite::{Callsite, Operation, Event}`; `guarded_id()` is `None` for an event), the
  typed edge set `edges: Vec<CrossingEdge { from, to, kind, site }>` (sorted, deduplicated),
  `targets` (distinct dependency stable ids, sorted) and `sites: Vec<SiteAnchor>` (distinct source
  anchors in edge order; several for `ev:`). Built once per tree read in
  `build_fingerprint_indexes` (`build_crossings`) from every edge of a primary routine into a
  dependency routine, grouped by the boundary identity `boundary_terminal` gives it. Readers today:
  the multi-target boundary display (`boundary_displays`, now derived from each crossing's targets
  through `boundary_display`) and `LiteralReach::unreachable_effects`' "crossing cannot run" check
  (it reads `crossing.site.guarded_id()`; a hand-built group with no record falls back to its
  evidence ids). The public index twin (`FingerprintIndexesPub`) carries the map too.
- **Event ids on `LiteralReach`'s adjacency** (`src/engine/l5/literal_reach.rs`): an out-edge's
  site is `EdgeSite::{Guarded(id), Event(event_id)}`; the searches apply guards and bindings only
  to `Guarded` sites, so results are unchanged; `event_edges_from(from)` reads the event edges.
- **`TerminalFacts`** (`src/engine/l5/terminal_facts.rs`): one `TerminalRecord { fact,
  effect_type, resource, owner, terminal: TerminalSite::{Operation, Callsite, None}, op_kind, temp:
  TerminalTemp::{Known, Unknown, ParameterDependent, Absent} }` per direct fact with an effect type,
  in fact order, looked up by kind (two `&str`-keyed levels, so a lookup borrows nothing) and by
  terminal id. `LiteralReach::new` builds it (it replaced the private `direct_by_kind` map, the
  same set) and `kind_dead` reads it; `LiteralReach::terminals()` exposes it. The root-frame class
  join (§3.4.1) is W2-0b's: the record keeps the owner-frame state and the producer mark it needs.
- **`OperationIndex`** (`src/engine/l4/operation_index.rs`): operation id → `OpClaim { row,
  record_operation, operation_site, call_site }` per claiming row (a record operation's id is also
  its operation site's, so one claim holds both), and call-site id → `(row, index)`. Scope
  `RecordOperations` (what the two legacy joins read; they build no more than before) or `All`;
  the all-ids readers (`claims`, `lookup`, `call_site_claims`) panic on a `RecordOperations` index
  (declared-needs rule 5). Readers today: `query effects`' `OperationJoin` (LAST claiming row,
  unchanged) and explain's `op_rows` (FIRST, unchanged). `original_kind` and `anchor` read a
  claim's original operation kind and source anchor.

Declared needs: nothing new is built for a command that did not build its predecessor. The
crossings replace the per-read `boundary_displays` loop (same edges, rule B only); the terminal
index replaces `LiteralReach`'s map (prove's read only); the operation index replaces the two
joins' private maps at the same scope. No profile, view need, substrate bit or plan changed.

### 11.2 Tests and discrimination proofs

In-process workspaces (`src/engine/l5/wave2_index_tests.rs`, written to temp folders, so no golden
family reads them) and the collided-row fixture (`detector_context_11b_tests.rs`). Every break was
made with Edit, run, seen to fail, reverted with Edit, and seen to pass.

| Test | What it pins | Break → result |
|---|---|---|
| `a_shorter_workspace_route_hides_both_crossings_today` | two calls of `PostWith` (literal `true` / `false`) after a shorter workspace route: two crossings, one typed edge each, disjoint; today's digest has no group row (the G1 shape W2-0b fixes) | crossing edges kept for any target (filter dropped) → the event and mixed tests fail; record dropped → see below |
| `a_crossings_group_rows_carry_its_record` | without the workspace route, only the `true` crossing has a group row, and it carries its own record | `crossing: None` on rows → fails (also the mixed and terminal tests) |
| `a_workspace_edge_sharing_a_crossings_site_is_not_in_it` | an interface call implemented in the workspace and in the dependency: one call site, two edges, only the dependency edge is the crossing's | "into a dependency routine" filter dropped → fails |
| `an_event_crossing_is_one_per_event_with_every_publish_site` | two workspace publishers of one event with a dependency subscriber (edges stated by hand) and a workspace subscriber: one `ev:` crossing, two edges, two sites, no guard; the search's adjacency names the event | event edge with no site → fails; only the first anchor kept → fails |
| `terminal_records_keep_class_producer_and_original_kind` | temporary vs persistent `Insert` on one table (temporary route shorter), dependency and workspace producers, the resource-less `Commit`, source-fact identity, `LiteralReach` holding the index; the digest's physical row ends at the persistent insert; `Fill` called with both arguments gives one temporary and one non-temporary group, each on its own crossing | temp state lost → fails; `op_kind` dropped → fails |
| `overloads_with_one_display_are_two_targets` | two overloads share a display; each crossing's target is its own stable id | targets built from the display → fails |
| `operation_index_keeps_every_claim_and_both_legacy_picks` | a collided id: both claims in row order, `Ambiguous`, original kinds per arm, `Missing`; explain's FIRST and query's LAST pick | explain pick reversed → fails; query pick reversed → fails (and the existing `operation_join_reads_the_last_owning_row` and `cli_11b_policy_collision::query_effects_origin_is_the_last_owning_rows_operation`); `lookup` returning the first claim → fails |
| `a_record_operations_index_refuses_an_all_ids_read` | the scope guard panics | — (a `should_panic` test) |
| existing `cli_prove_dead_paths::*`, `cli_prove_table_branches::*` | `kind_dead` reading the terminal index | records skipped in `kind_dead` → 4 of 5 fail |
| existing `cli_prove_table_branches::a_boundary_group_behind_a_dead_crossing_is_dropped_whole` | the crossing-dead check reading `crossing.site` | `guarded_id()` forced `None` → fails |
| existing `digest::tests::a_multi_target_boundary_names_every_entered_routine` | `boundary_displays` derived from crossing targets | "several displays" threshold raised → fails |

Not covered in W2-0a (left to the commit that needs it): two objects with one name in two apps,
and collided declarations sharing one stable id in **different source files** (the hop-file
provenance of §4.4.2, W2-2's T-W2-24). The operation-index fixture's collided rows are two `#if`
arms of one file.

### 11.3 Cost on CDO

`release-fast`, CDO baseline `bc3ccb18` passed as an argument, two runs each, `/usr/bin/time -v`
(no probes, so `PROBE_NO_COUNT` did not apply):

| Command | Before (master `367a664d`) | After (W2-0a) |
|---|---|---|
| `digest --routine "CDO Issue Document::IssueReminder" --format json --deterministic` | 20.25 / 20.82 s, 3,104,464 / 3,113,232 KB | 21.04 / 20.90 s, 3,131,076 / 3,116,436 KB |
| `digest --routine "CDO E-Mail Handler::RunReportID" …` | 20.23 / 20.67 s, 3,165,740 / 3,137,896 KB | 20.82 / 20.64 s, 3,145,412 / 3,115,620 KB |
| `prove … IssueReminder "writes-table:Issued Reminder Header" --format json` | 21.97 / 21.46 s, 3,134,444 / 3,107,952 KB | 21.62 / 21.37 s, 3,145,544 / 3,119,516 KB |
| `query effects … --routine "CDO Issue Document.IssueReminder" --format json` | 11.14 / 11.38 s, 3,124,712 / 3,105,128 KB | 11.31 / 11.32 s, 3,113,516 / 3,127,192 KB |

The differences are inside the run-to-run spread (about ±0.5 s and ±30 MB). Outputs: digest
byte-identical (7,339,040 and 4,952,155 B); prove and query identical except `generatedAt`.

### 11.4 For the owner

- The G1 fixture shape behaves as §1 item 11 says, and more strongly: with a shorter workspace route
  to the same table, the digest shows **no** group row for either crossing (the one row is
  witnessed through the workspace route). Without that route only the `true` crossing has a row:
  the witness walk already follows the literal argument at the crossing. W2-0b/W2-0c are measured
  against these two pinned outcomes.
- `ev:` crossings need a dependency subscriber to a workspace publisher's event, which compiled AL
  does not allow; the event test states its edges by hand, as `digest_rule_b_tests` does. Whether
  CDO has any `ev:` crossing is not measured yet (W2-0b should count them before building for
  them). (Measured in W2-0b: none, §11.5.)

### 11.5 W2-0b as built (G1)

**Event crossings, counted first** (`cdo_crossing_census`, CDO-gated, run by `scripts/cdo-gate`):
the CDO tree read for IR and RR has 828 crossings, 813 by call site and 15 by firing operation,
**no event crossing**; IR's 108 and RR's 91 group rows sit on none. So event crossings are **in
scope by construction** (the search matches an event edge by its `event_id`, as any other typed
edge) but are fixtured only by the hand-stated edges of W2-0a's test; no event-specific work
was built. The census pins "no event crossing" so a change shows.

**What runs** (`src/engine/l5/literal_reach.rs`, `src/engine/l5/group_path.rs`): prove's read
(`literal_reach_pass`) keeps the root tree (`live_tree`, unchanged, the prefix phase); per crossing,
`crossing_entries` collects every `(caller state, crossing edge, callee bindings)` at which one of
the crossing's typed edges is taken (matched by from, to and site; event edges by event id), in
root-tree visit order, distinct by callee state; per resource, `group_witness` takes each entry's
suffix tree from the read's `SuffixCache` and keeps the shortest prefix + 1 + suffix, ties by entry
order. The live tree now records each state's parent EDGE (site and kind) and, built on first use,
each routine's states by position, so a resource's terminal is a lookup, not a scan. The cache:
one tree per entry state `(target, bindings)`, at most 400,000 states per tree (`STATE_CAP`) and
1,600,000 over all trees of the read, evicting the oldest insertion first; a capped tree is cached
as capped; a cancelled search is not cached. Cancellation is checked every 4,096 states; the
production read passes "never" (a shared fill is not cancelled by a request, context-server
design §4.6). Outcomes: `Through` (an ordinary witness), `Outside` (a suffix search stopped early
and the root tree's own path, CHECKED for crossing membership, does not traverse the crossing),
`Unavailable(None)` (every search completed and none found: `no-witness`), `Unavailable(Some(stop))`
(stopped early and the root has no path either: `search-cap` / `cancelled`), plus
`no-operation-id` for a resource whose facts name no operation.

**What prove prints** (2.2.0, additive): for a writes-table group match, `groupPath` (unchanged:
`resource`, string `hops` in the old shape, now along the crossing-constrained witness),
`crossing`, `throughEntryPoint`, `throughEntryPointReason` (only when false) and `groupWitness {hops:
[{edge, edgeId, to, display, file, line}], terminal: {operationId, file, line}}` (1-based lines;
JSON keeps the raw `dep:<guid>:` source unit, as §3.8 rule 5 says JSON does); when the match has no
witness, `crossing` and `groupPathUnavailable: <reason>` instead (before, nothing was printed).
Verdicts, evidence rows and the resources a group lists are unchanged (W2-0c owns those).

**Goldens** (`scripts/check-goldens --regen`, every moved file read): the 16 `cli-b-goldens/prove/`
files, the 9 `cli-output-goldens/prove.*` files and `gate-goldens/batch/one-job/prove.stdout.golden`
move by `schemaVersion` 2.1.0 → 2.2.0 only, except the 5 writes-table answers of
`cli-output-goldens` on dependency tables (`base-doc-line`, `base-ledger-01`, `base-ledger-12`,
`base-price`, `base-register`) whose group evidence gains `crossing`, `throughEntryPoint: true` and
`groupWitness`; no line was removed but the version line, so no `groupPath.hops` string, verdict
or evidence row moved on any fixture (every fixture group path already traversed its crossing).
Hand-edited, as their tests say they are: `tests/cli-b-goldens/single-app/prove.dd.CallChain.
may-commit.json` (version) and the version assertion of
`cli_digest_changed_spans::entry_anchor_is_the_declaration`. `tests/r4f-goldens/` and every
non-prove family are unchanged.

**Tests and discrimination proofs** (`wave2_index_tests.rs`; G1 fixture: `TwoCalls` with the
shorter workspace route, its `true`/`false` crossings into `PostWith`, and a hand-stated re-entry
edge, an implicit trigger fired by `PostWith`'s `if Flag then L.Insert()` into `WriteLedger`):

| Test | Pins | Break → result |
|---|---|---|
| `g1_a_group_witness_traverses_its_crossing_with_its_bindings` | through the `true` crossing the path is `TwoCalls → PostWith → WriteLedger` (edge ids, kinds, anchors, the qualified display, the unchanged string hops); the `false` crossing has `no-witness` | crossing condition dropped (best ignored, root-ordered path) → fails (`throughEntryPoint` false via the workspace route); entry bindings reset to empty → fails (the `false` crossing gets a witness) |
| `g1_a_capped_suffix_falls_back_to_a_checked_root_path` | capped suffix: the workspace route as `Outside`/`search-cap`; a root path that does traverse the crossing → an ordinary witness; cancelled → `cancelled`; unreachable op → `search-cap` capped, `no-witness` uncapped | membership check skipped → fails; `throughEntryPoint: true` for `Outside` → fails; cancellation never checked → fails |
| `g1_the_suffix_cache_evicts_oldest_first_and_is_order_free` | a one-tree budget evicts the older tree and keeps the newer; a rebuilt tree answers the same; two asking orders give equal witnesses | no eviction → fails |
| `a_mixed_dispatch_is_entered_only_by_its_dependency_edge` | the workspace implementer's edge sharing the call site is no entry | site-id-only match (from/to dropped) → fails (2 entries) |
| `cdo_crossing_census` (CDO-gated) | no event crossing on CDO; prints the suffix cache's outcomes and cost | — |

**CDO before/after** (`release-fast`, master `cc9365ae` → W2-0b; prove JSON, `--deterministic`):
- RunReportID `writes-table:"CDO E-Mail Log"`: answer `yes` → `yes`, 3 evidence rows, every field
  but the group-path ones identical. Evidence 3 (`DB_MODIFY` beyond `Codeunit Sales-Calc.
  Discount::OnRun`): before, a 10-hop `groupPath` through workspace code only (`… →
  CreateLogEntries → implicit-trigger → CDO E-Mail Log.OnInsert → MoveEMailToFile →
  UpdateStoredAtBasedOnFilePath`, §1 item 11); after, `groupPathUnavailable: "no-witness"`: the
  completed search finds no live route through that crossing to the operation. W2-0c's
  crossing-local status (§3.3.1) is where the resource itself is judged; W2-0b lists it unchanged.
- IssueReminder `writes-table:"Issued Reminder Header"`: answer `yes` → `yes`, 2 evidence rows;
  both `groupPath`s identical (they already traversed `Codeunit Reminder-Issue::OnRun`), now with
  `throughEntryPoint: true` and a `groupWitness` (e.g. `object-run-resolved` at
  `ws:src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al:19`, `direct-call` to
  `InsertIssuedReminderHeader` at `…ReminderIssue.Codeunit.al:157`, terminal line 474).
- IssueReminder `may-commit`: identical but `schemaVersion`.

**Cost** (the machine carried heavy outside load during the runs, load average 100–400, so wall
times are not comparable; in-process spans): `prove.reach_effects` RR 16.26 s → 3.45 s, IR header
3.14 s → 0.17 s, IR may-commit 0.66 s → 0.65 s; `prove.literal_reach` (model-wide, unchanged
code) 0.3–1.3 s either way; peak RSS 3,104,604–3,122,548 KB before, 3,102,616–3,125,416 KB after.
The census over all 16,776 group resources of IR and RR (CDO-gated test, `cdo` profile): 30 suffix
trees built, peak 48,840 states held, 0 evicted. The first version scanned each suffix tree per
resource and cost far more; the per-tree routine index fixed it before landing.

### 11.6 W2-0c as built (crossing-local resource status)

Two commits, so the one that can move a verdict is reviewed on its own.

**Step 1 (status recorded, nothing dropped).** `LiteralReach::resource_status(entries, ops)`, over
the W2-0b machinery: for each of the crossing's entry states (its incoming binding states), the
cached suffix tree; `Reachable` at the first live state of a candidate's owner; else `Disproved`
when every suffix search completed AND some candidate's owner is reached from an entry target with
guards ignored (the module's existing rule: a graph or id mismatch never drops anything); else
`NotEstablished(reason)`: `NoEntry` (the root's search never takes the crossing's edges, e.g. a
crossing G48 already judges dead), `NoOperation`, `SearchStopped` (cap or cancellation; a capped
search never deletes), `OwnerNotReached`, `KindNotJudged` (events: only table writes are judged).
Candidates (`LiteralReach::candidate_ops`): every terminal record of `(effect type, table)` whose
class can be the group's (`kt` group: known-temporary records; `nt` group: every other record;
a parameter-dependent producer in both), with the group's representatives; a candidate with no
operation id makes the status `NoOperation`. The group path (W2-0b) reads the same candidates,
so a path and a status never disagree. Raw versus refined (§3.1): a prove group whose refined list
is shorter than its raw one prints `resourceCount` of the refined list, `resources` of its first
ten, and `resourcesOriginalCount`. Digest has no literal pass until W2-1, so its output is
unchanged.

CDO census (`cdo_crossing_census`, prove's pass on IR and RR):

| Root | Kind | Reachable | Disproved | Not established |
|---|---|---|---|---|
| IR | DB_INSERT | 157 | 1 | 325 (NoEntry) |
| IR | DB_MODIFY | 58 | 0 | 192 (NoEntry) |
| IR | DB_DELETE | 82 | 0 | 229 (NoEntry) |
| IR | EVENT_PUBLISH | — | — | 7,366 (KindNotJudged) |
| RR | DB_INSERT / MODIFY / DELETE | 482 / 266 / 293 | 0 | 0 |
| RR | EVENT_PUBLISH | — | — | 7,325 (KindNotJudged) |

IR's `NoEntry` resources sit on crossings the root's search never takes (G48 drops those rows
whole already, e.g. `Sales-Calc. Discount::OnRun` behind `if RecRef.Number <> Database::"Sales
Header" then exit;`). The pass costs 0.58 s for IR and 3.28 s for RR (`cdo` profile, census).
The one disproved resource: IR's `DB_INSERT` `Email Connector Logo` beyond `Codeunit Email
Account::IsAnyAccountRegistered` (call site `ws:src/Features/Email/Sending/CDOEMail.Codeunit.al:
254`); the only candidate is `Email Account Impl.::ImportLogo`. Checked in System Application's
source: `IsAnyAccountRegistered` calls `GetAllAccounts(false, EmailAccount)`, and
`GetAllAccounts` calls `ImportLogo` only `if LoadLogos then`; so that insert cannot run beyond
this crossing. No other row of IR writes the table, so dropping it moves prove's
`writes-table:"Email Connector Logo"` answer (step 2).

**Step 2 (the refinement):** §11.7.

Not covered: the dual cap where the prefix (root) tree itself caps; `live_tree` returns `None`
then and no group path is computed, exactly as before W2-0b (no fixture reaches 400,000 states).

---

## Appendix A. How the numbers were produced

- Build: `TREE_SITTER_AL_PATH=/home/sshadows/Projects/al-sem/tree-sitter-al
  CARGO_TARGET_DIR=/home/sshadows/Projects/al-sem/target-w2r2 cargo build --profile release-fast
  --bin alsem` in the `mcpout/wave0-1` worktree at `a2464efc` (2 min 13 s).
- One `alsem batch` manifest, 21 jobs on the CDO baseline (passed as an argument; `CDO_WS` not
  set): explain human/JSON, digest JSON/human (`--deterministic`), query effects JSON/human, prove
  may-commit JSON/human for IR and RR; prove `writes-table:CDO E-Mail Log` (RR) and
  `writes-table:Issued Reminder Header` (IR); touches IR vs `CDO File` `--direction down` JSON.
  Wall 5 min 43 s, peak RSS 3.15 GB.
- Class, union, crossing, event, no-path and gap counts: a script over the digest JSON applying the
  §3.2 rules to `detail.tempState` (`classify`), unions by resource id, crossings by the group
  evidence site; owner apps of no-path operations from the app-hash part of their operation ids
  (mapped through query rows).
- Digest v3 sizes: a script applying §4.4.2 to the real 2.1.0 documents (structured hops, hashed
  routine keys, crossing records with prefix, one path on decision effects, non-trivial scoped
  guarantees, folded diagnostics and dependency unresolved), serialized with sorted keys and
  two-space indent.
- §1 item 9: query rows for `CDO Log Attachment` against the digest's rows for it, then the CDO
  source (`CDOLogAttachment.Table.al:38-50, 88-110`, `CDOEMailLog.Table.al:340, 901-945`), read only.
- Mock-up lines are 1-based as the branch prints them. Scratch outputs were removed after the
  measurements; the build directory `target-w2r2` was kept for re-runs.
