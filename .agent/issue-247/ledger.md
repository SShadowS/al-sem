# Issue #247 — `alsem analyze`: a routine-independent finding key (`looseFingerprint`)

## Claim

```json
{"run_id":"20261023-155005-cfbe0a","issue":247,"attempt":1,"branch":"issue/247-analyze-routine-independent-fin-a1","worktree":"/work/al-sem-issue-247-a1","body_hash":"0043e8909e713f0898f01ce1f31131abbf08f94422bf65ac43959ec35a288cd6"}
```

Base: `40c84c13`. Issue body hash: `0043e8909e713f0898f01ce1f31131abbf08f94422bf65ac43959ec35a288cd6`.
Reporter: the CentralGauge evaluation (presentation 2026-10-27).

## Classification

**bounded.** The change is additive. One new method on the existing `FingerprintIndex`
(`src/engine/l5/fingerprint.rs`), one new field on `FindingSummary`, one new key in the
analyze JSON, docs, one new test file, and a regen of one golden family
(`tests/cli-a-goldens/json`, +1 line per finding). No detector changes. `fingerprint`
is not touched, so no baseline, SARIF or r4 golden moves.

## Assumption probes

Run 2026-10-23 on the base tree (code read) and on the pinned CDO workspace
`<CDO_WS>` (measurements). The CDO runs used the existing
`/work/alsem/target/cdo/{alsem,aldump}` binaries (built 2026-10-22, before base); they
show key SHAPES, which no recent commit changed. Re-measure on a base build before quoting.
Probe scripts: `scratchpad/loose_probe.py` (analyze JSON) and `scratchpad/loose_exact.py`
(aldump stable findings), both in the session scratchpad.

