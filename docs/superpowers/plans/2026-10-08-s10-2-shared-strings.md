# S10.2 — Strings stored once (shared `Arc<str>`)

Spec: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §8 item 2 (re-priced
2026-10-08: second). Base: `tools/census-probe/runs-s10-1b/`.

## Owner decision (2026-10-08)

Shared `Arc<str>`, not `u32` symbols. The spec's "string table" is realised as one pool of
shared strings per dependency tier: equal strings become one allocation, and every clone of a
tier value (event links, `incoming`, per-root copies of dependency ids) shares it. Comparison,
hashing, `Borrow<str>` lookups, sort order and serde output (`rc` feature) are those of the
text, so no output and no golden may move. Memory frees with its last holder, so nothing grows
in a long-running server (a `u32` table would need lifetime rules for its per-root part, and
every reader would need the table).

## What the bytes say (CG 7 roots, `embedded`, `runs-s10-1b/cg-embedded-base.txt`)

1.15 M strings, 27.6 MiB of text; 175 k distinct (5.4 MiB). A `String` field is 24 B inline,
an `Arc<str>` 16 B. The largest fields: `dep_meta` `virtual_path` 5.58 MiB (94 % duplicate),
`param_sig_key` 2.59, `param.ty` 2.50 (96 %), routine `name`/`name_lc` 2.2 each in both
`dep.routine` and `dep_meta`, event-link `rid.name_lc` 1.89 (95 %). S10.3 and S10.4 later
remove many id and name COPIES; this step makes the copies that remain share one text.

## Design

- **`StrPool`** (`src/program/str_pool.rs`): a `HashSet<Arc<str>>`; `share(&mut Arc<str>)`
  replaces the value with the pool's copy (inserting it when new). A trait `ShareStrings`
  (`fn share_strings(&mut self, pool: &mut StrPool)`) is implemented by every type below; the
  impls name each field, so a new string field is a visible decision.
- **The pass:** `build_dep_nodes` runs one pool over objects, routines and `dep_meta` after its
  sort and dedup, before the tier is wrapped. Single-threaded; the workspace part (a few
  hundred strings per root) is not pooled.
- **Fields that become `Arc<str>`:**
  - T1 identity: `RoutineNodeId.name_lc`, `.enclosing_member_lc`; `ObjKey::Name`.
  - T2 nodes: `ObjectNode` (`name`, `extends_target`, `implements`, `protected_vars`),
    `FieldNode`, `PageControlNode`, `DataitemNode`, `QueryColumnNode`, `RoutineNode`
    (`name`, `param_sig_key`, `return_type`, `return_type_id.0`), `ParsedSubscriberArgs`
    (string fields), `AbiParamRetained` (string fields). **`ObjectRef::Name` stays
    `String`** (decided in T2): the resolver builds a transient `ObjectRef` per lookup,
    so `SharedStr` would add an allocation and copy on a hot path, and the stored ones
    total 0.24 MiB on CG.
  - T3 metadata: `RoutineMeta` (`name`, `enclosing_member`, `virtual_path`), `ParamMeta`
    (`name`, `ty`).
- **Not in scope:** dependency bodies (`Keep`), edge witness/evidence (S10.5), `dep_lines`
  keys (0.6 % duplicate).

## Tasks (one commit each; `scripts/ci-steps task` green; CDO count unchanged; no golden moves)

- **T1** `StrPool` + `ShareStrings`; identity fields; the pass over ids. Test: after a two-root
  build, equal `name_lc` texts across the tier's routines, `dep_meta` keys and a second root's
  event links are one allocation (`Arc::ptr_eq`). Discrimination: skip the pass and it fails.
- **T2** node fields, extend the pass; same test over node fields.
- **T3** metadata fields, extend the pass; same test over `dep_meta` values.
- **T4** probe (update for `Arc<str>`: count each distinct allocation once), measure CG + CDO,
  both modes, with and without updaters; measurement-auditor; CHANGELOG; spec status line.

