# L4 summary frozen baseline (Task B1 — old-Jacobi retirement)

These files are the **complete-internal-surface** `RoutineSummary` baseline that
`tests/l4_summary_differential.rs` asserts the v2 interned-columnar
`EffectStore` solver against, after the old Jacobi solver was retired (spec
`docs/superpowers/specs/2026-07-22-l4-dbeffect-store-and-retirement-design.md`,
Part B).

## What is frozen

Each `<fixture>.baseline.txt` is the pretty-`Debug` (`{:#?}`) rendering of the
fixture's v2 output `HashMap<String, RoutineSummary>`, with the outer map sorted
by `routine_id` for determinism. This captures **every** `RoutineSummary`
field — `db_effects` (sequence + order, `effect_key`, op/table/operation, temp,
via, `record_variable_id`), `uncertainties`, `has_unresolved_calls`,
`parameter_roles`, `in_recursive_cycle` — i.e. the complete internal surface,
NOT the `stable_summary_fingerprint` (which omits internal fields).

`cdo-whole-program-digest.txt` is the SHA-256 (hex) over the same canonical
`{:#?}` rendering of the CDO whole-program v2 output (the DO source-only
workspace's ~3685-routine population — too large to commit readably, so frozen
as a digest).

`cdo-reverse-index-digest.txt` is the SHA-256 (hex) over a *different*
canonical serialization: `ReverseEffectIndex::build`'s answer, not
`RoutineSummary`. For every table id in the frozen effect universe (sorted),
the sorted list of `bundle.routine_id(ix)` that `up_table(table_id)` returns,
rendered as a `BTreeMap<String, Vec<String>>` (`{:#?}`) so ordering is
unambiguous both across table ids and within each table's routine list. See
"The reverse-index digest" section below for corpus, coverage, and the
regeneration command — it follows a different (stronger) capture discipline
than the `RoutineSummary` baselines above.

## Provenance — captured at parity with the old Jacobi solver

- **Pre-deletion tag (old oracle one `git checkout` away):** `l4-pre-jacobi-deletion`
  (an annotated tag pointing at `f295ef8`, `docs(l4): 8020/CDO re-measure note —
  Part A store redesign`), the HEAD of `feat/l4-summary-redesign` immediately
  before Part B began. Citing the tag rather than the bare SHA keeps this
  instruction reachable across a squash-merge, which would otherwise leave
  `f295ef8` unreferenced by any branch and eligible for GC. `git checkout
  l4-pre-jacobi-deletion -- src/engine/l4/summary_runner.rs` restores the old
  `compute_summaries_with_leaves` for forensic re-differencing.
- The baselines were regenerated from **v2** while the old solver still existed.
  In the same working tree the differential's `cross_check_v2_equals_old_*`
  tests (fixtures) and `cdo_whole_program_v2_parity` (CDO) were **green** — so
  at capture `baseline == old == v2`. Those two v2==old cross-checks were kept
  live through the R3b/aldump-cut steps and deleted only in the final commit
  that deletes the old solver, leaving `v2 == frozen-baseline` as the permanent
  regression anchor.

## Re-freeze log — `cdo-whole-program-digest.txt`

Every regeneration of the CDO digest is recorded here, with the evidence that
root-caused the movement. The frozen baseline's independent oracle (the old
Jacobi solver) was retired, so a bare "regenerated, tests green" entry proves
nothing — see the warning at the end of this file.

### 2026-07-25 — `d3fc4f0e…d42d401` → `d9eac0c7…37def214` (3685 → 4842 routines)

**Cause:** the conditional enclosing-member discriminator on the INTERNAL routine
id (commit `c4c3d03`, `feat/l3-substrate-and-parked-items` task 3). `canonical()`
sorts the summary map by `routine_id` and the map is KEYED by it, so this moves
both the ordering and the population: member triggers that previously collided on
one id now get one entry each.

**Evidence — exact single-variable attribution, not a masked-diff argument.**
Disabling the one behaviour this task added (short-circuiting the conditional
7th-key-part append in `ids::encode_canonical_routine_key`, leaving the rest of
the tree at `c4c3d03`) reproduces the OLD frozen digest `d3fc4f0e…` **byte for
byte**, with the routine count back at 3685. Nothing else in the tree contributes
to the movement — an unrelated regression could not have hidden, because it would
have moved that run too. This is strictly stronger than the "mask the ids and
compare the remainder" method used for the committed goldens, which is available
only when you cannot toggle the schema.