| probe | result | evidence |
|---|---|---|
| P1 where `fingerprint` is computed | Only in `FingerprintIndex::fingerprint_of`: `sha256(detector \| objectType/objectKey \| routineName \| affectedTables \| stabilized rootCauseKey)[..16]`. Called at 74 sites in 60 files (each detector, plus the 3 policy findings) right after the finding is built | `fingerprint.rs:148-194`; `grep -rn "fingerprint_of(" src \| wc -l` = 74 |
| P2 one central place for analyze | `run.rs:539-547` projects every finding with `ProjectionIndex::build(&resolved.workspace.objects, &resolved.workspace.routines)` and `project_finding`. The detectors' `FingerprintIndex` is built from the same slices (`detector_context.rs:1070-1071` via `:1108`, `ws.routines`/`ws.objects`). So a `FingerprintIndex` built next to the `ProjectionIndex` gives the same ids | code read |
| P3 who reads `FindingSummary.fingerprint` | JSON (`format_json.rs:179`), SARIF `fingerprints["al-sem/v1"]` (`format_sarif.rs:158-163, 297-299`; no `partialFingerprints`), baseline load/save/apply (`baseline.rs:82,94`; `run.rs:599`), pr-summary sort (`format_pr_summary.rs:70`). HTML and terminal do not show it. Policy output and r4 `StableFinding` copy `Finding.fingerprint`, not the summary | code read |
| P4 goldens that hold analyze JSON | Only `tests/cli-a-goldens/json/` (19 files, 37 findings; `--test cli`, `cli_a_json_differential.rs`). `"fingerprint"` also appears in `tests/r4-goldens` (77), `tests/cli-c-policy-goldens` (10), `tests/r3a2-vectors` (1); those read `Finding.fingerprint` and do not move | `grep -rl '"pathCount"' tests`; `grep -rlE '"fingerprint"\s*:' tests` |
| P5 analyze output docs | No analyze-output schema doc exists. The closest is `README.md` "What `alsem analyze` finds" + flags table (`README.md:86-133`). JSON envelope `schemaVersion` is `1.0.0`, `1.1.0` only under `--with-evidence` (`format_json.rs:268-271`) | `grep -rln pathCount --include=*.md` finds only CHANGELOG/OUTSTANDING/measurements |
| P6 rootCauseKey shapes | Sub-ids are `{routineId}/op{N}`, `/cs{N}`, `/loop{N}`, `/u{N}` (walk-order ordinals, `ir_walk.rs:565,694,990,1066`) and `/p{N}` (parameter index, `d19.rs:110`, `d32.rs:263`). Shapes found (corrected in round 1, S10/O5): **(a) anchor routine + ordinals only**: d1, d3, d5, d8, d9, d10, d11, d13, d14, d19, d20, d21, d29, d32, d33, d34, d35, d36, d37, d46, d50, d52, d53, d54 (key `d54/{rid}`; the publisher is in `id` only, `d54.rs:126-127`), d55, d56, d58, d61, d62, d63 (e.g. `d10/{rid}/{rid}/op12`). d48's key is `d48/{loopRoutine}` (`d48.rs:520-525`) but it anchors on the IO site, which is in another routine when transitive. **(b) anchor + subject text**: d4 (`{rid}/{loop}/{recVar}/{literalKey}`), d18 (`…/loop0/{recVar}/{field}`, `d18.rs:157-168`), d22 (`/{recVar}/{field}`), d57, d60 (var name), d38 (`{sub}/{eventId}`), d39/d40/d41/d42 (`…/cs{N}/{paramIdx}`), d59 (bare `/{paramIdx}`, `d59.rs:66`). **(c) a second routine in the key**: d7 (`d7/{sorted SCC routine ids}`, `d7.rs:243`), d16 (`{caller}/{cs}/{callee}`), d43 (`event\|caller\|cs\|setter\|table`, anchors on the setter), d45 (`{publisher}\|{table}`; anchors on the publisher when it is primary, `d45.rs:84-88,207-210`). **(d) no routine id**: d2 (`d2/{eventId}`), d12 (`{publisherObjectId}/event/{name}`, `d12.rs:9-12`), d17, d44. **(f) object id**: d64 (`d64/{objectId}`, anchor = the object id, `d64.rs:62,162`). **(e) the anchor's STABLE id** (not internal): d47, d49 (`d47/{guid}:Codeunit:N#{hex}`; `of` is `OrderingFacts` keyed by the stable id, `d47.rs:388`), d51 (same `of.routine_id`, not seen on CDO). d32 (`d32/{callee}/p{N}`) anchors on the callee (`d32.rs:288`), so it is shape (a) | `grep root_cause_key\|let id = format!` over `detectors/`; CDO analyze JSON `id`s and `aldump --r4-findings-cross-app` `rootCauseKey`s |
| P7 generic strip works? | Yes, if it strips only the **anchor** routine (`primary_location.enclosing_routine_id`) in both forms (internal and stable) plus the ordinal chain after it. Stripping **every** routine id is wrong: it deletes d16's callee and a dependency publisher in d45, which are the subject (a primary d45 publisher IS the anchor and is stripped anyway, O3). Stripping every routine **of the anchor's object** differs from anchor-only only on d16 (34 vs 40 keys on CDO): an obsolete LOCAL callee is the subject and must stay. So: anchor-only | `loose_exact.py`: per-detector columns `looseObjRule` vs `looseAnchorRule` |
| P8 before/after keys (CDO, stable form) | d10: `d10/{G}:TableExtension:6175381#b645…f842/{same}/op5` → `d10/~/~`. d18: `d18/{rid}/{rid}/loop0/networkparticipation/status` → `d18/~/~/networkparticipation/status`. d16: `d16/{rid}/{rid}/cs1/{calleeRid}` → `d16/~/~/{calleeStable}`. d47: `d47/{G}:Codeunit:6175376#5b40…f26d` → `d47/~`. d44: unchanged | `loose_probe.py … x`, `loose_exact.py … 1` |
| P9 collisions (CDO, primary scope, all 54 detectors) | 2 671 findings, 2 638 distinct fingerprints, **1 270 distinct loose keys** (anchor rule). Biggest groups: d1 1 096 → 380 (max 64 in one key), d40 143 → 57 (max 35), d11 112 → 28 (max 18), d21 149 → 41, d19 96 → 35, d14 81 → 41, d55 31 → 6 (max 20). No collisions: d12, d17, d43, d44, d38, d46. Scope all (31 125 findings, approximate rule on analyze `id`): 10 121 keys; d14 17 512 → 1 197 | `aldump --r4-findings-cross-app` + `loose_exact.py`; `analyze --scope all --detector <all 54> --format json` + `loose_probe.py` |
| P10 SARIF and loose keys | SARIF result matching treats two results with an equal `fingerprints` value as the same result. A loose key there would merge distinct alerts. So the loose key must NOT go into SARIF `fingerprints`/`partialFingerprints` | SARIF 2.1.0 §3.27.16-17 (design judgement) |

**Verdicts.** P1/P2: one computation point and one projection point exist; reuse
`FingerprintIndex`. P6/P7: a generic rule (strip the anchor routine) gives the
movement guarantee for the shape-(a)/(e) detectors only; the others have stated limits
(Design, limits table). P9 is the surprise: the loose key is **not unique** —
about half of all findings share one with another finding (same detector, object and
tables). That is inherent in the issue's definition. A "new findings" metric must
therefore match **by count per key**, not by set membership.

## Design

Revised after spec round 1 (sol S1-S14, opus O1-O9; register `findings.json`).

**Goal.** Each analyze JSON finding gets `looseFingerprint`: a key that stays equal when
the same problem moves to another routine of the same object. `fingerprint` stays
byte-identical.

**What "moved" means here.** The same code, with the same local names (record and other
variables) and the same parameter position, moved into another routine of the same
object. A move that also renames a variable or reorders parameters is not covered.

