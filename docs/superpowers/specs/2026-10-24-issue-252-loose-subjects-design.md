# #252: each detector states its loose subject

Status: revision 2 (spec panel round 1: sol 1-5, opus 1-10). Base `ae3439c5`. Builds on
#247 (`7c35891a`).

## Goal

`looseFingerprint` (#247) is the key for "the same problem in the same object, with the
routine ignored". Today `fingerprint.rs` builds it by cutting the anchor routine's id out
of `rootCauseKey` (`strip_anchor`, `src/engine/l5/fingerprint.rs:244`). For 16 detectors
that leaves routine-bound text in the key (local variable names, parameter positions,
other routines), or cuts too much (d45, d57).

After this change each of the 16 detectors states its **loose subject**: a typed value
it builds next to its `rootCauseKey`, holding only facts that do not depend on which
routine of the object holds the code. `fingerprint.rs` hashes that subject instead of
parsing `rootCauseKey`. `rootCauseKey`, `id` and `fingerprint` do not change.

Words used below:
- **anchor**: the routine in `primaryLocation.enclosingRoutineId`. By default it gives the
  loose key's object part.
- **subject**: the new per-finding value `LooseSubject { object_id, key }`. The loose key
  becomes `sha256(detector | looseObject | "" | affectedTables | stabilize(key))[..16]`,
  the same shape as #247 with `key` in place of the stripped `rootCauseKey`.
  `looseObject` is the object `object_id` names when the detector sets it (d7, d45), and
  the anchor's object otherwise. It is a hash input only: `primaryLocation` and
  `fingerprint_of` do not change.
- **move**: the same problem moved to another routine of the same object. Where the
  subject allows it, the move may also rename the routine-bound part (a local record
  variable, a parameter's position).

## Non-goals

- No change to `rootCauseKey`, `id`, `fingerprint`, baselines, inline suppression, SARIF,
  HTML, terminal, pr-summary, policy output or the r4 `StableFinding` projection. The
  subject is read only by `loose_fingerprint_of`.
- The other 38 detectors keep the #247 `strip_anchor` path, and their loose values do not
  move. That includes d19, d32 and d38, which the README limits list also names but the
  issue does not. Moving them would move cli-a golden lines the issue does not license.
  The fallback stays for them (one `None` arm); a follow-up can give each of them a subject.
- No `schemaVersion` bump: `looseFingerprint` stays a 16-hex string. Only its value moves.
- `looseFingerprint` stays non-unique by design (#247). Several subjects below are
  coarser than today's stripped key, so more findings share one key. Counting per key
  (`new = Σ max(0, after(k) − before(k))`) handles that.

## Assumption probes

Binary: master `ae3439c5`, `cargo build --profile release-fast --bin alsem`, copied to
`/work/bench/252/before/alsem`. Workspaces: `/work/bench/252/p2/*`. CDO: the pinned
`/work/src/cdo-baseline/Cloud` at `bc3ccb18`, 0 dirty paths, run with
`--detector <the 16 names>` (list in `/work/bench/252/p3/detectors.txt`), outputs
`/work/bench/252/p3/before-{primary,all}.json` (18 s each).

| probe | result | evidence |
|---|---|---|
| P1 d4 | key `d4/{rid}/{loopId}/{recVar}[/{literal}]` (`d4.rs:150-158`); routine-bound: rid, loop id, **record variable name**. Strip leaves `d4/~/~/{recVar}[/{literal}]`. The literal key is added only when one variable has 2+ key groups (`d4.rs:145-149`). | code |
| P1 d7 | key `d7/{sorted SCC routine ids}` (`d7.rs:243`); routine-bound: **every cycle member**. Anchor = first sorted id (`d7.rs:203-209`), only it is stripped. | code |
| P1 d12 | key `d12/{eventId}`, `eventId = {publisherObjectId}/event/{name_lc}` (`d12.rs:72`). It holds **no routine id**, so strip is a no-op. The routine-bound part is the event name, and the event name IS the anchor routine's name. | code |
| P1 d16 | key `d16/{from}/{callsite}/{to}` (`d16.rs:75`). Strip removes caller and its `/csN`, keeps callee. **Self-call** (`from == to` == anchor): both removed, key `d16/~/~/~`. | code; `fingerprint.rs:780` unit test pins the `d16/~/~/rB` shape |
| P1 d18 | key `d18/{rid}/{loopId}/{recVar}/{field_lc}` (`d18.rs:161-168`); routine-bound: **record variable name**. | code |
| P1 d22 | rck `d22/{rid}/{recVar}/{field_lc}` (`d22.rs:143`); routine-bound: **record variable name**. **Claim confirmed**: a variable named `op2`/`cs1`/`loop0`/`u3` sits straight after the rid, so `site_segment_len` (`fingerprint.rs:266`) eats it (`d22/~/{field}`). Effect is small here: the field stays, so only same-table same-field reads through two such variables merge. | code |
| P1 d39–d42 | key `dNN/{caller}/{cs}/{calleeParamIndex}` (`d39.rs:245`, `d40.rs:194`, `d41.rs:199`, `d42.rs:277`). `affectedTables` is empty (`d39.rs:267`, `d40.rs:217`, `d41.rs:219`, `d42.rs:302`). Strip leaves `dNN/~/~/{idx}`: **neither the callee nor the table is in the loose key. Claim confirmed** (runtime, P2 d41). Note: a move of the caller code is already stable today; the defect is the collision. | code + P2 |
| P1 d43 | key `d43/{eventId}\|{caller}\|{caller}/csN\|{setter}\|{table}` (`d43.rs:508-511`); anchor = setter subscriber (`d43.rs:569`). Strip removes the setter only; **the caller routine and its call site stay**. | code |
| P1 d45 | key `d45/{publisher}\|{table}` (`d45.rs:169`). Primary publisher = anchor → stripped → `d45/~\|{table}`. **Claim confirmed**: two primary publishers on one table share a key (P2, and 3 real pairs on CDO, P3). | code + P2 + P3 |
| P1 d48 | rck `d48/{loopRoutine}` for both shapes (`d48.rs:374`, `d48.rs:525`). Direct: anchor = loop routine → `d48/~`. Transitive: anchor = IO routine (`d48.rs:534`), **loop routine stays**. A direct↔transitive move flips whether it is stripped. | code |
| P1 d57 | rck `d57/{rid}/{globalVar_lc}` (`d57.rs:214`). The variable is a **global** (object-level), but **claim confirmed**: a global named `op1`/`op2` is eaten with the rid, so all such findings in one object share `d57/~`. | code + P2 |
| P1 d59 | key `d59/{rid}/{paramIndex}` (`d59.rs:66`); anchor = the event publisher. Routine-bound: **parameter position**. | code |
| P1 d60 | rck `d60/{rid}/{loopId}/{recVar_lc}` (`d60.rs:223`); routine-bound: **record variable name**. | code + P2 |
| P2 d60 (issue reproducer) | Loop moved from `OnUpgradePerCompany` to `Helper` **and** `P1` renamed `Itm`: fingerprint `43a4b595077e5004`→`0d78ed49316d7d57`, loose `abdecf801b735bf1`→`31764dfee4842f10` (**both change: reproduced**). Control, moved but not renamed: loose stays `abdecf801b735bf1`. So the rename alone moves the loose key. | `alsem analyze p2/d60{a,b,c} --format json --deterministic --detector d60-upgrade-loop-should-be-datatransfer` |
| P2 d57 | Globals `op1`, `op2` grown in two subscribers of one SingleInstance codeunit: both loose `6915b514d2725038` (fingerprints differ). | `p2/d57` |
| P2 d45 | Two primary integration events `OnFirst`, `OnSecond` of one codeunit, each with a subscriber inserting `Customer`: both loose `14739fcbd0e1bc9e`. | `p2/d45` |
| P2 d41 | `CallerC` passes `Customer` to `HelperC` (Reset), `CallerI` passes `Item` to `HelperI`: both loose `390600826583e83b`, `affectedTables: []`. Different tables, different callees, one key. | `p2/d41`, `--detector d41-transitive-filter-loss` |
| P7 table mismatch (Task 1 stop-and-revise) | On master: the P5 generic-`Record` shape gives 0 findings for d39 (`Validate`+`exit` helper, two variants), d40 (`Modify`) and d42 (`SetLoadFields` callers, helper reads `Address`); only d41 fires. A callee declared `var R: Record Customer` called with an `Item` (compile-invalid, parses clean) gives 2 findings per detector with one shared loose key: d39 `d5c2ded7b4a5f997`, d40 `e9f74b1117eaf6a9`, d41 `caa8c009477bf228`, d42 `f8cea889e143e973`. The same pass confirmed the d48 move/direct-vs-transitive/two-object shapes and d7's two-cycle shape fire | `/work/bench/252/p7/{d39g,d39g2,d40g,d42g,d39gm,d40gm,d41gm,d42gm,d48*,d7two}`, `before/alsem analyze … --detector <name>` |
| P5 generic `Record` (r2, sol 1) | One helper `ResetIt(var R: Record)` (no table), called from `CallerC` with `Customer` and `CallerI` with `Item`. d41 **fires twice**, both loose `caa8c009477bf228`, `affectedTables: []`. alsem warns "1 recovered file": bare `Record` is not valid AL and the parser recovers, but the engine still builds the role and the finding. So same callee + same index + different caller tables is reachable, and callee/index alone does not separate tables. **sol 1 confirmed.** | `p2/d41g`, `--detector d41-transitive-filter-loss` |
| P6 d7 anchor object (r2, sol 2) | Two-object cycle: `D7X` (`OnA`, `RaiseA`, a subscriber to `D7Y.OnB` calling `RaiseA`), `D7Y` (`OnB`, `RaiseB`, `SubA` subscribing to `OnA`, calling `RaiseB`). Renaming only `D7Y`'s subscriber: `SubA` → anchor `D7X`/`RaiseA`; `Q3`, `Q6`, `Q7` → anchor `D7Y`/that routine; `SubA2`, `HandleA`, `Q1`, `Q2`, `Q4`, `Q5`, `Q8`, `Q9` → `D7X`. A rename inside one object moves the anchor to the other object. **sol 2 confirmed**, and the pair `SubA`/`Q3` gives a pinned test. Master loose differs for every rename (the other members stay in the key). | `p2/d7x-*`, `p2/d7y-*`, `--detector d7-recursive-event-expansion` |
| P3 CDO population (findings / distinct loose today → predicted after) | **primary**: d12 23/23→23; d16 71/40→40; d18 10/10→10; d39 3/1→1; d40 143/57→87; d42 2/1→1; d43 5/5→5; d45 150/147→150; d48 78/28→~4–5. **all**: as primary, plus d18 64/60→59, d22 86/58→52, d43 4573/4573→4573, d48 79/29→~5, d59 72/59→72. **0 findings** in both scopes: d4, d7, d41, d57, d60. | jq over `p3/before-*.json` (group by detector: length, unique looseFingerprint). "After" is a jq prediction from the JSON (object + tables + the subject's parts, read from `id`/`rootCause`), not a run; d48's prediction lacks the loop object, which the JSON does not show. The measurement plan replaces it with real numbers. |
| P3 d45 pairs | The 3 CDO d45 collisions are `OnAfterEnableCDOFeature` vs `OnAfterDisableCDOFeature` on tables 471, 472, 9178. The new subject separates them. | jq group_by looseFingerprint |
| P4 consumers of `looseFingerprint` | Computed once: `projection.rs:111` (`loose_fingerprint_of`). Emitted once: `format_json.rs:211`. Test-only literals `""`: `baseline.rs:132`, `filter.rs:102`, `inline_suppression.rs:302,344`, `format_pr_summary.rs:285`, `format_terminal.rs:495`, `tests/cli/cli_analyze_coverage_ledger.rs:285`, `tests/cli/gate_suppress_baseline_differential.rs:469`. Goldens: only `tests/cli-a-goldens/json/*` (19 files). Not in SARIF (`format_sarif.rs:298` uses `fingerprint`), HTML, baselines or r4. | `grep -rn 'loose_fingerprint\|looseFingerprint' src tests` |
| P4 cli-a lines that will move | Findings of listed detectors in cli-a: d45 in `ws-d8-commit-in-tx.{all,default}.json` (1 each), d48 in `ws-txn-d48-pos.all.json` (2). **4 lines.** d12 is listed but its subject equals today's key, so it does not move. | jq over `tests/cli-a-goldens/json/*.json` |
| P4 consumers of `rootCauseKey` | `fingerprint_of` (`fingerprint.rs:160`), `loose_fingerprint_of` (`:175`), `path_merge.rs:208` (d1/d2 only), sort ties (`finding.rs:972,1072`, `registry.rs:746`), `d45.rs:241` (cap grouping), policy JSON (`format_policy.rs:119`), r4 `StableFinding` (`finding.rs:535`). None changes, because `rootCauseKey` does not change. | grep |
| P4 can `fingerprint` stay fixed? | Yes. `fingerprint_of` (`fingerprint.rs:157-162`) reads `root_cause_key`, `affected_tables`, detector and anchor only. The subject is a new field it never reads; no detector's `affected_tables` changes. | code |

## Design

### The field

`src/engine/l5/finding.rs:305` (next to `fingerprint`):

```rust
/// #252: the routine-independent part of this finding, stated by the detector. Read
/// only by `FingerprintIndex::loose_fingerprint_of`. `None`: the #247 fallback (strip
/// the anchor out of `root_cause_key`).
pub loose_subject: Option<LooseSubject>,

/// #252: what `looseFingerprint` hashes in place of the anchor object and the stripped key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LooseSubject {
    /// Internal object id whose `type/key` replaces the anchor's object part. `None`:
    /// the anchor's object (all detectors but d7 and d45).
    pub object_id: Option<String>,
    /// The key text (`dNN/…`), hashed after `stabilize`, never stripped.
    pub key: String,
}
```

Every other `Finding { … }` literal gains `loose_subject: None` (the compiler lists them;
`grep -rn 'cohort_contexts:' src tests` finds the same sites, about 76). No serializer reads
`Finding` directly, so no output gains a key.

### `loose_fingerprint_of`

`src/engine/l5/fingerprint.rs:171-182` becomes (sketch):

```rust
let (routine, anchor_obj) = self.anchor_of(finding);
let (obj_part, key) = match (&finding.loose_subject, routine) {
    (Some(s), _) => (
        s.object_id.as_deref().map_or(anchor_obj, |id| self.object_part(id)),
        s.key.clone(),
    ),
    (None, Some(r)) => (anchor_obj, strip_anchor(&finding.root_cause_key, &[r.id.as_str(), r.stable_routine_id.as_str()])),
    (None, None) => (anchor_obj, finding.root_cause_key.clone()),
};
hash_parts(finding, obj_part, String::new(), self.stabilize(&key))
```

`object_part(id)` is the `type/key` formatting that `anchor_of` already does
(`fingerprint.rs:201-207`), moved into one helper both call; an id that `objects_by_id`
does not hold is hashed verbatim (internal object ids are `{appGuid}/{type}/{key}`,
`ids.rs:177`, so still stable). `anchor_of` and `fingerprint_of` keep their bytes.

The key is **not** stripped: it holds only what the detector chose, so an id in it is
meant to be there (d16 self-call). `stabilize` still turns internal routine ids in it
(d16 and d39–d42 callees) into stable ids. Doc comment (`:164-170`) updated to say so.

### Subjects

Each detector sets `finding.loose_subject = Some(LooseSubject { … })` just before its
existing `finding.fingerprint = Some(fp_index.fingerprint_of(&finding))` line (or in the
literal). Every key starts with the detector's `dNN/` prefix (readable, and it keeps d12's
bytes). "table" below is the finding's `affectedTables`, which is already a hashed part.
`object_id` is `None` unless the row says otherwise.

| det | subject | where | why it is stable under a move | what stays distinct |
|---|---|---|---|---|
| d4 | `d4/{literal_key}`; when `first.table_id` is `None`: `d4/{recVar_lc}/{literal_key}` | `d4.rs:150-159` | table (hashed part) + literal key are the code, not the routine | different literal keys; different tables |
| d7 | key `d7/{event ids}\|{member object ids}`: sorted, distinct `event_id` of the `event-dispatch` edges inside the SCC, then the sorted `affected_objects`. **`object_id` = the first of the sorted `affected_objects`** (sol 2, P6) | `d7.rs:243` (edges as in `d7.rs:156-164`, objects `d7.rs:228-238`) | event ids name events, not routines; object ids are app-guid based. The loose object no longer follows the anchor, which a rename can move to another object (P6) | cycles over different events |
| d12 | `d12/{ev.id}` (= `root_cause_key`, so bytes unchanged) | `d12.rs:72` | the anchor routine IS the event; a move without renaming keeps `ev.id` | every event |
| d16 | `d16/{e.to}` (the callee) | `d16.rs:75` | the caller is dropped; the callee is the subject | different obsolete callees; **self-calls no longer merge** |
| d18 | `d18/{field_lc}`; when `op.table_id` is `None`: `d18/{recVar_lc}/{field_lc}` | `d18.rs:161-168` | table + field | different fields/tables |
| d22 | `d22/{field_lc}` (table always resolved: `d22.rs:76-91` skips otherwise) | `d22.rs:143` | table + field | different fields/tables |
| d39–d42 | `dNN/{callee.id}/{calleeParamIndex}/{callerTable}`. `callerTable` = the resolved `table_id` of the caller's source record variable for this binding: `caller.record_variables` entry whose `id == binding.source_record_variable_id`, else the one whose name folds equal to `binding.source_variable_name`; `?` when neither resolves. One new helper `binding_source_table(caller, binding) -> Option<&str>` in `detectors/mod.rs`, used by all four. `affected_tables` is NOT touched (it feeds `fingerprint`) | `d39.rs:245`, `d40.rs:194`, `d41.rs:199`, `d42.rs:277` | the caller and its call site are dropped | different callees, parameters, **or caller tables**. The table is stated outright (sol 1, P5): a generic `Record` callee takes two tables through one callee and index. `RecordRef`/`Variant` parameters get no record roles (`ir_walk.rs:2778-2796`, `summary_runner.rs:600-610`), so they do not reach these detectors |
| d43 | `d43/{site.event_id}` | `d43.rs:508` | caller, call site and setter dropped; event and table (hashed) stay | different events/tables |
| d45 | key `d45/{publisher's event ids joined ","}` (`publisher_events`, `d45.rs:128-133`); empty list: `d45/{publisher}`. **`object_id` = `pub_routine.object_id`** (the publisher's object) | `d45.rs:169` | the anchor publisher IS the event. A dependency publisher anchors on the first primary subscriber by routine id (`d45.rs:79-96`), which a rename can move to another object; the publisher's object does not move (sol 2, opus 4). For a primary publisher it equals the anchor object, so nothing changes there | **two primary publishers on one table no longer merge** |
| d48 | `d48/{loopRoutine.object_id}/{io_kind}/{method or ""}`, from `routine` and `terminal` (direct, loop at `d48.rs:330`) / `matched` (transitive, `d48.rs:493`) | both builders | loop routine and IO routine dropped; the loop's object stays, so a **direct↔transitive move within one object keeps the key** | different IO kinds/methods; loops in different objects reaching one IO helper |
| d57 | `d57/{var_lc}` (a global) | `d57.rs:214` | globals belong to the object | **`op1`/`op2` globals no longer merge** |
| d59 | `d59/{encode_event_id(&routine.object_id, &routine.name)}/{param.name_lc}` (the event id as d12/d45 use it, `program/model/events.rs:141`; parameter name unquoted, as `is_guard_name` folds it) (opus 8) | `d59.rs:66` | position dropped; the event (= anchor) and the parameter name stay | different events; different parameters |
| d60 | `d60` when `op.table_id` is `Some`; else `d60/{var_lc}` | `d60.rs:223` | table (hashed) is the subject; the record variable is dropped | different tables |

### Docs

Docs state only what a test in the matrix proves (sol 5).

- README `README.md:144-147`: "same variable names, same parameter position" becomes
  "for the checks listed below, a renamed record variable (when its table is known) or a
  moved parameter does not matter either".
- README `README.md:163-180` (limits). Replace the d4/d18/d22/d57/d60, d59/d39–d42, d7,
  d12, d43, d48, d16, d45 and d39–d42-table bullets with one short list of what each check
  keeps in its loose key:
  - d4, d18, d22, d60: the table (and the literal key or field). The record variable
    name only when the table is unknown.
  - d39–d42: the called routine, its parameter position and the caller's record table.
  - d16: the obsolete routine called. d43: the event. d57: the global variable.
  - d12, d45, d59: the event (the event is the routine, so renaming it is a new key);
    d59 also the parameter name; two overloads of one event whose guard parameter has
    the same name share a d59 key.
  - d48: the object that holds the loop, and the IO kind and method.
  - d7: the events and objects of the cycle.
  Delete the site-id bullet (`README.md:177-179`, opus 7): it names d22/d57, which no
  longer use the strip. Keep the d19/d32/d38 limits and the counting limit.
- CHANGELOG `### Changed` under `[Unreleased]` (`CHANGELOG.md:10`) (opus 10, sol 5):
  "`looseFingerprint` for d4, d7, d12, d16, d18, d22, d39–d42, d43, d45, d48, d57, d59 and
  d60 is now built from what each check names as its subject, not cut out of
  `rootCauseKey`. Moving the problem to another routine of the same object keeps it. A
  renamed record variable keeps it too when its table is known (d4, d18, d22, d60), and a
  moved parameter keeps it for d59. Each check still keeps its event, called routine or
  global variable. d39–d42 findings on different record tables, two primary d45 publishers
  on one table, d57 globals named like `op1`, and d16 self-calls no longer share a key.
  These values change once; `fingerprint` and `rootCauseKey` do not (#252)."

## Acceptance matrix

All new tests extend `tests/cli/cli_loose_fingerprint.rs` (its helpers `temp_workspace`,
`analyze_json_with`, `findings_of`, `str_of`). Generalize `write_workspace` to take a list
of `(path, text)` files, and give `analyze_json_with` a `with_evidence: bool`
(`cli_loose_fingerprint.rs:155`), so a test can read `evidencePath[i].routineId`. The one
cross-app test lives in `tests/r4/r4_cross_app_program.rs`, next to its
`dep_publisher_workspace` helper (`:1027`; it builds the dependency `.app` with
`tests/common/symbol_app.rs:14` `write_source_app`), and also calls
`run_analyze_with_exit`. Every test drives analyze JSON with `--detector <name>` on
inline temp workspaces shaped after the named `tests/r0-corpus` fixture.

Test kinds:
- **move**: two runs, one finding each, same `primaryLocation.objectId`, `fingerprint`
  differs, `looseFingerprint` equal, plus the **move precondition** in the row (sol 4):
  which routine moved and whether the anchor moved with it. Where the anchor does not
  move (d43, transitive d48), the test reads the moved routine from `evidencePath` and
  asserts the anchor `routineId` is the same in both runs.
- **distinct**: one run, two findings in one object that differ only in the named
  component (asserted as the precondition), `looseFingerprint` differs.
- **object switch**: a move whose precondition is that `primaryLocation.objectId`
  DIFFERS between runs; `looseFingerprint` equal.

Every break is made with Edit, run, reverted with Edit, and recorded in the ledger
(`discrimination-proof` skill). Every break listed must turn its test red. A break that
stays green is a defect in the test, not a pass (CLAUDE.md "A discrimination proof that
PASSES is evidence about the TEST"). "`None`" means "this detector sets
`loose_subject: None`", i.e. the #247 strip path, which is master's behaviour.

| det | test | kind / precondition (fixture shape) | break → red |
|---|---|---|---|
| d4 | `d4_move_rename` | move: loop with 2× `Cust.Get('C1')` from `A` to `B`, variable renamed `C`; anchor routine differs (`ws-d4-repeated-get`) | `None` |
| d4 | `d4_literal_distinct` | distinct: loops in `A` and `B`, both `Cust: Record Customer`, keys `'C1'` vs `'C2'` (one key per routine, so master's key has no literal and collides) | drop the literal key from the subject |
| d7 | `d7_move` | move: one-codeunit cycle (`ws-d7-event-cycle`), `SubB` renamed `SubB2` (the code moves to another routine); raw `id` differs | `None` |
| d7 | `d7_anchor_object_switch` | object switch: P6 pair, `D7Y` subscriber `SubA` vs `Q3`; precondition anchor object `D7X` vs `D7Y` (pinned by P6) | `object_id: None` |
| d7 | `d7_events_distinct` | distinct: one codeunit, two cycles (`OnA`/`OnB` and `OnC`/`OnD`) | drop the event ids from the key |
| d12 | `d12_events_distinct` | distinct: two dead events in one codeunit (`ws-d12-dead-event`). A move does not apply: the anchor IS the event (opus 4) | key = `"d12"` |
| d16 | `d16_self_calls_distinct` | distinct: two obsolete procedures in one codeunit, each calling itself (`ws-d16-obsolete`) | `None` (master collapses both to `d16/~/~/~`) |
| d16 | `d16_move` | move: a call to obsolete `Old` moved from `A` to `B`; anchor differs | put the caller id `e.from` into the key (opus 5; `None` stays green here, the strip already drops the caller) |
| d18 | `d18_move_rename` | move + rename (`ws-d18`) | `None` |
| d18 | `d18_field_distinct` | distinct: constant `SetRange` in loops on `Customer."No."` vs `Customer.Name` | drop the field from the key |
| d22 | `d22_move_rename` | move + rename (`ws-d22`, FlowField) | `None` |
| d22 | `d22_field_distinct` | distinct: two FlowFields of one table | drop the field from the key |
| d39–d42 (×4) | `dNN_move_rename` | move: caller code from `A` to `B`, variable renamed; anchor (the caller) differs (`ws-d39`…`ws-d42`) | put the caller id into the key (opus 2/3; `None` stays green, P1) |
| d39–d42 (×4) | `dNN_callees_distinct` | distinct: two callers, two different callees, **same table** and index | drop the callee from the key |
| d39–d42 (×4) | `dNN_same_callee_tables_distinct` | distinct: one callee whose parameter is declared `var R: Record Customer`, called from `CallerC` with a `Customer` and from `CallerI` with an `Item`, same index (sol 1; P7 shape — a type mismatch the AL compiler rejects, but the file parses with no recovery, and on master each of d39–d42 gives two findings sharing one loose key). The generic-`Record` P5 shape fires only for d41 (P7), so it is used only as an extra d41 case. The test carries a comment that the code is compile-invalid on purpose: the engine still builds the findings, and that is exactly the shape that defeats callee+index alone. Precondition: both findings' second evidence step has the same callee `routineId` | drop the caller table from the key; also `None` |
| d43 | `d43_move_caller` | move: the guarded dispatch moved from caller `X` to `Y` of one codeunit (`ws-event-ishandled`). Precondition: anchor (setter) `routineId` equal across runs; `evidencePath[0].routineId` (the caller) differs | `None` |
| d45 | `d45_publishers_distinct` | distinct: P2 d45 (`OnFirst`, `OnSecond` → `Customer`) | `None` |
| d45 | `d45_dep_anchor_switch` (r4 file) | object switch: a dependency publisher (`dep_publisher_workspace`), two workspace subscribers in two codeunits both writing one table; one subscriber renamed so the first primary subscriber by id moves to the other codeunit. Precondition: anchor objects differ. Move for primary publishers does not apply (the anchor IS the event) | `object_id: None` |
| d48 | `d48_move_transitive` | move: a loop calling HTTP helper `H.Send` moved from `A.P` to `A.Q`. Precondition: anchor routine (in `H`) equal; `evidencePath[0].routineId` (the loop) differs | `None` |
| d48 | `d48_direct_to_transitive` | move: `A.P` loop does `Send` itself vs `A.P` loop calls `A.Helper` that does `Send`; anchors `A.P` vs `A.Helper`, same object | `None`; and: the direct builder (`d48.rs:372-374`) omits the loop object |
| d48 | `d48_loop_objects_distinct` | distinct: codeunits `A` and `B` each loop calling `H.Send`; both findings anchor in `H` (sol 3, opus 1) | the transitive builder (`d48.rs:519-525`) drops the loop object |
| d57 | `d57_globals_distinct` | distinct: P2 d57 (`op1`, `op2`) | `None` |
| d57 | `d57_move` | move: the `.Add` moved to another subscriber of the same codeunit | put the anchor routine id into the key (opus 5) |
| d59 | `d59_param_reorder` | position change (not a move: the anchor is the event, opus 4): `var SkipCheck: Boolean` from index 0 to 1. Precondition: anchor `routineId` equal, raw `id` last segment `0` vs `1` | `None` |
| d59 | `d59_params_distinct` | distinct: one event with `var SkipA` and `var SkipB` | drop the parameter name from the key |
| d59 | `d59_events_distinct` | distinct: two events in one codeunit, each `var SkipCheck: Boolean` at index 0 (master collides: `d59/~/0`) | drop the event id from the key; also `None` |
| d60 | `d60_move_rename` | move + rename: the issue reproducer, P2 `d60a`/`d60b` | `None` |
| d60 | T6 rewritten (`cli_loose_fingerprint.rs:300-337`) | T6 asserted two same-table loops differ: the old contract, which conflicts with the issue. New T6: `P1`, `P2` on `Item` in one trigger share a key; a third loop on `Customer` differs | put the variable name into the key → the "share" half red |

Plus:

| # | item | proof | break → red |
|---|---|---|---|
| A3 | `fingerprint` bytes and non-cli-a goldens unchanged | `scripts/check-goldens --regen`, then `git diff --stat -- 'tests/*-goldens' tests/r2-5a-vectors tests/r3a2-vectors tests/l4-summary-baseline tests/goldens` lists only the 3 cli-a files, and `git diff -U0 -- tests/cli-a-goldens \| grep '^[-+] '` shows only `"looseFingerprint"` lines, exactly 4 removed + 4 added, on the d45/d48 findings at `ws-d8-commit-in-tx.all.json:129`, `ws-d8-commit-in-tx.default.json:129`, `ws-txn-d48-pos.all.json:25,59`. CDO: fingerprint multiset before == after | let `fingerprint_of` hash `loose_subject` → cli-a/r4 goldens |
| U1 | the key is hashed verbatim | `fingerprint.rs` unit: a subject key holding the anchor's id keeps it (the d16 self-call shape) | strip the subject key |
| U2 | `None` keeps the #247 bytes | the existing `strip_anchor_shapes` and T1–T5 (T4 pinned `406a88f5e0b896a6`) stay green unchanged | (#247's own proofs) |
| U4 | an `object_id` not in `objects_by_id` is hashed verbatim, not dropped | `fingerprint.rs` unit, two hand-made findings with unknown object ids `x/A`, `x/B` differ | map an unknown id to `""` |

U3 from revision 1 (hand-made d48 findings) is dropped: it pinned a helper, not the
builders; `d48_loop_objects_distinct` and `d48_direct_to_transitive` replace it.

`fingerprint.rs` unit-test comments at `:754-756` and `:767-770`, `:787-788` say "the real
detector layouts" / "two d60 findings". After this change those are the **fallback
path's** behaviour on those key shapes, not what d60 or d18 do. Reword them to say so
(opus 6); the tests themselves stay.

The existing T1–T5 and the other `fingerprint.rs` #247 unit tests stay unchanged and green
(d11/d64/d19 are not listed; their loose values do not move).

## False-positive and failure shapes

- *Coarser keys.* d4, d18, d22 and d60 drop the variable name; d43 drops the caller;
  d48 drops the loop routine. Two same-shaped problems in one object then share one key.
  This is the issue's definition (the routine is ignored); counting per key handles it.
  d48 is the largest: on CDO primary about 78 findings over ~4–5 keys (today 28).
- *Unknown table* (d4, d18, d60). The record variable name stays in the subject, so a
  rename there still changes the key. On CDO: 0 such d18 findings, 0 d4/d60 findings.
- *Anchor that switches objects* (d7, dependency d45). The anchor is the first routine
  by internal id, so a rename can move it to another object (P6). Fixed by the
  detector-chosen `object_id`, a hash input only (sol 2; revision 1 wrongly called this
  unfixable). `primaryLocation` and `fingerprint` still follow the anchor, as today.
- *Unresolved caller table* (d39–d42). `?` in the key: two such findings through one
  callee and index share a key. Stated in the README list.
- *Event identity* (d12, d45, d59). The anchor routine is the event declaration. Renaming
  the event is a different event and changes the key, by design.
- *d59 event overloads*. `encode_event_id` is the object id plus the lowercased event name
  (`program/model/events.rs:140-143`), with no overload discriminator (the signature is
  stored separately, `:146-157`). Two same-named overloads of one event in one object
  whose guard parameter has the same name therefore share a d59 loose key. Accepted and
  stated in the README list: a signature-based discriminator would break the
  parameter-position test (a reorder changes the signature), and on CDO d59 has 72
  findings over 72 predicted keys (P3), so no real case is known.
- *d16 callee*. Renaming or moving the obsolete callee changes the key, by design.
- *Stale subject*. A later step that rewrites `root_cause_key` would leave the subject
  stale. Only `path_merge` (d1/d2) groups by key, and it rewrites nothing; no listed
  detector goes through it.
- *Equality*. `Finding` derives `PartialEq`; both runs of any comparison build the same
  subject, so equality checks (single-app vs cross-app contract) are unaffected.

## Measurement plan

`cdo-measure` skill, before = `/work/bench/252/before/alsem` (master `ae3439c5`), after =
branch HEAD, both `release-fast`, on `/work/src/cdo-baseline/Cloud` at `bc3ccb18`:

1. Default `measure.py run` on both sides: findings per detector identical; fingerprint
   multiset identical (`comp` prints 0 added, 0 removed).
2. The P3 command on both sides (`--detector` the 16 names, `--scope primary` and `all`):
   per detector, findings identical, `fingerprint` multiset identical, distinct
   `looseFingerprint` before → after. Expected: the P3 predicted column; d48 is a range
   because the prediction lacks the loop object. Any number outside the prediction is
   explained by a subject in the table above, or it is a discovery.
3. Non-listed detectors: their `looseFingerprint` multiset identical (default run).
4. Wall time: indicative only. The change adds one `LooseSubject` per listed finding
   (~5 300 on `--scope all`); no new index.

## Pre-stated fallbacks

Only one fallback changes HOW a test is built without changing what it proves:

- A fixture that does not fire as an inline workspace: copy its `tests/r0-corpus` fixture
  into the temp dir (`crate::copy_dir`, as T5 does) and edit the copy in the test. The
  assertions and breaks stay as in the matrix.

Everything else is **stop-and-revise** (sol 5): the implementer stops, records the probe
in the ledger, and the spec goes back to the panel. It is never a completion path, and
README/CHANGELOG never claim a behaviour whose test was dropped.

- A detector in the matrix gives no finding for its pair, or one finding where two are
  needed (for example d39/d40/d42 not firing on the generic `Record` callee, d48 not giving
  one finding per side of the direct↔transitive pair, the d45 dependency fixture not
  switching its anchor after 20 names tried).
- A listed break stays green.
- A move test shows a routine-bound part the table above did not list.
- Any golden line other than the 4 d45/d48 `looseFingerprint` lines moves, or the CDO
  fingerprint multiset changes.

## Files the implementer touches

- `src/engine/l5/finding.rs:305` (field + `LooseSubject`) and every `Finding { … }` literal
  (compiler-driven).
- `src/engine/l5/fingerprint.rs:164-182` (`loose_fingerprint_of`), `:201-207` (the
  `object_part` helper, shared with `anchor_of`), comments at `:754-756`, `:767-770`,
  `:787-788`, unit tests U1 and U4 after `:823`.
- `src/engine/l5/detectors/mod.rs`: `binding_source_table` (new, for d39–d42).
- `src/engine/l5/detectors/`: `d4.rs:150-159`, `d7.rs:243`, `d12.rs:72`, `d16.rs:75`,
  `d18.rs:161-168`, `d22.rs:143`, `d39.rs:245`, `d40.rs:194`, `d41.rs:199`, `d42.rs:277`,
  `d43.rs:508`, `d45.rs:169`, `d48.rs:372-374` and `:523-525`, `d57.rs:211-214`,
  `d59.rs:66`, `d60.rs:222-223`.
- `tests/cli/cli_loose_fingerprint.rs` (new tests; `with_evidence` parameter at `:155`;
  T6 at `:300-337` rewritten; module doc).
- `tests/r4/r4_cross_app_program.rs` (`d45_dep_anchor_switch`, next to `:1027`).
- `tests/cli-a-goldens/json/ws-d8-commit-in-tx.all.json`, `ws-d8-commit-in-tx.default.json`,
  `ws-txn-d48-pos.all.json` (regen; `looseFingerprint` lines only).
- `README.md:144-147`, `:163-180`; `CHANGELOG.md:10`.