Landed: T1 `7ac7a9ce`, T2 `4e36888f`, T3 `e7c86a6e`, and `256d7942` from the audit (a
synthesized platform-event publisher id, built per root, copied its subscriber's text: CG event
links held 7,526 allocations for 3,134 distinct texts; it now clones it, and the T2 test pins it
with a dependency subscriber to `OnAfterInsertEvent`, failing when the copy is restored).
Beyond the plan, T3 makes one
`virtual_path` text per file in `file_routine_meta` and keys `dep_lines` by `SharedStr` (a
position lookup used to allocate a `String`). Discrimination, each break made with Edit, run and
reverted (observed in the session's test logs, not saved in the repo): skipping the routines'
pass fails the id test (`"post" is a second allocation`); dropping `name` from `RoutineNode`'s
impl fails the direct `Run` check; skipping the objects' pass first PASSED (the fixture had no
repeated object-field text), so the fixture gained a table with two `Code[20]` fields, after
which the same break fails; skipping the `dep_meta` values' pass fails the cross-file `Run`
check. CDO: after each task's commit the two JSON files were `cmp`-identical to the ones the
S10.1b binary wrote just before T1 (SHA-256 `--program-call-graph-stats`
`2009fab1ca0849ab5c065759b873806e585b429e5b45c3cbdee178302e343c25`,
`--dependency-bodies-stats --sites`
`659e744a8fa0a5f3f748679e069ed97637f4a7b2c106cab4f68d51bece16f1cd`; the JSON files themselves
were not kept). No golden moved.

## Result (T4, 2026-10-08; `tools/census-probe/runs-s10-2/` vs `runs-s10-1b/`)

Counted heap, MiB; same matrix (CG 7 roots, CDO; both modes; with and without updaters).
Measured at `256d7942`; the probe's string walk runs after the heap figures are taken and frees
its maps first, and it is skipped in `--with-updaters` runs, so its new "held" column cannot
move any figure below (audited).

| Cell (file) | S10.1b | S10.2 |
|---|---:|---:|
| CG 7 roots live (`cg-embedded-base`) | 193.8 | 156.6 |
| CG 7 roots idle with updaters (`cg-embedded-updaters`) | 228.4 | 186.0 |
| CG root 1 build peak (`cg-embedded-base`) | 297.2 | 274.8 |
| CG roots 2-7 build peak (`cg-embedded-base`) | 24.8-25.1 | 19.2-19.6 |
| CDO retained / idle with updater (`cdo-embedded-base` / `-updaters`) | 275.7 / 288.0 | 231.7 / 243.0 |
| CDO build peak (`cdo-embedded-base`) | 410.5 | 363.6 |
| CG `symbols` live / idle (`cg-symbols-base` / `-updaters`) | 71.6 / 106.0 | 62.0 / 91.4 |
| CG `symbols` build peak, root 1 / roots 2-7 (`cg-symbols-base`) | 138.9 / 15.8-16.2 | 126.4 / 12.0-12.6 |
| CDO `symbols` retained / idle (`cdo-symbols-base` / `-updaters`) | 148.7 / 161.2 | 134.0 / 145.3 |
| CDO `symbols` build peak (`cdo-symbols-base`) | 194.5 | 183.1 |
| CG live allocations, 7 roots (`cg-embedded-base`) | 1,658,289 | 595,730 |

Where the CG live drop (−37.2) comes from, from the walk's own sections (`cg-embedded-base`,
"all 7 roots"): text −19.73 (1,151,282 strings with 27.63 MiB of text, each its own allocation
before since every field was a `String`, are now held in 239,224 allocations, 7.90 MiB; the
"held" column counts each allocation once by data pointer); container elements −13.61
(77.12 → 63.51) and Vec buffers −3.10 (16.93 → 13.83), from the smaller fields
(`RoutineNodeId` 96 → 72 B, `ObjectNodeId` 32 → 24, `RoutineNode` 280 → 224, `RoutineMeta`
200 → 176, `ClassifiedEdge` 416 → 344). Together −36.44. The walk does not count the terms that
pull the other ways: each `Arc` adds a 16 B header (at most 239,224 × 16 B = 3.65 MiB, fewer
because some held strings are still `String`), while emptier hash-table slots and `String`
capacity slack were savings. They are unmeasured, so the remainder is not bounded.
Held still exceeds distinct globally (239,224 against 175,458): the remaining duplicates are
fields still `String` (largest: event-link `witness.file` 23,871 for 583 texts and
`span.unit`, left for S10.5; `ObjectRef`, deliberately), unpooled workspace text and the
`dep_lines` keys. The 1.06 M fewer allocations also remove allocator overhead the counter does
not include (estimated at 24 B each in the spec, about 24 MiB; not measured, and not part of
any figure above). RSS (one run each, context only): peak working set CG 430.5 → 430.3, CDO
522.6 → 496.6 (`*-embedded-updaters`). Build times are not a result (CG roots 2-7 0.9-1.0 s
against 1.0-1.2 s; root 1, identical work, went 3.8 → 3.1 s in `cg-embedded-base`).
Residual after the roots and the cache drop: 0.14 MiB in 65 allocations, as in S10.1b
(another run of the same code read 0.29; run-to-run range 0.14-0.29).