**Non-goals.**
- No change to `fingerprint`, any detector's `rootCauseKey` or `id`, baselines, inline
  suppression, `--limit` or `--fail-on`.
- JSON only (operator input 2026-10-23: CentralGauge reads `alsem analyze --format
  json`). No `looseFingerprint` in SARIF, pr-summary, HTML, terminal, policy output or the
  r4 `StableFinding` projection.
- No per-detector loose subject (finer keys). That is the Discovery below.
- No `schemaVersion` bump. The field is additive, so a reader that ignores unknown keys is
  unaffected. `1.1.0` already means `--with-evidence`, and
  `tests/cli/cli_a_with_evidence.rs:212-222` pins the default at `1.0.0`; that test stays
  green because both its runs gain the same key.
- This issue does not implement the "new findings" metric. It ships the key and documents
  how to use it.

**Formula.** In `FingerprintIndex`, add `loose_fingerprint_of(&Finding) -> String`:

1. `anchor = finding.primary_location.enclosing_routine_id` (the same anchor as
   `fingerprint_of`, NOT the actionable anchor).
2. **Only when `routines_by_id` holds `anchor`** (O2, S4): in `finding.root_cause_key`,
   replace every occurrence of the anchor's internal id, and of its stable id
   (`routines_by_id[anchor].stable_routine_id`), each followed by at most ONE
   `/(op|cs|loop|u)<digits>` site-id segment (the shapes `ir_walk.rs` mints on a routine
   id, never nested; the segment must end at `/`, `|` or end-of-string), with `~`. A
   parameter position `/p{N}` (d19, d32) is kept. When the anchor is not a routine (d64's
   object id, or an unresolved id) nothing is stripped.
   *(Revised in final review round 1, CR1/CR2: was "any chain of `/(op|cs|loop|u|p)<digits>`
   segments, ending before a non-alphanumeric byte", which also ate a following variable
   named like `p1`, and `p1_old`.)*
3. `key = substitute_stable_ids(key, …)` exactly as `fingerprint_of` does (other routine
   ids stay as their stable id).
4. `looseFingerprint = sha256_hex(join("|", [detector, obj_part, "", affectedTables.join(","), key]))[..16]`
   — the fingerprint's own five parts with `routineName` blanked. `obj_part` comes from
   the same code as `fingerprint_of` (shared, not copied).

Consequence: when the anchor is not a routine, step 2 does nothing and the routine name is
already empty, so `looseFingerprint == fingerprint`. Pinned through the analyze JSON (T5).

**Movement guarantee (Acceptance 2): delivered for these detectors.** Their
`rootCauseKey` holds only the anchor routine (internal or stable id) and its ordinal
segments, so step 2 removes every routine-bound part:
d1, d3, d5, d8, d9, d10, d11, d13, d14, d20, d21, d29, d33, d34, d35, d36, d37,
d46, d47, d49, d50, d51, d52, d53, d54, d55, d56, d58, d61, d62, d63; d19 and d32 when the
parameter keeps its position (CR2: `/p{N}` is kept). Also d2, d17 and d44:
their key holds no routine at all, so it never changed with the routine. d64 is
object-level: there is no routine to move from.

**Not delivered: per-detector limits.** These keep a routine-bound part. They are not
fixed in this issue (no key changes). They all go into the one Discovery entry.

| detector | part that stays routine-bound | effect of a move |
|---|---|---|
| d4, d18, d22, d57, d60 | local names: record variable (d4 `{recVar}/{literalKey}`, d18 `{recVar}/{field}`, d22 `{recVar}/{field}`), variable name (d57, d60) | equal key only when the moved code keeps its names (within the definition above); a rename changes it |
| d59, d19, d32 | the parameter index (`d59/{rid}/{idx}`, `d59.rs:66`; d19/d32 `/p{N}`, kept since CR2) | equal key only when the parameter keeps its position |
| d39, d40, d41, d42 | the callee parameter index after `/cs{N}` | same as d59, for the callee's parameter |
| d7 | the other routines of the subscriber cycle (`d7/{sorted SCC ids}`, `d7.rs:243`); only the anchor (the first id) is stripped | moving any other cycle member changes the key |
| d12 | the publisher event name `{publisherObjectId}/event/{name}` (`d12.rs:9-12`); the anchor is the publisher | a renamed event changes the key; moving the publish into another routine under the same event name keeps it |
| d16 | the callee id; for a self-call (`e.from == e.to`) both ids are the anchor and both are stripped | a self-call to an obsolete routine collapses to `d16/~/~/~`; moving the obsolete CALLEE changes the key |
| d38 | the event id | moving the subscriber keeps the key; changing the event does not (intended) |
| d43 | the caller routine and its `/cs{N}`; the anchor is the setter subscriber | moving the setter keeps the key; moving the caller's code changes it |
| d45 | when the publisher is primary it IS the anchor and is stripped (`d45.rs:84-88,207-210`): two primary publishers in one object exposing one table share a loose key. **Accepted**: they then differ only in which routine publishes, which is exactly what the loose key ignores. For a dependency publisher the anchor is a primary subscriber and the publisher stays | see left |
| d48 | the key is the LOOP routine (`d48/{loopRoutine}`, `d48.rs:520-525`); a transitive finding anchors on the IO routine, so the loop routine is not stripped | moving the loop changes the key; moving the IO site keeps it only for a transitive→transitive move whose IO routine stays outside the loop routine. A direct↔transitive move changes whether the loop id is stripped, so it changes the key (S15) |

