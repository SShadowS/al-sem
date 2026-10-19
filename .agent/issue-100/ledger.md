# Issue #100 ledger

## Claim

```json
{"run_id":"20261019-004356-4443f1","issue":100,"attempt":2,"session":"claude-code-session:c26c6ca6-002c-49f4-bd91-33f31cbdf028","branch":"issue/100-agent-discovery-capability-extra-a2","worktree":"U:\\Git\\al-sem-issue-100-a2","body_hash":"153888d11543bd1c00e659f8ec926e11729cbe4fcf144cff825a59af21d2630f"}
```

Issue body hash (sha256): `153888d11543bd1c00e659f8ec926e11729cbe4fcf144cff825a59af21d2630f` (unchanged since attempt 1).
Worktree base (`make-worktree`): `214a7879098c161b266085c8f37b27386148a424`; build output at `H:\al-sem-targets\al-sem-issue-100-a2`.

**Seeded from attempt 1** (`U:\Git\al-sem-issue-100-a1`, base `60522971`, run `20261018-143233-61d4bd`, `blocked spec-panel-cap`): design, probes, acceptance matrix, measurement plan and register (25 entries) carried. Every file:line citation re-checked at `214a7879`: of the cited files only `capability_cone.rs`, `cone_derived.rs` and `program_calls.rs` changed since `60522971`, and every cited line in them is unchanged (`map_table_op` `:168-171`, call `:610`, `inherited_fact_key` `:1310-1318`, `TOUCHES_TABLE` set `:696`, physical read `:707`, `program_calls.rs:347`). The #160 specs name none of these symbols. The CDO baseline numbers below are from attempt 1's base and are re-measured at this base before the after-run.

Reviewer roster (operator instruction for this loop, 2026-10-18: "Dont use gemini. USe gpt 6.1 sol and local opus 5.5"): `sol` = gpt-6.1-sol via pi (astra slot), `opus` = a local Opus 5.5 subagent (flash slot). Attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** One entry in one function. `table_commit::map_op`
(`src/program/body/capability/table_commit.rs:24-40`) maps `TestField` to a
capability `read`. Both capability maps already use this ONE function: the L2
extraction (`extract_table`, same file) and the L4 cone
(`capability_cone::map_table_op`, `src/engine/l4/capability_cone.rs:169-171`, doc `:167-168`,
"Delegates to the one table op map shared with L2 (#69 D5)").

## Design

`TestField(Field)` checks the value the record variable already holds in
memory. As an INTRINSIC operation it issues no query, so it is not a read of
the table in the sense a capability `read` fact claims (a `Get`/`Find*`/
`Count`/`CalcFields` that fetches rows). The engine already says so everywhere
else: `op_classification.rs:65` classifies it `StateOnly`; d21
(`detectors/d21.rs:4`, `:33`) flags `TestField` on a record that was never
loaded, which only makes sense because `TestField` does not load it; the L4
store has no db-effect row for it (probe below). It does not calculate a
FlowField either (only `CalcFields`/`SetAutoCalcFields` do).

