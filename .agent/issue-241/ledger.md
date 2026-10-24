# Issue #241 — each routine's owner display names its own declaring object

## Claim

```json
{"run_id":"20261024-075409-2f089f","issue":241,"attempt":1,"branch":"issue/241-agent-discovery-l4-routinejoin-a1","worktree":"/work/al-sem-issue-241-a1","body_hash":"a553666ddf01e3109ff40988e6f01a069f296ae92423ad50e307192ffb26b92f"}
```

Base: `21266acd` (master after #239). Issue body hash:
`a553666ddf01e3109ff40988e6f01a069f296ae92423ad50e307192ffb26b92f`.
Ponytail: the smallest complete fix.

## Classification

**bounded.** One defect shape: three object-id -> name maps in one file
(`src/engine/l4/effect_query_cli.rs`) keep the last name for an id. The fix is one
shared lookup ("the object that declares this routine") used by those three sites. No
id, schema or golden changes. The hands-off files (`digest.rs`, `ordering_engine.rs`,
`fingerprint_query.rs`) read the owner only through `OwnerIndex`'s unchanged API, so
none of them needs an edit.

## Assumption probes

Run 2026-10-24 at base `21266acd`, debug `alsem` built in this worktree
(`TREE_SITTER_AL_PATH=/work/alsem/tree-sitter-al cargo build --bin alsem`).
Probe workspaces (scratch, not committed):
`/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/ws241`
(`#if A codeunit 50100 "Foo" { P: DeleteAll } #else codeunit 50100 "Bar" { Q: ModifyAll } #endif`,
plus table 50190) and `.../ws241b` (same, but both arms declare `P()` with one signature).

| probe | result | evidence |
|---|---|---|
| R1 `query effects` (ws241) | **Reproduced.** `--routine P` prints `--- effects of Codeunit Bar::P ---` and `— Codeunit Bar::P at ws:src/Split.Codeunit.al:15`. P is declared in `Foo` (line 12). `Q` correctly prints `Codeunit Bar::Q` | `alsem query effects --routine P --deterministic ws241`, exit 0 |
| R2 digest (ws241) | **Reproduced.** JSON: both entries have `"objectDisplay":"Codeunit Bar"` (P and Q). Human header `--- Codeunit Bar::P ---` | `alsem digest --file src/Split.Codeunit.al --deterministic` (json and human) |
| R3 fingerprint (ws241) | **Reproduced.** `--routine P` renders `Codeunit Bar::P  [public-procedure]` | `alsem fingerprint --routine P --deterministic --format human` |
| R4 #239 selector (ws241) | **Reproduced (L1).** `digest --routine 'Codeunit Foo::P'` and `'Foo::P'`: `no changed roots resolved`, exit 2. `fingerprint --routine 'Codeunit Foo::P'`: exit 2, `no routine of that name on the named owner; declared as: Codeunit Bar::P`. The wrong owner selects it: `digest --routine 'Codeunit Bar::P'` exit 0 | same binary |
| R5 `query effects` `Object.Routine` selector (ws241) | **Reproduced.** `--routine Foo.P`: `matched no routine`. `--routine Bar.P` selects P | `resolve_routine_selector`, `effect_query_cli.rs:299-303` |
| R6 same-signature split (ws241b) | One stable id for both arms' `P` (stable object id is the number). `query effects --routine P` is AMBIGUOUS, both candidates print `Bar.P (<same stable id>)`: two model rows that share BOTH the internal id and the stable id (the internal id has no body or location part, `ids.rs:333-356`, `workspace.rs:1048-1060`; corrected after review S2). digest shows one entry `Codeunit Bar::P`. Side finding (not this issue, see Discoveries D2): that entry lists `[DB_DELETE]` with evidence `ws:src/Split.Codeunit.al:25 "R.ModifyAll"` — the merged routine's DeleteAll evidence points at the other arm's ModifyAll line | same binary |
| S1 every id -> one-name map | Three, all in `src/engine/l4/effect_query_cli.rs`: (a) `resolve_routine_selector` `object_name_by_id` (`:299-303`), used for the `Object.Routine` tier and the ambiguity labels; (b) `RoutineJoin::object_name_by_id` (`:385-400`, read in `of`, `:416-421`), the `query effects`/`touches` owner; (c) `OwnerIndex::build` `name_by_object_id` (`:475-479`, read `:486-489`), the digest / fingerprint / prove owner and #239's `narrow_to_owner` key. Each is `HashMap<id, name>` from `collect()`, so the last object row wins | `grep -rn "o.id.as_str(), o.name"` |
| S2 other id -> object maps (same collision, not owner display) | `DetectorContext::objects_by_id` (`l5/detector_context.rs:1059`), policy `objects_by_id` (`gate/policy/policy_engine.rs:149`, read by `object_of` for `object.name`/`object.kind`/`object.appGuid`, `predicate_fields.rs:411-416`), `root_classification.rs:291,355`, `l5/test_coverage.rs:407,474` (Test-codeunit `object_subtype`), detectors via `ctx.objects_by_id` (`d14.rs:119`, `d17.rs:118`, `d50.rs:194`). They return the last arm's whole object (name, Subtype, PageType…). Policy `object.name` on P would read `Bar`. `l5/fingerprint.rs:98` is exempt: it reads type/key only, the same in both arms. Full list in Discoveries D1 | code read; `grep -rn objects_by_id src` |
| S3 snapshot identity table | Not an owner source since #78 (the tableextension-stub overwrite). Digest's routine `display` comes from it and is right (`P`). Nothing to change | `OwnerIndex` doc, `effect_query_cli.rs:451-462` |
| S4 does the routine know its declaring object? | No direct field: `ModelRoutine` has `object_id`, `object_type`, `object_number`, but not the name or object index. It has its own `source_anchor` (file, start/end line+column). `ModelObject.source_anchor` is `Some(anchor of the whole object declaration)` for native (workspace) rows (`workspace.rs:921-925`), `None` for ABI dependency rows. Both anchors use the same `source_unit_id`, so "the object whose anchor contains the routine's anchor" picks the declaring arm. Adding a field instead touches 39 `ModelRoutine { … }` literals | `workspace.rs:430-520, 921-985, 1421`; `grep -c "ModelRoutine {$"` = 39 |
| S5 what #83/#250 made of the ids | A numbered object's id key is its number, so both arms get `<guid>/Codeunit/50100`: two `ModelObject` rows, one id. The model keeps both: `rows_population` makes one population entry per physical object row (`workspace.rs:743-764`), and `project_ir` pushes one `ModelObject` per entry with no id dedup (`:789`, `:962`). File order puts the first arm first, so the last (`Bar`) wins every map. A numberless object's key is `@`+folded name (#83/#250), so two arms with DIFFERENT names get different ids and cannot collide; with the same name they collide but share the name | code read; R6 shows two routine rows |
| S6 tests/goldens holding object-level `#if` splits | One fixture: `tests/fixtures/analyze-test-quality/src/Tests/SplitObject.Codeunit.al` — same name (`"TQ Split Object Tests"`) in both arms, so its owner is unchanged. The `#if` cases in `l5/detector_context_11b_tests.rs` and `l4/summary_runner.rs` are inside one object. No golden or test holds a split with DIFFERENT names | awk scan of `tests/**/*.al` for an object header right after `#if/#else`; grep of `src/` test sources |
| S7 CDO | Zero object headers right after an `#if`/`#else` line in `/work/src/cdo-baseline/**/*.al`. Dependencies are symbols (no `#if`). No CDO number can move | same awk scan |
| S8 #239 narrowing reads `OwnerIndex` | `narrow_to_owner` keys each match by `owners.object_display(id)` (digest `digest_cli.rs:407`, fingerprint `fingerprint_query.rs:671`). Fixing `OwnerIndex` makes `Codeunit Foo::P` select P and `Codeunit Bar::P` unmatched, with no edit to `fingerprint_query.rs` | code read |

**Verdicts.** The issue holds as stated (R1, R2) and reaches further: fingerprint and
prove (same `OwnerIndex`), `query effects`' `Object.Routine` selector (R5), and #239's
L1 (R4). The root cause is that owner is looked up by object id, and an id does not
name one declaration. The fix is owner by declaration.

## Design

**Goal.** Every routine's owner display names the object declaration that contains it:
`query effects`/`touches` headers and lines, digest `objectDisplay` and human header,
fingerprint block header, prove `objectDisplay`, the `query effects` `Object.Routine`
selector and its ambiguity labels, and the #239 `<owner>::<routine>` narrowing. Closes
#239 Limit L1 **when the two arms' routine signatures differ** (see the same-signature
shape below, O4).