**Acceptance 3 for d39-d42.** They emit no `affectedTables` (`d39.rs:275-298`,
`d40.rs:221-247`, `d41.rs:237-257`, `d42.rs:303-329`), so "different tables" has no
meaning for them. Two findings of one of these detectors in one object, on the same
parameter index but on different record tables, share a loose key. The parameter's record
table is not on the `binding` today (a lookup would be new code), so this stays a limit and
joins the Discovery.

**Where.** `ProjectionIndex` gains a `FingerprintIndex` built inside
`ProjectionIndex::build` from the slices it already takes, and `project_finding` sets
`FindingSummary.loose_fingerprint`. So every production projection carries it with no
caller change. The production caller is `run.rs:539`; the other `ProjectionIndex::build`
calls are tests (`run.rs:1231`, `format_html.rs:1201`). `format_json` inserts
`"looseFingerprint"`. The six test-only `FindingSummary { … }` literals
(`baseline.rs:129`, `inline_suppression.rs:299,340`, `format_pr_summary.rs:282`,
`format_terminal.rs:492`, `filter.rs:99`) gain the field.
Implementation choice (O7): `ProjectionIndex` reads its `routines_by_id`/`objects_by_id`
through the held `FingerprintIndex` instead of building its own copies, so the only extra
cost is `stable_by_id` (a `String` clone of each routine's internal and stable id,
dependency routines included in cross-app mode) and the prefix list.

**Docs.** CHANGELOG `Added` and README (analyze section). Both state:
- `fingerprint`: the same problem in the same routine. Baselines use it. Inline
  suppression does not use either key: it matches detector, file and line
  (`inline_suppression.rs:228-238`).
- `looseFingerprint`: the same problem in the same object, with the routine ignored. Not
  unique: several findings can share one.
- The rule for a "new findings" count: **new = Σ max(0, after(k) − before(k))** over
  `looseFingerprint` values k. To say WHICH finding is new, first pair findings equal on
  BOTH keys, then pair on `looseFingerprint` among what is left (O8). Equal `fingerprint`
  does NOT imply equal `looseFingerprint` (d48: IO moved into a same-name overload keeps
  `fingerprint` and changes the loose key), so only this both-keys-first order agrees
  with the count.
- Preconditions for comparing two reports: no `--baseline`, no `--limit`, the same
  `--scope`, `--min-severity`, `--detector`/`--preset` and the same dependency coverage.
  Otherwise findings drop out before the JSON is written (`run.rs:578-622`) and look new
  or fixed.
- Limits: the counting cancels (fix one, add a same-key one in the same object = 0 new),
  and the per-detector limits above, in one short list.

**Acceptance matrix** (all tests in a new `tests/cli/cli_loose_fingerprint.rs`,
registered with `mod cli_loose_fingerprint;` in `tests/cli/main.rs` (S8); each drives
`run_analyze_with_exit` → JSON on tempdir workspaces; each discrimination proof is made
with Edit and recorded here):