**Characterization** (canonical `{:#?}` renderings dumped with and without the
discriminator, then compared):

| measurement | value |
|---|---|
| routine ids unchanged (member-less: procedures, object-level triggers) | 3231 |
| member-bearing ids replaced | 454 |
| member-discriminated ids minted | 1611 |
| population delta | **+1157** |
| ids violating the `{modelInstanceId}/{64 lowercase hex}` shape | **0** |
| lines retaining a BARE 64-hex run after masking `r0/`-prefixed ids | **0** (both dumps) |
| unchanged-id records byte-identical after masking | 3182 / 3231 |
| unchanged-id records that differ | 49 |
| new struct/variant shapes, new `via` / `op` / uncertainty-`kind` values | **none** — the value domains are identical |

- `+1157` matches, exactly, the independently-measured figure recorded in
  `src/engine/l5/detector_context.rs` ("1 157 of 4 842 DO routines (23.9 %) are
  erased by it"), taken by a different route before this change existed.
- The `454 → 1611` split reconciles with the CHANGELOG's independently-measured
  "DO 262 collision groups → 0": 454 member-bearing ids moved, of which 262
  actually COLLIDED (a member-bearing id with no same-named sibling moves without
  ever having collided); `1611 − 454 = 1157 = 1419 − 262` is consistent with 262
  groups holding 1419 routines and the remaining 192 ids being singletons.
- The 49 differing records change ONLY `DbEffect` membership (net 2277 lines
  removed vs 1471 added — a cone SHRINK, mostly `via: "implicit-trigger"`
  entries) and `has_unresolved_calls`. That is the documented mechanism: a
  colliding id's cone was `(one sibling's direct facts) ∪ (cone over ALL
  siblings' callees)`, so de-colliding removes cross-body splices. No field, no
  shape, and no value domain moved.

**Note on what runs this.** This paragraph is stale as written above (kept for
history) — a later fix wave (`scripts/cdo-gate`'s own `⟨task-3 fix wave, review
I-2⟩` comment) added `--test l4_summary_differential` to `cdo-gate` precisely
because this digest had no runner. As of this writing `scripts/cdo-gate`
DOES execute the whole `l4_summary_differential` binary (no test filter) under
`ENFORCE_CDO_WS=1`, so both `cdo-whole-program-digest.txt` and
`cdo-reverse-index-digest.txt` below run there, in addition to
`scripts/check-goldens` (whenever `CDO_WS` is set) and the explicit commands
below.

## The reverse-index digest — `cdo-reverse-index-digest.txt`

Added closing scope-reverse-index-consumer.md §4.2 level 2's third bullet (the
task-6 report flagged it as deliberately not done — no `CDO_WS` on that
machine, and this repo's `CDO_WS` **is** the DO workspace, so a capture against
it is honestly labelled `cdo-`, not a mislabelled DO capture).

- **Corpus:** `CDO_WS=U:/Git/DO.Support-SlowDOSetup/DocumentOutput/Cloud` (this
  repo's only `CDO_WS`), assembled the same way as
  `cdo_whole_program_v2_matches_frozen_digest` (at capture: L3's
  `assemble_and_resolve_workspace_default` → `SymbolTable::build` → `resolve_calls`
  → `build_event_graph` → `build_combined_graph` → `tarjan_scc`; since engine-switch
  S9.5d the production model's own calls and events, re-frozen then), then
  `compute_summaries_v2_bundle_with_leaves` to get the compact `SummaryBundle`
  `ReverseEffectIndex::build` reads. At capture: 4842 routines-with-rows, 61
  tables, 2787 effects in the frozen universe.
- **What it covers:** `ReverseEffectIndex::up_table`, exhaustively over every
  table id in the universe — the same `up_table` the exhaustive oracle
  comparison already checked table-by-table in the same test run. It does NOT
  additionally freeze `up_effect`/`touches_effect`/`ancestors_touching`/`down`;
  those stay under the LIVE oracle (§4.2 level 2's other bullets, in the same
  test), which is the stronger mechanism — see `scope-reverse-index-consumer.md`
  §4.4 for why a golden/digest with no oracle is explicitly NOT recommended as
  the sole mechanism. The digest exists only to additionally pin that this one
  answer does not drift silently between runs where nobody happened to break
  the oracle comparison's assertions but the OUTPUT still moved (e.g. an
  ordering change that both sides of a diff happen to agree on).
- **Capture is mismatch-guarded, not just at digest-write time but by
  construction:** the digest computation lives at the TAIL of
  `cdo_reverse_index_matches_slow_oracle`, textually after every exhaustive and
  sampled oracle assertion in that same function. Since Rust runs a test
  function's statements sequentially within itself (unlike separate `#[test]`
  functions, which may run in parallel or any order), reaching the digest code
  at all is proof the oracle already agreed in this run — a panic anywhere
  above prevents the digest from ever being read OR written. This is a
  same-run version of this file's own `baseline == old == v2`-at-capture
  discipline above, using the still-live oracle in place of the now-deleted old
  Jacobi solver.
- **Where it's minted/checked:** `tests/l4_summary_differential.rs`,
  `cdo_reverse_index_matches_slow_oracle` (the same function as the oracle
  differential — see its doc comment). Gated by `cdo_ws_or_enforce()`: skips
  silently when `CDO_WS` is unset, panics under `ENFORCE_CDO_WS=1`.
- **Regenerate:**
  ```bash
  CDO_WS=U:/Git/DO.Support-SlowDOSetup/DocumentOutput/Cloud ENFORCE_CDO_WS=1 REGEN_TEMP_GOLDENS=1 \
    cargo test --profile release-fast --test l4_summary_differential \
    cdo_reverse_index_matches_slow_oracle -- --nocapture
  ```
  Regen is a measurement: it re-runs the full oracle comparison first (so a
  regen against a broken index fails before it can write a new digest) and
  only then writes. Inspect why the digest moved before committing a new one —
  the table-touch population is real BC source, so a movement means either a
  real workspace change (DO/CDO app version bump) or an engine behaviour
  change, never a formatting artifact.

## Regenerating the `RoutineSummary` baselines (a deliberate, engine-intended re-freeze)

```bash
REGEN_TEMP_GOLDENS=1 cargo test -p al-call-hierarchy --test l4_summary_differential
# CDO digest additionally needs the workspace:
CDO_WS=<path> REGEN_TEMP_GOLDENS=1 cargo test -p al-call-hierarchy --test l4_summary_differential cdo_
```

Regen is a **measurement**, never a blind bless — inspect the diff and
root-cause any movement before committing (the old solver is at the
`l4-pre-jacobi-deletion` tag, `f295ef8`, for re-differencing).

## Re-freeze log

- **2026-10-07, engine-switch S8 gap 1** (`408926b3`, a table method's `Rec` follows its
  receiver's temp state). `90d246d6…` -> `c771d616…`. Measured by dumping the canonical
  serialization at `f221d190` (matches the old digest) and at the branch head. 334 of 5479
  routines moved; no routine was added or removed; every moved effect was `|f`
  (Known(false)) on an operation inside a table method. They are now
  `|p4294967295` (the method's own `Rec`, the receiver index), `|t` (a temporary
  receiver), `|pN` (the caller's own `var` parameter is the receiver), or `|u` (8, a
  receiver that is not a declared record variable). The event-edge substitution
  (`239a157d`) moved nothing here.
- **2026-10-07, engine-switch S9.0c** (the lowerer models ternary, `is`/`as` and the list
  literal instead of an opaque `Unknown`). `c771d616…` -> `9e72fb92…`. Measured by dumping
  the canonical serialization with the three new lowering arms disabled (reproduces
  `c771d616…` exactly) and enabled. 2 of 5479 routines moved, none added or removed:
  `SendElectronicDocument` gains the two dispatcher-factory interface calls inside its
  ternary (with their open-world and opaque-callee uncertainties, and its record parameter
  now requires `"E-Document Send Code"` loaded); a second routine gains
  `has_unresolved_calls`, because its ternary call (`eDocsFeatureEnabled`) is now visible to
  L3's legacy resolver, which this digest still uses and which cannot resolve it.