**Non-goals.** (1) The other id -> object maps (S2, Discoveries D1). Same root class,
but they feed policies, detectors and root kinds, not owner display. (2) The
same-signature split (R6): both arms are one internal and one stable id, so the
stable-id-keyed surfaces (digest, fingerprint, prove, #239) can show one owner only.
(3) The R6 side finding (wrong evidence line on the merged routine): Discoveries D2.
(4) No id, stable-id or schema change.

**Design.** One lookup, in the model, used by all three sites:

- Add `DeclaringObjects<'a>` next to `ModelObject` (`src/program/model/workspace.rs`,
  or a small sibling module): built once from `&'a [ModelObject]` as
  `HashMap<&str, Vec<&ModelObject>>` (rows sharing an id, in model order).
  `fn of(&self, r: &ModelRoutine) -> Option<&'a ModelObject>`:
  1. no row for `r.object_id` -> `None` (callers keep their `""` placeholder);
  2. one row -> it (every non-split object; the fast path);
  3. several rows -> the row whose `source_anchor` has the routine's
     `source_unit_id` and whose `(start_line, start_column)..=(end_line, end_column)`
     contains the routine's start position;
  4. none contains it (no anchors) -> the LAST row, which is today's behaviour, so
     nothing that works today changes.
- `resolve_routine_selector`: replace `object_name_by_id` with
  `DeclaringObjects::of(r).map(|o| o.name)` (both uses).
- `RoutineJoin`: replace the `object_name_by_id` field with a `DeclaringObjects`
  field; `of()` reads it.
- `OwnerIndex::build`: replace `name_by_object_id` with it. The
  `declaration_only_routines` loop already reads its own object and stays.
- Rewrite the doc comments that state the id-keyed lookup: `RoutineJoin`
  (`effect_query_cli.rs:383-384`, "plus object names", O3) and `OwnerIndex`
  (`:451-463`, the "last object per internal id" limit).
- CHANGELOG `[Unreleased]` `### Fixed`. It says #239's L1 closes only when the two arms'
  routine signatures differ; with one signature the routine is one stable id, its
  owner is the FIRST arm, so `Codeunit Bar::P` is now the unmatched selector (today it
  is `Codeunit Foo::P`) (O4).

Touched: `src/program/model/workspace.rs` (or sibling), `src/engine/l4/effect_query_cli.rs`,
`tests/cli/cli_object_display.rs`, `CHANGELOG.md`. Not touched: `digest.rs`,
`ordering_engine.rs`, `fingerprint_query.rs`, `digest_cli.rs`, `fingerprint_cli.rs`,
`prove.rs` (all read `OwnerIndex`/`RoutineJoin` through unchanged APIs).

**Acceptance matrix.** New tests in `tests/cli/cli_object_display.rs` (the #78/#239
member), each runs the real `alsem` binary on an inline workspace with
`#if A codeunit 50100 "Foo" { P } #else codeunit 50100 "Bar" { Q } #endif` (P and Q
each touch a table, so `query effects` has a line).

| # | assertion | discrimination proof (break with Edit, run, revert) |
|---|---|---|
| A1 | `query effects --routine P` header and effect line name `Codeunit Foo::P`; `Q` names `Codeunit Bar::Q` | revert `RoutineJoin::of` to the id map: A1 fails with `Codeunit Bar::P` |
| A2 | digest `--file`: P's entry `objectDisplay == "Codeunit Foo"`, Q's `"Codeunit Bar"`; human header `--- Codeunit Foo::P ---` | revert `OwnerIndex::build` to the id map: A2 fails |
| A3 | fingerprint `--routine P` block header `Codeunit Foo::P`; prove `objectDisplay == "Codeunit Foo"` | same break as A2 |
| A4 (#239 L1) | `digest --routine 'Codeunit Foo::P'` and `fingerprint --routine 'Codeunit Foo::P'` select P (exit 0, owner `Codeunit Foo`); `alsem prove <ws> 'Codeunit Foo::P' may-commit` (positional, S1/O2) exits 0 with `payload.routine.objectDisplay == "Codeunit Foo"` and `payload.result.answer == "no"`. `'Codeunit Bar::P'` is unmatched on all three: digest `selector-unmatched` exit 2, fingerprint `selector-unresolved` exit 2, prove exit 2 with `payload.result.answer == "unknown"` and `objectDisplay == ""`. (Base today, probed: `Foo::P` gives prove `unknown`/exit 2, `Bar::P` gives `no`/exit 0 with `Codeunit Bar`) | same break as A2: `Foo::P` goes back to exit 2 / `unknown` |
| A5 | `query effects --routine Foo.P` selects P; `Bar.P` matches nothing | revert `resolve_routine_selector`'s map: A5 fails |
| A6 | Swap the arm order (`Bar` first): A1/A2 still name each routine's own object (proves "by declaration", not "first wins") | replace step 3 (containment) with "first row wins": A6 fails on the Bar-first order (A1 alone catches "last row wins") (S3) |
| U1 | Unit test of `DeclaringObjects::of`, hand-stated rows: two `ModelObject` rows with one id and `source_anchor: None`, plus a routine of that id. Asserts step 4 returns the LAST row. A second case with anchors asserts step 3 picks the containing row whichever order the rows are in (S3) | flip step 4 to "first row": U1 fails. A6 cannot catch this, because anchored rows never reach step 4 |

The CLI tests pin the use (CLAUDE.md "pin the USE"); U1 pins the fallback order that
no CLI path can reach.

**False-positive and failure shapes.**
- Same name in both arms (`SplitObject.Codeunit.al`): either row gives the same name;
  no output changes. Existing test-quality goldens stay.
- Numberless object: different names -> different ids (`@name`), one row each, fast
  path. Same name -> same name.
- Dependency objects: ABI rows have `source_anchor: None` and one row per id; fast path.
  If a dependency ever had two rows for an id, step 4 keeps today's last-wins.
- Same-signature split (R6): one internal and one stable id -> `OwnerIndex`'s first
  insert wins (`or_insert`, `effect_query_cli.rs:490-492`), so the owner becomes the
  first arm (`Foo`) instead of the last. The failure moves: `Codeunit Bar::P` becomes the
  unmatched selector on digest/fingerprint/prove, where today `Codeunit Foo::P` is
  (O4). `query effects`' ambiguity list then prints `Foo.P` and `Bar.P`. **Stated Limit
  L1':** #239's L1 closes only when the arms' routine signatures differ; written in
  the CHANGELOG too. Not pinned beyond that.
- Two separate files declaring one id (a compile error in AL): the anchor's
  `source_unit_id` separates them; works.
- #239 narrowing: only the key changes; the rule in `narrow_to_owner` is untouched.

**Measurement.** No golden should move (S6): run `scripts/check-goldens` and the task
gate; any golden diff is a stop-and-explain. CDO: no object-level split (S7); no
`cdo-measure` needed, `cdo-gate` at the branch gate as usual. Cost: one extra
`HashMap<&str, Vec<_>>` per `RoutineJoin`/`OwnerIndex` build instead of `HashMap<&str,&str>`;
no measurement needed beyond the suites.

**Fallbacks, stated in advance.**
- If an object row's `source_anchor` does not span its routines (e.g. a grammar shape
  where the object origin is only the header), switch step 3 to "the row whose anchor
  start is the last one at or before the routine's start in the same file". Same site,
  same tests.
- If a native row ever has `source_anchor: None`, step 4 keeps today's behaviour; the
  tests then fail and say so.
- If a reviewer prefers a model field (`ModelRoutine.object_name`), that is the
  alternative root fix: 39 literal sites plus the two producers
  (`workspace.rs:1421`, `abi_rows.rs:279`). Not chosen: it duplicates data the anchors
  already hold.

## Discoveries

- **D1 — other id -> whole-object maps keep the last `#if` arm** (out of scope; both
  reviewers agree). For an object split across `#if` arms that share one id, these
  maps return the last arm's whole `ModelObject` (name, Subtype, PageType,
  InherentCommitBehavior…) for every routine, including the first arm's:
  - policy: `objects_by_id` (`src/engine/gate/policy/policy_engine.rs:149`), read by
    `object_of` (`predicate_fields.rs:411-416`) for `object.name`, `object.kind`,
    `object.appGuid`;
  - `DetectorContext::objects_by_id` (`src/engine/l5/detector_context.rs:1059`), read by
    detectors such as `d14.rs:119`, `d17.rs:118`, `d50.rs:194`;
  - `src/engine/root_classification.rs:291,355`;
  - `src/engine/l5/test_coverage.rs:407,474` (Test-codeunit `object_subtype`).
  Exempt: `src/engine/l5/fingerprint.rs:98` reads type/key only, which both arms share.
  The later fix swaps each site to `DeclaringObjects::of` (added by #241). Each swap
  changes what policies, detectors and root kinds evaluate, so it needs its own tests
  and a goldens/CDO check.
- **D2 — digest evidence of a same-signature `#if` split points at the other arm.**
  ws241b (both arms declare `P()`; `Foo`'s does `DeleteAll`, `Bar`'s `ModifyAll`):
  digest's one `Codeunit Bar::P` entry lists `[DB_DELETE]` with evidence
  `ws:src/Split.Codeunit.al:25 "R.ModifyAll"`, the other arm's line. Reproduced on base
  `21266acd`. Not this issue.

## Review round 1

Reviews: `/work/alsem/.agent/runs/20261024-075409-2f089f/spec-r1-sol.md` (S1-S3),
`spec-r1-opus.md` (O1-O4). Both approve. All seven are minor and fixed in the text above;
register `.agent/issue-241/findings.json`.
- S1 = O2: A4 calls `prove <ws> <selector> may-commit` and asserts the JSON answer.
- S2: R6 says the two rows share the internal and the stable id.
- S3: A6's break is "replace step 3 with first-wins"; U1 pins the step-4 order.
- O1: D1 names every site; `l5/fingerprint.rs` exempt.
- O3: the `RoutineJoin` doc is rewritten too.
- O4: L1 closes only for arms with different signatures; CHANGELOG and Limit L1' say so.

## Spec round 2 (conductor)

- Both reviewers accepted S1-S3 and O1-O4 and approve. Opus raised O5 (minor): with same-signature arms (R6), `RoutineJoin.by_id` (effect_query_cli.rs:393) is keyed by internal id, so the last row wins there; `query effects --routine Foo.P` can print a `Codeunit Foo::P` header while its effect lines name `Codeunit Bar::P`. Addition to Limit L1' (and its CHANGELOG sentence): "In that same-signature case, `query effects` effect lines may name the other arm's object." Implementer carries it into the CHANGELOG; both reviewers mark O5 at the final panel.

## TDD red (implementer, base 21266acd + tests only)

`cargo test --test cli cli_object_display::split_object` on unchanged engine code:
0 passed, 6 failed, each for the stated reason (the last arm's name wins):
- A1 `split_object_query_effects_names_each_routines_own_object`: P objectDisplay
  `Codeunit Bar`, want `Codeunit Foo`.
- A2 `split_object_digest_names_each_routines_own_object`:
  `[("P","Codeunit Bar"),("Q","Codeunit Bar")]`, want P -> `Codeunit Foo`.
- A3 `split_object_fingerprint_and_prove_name_the_declaring_object`: fingerprint block
  `Codeunit Bar`.
- A4 `split_object_owner_selector_names_the_declaring_object`: `Codeunit Foo::P` gives
  `digest 2 [] [selector-unmatched()]`, `fingerprint 2 [] [selector-unresolved]`,
  `prove 2 [|Codeunit Foo::P] unknown`.
- A5 `split_object_dotted_selector_names_the_declaring_object`: `Foo.P` exit 2 (unmatched).
- A6 `split_object_owner_does_not_follow_arm_order` (Bar first): Q objectDisplay
  `Codeunit Foo` (last row = Foo), want `Codeunit Bar`.

U1 `declaring_objects_contains_by_anchor_else_last_row` against a `DeclaringObjects`
stub whose `of` returns the last row (today's behaviour, not wired): FAILED at (b),
`Some("Bar")` for the routine inside `Foo`'s anchor, want `Some("Foo")`.

## Implementation

**Plan:** `docs/superpowers/plans/2026-10-24-issue-241-declaring-object.md`, 1 task.

**Commits** (base `21266acd`, not pushed):
- `e4fa9dce` plan: #241 owner by declaring object
- `f62714f8` owner display: each routine names its own declaring object (#241) —
  `src/program/model/declaring.rs` (new, `DeclaringObjects`), `src/program/model/mod.rs`,
  `src/engine/l4/effect_query_cli.rs` (three sites + `RoutineJoin`/`OwnerIndex` docs + U1),
  `tests/cli/cli_object_display.rs` (A1-A6), `CHANGELOG.md` (`[Unreleased]` Fixed, with
  Limit L1' and the O5 sentence).

Hands-off files (`digest.rs`, `ordering_engine.rs`, `fingerprint_query.rs`) untouched.

**Gate.** `scripts/check-goldens`: exit 0, no golden moved. `bash scripts/ci-steps task`:
run 1 failed clippy `disallowed_names` (a test variable named `foo`); renamed; final run
exit 0, 3995 tests passed, 8 skipped. `check-diff --base origin/master` (origin/master =
base, unmoved): `reasons: []` before and after each commit.

**Limit L1' / O5 probe** (ws241b, same signature in both arms, after the fix): digest
shows one entry `Codeunit Foo::P`; `Codeunit Foo::P` exit 0, `Codeunit Bar::P` exit 2;
`query effects --routine Foo.P` header `Codeunit Foo::P`, effect lines `Codeunit Bar::P`.
Matches the CHANGELOG limit.

**Discrimination proofs.** Each break made with Edit, a `BREAK Pn` marker grepped to
count exactly 1 (applied), the tests run, the break reverted with Edit, the tests rerun
green (`cli_object_display::` 22 passed, `effect_query_cli` lib 6 passed).

| # | break | test(s) | broken result | restored |
|---|---|---|---|---|
| P1 | `RoutineJoin::of` looks up an anchorless copy of the routine (= the old id map, last row) | A1 | A1 FAILED: P `Codeunit Bar`, want `Codeunit Foo` (A5, A6 also fail) | pass |
| P2 | same break in `OwnerIndex::build` | A2, A3, A4 | A2 `[(P,Bar),(Q,Bar)]`; A3 fingerprint `Codeunit Bar`; A4 `Foo::P` unmatched on all three (A6 also fails) | pass |
| P3 | same break in `resolve_routine_selector` | A5 | A5 FAILED: `Foo.P` exit 2, want 0; all other tests pass | pass |
| P4 | step 3 replaced by "first row wins" (early `return rows.first()`) | A6, U1 | A6 FAILED: P `Codeunit Bar` (Bar first); A1/A2 also fail on Q; U1 fails at (a) | pass |
| P5 | step 4 fallback `rows.last()` -> `rows.first()` | U1 | U1 FAILED at (a): `Some("Old")`, want `Some("New")`; all 22 CLI tests PASS (no CLI path reaches step 4, as the Design says) | pass |

**Fallbacks fired:** none. The object row's `source_anchor` spans its routines (A6 green
with containment), so the "last anchor start at or before" fallback was not needed; no
native row lacked an anchor.

**Not delivered:** nothing in scope. D1/D2 remain out of scope as specified.

## Final round 1 — code-review CR1-CR7 (implementer)

Commit `e477d595` on `f62714f8` (no rebase; check-diff `reasons: []`). Gate:
`scripts/check-goldens` exit 0, no golden moved; `bash scripts/ci-steps task` exit 0,
3996 passed, 8 skipped.

- **CR1 fixed.** Verified: `to_location` (`gate/projection.rs`) read the name via
  `FingerprintIndex::object`, an id map where the last row wins; on ws241 `analyze` JSON gave
  d33's finding in P `objectName: "Bar"`. Now `ProjectionIndex` holds a `DeclaringObjects`,
  and a routine-anchored location takes its object from it. The object-level (d64) fallback
  still uses the id map. That is a new CHANGELOG limit: no routine, so the lookup has
  nothing to contain. Fingerprints unchanged: `l5/fingerprint.rs` reads type/key only.
  ws241 fingerprints before and after are `702adb9f2312b942` / `94571b64c23b332e`, and no
  golden moved. New test A7 `split_object_analyze_locations_name_the_declaring_object`.
  CHANGELOG now lists analyze locations.
- **CR2 refuted.** Verified against code and by probe: making `RoutineJoin.by_id`
  first-wins does NOT make `query effects` effect lines agree with the header. Effect lines
  take their owner from `OperationJoin` (`effect_query_cli.rs:527-537`), keyed by operation
  id. It deliberately keeps the LAST owning row (#160 step 11b §4.6, class P), and in the
  same-signature case both effects carry Bar's operation (line 25). With the change built,
  ws241b `--routine Foo.P` still printed `Codeunit Bar::P` on both effect lines. Making
  them agree means changing `OperationJoin`, which is D2's evidence-merging shape and a
  #160 decision. The change was reverted; the O5 sentence and Limit L1' stay as written
  (still accurate).
- **CR3 fixed.** The doc and mutation note of
  `owner_index_filters_to_primary_and_follows_routine_join` now describe
  `DeclaringObjects` and its no-anchor last-row fallback.
- **CR4 fixed.** Of several containing anchors, the smallest one (`PAnchor::extent_cmp`;
  ties keep model order, `min_by` keeps the first) declares the routine. U1 case (f):
  outer 0-40 and inner 12-20 in both orders, routine at 14 gives `Inner`.
- **CR5 fixed.** Added `PAnchor::contains(line, col)` and `PAnchor::extent_cmp` in
  `program::body::features`. Both are moved verbatim from `gate/run.rs`'s private
  `range_contains` and `range_extent_cmp`: the same inclusive tuple bounds and the same
  lines-then-columns order, so behaviour is identical. run.rs and declaring.rs use them.
  `extent_cmp` is moved too so CR4 reuses run.rs's order instead of copying it.
- **CR6 refuted.** Storing the declaring row on `ModelRoutine` was weighed by the spec
  panel against the 39 `ModelRoutine` constructors and two producers. The lookup was
  chosen (Design, "Fallbacks"). D1 records the later sites, which swap to the same lookup.
- **CR7 fixed.** `resolve_routine_selector(resolved, declaring, selector)` takes the
  `RoutineJoin`'s `DeclaringObjects`. Both callers (`query touches` `--from`,
  `query effects`) build the join first, so there is one per run. The SmallVec idea was
  skipped: the gain is not measured, and it adds a dependency use for one Vec per id.

**Proofs (new):**

| # | break | test | broken result | restored |
|---|---|---|---|---|
| P6 | `to_location` takes the object from `idx.object(r.object_id)` again | A7 | FAILED: `[("P","Bar",…),("Q","Bar",…)]`, want P -> `Foo`; other 22 CLI tests pass | pass (23/23) |
| P7 | `.min_by(extent_cmp)` -> `.next()` (first containing row) | U1 (f) | FAILED: `Some("Outer")`, want `Some("Inner")` | pass |

Each break was made with Edit and its marker counted to be exactly 1. Before the gate, a
grep over `src` and `tests` found no marker left.

## Final round 2 — CR4 re-raised by sol (implementer)

- **CR4 fixed again.** sol's re-raise was correct. `extent_cmp` subtracts columns per
  anchor, so outer `(0,0)..(40,0)` and the strictly nested inner `(0,5)..(40,0)` both have
  extent `(40,0)`, and `min_by` kept whichever row came first. Among containing anchors,
  `DeclaringObjects::of` now takes the INNERMOST: the latest start `(line, col)`, then the
  earliest end `(line, col)`, then model order (`min_by` keeps the first). That is exact
  for nested containment. `PAnchor::extent_cmp` is unchanged and still used by
  `gate/run.rs` (CR5's identical-behaviour promise holds). `declaring.rs` no longer uses
  `extent_cmp`.
- U1 gains case (g), sol's tie, in both row orders, expecting `Inner`. Case (f) is kept.
- **Proof P8:** the code went back to `.min_by(|(_, a), (_, b)| a.extent_cmp(b))`, with the
  marker counted to be exactly 1. U1 FAILED at (g): `Some("Outer")`, want `Some("Inner")`.
  Reverted; U1 passes.
- **Rebase:** onto origin/master `83ba322a` (one agentflow-only commit), with no
  conflicts. New SHAs:
  - `575f7e7c` plan
  - `3b37239c` fix
  - `c99ad275` final-1
  - `62473c34` final-2, the head
- **Gate on the rebased head:** `scripts/check-goldens` exit 0, no golden moved.
  `bash scripts/ci-steps task` exit 0: 3996 passed, 8 skipped. check-diff
  `--base origin/master --issue 241`: `reasons: []`.
- **Register:** CR4 has new evidence and is re-anchored to
  `src/program/model/declaring.rs:39-51` (hash `e67f3bcb…`); its marks stay `unreviewed`.
  Every other anchor was re-hashed after the rebase and is unchanged. The commit references
  in the CR evidence were updated from `e477d595` to `c99ad275`.

## Final panel (conductor)

- Final round 1 (head f62714f8): sol and opus both ship (O5 accepted); built-in code-review CR1-CR7 (CR1 important: analyze finding locations still named the last arm). Fixed in e477d595 (CR2 refuted by experiment: effect-line owners come from OperationJoin, last-wins by #160 step 11b; CR6 refuted: spec-panel choice).
- Final round 2 (head e477d595): opus accepted CR1-CR7; sol accepted all but CR4 (re-raised: extent_cmp ties a nested anchor with its container). Fixed in 62473c34 (innermost = latest start, then earliest end; U1 (g), break P8), rebased onto 83ba322a.
- Final round 3 (head 62473c34): both accepted CR4; both ship. Register: all entries accepted by both. final_rounds remaining 2.
- Step 11 gates on 62473c34 (base 83ba322a, supervised, run 20261024-075409-2f089f): check-goldens-coverage exit 0 (0.0 s); ci-steps-all exit 0 (375.8 s); check-goldens exit 0 (0.1 s, tree-stamp skip); cdo-gate exit 0 (287.6 s).