| # | Acceptance item | Proof |
|---|---|---|
| 1 | `fingerprint` and goldens unchanged except the new field | `scripts/check-goldens --regen`. Then on golden paths only (S9): `git diff --stat -- 'tests/*-goldens' tests/r2-5a-vectors tests/r3a2-vectors tests/l4-summary-baseline tests/goldens` lists only `tests/cli-a-goldens/json/*`, and `git diff -U0 -- tests/cli-a-goldens \| grep '^[-+] ' \| grep -v '"looseFingerprint"'` is empty. Test-file changes are reviewed separately. CDO: fingerprint multiset before == after. Break: change a byte in `fingerprint_of`'s join → cli-a/r4 goldens red |
| 2 | Moved finding: equal loose, different fingerprint (listed detectors) | T1 (d11): workspace A = `ws-d11-no-get` shape, the bad `Modify` in `BadNoGet`, `Other` empty. Workspace B = `Other` has one record op first (`Customer.Get('Y');` on a separate variable), then the moved bad code; `BadNoGet` empty. Assert the precondition: the two raw `id`s end in different `/op{N}` (O4). Assert loose equal, fingerprint different. Break 1: skip the anchor strip → red. Break 2: keep `routineName` in the parts → red. Break 3: strip the id but keep the ordinal chain → red |
| 3 | Different problem, same object → different loose | T2: one object, d11 on `Customer` and d11 on a second table → loose differ. Break: drop `affectedTables` from the parts → red. T3 (O1, S7): the detector part is redundant with the key's `dNN/` prefix on the production path, so no real pair can tell it apart. T3 is therefore a `fingerprint.rs` unit test with a hand-made pair: two `Finding`s with equal `root_cause_key` (`x/{anchor}/op0`), equal tables and anchor, detectors `d11-…` and `d21-…` → loose differ. Break: drop `detector` → red. The production path is still covered by T2 and T1. Not delivered: d39-d42 (above) |
| 4 | Deterministic and documented | T4: run analyze twice on T1-A, and once on a copy at another temp path → identical `looseFingerprint`. Plus one pinned literal value for T1-A. The cli-a goldens pin it too. Break: add the model instance id or the file path to the parts → red. Docs: CHANGELOG + README |
| 5 | Tests pin the production path | Every test except T3 reads analyze JSON. T5: a d64 finding (API page with write surface, from the d64 r0 fixture) has `looseFingerprint == fingerprint` in the JSON. Break: remove the `routines_by_id` gate → T5 red. Extra break: remove the `format_json` insert → T1/T2/T4/T5 red |

Helper unit tests in `fingerprint.rs` (in addition, not instead): the suffix chain
(`/loop0/…/op25`), a var name like `/p2x` NOT stripped, a callee id kept (d16 shape), the
stable-id form (d47 shape), T3.

**False-positive and failure shapes.**
- *Collisions (by design).* Same detector + object + tables + remaining key text share a
  key (P9: ~2.1 findings per key on CDO primary). Counting per key handles it, with the
  cancellation limit above.
- *Per-detector limits.* The table above.
- *Ordinal-looking text.* A variable or field named like `p2`/`u1`/`op3`/`loop1` right
  after the anchor id would be stripped (key only gets looser). Not seen on CDO.
- *Anchor not a routine (d64, unresolved).* Nothing stripped; `looseFingerprint ==
  fingerprint`.
- *Dependency routines (`--scope all`).* Same rule; dep routines have stable ids in the
  same map.