**Residual: just-in-time loading (S1-1).** Under partial records
(`SetLoadFields`), touching a field that was not loaded makes the platform
re-read the row
([Microsoft Learn, partial records: JIT loading](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-partial-records#just-in-time-jit-loading)).
That is a property of ANY field access, not of `TestField`: `Rec."No."` on the
right of an assignment triggers it too, and the engine models no field access
as a `read`. The row being re-read was loaded earlier, USUALLY by a `Get`/
`Find*` that carries its own `read` fact (in this routine or the caller that
passed the record). Not always: the platform itself loads a page's `Rec`, a
report dataitem's record, a table trigger's `Rec`, and a subscriber parameter
loaded in dependency code, with no AL read op anywhere (S1-1, S2-N3). So the
residual is: a JIT re-read is not modelled for any field access, and where the
platform did the original load, a read (and a `TableData R` need) can go
unreported. It is stated here and in the CHANGELOG, not special-cased for
`TestField`. Measurement plan item 4 measures both cases in a BC container when
the container is available.

1. Remove `"TestField"` from `map_op`'s `read` arm and list it in the
   "not capability-relevant" comment with the reason (an in-memory value
   check; `op_classification.rs:65`; the JIT residual above).
2. **One op table, by construction (S1-4).** Delete the cone's pass-through
   wrapper `capability_cone::map_table_op` (`capability_cone.rs:169-171`) and
   call `table_commit::map_op` at its one call site (`:610`), so the cone holds
   no function a second table could hide in. The two doc comments that name the
   wrapper (`capability_cone.rs:115`, `table_commit.rs:16`) are updated in the
   same change (S2-N2, S2-N4). Acceptance row A2 adds an agreement test over
   the whole `RecordOpType` vocabulary through both consumers. A test cannot
   prove that no second table exists anywhere; that limit is stated.
3. **`touches_db` made honest, unconditionally (S2-N1, S3-1).** d1/d2 prune
   callees by `touches_db` (`capability_query.rs:115`), which is the cone's
   `TOUCHES_TABLE` bit, set from any `resource_kind == "table"` capability fact
   (`cone_derived.rs:694-696`); their terminals are every op
   `is_db_touching_class(classify_op(op))` accepts (`op_classification.rs:84-89`:
   `DbRead | DbWrite | DbLock`). `map_op` leaves two of those unmapped: `Rename`
   (`DbWrite`) and `LockTable` (`DbLock`). Today a callee whose only database
   ops are those is already pruned, and removing `TestField` would add every
   callee that had `TestField` beside them. Fix: `map_op` gives them table
   capability facts with their own op names, `"Rename" => Some("rename")` and
   `"LockTable" => Some("lock")`. The cone then carries them transitively like
   every other fact, so `TOUCHES_TABLE` is set wherever d1/d2 have a terminal.
   This is the existing transitive machinery, not a second reachability pass
   that would have to copy the cone's traversal rules (typed edges, events,
   object runs, truncation).
   **Why new op names are safe** (answering opus's round-3 concern that facts
   surface as `TableData` rows): every consumer that turns a table op into
   something user-visible skips an op it does not know. Permissions:
   `snapshot_full.rs:677` (`let Some(right) = table_op_to_right(f.op) else
   continue`), the fingerprint query `fingerprint_query.rs:510` (same); diff
   gains count only read and insert/modify/delete (`passes.rs:809`, `:863`);
   digest effects map neither (`digest.rs:56-77`); `cone_derived` physical
   reads take only `read` (`:707`) and physical writes only
   insert/modify/delete; d44 and `program_calls.rs:347` are unaffected. So no
   permission row, no digest effect, no diff GAIN, no d44 change. What DOES
   surface (S4-1, S4-3, S4-O2): the facts in `aldump --r4f-snapshot`; in `alsem
   fingerprint`, which lists every table op per resource and fact ids
   (`fingerprint_query.rs:393-420`, `:755-767`), so a `Rename`/`LockTable`-only
   routine gains a table family with no permission line; a diff LOSS, since
   `CapabilityLost` takes any removed fact (`passes.rs:977-996`), so between two
   new snapshots a vanished `rename`/`lock` fact is reported (an old snapshot
   has none, so upgrading causes no loss churn); and policies. The policy
   vocabulary `CAPABILITY_OPS` (`policy/predicate_fields.rs:103-122`, validated
   at `predicate_compiler.rs:153-166`) gains `"rename"` and `"lock"`, so a
   policy can name them, and a user policy matching `capability.resourceKind:
   table` with no `op` filter now also matches them (every rule in the bundled
   `policy-default.yaml` already excludes the new table ops, by an `op` filter
   or by a non-table `resourceKind` filter such as `http`; S5-2). They are true facts (a rename
   writes the table; `LockTable` locks it), so exposing them is honest; the
   CHANGELOG names every surface above. `Rename` still maps to no permission letter, as #9
   requires (its TableData right is unmeasured).
   **d2 prunes on its own presence bit (S4-2, attempt 2).** `cone_derived`
   gains `TOUCHES_PHYSICAL_TABLE` (next free bit, `1 << 4`), set in `fold_fact`
   (`cone_derived.rs:694`) for a `table` fact UNLESS it is `fact_is_known_temp`
   AND its `op` is not `read`. So every `read` sets it, temporary or not, and
   only a known-temp `insert`/`modify`/`delete`/`rename`/`lock` does not.
   Why reads stay (rounds 2-3, sol): a temporary host's FlowField is still
   computed against the physical flow-target tables (`CalcFields`, d1's RV-1
   gate, `d1.rs:133`, `:151-166`), and a fold predicate may read only fields of
   the dedup key `inherited_fact_key` (`op|kind|rid|conf|temp`,
   `capability_cone.rs:1310-1318`; the dedup-invariance contract,
   `cone_derived.rs:59-64`): an `op_subtype` test would be defeated by a shorter
   temp `Count` on the same table winning the key. `op` and temp state ARE in the
   key, so this rule is dedup-invariant. Cost: a temp-read-only branch is still
   walked, as today (not widened).
   The bit is set BEFORE the unresolved-resource-id early return (`:696-700`), exactly
   like `TOUCHES_TABLE` (which stays: any table fact), with the store accessor
   `touches_physical_table(routine_id)` and `capability_query::
   touches_physical_table_derived(store, summary) -> EffectPresence` (same
   tri-state `presence` arm as `touches_db_derived`, `capability_query.rs:115-136`),
   plus a fixture-only raw oracle `touches_physical_table_of` beside
   `touches_db_of`/`may_commit` (`capability_query.rs:60-87`) in the
   derived-vs-raw parity tests (`cone_derived.rs:1289`, `:1470-1472`).
   d2 uses it at its two pruning sites: `D2Policy::expand` (`d2.rs:136`) and the
   subscriber pre-filter (`d2.rs:390`). When the callee's cone is complete
   (`presence` is `No`, not `Unknown`), a callee whose only reachable table
   facts are known-temporary NON-reads (a temp `Rename`/`LockTable`/write)
   is no longer walked and cannot spend d2's 500-node budget; an incomplete cone
   is `Unknown` and still walked, as today. d1 keeps `touches_db_derived`: its
   temporary terminals are deliberate (`d1_graph.rs:127-160`, `:237`).
   **Where the bit and d2's terminal filter disagree** (round 1, both
   reviewers). The bit reads cone facts, which are resolved in the REACHING
   routine's frame; d2's terminals read model ops in the op owner's own frame
   (`d2.rs:103-118`). Three cases, each a stated mechanism under "Outputs that
   move":
   - (e) **caller-frame temp**: `pd_solve` rewrites a parameter-dependent op's
     temp state into the caller's frame (`pd_solve.rs:221-247`, `rep_for`;
     also `trigger_receiver_temp_state`, `:231`). A routine that passes its own
     LOCAL `Temporary` record to `B(var Rec)` holds only a known-temp fact for
     B's `Rec.Modify()`, while d2's terminal sees B's op as parameter-dependent
     (a terminal). Pruned now, so that d2 finding is REMOVED. Intended: at
     runtime that `Modify` is on a temporary buffer, so the finding was a false
     positive on that path; a caller passing a physical record keeps it (the
     solver keeps the two temp states as separate facts, `pd_solve.rs:286-287`,
     `:365-369`, `capability_cone.rs:1310-1318`). A record is known-temporary only
     when its declaration says `Temporary` (`ir_walk.rs:2012-2023`); the frame
     rewrite never invents that state. Only NON-read ops: a forwarded temp
     `CalcFields` (or any read) is a `read` fact and keeps the bit (above), so (e)
     never removes a FlowField path, whichever read wins the dedup key. A2e and
     A2f pin both.
   - (f) **dead code**: the cone drops unreachable ops (`capability_cone.rs:580-584`)
     while d2 terminals read every op (`d2.rs:103`); a routine whose only
     REACHABLE table facts are known-temp NON-reads (a reachable temp read keeps
     the bit set) and whose physical op is unreachable is pruned now. Intended (no finding for dead code), pre-existing for
     `TOUCHES_TABLE`.
   - (g) **virtual/system tables and terminator `Next`** (`d2.rs:105-115`) have
     no fact-level counterpart (`op_targets_virtual_system_table` decides by the
     record variable's declared type name, `detectors/mod.rs:952-977`). A
     non-temp `Rename`/`LockTable`-only branch on a virtual table is a newly
     walked no-terminal branch, i.e. mechanism (d) on that narrow shape: stated,
     not closed. Why it is left open: the shape is meaningless AL. The listed
     tables are platform-computed (`Integer`, `Date`, `Field`, `Session`, …,
     `detectors/mod.rs:925-940`), so a `Rename` of one of their rows is not a
     working operation and a `LockTable` on one locks no SQL table; closing it
     would need the virtual-table test at the fact producer (the record
     variable's declared type name, which no fact carries), in both capability
     maps, for code that should not exist. Measurement item 1 counts
     `rename`/`lock` facts on `VIRTUAL_SYSTEM_TABLES` on CDO, and any d2 loss
     traced to it blocks the merge, as (d) does.
   The bit also narrows d2's pre-existing exposure to temp-WRITE-only branches,
   which can ADD a d2 finding such a branch used to starve (trace class (a)
   budget); temp-read-only branches keep today's exposure.
   **Invariant test:** for every op in the `RecordOpType` vocabulary,
   `map_op(op).is_some() == is_db_touching_class(classify_op(op))`. It ties the
   capability table to the class table the detectors use, and it pins
   `TestField` (`StateOnly` → `None`).
4. Nothing else changes: the `RecordOpType` vocabulary (`record_op.rs:29`,
   `record_ops.rs:20`) still records `TestField`, so d21, d60 and every other
   reader of record operations keep seeing it.

The issue's second acceptance item ("one op table serves both capability maps,
no duplicate") is already true on the base (`capability_cone.rs:169-171`
delegates); item 2 makes it structural.

**Outputs that move** (S1-2, S1-O1, S1-O7). Four kinds of change. The first
three come from facts whose `opSubtype` is `TestField`; the fourth from Design
item 3:
- **Fact removal**: the direct `read` fact of every `TestField` op disappears
  (r1a/r3a3 goldens, `aldump --r4f-snapshot`).
- **Presence loss**, only where `TestField` was the routine's or cone's only
  table fact of that kind: `cone_derived` physical table reads
  (`cone_derived.rs:707`, d44's read-after-write family);
  `TOUCHES_TABLE` (`cone_derived.rs:694-696`, any table fact) and so
  `touches_db_derived` (`capability_query.rs:115`), read by d1 (`d1.rs:723`,
  `:871`, `:1132`; `d1_graph.rs:135`, `:237`, `:287`) and d2 (`d2.rs:136`,
  `:390`); the `R` letter in the fingerprint query (`fingerprint_query.rs:291`)
  and required `TableData R` permission rows (`snapshot_full.rs:676-701`,
  `:736`); `gate/diff/passes.rs:860` (`alsem diff` of an old snapshot against a
  new one reports `CapabilityLost` for each such read); a user policy matching
  capability `op = read` (`policy/predicate_fields.rs:104`) matches fewer facts.
  d1/d2 findings CAN move (S1-O1, S2-N1). `TestField` was never their
  terminal (`d1_graph.rs:151`, `d2.rs:105` use `classify_op`, `StateOnly`), but
  they prune callees whose `touches_db` is `No`:
  (a) **budget**: d2 walks within a shared 500-node budget
  (`d2.rs:56-58`, `:390-418`; `path_walker.rs:265-287`), so pruning a callee
  that only did `TestField` can let a walk reach a real terminal it starved
  before, ADDING a finding;
  (b) **flag mismatch**: closed by Design item 3. Without it, a callee with
  `Rename`/`LockTable` and a `TestField` would lose `touches_db` and a real
  finding would disappear; with it, `touches_db` stays `Yes`, so no finding is
  lost this way;
  (c) **newly reached callees**: Design item 3 also turns on the pre-existing
  case, a callee whose only database ops are `Rename`/`LockTable`, which d1/d2
  pruned until now. That ADDS d1/d2 findings (S3-1);
  (d) **budget starvation (S4-2)**: a newly walked callee with no terminal for
  d2 (a `Rename`/`LockTable` on a known-temporary record: d2's terminals skip
  known-temp ops, `d2.rs:117-118`, while `TOUCHES_TABLE` takes any table fact)
  spends d2's shared budget and can starve a real terminal, LOSING a finding.
  This is the same exposure every already-walked branch has today (a temp
  read sets `TOUCHES_TABLE` too); preventing it means redesigning d2's budget,
  which is not this issue. Attempt 2 closes it for d2 with d2's own presence
  bit (Design item 3, "d2 prunes on its own presence bit"); d1 is not exposed,
  because a known-temp `Rename`/`LockTable` IS a d1 terminal. The remaining d2
  exposure: terminator `Next` (pre-existing, not widened) and virtual/system
  tables, which Design item 3 (c)/(g) DOES widen for a `Rename`/`LockTable`-only
  branch on a virtual table (stated, measured, a loss blocks the merge). So the
  claim is still not "additions only" in general.
  (e) **caller-frame temp**, (f) **dead code** and (g) **virtual tables**: the
  three places d2's new pruning bit and its terminal filter disagree, defined
  under Design item 3. (e) and (f) REMOVE d2 findings and are intended
  (false positives on a temporary buffer, and dead code); (g) is a narrow case
  of (d).
  Measurement item 1 diffs d1/d2 payloads on CDO at both scopes and traces
  every change to (a), (c), (d), (e), (f) or (g). A loss traced to (d), (g) or to
  no cause blocks the merge; a loss traced to (e) or (f) is recorded with its
  source site.
- **Witness replacement**, where the read SURVIVES: the cone keeps one
  inherited fact per `op|kind|rid|conf|temp` key (`capability_cone.rs:1310-1318`)
  and required permissions keep the first row per `subject|table|right`
  (`snapshot_full.rs:690-693`), so when the kept representative was the
  `TestField` fact, a `Get` on the same table becomes the witness
  (`witnessOperationId`, `opSubtype` change, the read stays).
- **Fact addition**: a direct `rename` / `lock` table fact for every `Rename`
  / `LockTable` op, its inherited copies in callers, `TOUCHES_TABLE` newly set
  for their cones, and the d1/d2 findings of mechanism (c) above.
Golden triage (A4) accepts exactly these four kinds; anything else is
unexplained.

**Expected golden moves** (S1-O5, from grep): r1a `ws-d21.l2` (3 facts,
`opSubtype: TestField`) and `ws-member-call-codeunit.l2` (1); r3a3
`ws-d21.r3a3` (3) and `ws-member-call-codeunit.r3a3` (the `OnInsert` trigger's
`Rec.TestField` at `tests/r0-corpus/ws-member-call-codeunit/src/FileArchive.Table.al:17`
reaches its callers). Design item 3's fact addition moves NO r1a/r3a3 golden: grep over `tests/**/*.al` finds `Rename` only in `tests/r0-corpus/ws-d60/src/Codeunit.al` and `LockTable` nowhere; `ws-d60` feeds r4, r4f and `l2_features.snapshot`, which may move only by the new facts or mechanism (c). A2b's fixtures are the only coverage of the new facts (S4-O3). Expected NOT to move: r2a `l3rt` (record ops), r0, r4
`ws-d21` (d21 reads record ops), `l2_features.snapshot` (verify). Any other
moved golden is unexplained until traced.

**CHANGELOG** (S1-O7): a `Fixed` entry naming the change, the new `rename` / `lock` table capability facts (no permission letter; visible in `--r4f-snapshot`, `alsem fingerprint`, diff losses between new snapshots, and policies: new `CAPABILITY_OPS` values, and an op-less `resourceKind: table` predicate now matches them; d1/d2 now see callees whose only database ops are `Rename`/`LockTable`), the `CapabilityLost`
diff churn against older snapshots, the narrower `op = read` policy matches,
the removed `TableData R` rows, and the JIT residual.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| A `TestField`-only routine gets a capability `read` fact today | TRUE | baseline `aldump --r4f-snapshot` on a one-routine fixture (session scratchpad `100/probe-ws`): `capabilityFacts[0] {op: read, resourceId: …/table/50100, extra.opSubtype: TestField}` |
| … and no L4 db-effect row | TRUE | baseline `alsem query effects <fixture> --routine OnlyTestField`: `effectCount 0` |
| The engine elsewhere treats `TestField` as state-only | TRUE | `src/engine/l5/op_classification.rs:65` `"TestField" => OpEffectClass::StateOnly`; d21 doc `detectors/d21.rs:4` |
| Both capability maps share one op table | TRUE | `capability_cone.rs:169-171` calls `table_commit::map_op`; no other capability PRODUCER maps `TestField` (grep: the only other `"TestField" =>` arm is `op_classification.rs:65`, an op CLASS table, not a capability map; `digest.rs:56`'s `map_op` maps capability ops to effect types and returns `None` for `read`) |
| `TestField` performs no database read | DOCUMENTED, not measured | Microsoft Learn: Record.TestField "Tests whether the contents of a field match a given value" on the record variable. Engine consistency above. A BC-container measurement (`bc-measure`) is not part of this change |

## Base/Head SHAs

- Attempt 2: `B` = `origin/master` = `214a7879098c161b266085c8f37b27386148a424` (the worktree base; `origin/master` did not move, so step 11 needed no rebase). `H` = `725fe4a0f71973e164ed2fb7e69fa18899de9b56` (gates and CDO re-measure on this head). `check-diff --base B --head H`: no reasons.

## Timeline

- make-worktree: `{worktree: U:\Git\al-sem-issue-100-a1, base: 60522971}`.
- baseline CDO (base `60522971`, release-fast, workspace `bc3ccb18`, 0 dirty paths): fingerprint 6.4 s, analyze primary 37.7 s (2277 findings, d44 20), analyze all 38.2 s (30713 findings, d44 258). Binaries and outputs: session scratchpad `100/before`, `100/measure/before`.
- classify: bounded; probes: 5 rows.
- spec round 1 (sol + opus): 10 entries. JIT residual, three kinds of output change, full-payload d44 triage, wrapper deletion + vocabulary agreement test, d1/d2 consumers named.
- spec round 2: sol re-raised S1-1 (platform-loaded records) and S1-O1 (d2 budget); opus raised S2-N1 (TOUCHES_TABLE vs classify_op: Rename/LockTable), proposed a conditional CDO gate.
- spec round 3: BOTH rejected the conditional gate (S3-1): fix the flag unconditionally.
- spec round 4: Design item 3, `map_op` gives `Rename`/`LockTable` their own table capability ops (`rename`, `lock`); opus accepted the mechanism on evidence (no permission row, digest effect, diff gain or d44 change). sol re-raised A2b's d2 topology; new: policy vocabulary (S4-1), budget starvation (S4-2), diff loss asymmetry, surfaces, goldens, cost.
- spec round 5: opus accepted all 24 entries, one new minor S5-1 (wording). sol accepted 23, re-raised S4-2 (the suggested TOUCHES_TABLE temp gate would prune d1's temp terminals; needs a d2-specific mitigation and an adversarial fixture), one new minor S5-2 (bundled-policy wording). Both minors fixed in the ledger; S4-2's direction recorded under mechanism (d).
- `charge spec_rounds` remaining 0. Not converged (S4-2 re-raised by sol; S5-1/S5-2 unmarked), handoff rule not met (unmarked entries). Outcome: `blocked spec-panel-cap`. Caps used: spec_rounds 5/5, pi_calls 5, subagents 5 (+ ranking agent).
- **attempt 2** (run `20261019-004356-4443f1`, base `214a7879`): claim + make-worktree `{base: 214a7879, target: H:\al-sem-targets\al-sem-issue-100-a2}`. Seeded from attempt 1; citations re-checked (Claim section). S4-2 addressed per its recorded direction: Design item 3 "d2 prunes on its own presence bit" + acceptance row A2d; mechanism (d) text updated.
- attempt 2 baseline CDO (base `214a7879`, release-fast, workspace `bc3ccb18`, 0 dirty paths), via `cdo-measure`: fingerprint 6.7 s, analyze primary 45.7 s, analyze all 44.5 s, all exit 0. Binaries and outputs: session scratchpad `100b/before`, `100b/measure/before`.
- attempt 2 spec round 1 (sol `spec-r1-sol.md`, opus `spec-r1-opus.md`, under `.agent/runs/20261019-004356-4443f1/`): opus accepted all 25; sol accepted 24 and re-raised S4-2 (virtual-table branches). BOTH found the frame mismatch (pd_solve caller-frame temp vs d2's own-frame terminal). New: A2-1 (frame mismatch, important, both), A2-2 (virtual tables, sol's S4-2 re-raise), A2-3 (raw oracle), A2-4 (A2d wording), A2-5 (dead code), A2-6 (count 25). Fixed: mechanisms (e)/(f)/(g), `Unknown` qualifier, bit set before the rid early return, raw-oracle parity, A2d rewritten, new A2e.
- attempt 2 spec round 2 (`spec-r2-sol.md`, `spec-r2-opus.md`): opus accepted all 32, three minors. sol accepted 28, re-raised S4-2/A2-2 ((g) contradicted by the old mechanism (d) sentence), A2-1 (A2e assertions per event), A2-6 (stale "Not delivered"); new important: a temp host's FlowField is physical work, so (e) must not remove it. Fixed: the bit keeps known-temp `FLOWFIELD_GATED_OPS` facts (d1's constant), (e) restated, new A2f, A2e on two events, (g)'s reason stated (opus) and the contradicting sentence fixed, A2d wording (demand, non-temp no-rid pin, temp `CalcFields` pin), "Not delivered" refreshed; discovery candidate 1 (d2 drops temp FlowField terminals). New register entries A2-8..A2-12.
- attempt 2 spec round 3 (`spec-r3-sol.md`, `spec-r3-opus.md`): opus accepted all 37, three minors (stale "temp read" wording; the `SetAutoCalcFields` half could never fire, `map_op` gives it no fact; the raw oracle must apply the same rule). sol accepted 36, re-raised A2-8: the `op_subtype` clause is not dedup-invariant (`inherited_fact_key` lacks `op_subtype`, so a shorter temp `Count` hides the `CalcFields`). Fixed by changing the rule: the bit is clear only for a known-temp NON-read; every `read` sets it (dedup-invariant: `op` and temp state are in the key). (e) restated; A2d pins and A2f (dedup variant, discrimination) updated. New entries A2-13..A2-16.
- attempt 2 spec round 4 (`spec-r4-sol.md`, `spec-r4-opus.md`): both confirmed the new rule is dedup-invariant (key, guard merge, retag, pd_solve rewrite, SCC fold), that (e) now removes only temp-buffer writes/rename/lock, and that A2f's dedup variant can fail. opus accepted all 41; sol accepted 40, re-raised minor A2-14 (mechanism (f) wording). Both minors fixed in the ledger: (f) says known-temp NON-reads; discovery 1 lists only `CalcFields` (A2-17).
- attempt 2 spec round 5 (`spec-r5-sol.md`, `spec-r5-opus.md`): BOTH accepted all 42 entries against the current hashes; no blocking findings, none re-raised. **Converged** (executor rule `_register_failures` over roster sol/opus: no failures). Caps: spec_rounds 5/5, pi_calls 5, subagents 7 (+ ranking).

- step 6 acceptance tests (opus): 8 red at runtime + 4 lib pins compile-red; see "Acceptance tests".
- step 7 plan `docs/superpowers/plans/2026-10-19-issue-100-testfield-capability.md` (2 tasks, `plan_tasks` charged 2), committed `d1ca271f`.
- step 8 Task 1 (opus implementer, sonnet review approve + 1 important: per-site proofs; sonnet fix added `a2e_subscriber_forwarding_its_own_temp_is_pruned_by_the_prefilter`). Task 2 (opus implementer; sonnet review approve, 3 minors: two fixed by the conductor (a `use` for `table_commit`, a CHANGELOG clause on budget-freed d2 additions), one recorded as a Ruling). `ci-steps task` exit 0 (3440 tests, nextest). `check-diff` (base master, head HEAD): no reasons. Commit `aa54319d` (pre-commit goldens OK, 872 tests).

- step 10 final panel: `code-review` skill (high) on the branch: 9 findings (F1-CR1..9; 4 fixed in `ec5106ee`, 5 refuted). Round 1 (`final-r1-*.md`): opus 51/51 accepted + 1 important (re-gate on the attested head) + 3 minor; sol 49/51 + 1 important (same) + 1 minor. Fixed: proof for the rename/lock A2d test, CHANGELOG d44 + dead-code wording (`725fe4a0`), "Not delivered"; re-gate on `725fe4a0` all green; CDO re-measured, identical; peak working set measured. Round 2: opus 55/55; sol 54/55 (S4-O4: both scopes). Primary-scope peak measured. Round 3: both 55/55, no new findings. **Converged.** Caps: final_rounds 3/5.
- step 11: `git fetch`; `origin/master` = base `214a7879`, no rebase.

## Plan notes (round 5, non-blocking, carried into the plan)

- A2f competing-read variant: assert that `Count` actually wins the inherited witness at `S3` (the subscriber pre-filter), so the dedup precondition is established (sol, opus).
- A2d: reach >500 visits with a diamond of calls unless the test writer confirms repeated call sites are separate edges (opus).
- The bit's fold (`cone_derived::fold_fact`) and the raw oracle `touches_physical_table_of` call ONE shared predicate (e.g. `fact_sets_physical_table_bit(&CapabilityFact) -> bool`), so the rule has one copy (opus).
- Mechanism (f) also covers dead code beside reachable temp NON-read facts: intended (opus).

## Acceptance tests (step 6)

Written by an opus subagent (report `.agent/runs/20261019-004356-4443f1/acceptance-tests.md` in the main checkout), not committed (they go in with the implementation). Files: `tests/gap/gap_issue100_testfield_capability.rs` (new), `tests/gap/main.rs` (one `mod` line), `src/engine/l5/capability_query.rs` (a test module at the end).
- `cargo test --test gap gap_issue100` at base: **8 FAILED, 5 ok** (verified by the conductor). Red: `a1_testfield_is_not_a_capability_read_in_the_snapshot`, `a1_..._in_the_l2_projection`, `a2_fact_exists_iff_db_touching_class`, `a2b_d1_sees_rename_and_locktable_callees`, `a2b_d2_sees_rename_and_locktable_callees`, `a2c_policy_vocabulary_accepts_rename_and_lock`, `a2d_d2_not_starved_by_temp_write_branch`, `a2e_temp_forwarded_to_var_param_write_is_pruned`. Green by nature (red only under their discrimination step): `a2_l2_and_cone_agree_over_the_whole_vocabulary`, `a2d_d2_not_starved_by_temp_rename_lock_branch`, `a2f_temp_forwarded_calcfields_keeps_the_finding`, `a2f_dedup_variant_count_wins_witness_and_finding_stays`; plus the control `a2d_controls_physical_first_and_small_diamond_are_found`.
- `--lib` unit pins (bit pins, raw-oracle parity): compile-fail naming exactly the missing symbols `TOUCHES_PHYSICAL_TABLE`, `fact_sets_physical_table_bit`, `touches_physical_table_of`, `touches_physical_table_derived`, `ConeDerivedStore::touches_physical_table`.
- Probes: A2d, d2 walks the temp branch first (call-site order; an 8-layer diamond reaches its leaf within depth 20, so the loss is the 500-visit budget; controls: physical-first and a 2-layer diamond both find it). A2f, `Count` wins `S3`'s inherited read witness (asserted through the full snapshot before the finding).
- Ruling: A2d as written (temp `Rename`/`LockTable` branch) is green at base, because those ops have no capability fact yet, so the branch is already pruned; it goes red only in the intermediate state where `map_op` maps them but d2 still prunes on `touches_db`. Kept as written, plus a known-temp `Modify` variant (same rule, red at base). Cost if wrong: none, both run.
- Process note: the subagent appended the `capability_query.rs` test module with a shell heredoc instead of Edit (repo rule); the content is reviewed in the diff and goes through the task review.

## Acceptance matrix

| # | Acceptance item | Proof |
|---|-----------------|-------|
| A1 | TestField produces no capability read fact | Test through the snapshot pipeline (the use, not `map_op`), on a program-built fixture with: `OnlyTF` (only `Rec.TestField`), `GetOnly` (`Rec.Get`), `Mixed` (`Rec.Get` then `Rec.TestField`), `Caller` calling `OnlyTF`, and `Both` calling `OnlyTF` then `GetOnly` (competing inherited branches, S2-1). The composed snapshot: `GetOnly` and `Mixed` keep a `read` (Mixed's witness is the `Get`); `Both` keeps an inherited `read` whose witness is `GetOnly`'s `Get` (witness replacement); `OnlyTF` and `Caller` have no `read` on that table and no `TableData R` row; `TestField` is still in `OnlyTF`'s record operations (the negative is not vacuous, S1-4). L2 half through the L2 workspace path that fills `capability_facts_direct` (`l2_workspace.rs:352`, the r1a projection), not `extract_table` (S1-O4). Discrimination: restore `TestField` in `map_op` → both halves fail |
| A2 | One op table serves both capability maps | structural: the cone's wrapper is deleted (Design item 2). Test: for EVERY name in the `RecordOpType` vocabulary, the L2 workspace facts and the cone's direct facts agree on whether (and as what op) it is a capability fact. Discrimination: add a private copy of the CORRECTED table in the cone with one entry changed → the agreement test fails (S1-4). Plus the invariant test of Design item 3 (`map_op(op).is_some() == is_db_touching_class(classify_op(op))` over the vocabulary). Stated limit: no test proves no second table exists anywhere |
| A2b | d1/d2 still see a callee whose database ops are `Rename`/`LockTable` beside a `TestField`, and newly see a `Rename`/`LockTable`-only callee | Tests through the detectors (the use), four callee shapes on physical records: (i) `Rec.TestField(F); Rec.Rename(…)`, (ii) `Rec.TestField(F); Rec.LockTable()`, (iii) only `Rec.Rename(…)`, (iv) only `Rec.LockTable()`. **d1**: a caller calls each callee inside a loop; each d1 finding asserted present. **d2** (it fires only through event dispatch, `d2.rs:329-349`): a publisher raised inside a loop, with a subscriber that calls each callee; each d2 finding asserted present. Discrimination: remove the `Rename`/`LockTable` arms from `map_op` → every shape loses its findings (i, ii: the regression this issue would otherwise ship; iii, iv: the pre-existing gap closed) (S2-N1, S3-1, S4-S1) |
| A2d | d2 is not starved by a known-temporary branch (S4-2) | d2 test through the detector: an event publisher raised inside a loop; its one subscriber has no physical op of its own, and first calls a temp branch, then a callee with a physical `Rec.Modify()`. In the temp branch every table op is a `Rename`/`LockTable` on a LOCAL `Temporary` record variable of the routine that does it (not a parameter: a forwarded temp is case (e)), and every routine's cone is complete. The branch DEMANDS more than 500 walk visits before the physical callee (the cap stops the walk at 500, so that is what spends the budget); the walker counts visits, not distinct routines (no global visited set, `path_walker.rs:222`, `:265-275`), so a few routines with repeated call sites or a diamond reach it within depth 20. Assert the d2 finding for the physical callee is present. The test writer first confirms and records that the temp branch is walked first (`edges_by_from` order). Discrimination: switch d2's two pruning sites back to `touches_db_derived` → the finding disappears (budget exhausted, `NodeBudgetCut`). Plus unit pins: a routine whose only table fact is a known-temp `rename` (and, separately, `lock` and `modify`) has `TOUCHES_TABLE` set and `TOUCHES_PHYSICAL_TABLE` clear; a known-temp `read` sets both; a NON-known-temp fact with no resource id sets both; and the raw-oracle parity test, whose oracle `touches_physical_table_of` applies the SAME rule (Design item 3) |
| A2e | Case (e): a temporary record forwarded to a `var` parameter | d2 test through the detector. d2 reports one finding per looped publish (`d2.rs:13-18`, `:461-479`), so TWO publishers, each raising its own event inside a loop: event `E1`'s only subscriber `S1` (no physical op) calls `A`, which passes its LOCAL `Temporary` record to `B(var Rec: Record X)`, which does `Rec.Modify()` → no d2 finding for `E1`; event `E2`'s only subscriber `S2` calls `A2`, which passes a PHYSICAL record to the same `B` → the `E2` finding is present. Discrimination: switch d2 back to `touches_db_derived` → the `E1` finding reappears (the (e) removal is caused by the new bit); the `E2` finding is present in both versions |
| A2f | Case (e) does not remove a FlowField path | d2 test through the detector: event `E3` raised inside a loop; its only subscriber `S3` calls `A3`, which passes its LOCAL `Temporary` record to `C(var Rec: Record X)`, which does `Rec.CalcFields(<a FlowField>)` → the `E3` finding is present. A second variant (sol round 3, the dedup case): `A3` also does `n := T.Count()` on its local temp `T` BEFORE calling `C(T)`, so a distance-1 temp `read` competes with the distance-2 `CalcFields` read under one dedup key → the `E3` finding is still present. Discrimination: make the bit exclude every known-temp fact (reads included) → the `E3` finding disappears in both variants |
| A2c | The policy vocabulary accepts the new ops | policy test: a predicate `capability.op: [rename]` (and `[lock]`) compiles and matches the A2b fixture's fact; an existing `op: [read]` and a negated predicate still compile and evaluate as before (S4-1) |
| A3 | d44 findings on CDO re-triaged at both `--scope` values | measurement: `cdo-measure` before/after; compare COMPLETE d44 finding payloads (not ids) at `--scope primary` and `--scope all`: removed ids, changed ids (same `d44-rw/{event}\|{table}` id, shorter reader list in the text/evidence, `d44.rs:231-266`), and cap-induced additions (the shared per-event cap of 32 can expose a previously hidden finding, `d44.rs:297-311`); every one triaged against source and recorded here (S1-3) |
| A4 | Nothing else moves without a cause | `check-goldens`: every moved golden line is one of the four kinds in "Outputs that move" (fact removal, presence loss, witness replacement, fact addition) and in the expected list, checked by `golden-diff-triager`; `cdo-measure` fingerprint and other detectors' deltas explained; every d1/d2 change on CDO traced to mechanism (a), (c), (d), (e), (f) or (g); a loss traced to (d), (g) or to no cause blocks the merge |

## Measurement plan

On the pinned CDO baseline (`CDO_WS`), release-fast, before (base) and after (HEAD):
1. `cdo-measure`: findings per detector at both scopes, fingerprint added/removed. d44: full-payload diff (A3). d1/d2: full-payload diff at both scopes; every change traced to mechanism (a) budget, (c) newly reached `Rename`/`LockTable` callee, (d) budget starvation, (e) caller-frame temp, (f) dead code or (g) virtual tables; a loss traced to (d), (g) or to no cause blocks the merge, and each (e)/(f) loss is recorded with its source site. Plus the count of `rename`/`lock` table facts whose record variable's declared type is on `VIRTUAL_SYSTEM_TABLES` (the (g) population). Also `alsem analyze` wall time and peak working set before/after at both scopes (baseline 37.7 s / 38.2 s), and the cone's inherited-fact count, because every `lock` fact is a new dedup key inherited by its callers (`capability_cone.rs:1310-1318`; S4-O4). Every other detector change explained.
2. Required `TableData R` rows removed (from `fingerprint` / the full snapshot's required permissions, `snapshot_full.rs:676-701`): count before/after, and a sample traced to `TestField`.
3. `aldump --program-call-graph-stats` byte-identical (resolver untouched).
4. BC container (`bc-measure`), if `Cronus28` is available: `SqlStatementsExecuted` around `Rec.TestField(F)` on a fully loaded record (expected 0) and on a record loaded with `SetLoadFields` excluding `F` (expected a JIT re-read). If the container cannot run it, the claim stays documented and the residual stated (S1-1, S1-O8).

## Discrimination proofs

Task 1 (d2's own presence bit; implementer report `.agent/runs/20261019-004356-4443f1/task1-report.md`). Each break made with Edit and checked applied before the run, reverted with Edit, re-run green. Not yet committed (Task 2 commits code and tests together).

| Test(s) | Break | Fail output | After revert |
|---|---|---|---|
| `a2d_d2_not_starved_by_temp_write_branch`, `a2e_temp_forwarded_to_var_param_write_is_pruned` | (i) `d2.rs:137` and `:392` back to `touches_db_derived` | a2d `d2 must reach Physical's Modify past a temp Modify branch: []`; a2e `E1 (temp record forwarded to B) has no finding` assertion; gap `4 passed; 2 failed` | gap `6 passed` |
| `a2f_temp_forwarded_calcfields_keeps_the_finding`, `a2f_dedup_variant_count_wins_witness_and_finding_stays`, `known_temp_read_sets_both_bits` | (ii) `cone_derived.rs:212` predicate excludes every known-temp fact (reads too) | both a2f `left: 0 right: 1`; lib `known_temp_read_sets_both_bits FAILED` | gap `6 passed`, lib `11 passed` |
| `non_temp_fact_without_resource_id_sets_both_bits`, `physical_table_derived_matches_raw_oracle` | (iii) `cone_derived.rs:720-725` bit set moved after the `resource_id` early return | both FAILED (`left: 0 right: 0`; `r/no-rid: derived vs raw left: No right: Yes`) | lib `11 passed` |
| `known_temp_non_reads_set_touches_table_but_not_the_physical_bit` | (iv) predicate ignores temp state | `rename: a known-temp non-read must not set TOUCHES_PHYSICAL_TABLE left: 16 right: 0`; parity `left: Yes right: Unknown` | lib `11 passed` |

Task 1 review (sonnet, `task1-review.md`): approve, 1 important: proof (i) reverted BOTH d2 sites together. Review fix (`task1-fix.md`), one site at a time:

| Test | Break | Fail output | After revert |
|---|---|---|---|
| `a2d_d2_not_starved_by_temp_write_branch` | `D2Policy::expand` only back to `touches_db_derived` | fails at `gap_issue100_testfield_capability.rs:855` | baseline |
| `a2e_subscriber_forwarding_its_own_temp_is_pruned_by_the_prefilter` (NEW: subscriber `S1` passes its own LOCAL `Temporary` record to `B(var Rec)`; control `S2` with a physical record) | subscriber pre-filter only back to `touches_db_derived` | fails at `:1046`, a finding appears for `OnE1` | gap 8 pass / 6 fail (the 6 = Task 2's tests) |

The fixer first concluded the pre-filter could not be pinned; the conductor pointed out subscriber-level (e) (the subscriber's own row is caller-frame known-temp while B's own PD fact sets B's bit), and the probe confirmed it.

Final round 1 (opus minor): `a2d_d2_not_starved_by_temp_rename_lock_branch` had no failing break (green at base, before `map_op` mapped rename/lock). Proof at HEAD `ec5106ee`, by the conductor: both d2 sites back to `touches_db_derived` (Edit, grep count 2) → it FAILS at `gap_issue100_testfield_capability.rs:840` (with a2d-write `:855`, a2e `:991`, a2e-subscriber `:1046`); reverted with Edit (no diff against HEAD) → 14 passed.

Task 2 (implementer report `task2-report.md`): goldens moved only in the 4 expected files (r1a/r3a3 `ws-d21`, `ws-member-call-codeunit`), every moved line a removed `TestField` fact (kind: fact removal); gap file 14 passed, `--lib` 2108 passed, `ci-steps task` exit 0; 4 discrimination proofs fail-on-break / pass-after-revert (rows in the report).
Task 2 Ruling: `tests/r3a3-goldens/manifest.json` matrix numbers are hand-maintained Rust-owned oracles (not written by the regen); updated with Edit to the current Rust values, as #96 did. Cost if wrong: a stale oracle the r3 test would flag.
Task 2 Ruling: `tests/r3/r3_temp_class_parity_arena.rs` a5's "a read producer is skipped in this fixture" guard listed `ws-d21`, whose only read producer was a `TestField`; replaced by `ws-path-facts` (7 read producers). The guard still covers three fixtures and the parity comparison is unchanged; this is a fixture list, not a rescued assertion. Cost if wrong: one fixture less coverage of the verb filter.

Task 1 Ruling: the plan's Step 5 gap command needs `--` before several libtest filters (`cargo test --test gap -- f1 f2 f3`); cargo rejects the form without it. Cost if wrong: none.

## Findings register summary

55 entries, all `accepted` by both reviewers (sol = gpt-6.1-sol in the astra slot, opus = local Opus in the flash slot; operator's substitution) against the current hashes; executor rule `_register_failures` reports none. Spec panel: 42 entries (S1..S5 from attempt 1, A2-1..A2-17), converged in round 5. Final panel: F1-CR1..CR9 from the `code-review` skill (4 fixed, 5 refuted with evidence), F1-N1..N4 from round 1 (all fixed); converged in round 3. Severity: 0 critical; important: S1-1..S1-4, S1-O1, S2-N1, S3-1, S4-1, S4-2, A2-1, A2-2, A2-8, A2-13, F1-CR1, F1-CR4, F1-CR5, F1-N1, all fixed or refuted, none deferred.

## Gate results

Step 9 on `aa54319d` (tree `f635c9de2dc1`), each through `agentflow run`, all `supervised: true`; `git status --porcelain -- . ':!.agent'` empty afterwards. Logs under `.agent/runs/20261019-004356-4443f1/logs/` in the main checkout.

| Gate | Exit | Seconds | Queued | Log |
|---|---|---|---|---|
| `ci-steps-all` | 0 | 821.1 | 810.1 (behind the other session's gate) | `ci-steps-all/20261019T022442-cc36e748.log` |
| `check-goldens-coverage` | 0 | 0.7 | 0 | `check-goldens-coverage/20261019T023824-aa739c8d.log` |
| `check-goldens` | 0 | 40.6 | 0 | `check-goldens/20261019T023825-05bdce20.log` |
| `cdo-gate` | 0 | 278.5 | 0 | `cdo-gate/20261019T023907-2d98d41f.log` |

**Re-gate on the final head `725fe4a0`** (after the final-panel round 1 fixes `ec5106ee` + `725fe4a0`), same supervisor, all `supervised: true`, tree clean afterwards:

| Gate | Exit | Seconds | Queued | Log |
|---|---|---|---|---|
| `ci-steps-all` | 0 | 671.1 | 0 | `ci-steps-all/20261019T032814-0ead8af9.log` |
| `check-goldens-coverage` | 0 | 0.3 | 0 | `check-goldens-coverage/20261019T033925-34116f84.log` |
| `check-goldens` | 0 | 28.8 | 0 | `check-goldens/20261019T033926-af58bf73.log` |
| `cdo-gate` | 0 | 183.1 | 0 | `cdo-gate/20261019T033956-d80f6d40.log` |

CDO re-measure at `725fe4a0` (release-fast, same pinned workspace, 0 dirty): primary 2277 → 2277 (0 fingerprint changes); all 30713 → 30712, the same one d44 removal; nothing else moved. **Peak working set** (S4-O4), `alsem analyze <CDO> --scope all`, interleaved, psutil tree-RSS sampler at 0.5 s: before 2.80 / 2.78 GB (51.3 / 52.2 s), after 2.79 / 2.78 GB (48.1 / 49.8 s): unchanged within noise. `--scope primary` (final round 2, sol): before 2.78 / 2.79 GB (53.9 / 51.5 s), after 2.78 / 2.79 GB (50.1 / 50.0 s): unchanged.

### CDO measurement (A3, measurement items 1-4)

The pinned CDO baseline (`CDO_WS`) at `bc3ccb18`, 0 dirty paths, on both sides (valid). Release-fast binaries: before = base `214a7879`, after = `aa54319d`. `cdo-measure` run + comp; triage report `.agent/runs/20261019-004356-4443f1/cdo-triage.md` (main checkout).
- Wall: fingerprint 6.7 → 8.5 s, analyze primary 45.7 → 45.1 s, analyze all 44.5 → 47.6 s (loaded dev machine; indicative only). All exit 0.
- Findings: `--scope primary` 2277 → 2277, no fingerprint change. `--scope all` 30713 → 30712: d44 258 → 257, every other detector (d1 and d2 included) unchanged at both scopes. So no d1/d2 change to trace; no (d)/(g) loss.
- d44 full-payload diff: primary 20 → 20, 0 changed payloads; all: one REMOVED, `d44-rw/…/Codeunit/80/event/onbeforepostsalesdoc|…/table/36` (Sales Header), 0 changed payloads, 0 added. Traced: the reader subscriber `CDO Subscribers.OnBeforeSalesPost` (cu 6175310, `CDOSubscribers.Codeunit.al:1072`) held exactly ONE table-36 fact in its cross-app cone, a `TestField` read in `CTS-CDN OIOUBL Chk. Sales Hdr.` (cu 6086277), reached through `RunPrePostValidation`; 0 table-36 db effects before or after. Kind: presence loss (intended).
- Required `TableData R` rows (item 2): 20666 → 20620; 46 rows lost, each carried only by `TestField` (sample traced to source); 32 rows changed witness only.
- Fingerprint: 230 `TestField` read facts removed (115 direct, 115 inherited); 1040 `rename`/`lock` facts added; 88 inherited reads changed witness (pair 1:1 with removed facts under the dedup key); 27 inherited reads lost outright. Nothing else moved.
- (g) population: exactly 0 of 264 distinct `rename`/`lock` ops in the cross-app model target a `VIRTUAL_SYSTEM_TABLES` table (a source scan of the ~13k `.al` files agrees).
- `aldump --program-call-graph-stats`: byte-identical (sha256 `c032c8db…`), item 4.

Executor gate-run records (`gate-runs.jsonl`): every gate on tree `f635c9de2dc1`, clean before and after, `rustc 1.96.0`, grammar clean, `cdo-gate` with the CDO workspace identity (`CDO_WS` = the pinned baseline, passed per call).

## Discoveries

Candidates (JSON written at step 10):
1. d2's terminal filter drops every known-temp op (`d2.rs:117-118`), including a temporary host's `CalcFields`, which still queries the physical flow-target tables (`SetAutoCalcFields` is never a d2 terminal at all); d1 keeps them through its RV-1 FlowField gate (`d1.rs:133`, `:151-166`). So d2 misses a FlowField calculation on a local temporary record inside an event fan-out. Pre-existing; this issue only keeps the new bit from widening it (Design item 3).

2. `Rename` is now a capability fact (`rename`) but no consumer counts it as a WRITE: `cone_derived::write_op_bit` (physical writes, `pending_writes`, d43/d44), the digest's `map_op` (DB_* effects, `db_write` roots, WRITE_PENDING_AT_UI) and the diff's `gained_kind_for` know only insert/modify/delete, and `table_op_to_right` gives it no permission letter (#9, unmeasured). A rename changes a row's primary key, a real table write. Deliberately out of scope here (no detector output moves); capability: the write consumers and the permission map treat `rename` as a write once its TableData right is measured (code review F1-CR1/2/3/6).

Filing JSON (the shape `/orchestrate` step 10 files):

```json
[
  {"subsystem": "l5 / d2 terminal filter (src/engine/l5/detectors/d2.rs)",
   "locator": "src/engine/l5/detectors/d2.rs:117-118 (D2Policy::terminals_at, is_known_temp filter)",
   "symptom": "d2 drops every known-temporary record op as a terminal, including CalcFields on a temporary host, whose FlowField is still computed against the physical flow-target tables; d1 keeps such ops through its RV-1 FlowField gate (d1.rs:133, :151-166). So d2 misses a FlowField calculation on a local temporary record inside an event fan-out.",
   "kind": "bug", "origin_issue": 100,
   "reproducer": "Event raised inside a loop; its subscriber declares `TempRec: Record X temporary;` and calls `TempRec.CalcFields(<a FlowField of X>)`. alsem analyze --detector d2-event-fanout-in-loop gives no finding; the same code with d1's loop shape gives the d1 temp-FlowField note.",
   "pre_existing": true,
   "capability": "d2's terminal filter applies d1's FLOWFIELD_GATED_OPS gate (flowfield_gate_blocks_downgrade) instead of dropping every known-temp op.",
   "acceptance": "A d2 test through the detector: a looped publish whose subscriber does CalcFields on a FlowField of a local temporary record has a d2 finding; the same with a Normal field (confirmed non-FlowField) has none."},
  {"subsystem": "capability consumers of `rename` (cone_derived write ops, digest effects, diff gains, permission map)",
   "locator": "src/engine/l4/cone_derived.rs:130-137 (write_op_bit); src/engine/l5/digest.rs:56-75 (map_op); src/engine/gate/diff/passes.rs:856-865 (gained_kind_for); src/engine/l5/fingerprint_query.rs:289-297 and snapshot_full.rs (table_op_to_right)",
   "symptom": "Since #100 `Record.Rename` is a capability fact (`rename`), but no consumer counts it as a table WRITE: physical writes / pending_writes / d43 / d44, the digest's DB_* effects and WRITE_PENDING_AT_UI, and diff gains know only insert/modify/delete, and it maps to no TableData permission letter (its right is unmeasured, #9). A rename changes a row's primary key.",
   "kind": "enhancement", "origin_issue": 100,
   "reproducer": "A routine with `Cust.Rename('NEW'); Commit();` on a physical record: alsem digest shows no DB_* write effect for it, and alsem fingerprint shows a `rename` fact with no permission line.",
   "pre_existing": false,
   "capability": "Once Rename's TableData right is measured (#9), `rename` is a write in write_op_bit, the digest map_op, gained_kind_for and table_op_to_right.",
   "acceptance": "Tests through each consumer on a Rename fixture: a physical write in writes_physical_tables_of, a DB_MODIFY-class digest effect, a CapabilityGainedWrite diff, and the measured permission letter; CDO d43/d44 deltas triaged."}
]
```

(The pre-existing `Rename`/`LockTable` pruning gap, a round-2 discovery candidate in attempt 1, is now delivered work: Design item 3.)

## Not delivered

Every acceptance row is met (A1, A2, A2b, A2c, A2d, A2e, A2f by tests; A3 by the CDO measurement; A4 by the golden triage and CDO deltas). Not delivered, by design: `rename` as a WRITE in the write consumers and a permission letter for it (discovery candidate 2, #9); d2's terminal filter for temp FlowFields (discovery candidate 1); mechanism (g) is a stated residual (population 0 on CDO); the JIT re-read residual (measurement item 4, BC container) was not measured.