- *Actionable anchor (d1).* The key uses the internal primary (the terminal op's routine),
  like `fingerprint`, not the JSON `primaryLocation`.

**Measurement plan** (`cdo-measure` skill, base vs branch, `--scope all` and primary,
all 54 detectors): (1) fingerprint multiset identical, finding count identical;
(2) distinct `looseFingerprint` per detector (expect ≈ P9: 1 270 primary);
(3) analyze wall time and peak RSS. The extra work is the `stable_by_id` map (two `String`
clones per routine), the prefix list, and, unless shared as in Where, two more hash maps of
borrowed keys (S14).

**Fallbacks.**
- If a test compares whole finding objects and breaks, add the field there (an intended
  shape change); never drop the field.
- If wall time rises more than 1 % or peak RSS more than 1 % on CDO, pass the detectors'
  `FingerprintIndex` through `RunOutput` instead of building one in `ProjectionIndex`.
- If d11 does not fire on the minimal T1 body, use `tests/r0-corpus/ws-d21` (d21). If the
  `Get` in T1-B suppresses the finding, put a different record op first (e.g. a `Modify`
  on a third variable) so the ordinals still differ.
- If no d64 fixture yields a finding in single-app JSON, use any other object-level
  finding, or a finding whose anchor is unresolved, for T5.
- If the cli-a regen moves any line other than `+ "looseFingerprint"`, stop and find out
  why before blessing.

## Discoveries

- **Finer per-detector loose subjects (one entry).** Not delivered by #247; file one
  issue. Covers: the limits table (d4/d18/d22/d57/d60 local names, d59 and d39-d42
  parameter positions, d7 other cycle members, d12 event name, d16 self-call and callee,
  d43 caller, d45 primary-publisher merge, d48 loop-routine identity) and Acceptance 3 for
  d39-d42 (no table subject: derive the parameter's record table as a loose-only
  subject). Any fix must keep `fingerprint` bytes identical.

## Timeline (conductor)

- Spec panel round 1 (sol + opus, fresh threads): S1-S14, O1-O9; both "do not approve". Design revised by the design author (opus), 23/23 fixed, none refuted.
- Spec panel round 2: opus accepted all 23, approve; sol accepted 22, re-raised O8, new minor S15. Conductor fixed both (wording: both-keys-first matching; d48 transitive→transitive).
- Spec panel round 3: both accepted O8, S15, S1, S3, S13; both approve. Register: 24 entries, all accepted by both. spec_rounds remaining 2.
- Operator input via alsem-main (2026-10-23): CentralGauge reads `alsem analyze --format json`; JSON only; CHANGELOG states new = Σ max(0, after(k) − before(k)).

## Implementation

Implementer (opus), 2026-10-23. Plan: `docs/superpowers/plans/2026-10-23-issue-247-loose-fingerprint.md`, **2 tasks**.

**Commits** (base `40c84c13`, not pushed):
- `9ab85011` plan: #247 looseFingerprint on analyze findings
- `bdc6dc30` analyze: looseFingerprint, a routine-independent finding key (#247)
- `5af9ca9f` docs: looseFingerprint in CHANGELOG and README (#247)

**check-diff.** Against `origin/master` it reported `cargo-version-changed` before any commit:
origin/master moved to `5b8d198f` (Bump version to 2.0.2) after the base, and the check is
two-dot. Conductor ruled (a): run it with `--base 40c84c13`; the conductor rebases before the
branch gates. Every commit: `--base 40c84c13` → `reasons: []`.

**TDD red** (`cargo test --test cli cli_loose_fingerprint::`, before any implementation): 4
failed, each at `finding has no string \`looseFingerprint\``, after its preconditions passed
(T1: the two raw ids end in different `/op{N}`; T2: one object, two tables; T5: object-level
anchor). Two test-setup fixes on the way, before the red run counted: the findings live at
`.payload.findings`, and d64 is opt-in (`bcquality` preset), so T5 passes `--detector`.

**Gate.** `bash scripts/ci-steps task` exit 0: 3 841 passed, 8 skipped. `scripts/check-goldens`
exit 0 (1 000 passed).

**Goldens.** `scripts/check-goldens` before regen: 1 failure (`cli_a_json_byte_match`), 19
file/mode pairs, every divergence `looseFingerprint: not in golden, present in rust`. The
regen guard needed a triage receipt (`.agent/golden-triage.md`, not committed). After
`scripts/check-goldens --regen`: only `tests/cli-a-goldens/json/*` changed (19 files, +37
lines = the 37 findings); `git diff -U0 -- tests/cli-a-goldens | grep '^[-+] ' | grep -v
'"looseFingerprint"'` is empty. Note: the Design's pathspec `'tests/*-goldens'` matches
nothing in git (needs `:(glob)tests/*-goldens/**`); I checked with `git diff --name-only --
tests/ | grep -v cli-a-goldens/json/`, which lists only the 3 test source files.

**Discrimination proofs** (each break made with Edit, run, reverted with Edit; final run
all green; all in commit `bdc6dc30`):

| test | break | fail output | pass after revert |
|---|---|---|---|
| T1 | skip the anchor strip (`match routine.filter(\|_\| false)`) | T1 `looseFingerprint ignores the routine` left `c64cc21b22265db4`; T4 pin; unit `loose_fingerprint_ignores_the_anchor_routine` | yes |
| T1 | keep `routineName` in the loose parts | T1, T4 pin (`a1bee5f1558ed994`), unit ignores-anchor | yes |
| T1 | strip the id, keep the ordinal chain | T1, T4 pin (`cb3dbafaaaca647b`), unit ignores-anchor, `strip_anchor_shapes` (`d1/~/loop0/cs3/op25`) | yes |
| T2 | drop `affectedTables` from the parts | T2 `left != right` (`2573a11a063e099c`); T4 pin | yes |
| T3 (unit) | drop `detector` from the parts | `loose_fingerprint_hashes_the_detector`; T4 pin. T1/T2 stay green, as the Design predicts (detector part redundant with the `dNN/` key prefix) | yes |
| T4 | append the internal anchor id (carries the model instance id) to the loose key | T4 at the PIN only (`d043e11096fb38e5`); also T1, T5, unit. The run-twice and other-path equalities stay green: the model instance id is content-derived, so a same-content copy at another path shares it. The pin is what catches it | yes |
| T5 | remove the routine gate (strip the enclosing id even when it is not a routine) | T5 `looseFingerprint == fingerprint` fails | yes |
| T1/T2/T4/T5 | remove the `format_json` insert | all four: `finding has no string looseFingerprint` | yes |
| unit `strip_anchor_shapes` | drop the end-of-segment check | `d60/~x` (the `/p2x` case) | yes |
| unit ignores-anchor | strip only the internal id, not the stable id | **first came back GREEN**: the fixture stable id `stable::rA` contained the internal id `rA`, so stripping `rA` also erased it (test defect). Fixed the fixture (`S#A`/`S#B`), re-ran: red on `d47/S#A` (`2b423600c8eea6fd`) | yes |
| Acceptance 1 | `fingerprint_of` routine name + `"x"` | `check-goldens` red: cli-a JSON, r4, SARIF, pr-summary, baseline/suppression, policy goldens | yes (gate green) |

T4's pinned literal `406a88f5e0b896a6` was checked independently:
`printf 'd11-modify-without-get|Codeunit/50247||11111111-0000-0000-0000-000000000247/table/18|d11/~/~' | sha256sum` → same 16 hex.

~~Limit: the stable-id strip (d47/d49/d51 shape) is pinned by a unit test only.~~
*Corrected in final round 1 (G2):* it is also pinned on the production path by the cli-a
JSON goldens (`ws-txn-d47-pos-*`, `ws-txn-d49-pos-*`, `ws-d51-*`); removing the strip turns
`cli_a_json_byte_match` red (proof row below).

**Fallbacks fired:** none of the Design's (d11 fired on the minimal body; the `Get` in T1-B
did not suppress; d64 yields findings; the regen moved only `looseFingerprint` lines; CDO
wall time and peak RSS stayed under +1 %).

**Measurement (CDO).** Workspace `<CDO_WS>` at `bc3ccb1`, 0 dirty paths.
Binaries: `cargo build --profile release-fast --bin alsem`, after = branch `bdc6dc30` (same
code as HEAD), before = a scratch worktree at `40c84c13` with its OWN target dir. (A first
base build sharing the branch's target dir came back "fresh" in 0.11 s and was byte-identical
to the branch binary — cargo's mtime freshness; caught by `cmp`, discarded.) Command per run:
`alsem analyze <ws> --format json --deterministic --scope {primary|all} --detector <all 54>`,
3 reps, before/after interleaved; peak RSS from `getrusage(RUSAGE_CHILDREN)` in a fresh
wrapper per run (`scratchpad/247/runmeasure.py`). Dev machine, indicative only.

| scope | wall s median before → after (ranges) | Δ | peak RSS KB median before → after | Δ |
|---|---|---|---|---|
| primary | 23.36 → 23.57 ([23.30–23.72] / [23.51–23.90]) | +0.9 % | 3 150 556 → 3 161 720 | +0.35 % |
| all | 24.12 → 23.99 ([24.07–24.36] / [23.93–24.14]) | −0.5 % | 3 148 556 → 3 128 440 | −0.64 % |

Both under the 1 % fallback threshold (primary wall time is close, with overlapping ranges:
noise-level). Correctness:
- Findings: primary 2 717 = 2 717, all 31 213 = 31 213. Fingerprint multiset IDENTICAL in
  both scopes (2 684 / 31 159 distinct). The after JSON with `looseFingerprint` deleted is
  equal (`jq -S`) to the before JSON in both scopes. Every finding carries the key.
- Distinct `looseFingerprint`: **primary 1 282 for 2 717 findings** (P9 probe: 1 270 for
  2 671), **all 10 118 for 31 213**. Per detector (primary, findings → keys): d1 1096→380,
  d3 335→230, d21 149→41, d45 150→147, d40 143→57, d11 112→28, d19 96→35, d14 81→41, d48
  78→28, d10 75→46, d16 71→40, d35 52→13, d34 45→18, d9 39→34, d33 31→30, d55 31→6, d12
  23→23, d44 20→20, d5 22→14, d37 12→6, d18 10→10, d8 9→8, d47 2→1, d49 6→4, d54 6→2, the
  rest equal or 1–3 findings.
  *These CDO numbers are for the pre-round-1 rule (chain walk, `p` stripped). Round 1
  makes keys finer (d19/d32 keep `/p{N}`, variables after a site id are kept), so distinct
  counts can only rise; not re-measured.*

## Final round 1 fixes

Branch head before: `fb96c12e` (rebased by the conductor onto origin/master `5b8d198f`).
Commit: **`4c677ca0`** looseFingerprint: strip one minted site id, keep /p{N} (#247 final r1).
check-diff `--base origin/master`: `reasons: []` before and after the commit.
Gate: `bash scripts/ci-steps task` exit 0, 3 843 passed, 8 skipped; `scripts/check-goldens`
exit 0 (1 001 passed).

Each finding checked against the code first:
- **CR1, real.** Site ids are minted ONLY in `program/body/ir_walk.rs` as
  `{routineId}/loop{n}` (:565, :669), `/u{n}` (:694), `/op{n}` (:990, :1310), `/cs{n}`
  (:1066): one segment on the routine id, never nested. On CDO (scope all, 31 213 ids) no id
  holds two stacked ordinal segments. d43 puts `|` right after `/cs1`. Fix: strip at most
  ONE `/(op|cs|loop|u)<n>` segment, which must end at `/`, `|` or the key's end.
- **CR2: keep `/p{N}`.** `/p{N}` is made only by d19 (`d19.rs:110`) and d32 (`d32.rs:263`)
  as a parameter index, not minted on the routine id, so it is no site id. Removed `p`.
  d19/d32 now behave like d59 (stated as a limit in the docs and the Design).
- **CR5.** `ROUTINE_SITE_TAGS` in `fingerprint.rs`; each of the six `ir_walk.rs` minting
  lines has a one-line pointer to it (comment, not a shared constant: the ids are built by
  `format!` literals).
- **CR3: done (borrow).** `stable_by_id: HashMap<&'a str, &'a str>`; the census tally
  drops its owned-string count (the census test pins only the bucket's presence). Removes
  2 String clones per routine in BOTH indexes (the detector context's and the
  projection's). Not re-measured.
- **CR4:** no change (conductor ruling).
- **CR7: refuted in its literal form.** `--disable-inline-suppression` is not a CLI flag
  (`AnalyzeArgs` field with "no CLI surface"; `src/bin/alsem.rs` always passes `false`).
  Documented the real precondition instead: the same inline `// al-sem-ignore` comments
  on both sides (suppression is always on).
- **F1/G1/CR6:** README limits now list d48 direct↔transitive, the d16 self-call, the
  primary-d45 merge and d19/d32; CHANGELOG matches. Plan and the Design formula updated.
- **CR8:** `strip_anchor_shapes` uses real layouts (d11, d18, d60 `…/loop0/p1` and
  `…/loop0/op2`, d57 `p1_old`/`op1_old`/`op2x`, d19 `/p2`, d43 `|`, d16, d47); a unit test
  for d60 `p1` vs `p2`; and **T6** through the analyze JSON: one upgrade trigger, two
  row-by-row loops on `P1`/`P2` (same table) → 2 d60 findings, same routine and tables,
  different loose keys.

**Golden moves** (receipt `.agent/golden-triage.md`, not committed): only
`tests/cli-a-goldens/json/ws-d34.{default,all}.json`, 4 lines, all `looseFingerprint`
values: the two d19-unused-parameter findings (`DoWork`, `Persist`) went
`d423df5be6102f86` → `4f4f18f6e72b44a1` because their key went `d19/~` → `d19/~/p0`
(CR2). They still share one key (both parameter 0). No golden holds a d60/d57/d4 finding,
so CR1 moves none.

**Proof rows** (break with Edit, run, revert with Edit; final run green):

| test | break | fail output | pass after revert |
|---|---|---|---|
| T6 (JSON), unit `loose_fingerprint_keeps_the_variable_after_the_site_id`, `strip_anchor_shapes` | restore the OLD rule in full (chain walk + `p` tag + alphanumeric boundary) | T6 `left != right` (`d44da06f73fd7916` both); unit `left != right`; shapes `d60/~/~` | yes |
| `strip_anchor_shapes` | chain walk only | `d60/~/~` (the `…/loop0/op2` case). T6 green: `p` is no tag any more | yes |
| `strip_anchor_shapes` | `p` back in the tags only | `d19/~` | yes |
| `strip_anchor_shapes` | old alphanumeric-only boundary only | `d57/~_old` (`op1_old`) | yes |
| G2: cli-a goldens | strip only the internal id, not the stable id | `cli_a_json_byte_match`: 7 divergences — ws-txn-d47-pos-file/http-commit-after/http-nocommit, ws-txn-d49-pos-modify-message/modify-runmodal, ws-d51-jobqueue, ws-d51-pos (all scope), e.g. d47 golden `c50b39728ea17f8a` rust `b6fa140c2d2efe0e` | yes |

Note on the G2 run: the first attempt ran before the regen, so it also showed the 4 CR2
d19 moves; it was redone against the regenerated goldens (the 7 above are the break's own).
Under plain `cargo test` three sibling tests in that module then fail with
`PoisonError` (they share a mutex the first panic poisons): fallout, not failures.

## Final panel (conductor)

- Final round 1 (head fb96c12e): sol F1 (minor), ship; opus G1, G2 (minor), ship; built-in code-review (high) CR1-CR8, of which CR1 (ordinal chain over-strip) was a real bug. Fixed in 4c677ca0 (see "Final round 1 fixes"); CR4 and CR7 refuted with evidence.
- Final round 2 (head 4c677ca0): both accepted all 16 listed ids, except sol re-raised CR6 (minor) and opus raised G3 (minor), the same issue: a variable named exactly like a site id right after the routine id (d22, d57) is still stripped. Conductor stated it as a limit in README and CHANGELOG (0d3de621).
- Final round 3 (head 0d3de621): both accepted CR6 and G3; both ship. Register: 36 entries, all accepted by both, none open. final_rounds remaining 2.
- Step 11 gates on 0d3de621 (base 5b8d198f, supervised, run 20261023-155005-cfbe0a): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (455.6 s); check-goldens exit 0 (0.2 s, tree-stamp skip); cdo-gate exit 0 (289.3 s).
