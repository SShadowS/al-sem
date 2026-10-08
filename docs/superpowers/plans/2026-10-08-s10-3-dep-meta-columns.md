# S10.3 — Dependency metadata as columns beside the routine rows

Spec: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §8 item 3 ("`dep_meta`
becomes columns indexed by routine number: removes the hash map, its empty slots and the
repeated id and name; optional and boolean node fields are packed"). Base:
`tools/census-probe/runs-s10-2/`.

## Re-priced (CG 7 roots, `embedded`, `runs-s10-2/cg-embedded-base.txt`)

- `dep_meta` is a `HashMap<RoutineNodeId, RoutineMeta>`: 111,637 entries × (72 B key + 176 B
  value) = 26.40 MiB of elements, plus the empty slots of its table (not counted by the walk;
  at hashbrown's 7/8 load the table has 131,072 slots, about 4.6 MiB more) and one control
  byte per slot. Parameter lists, 6.08 MiB, are values and stay.
- The key is a second copy of a tier routine's id: the tier holds 111,637 shared routines and
  `dep_meta` 111,637 entries (CDO 121,075 and 121,075). Each source routine of the tier has
  exactly one entry.
- Readers only `get` by id (`DeclSurface`, the registry, `LspSnapshot::decl_and_line_table`
  via `get_key_value`), iterate, and count.

## Design

- **`DepMeta`** (in `decl_surface.rs`, replacing `DepMetaMap`), built once per tier:
  - `routines: Arc<Vec<RoutineNode>>`: the tier's own rows (sorted by id), shared, not copied.
  - `metas: Vec<RoutineMeta>`, exact capacity, in id order.
  - `key_row: Vec<u32>`: per meta, the first tier row with its id (that row's id is the key).
  - `orphans: HashMap<RoutineNodeId, RoutineMeta>`: a meta whose id matches no row. Expected
    empty; kept so nothing is ever dropped (the probe reports its size).
  - `get(id)`: binary search of the metas by their key (`routines[key_row[i]].id`); else
    `orphans`. No per-row index: no reader goes from a row to its meta. `get_key_value`,
    `iter`, `keys`, `values`, `len`, `is_empty`, and set-like `PartialEq` (tests compare tiers).
- **Same semantics as the map**: the summaries arrive in parse order, and a same-id collision
  keeps the LAST meta (`HashMap::extend`). The build sorts `(id, meta)` stably by id and keeps
  the last of each run, then merges with the sorted rows.
- **Not changed**: `RoutineMeta` itself, so `get` still returns `&RoutineMeta` and no reader
  changes beyond the type name. The workspace part of `DeclSurface` (`local`) stays a map.
- **Packing** (spec's second half): priced after T1 with `size_of` (`RoutineNode` 224 B,
  `RoutineMeta` 176 B, of which the two `Origin`s are 96). Built only if worth it.

## Tasks (one commit each; `scripts/ci-steps task` green; CDO JSON byte-identical; no golden moves)

- **T1** `DepMeta` + build + readers. Tests: a two-root tier built twice answers `get` for every
  id exactly as the old map (built from the same summaries by `HashMap::extend`), including a
  same-id collision (last wins) and an ABI-only app (no meta); `orphans` empty on the fixtures.
  Discrimination: first-wins instead of last-wins fails the collision test; sending an
  unmatched meta nowhere (instead of `orphans`) fails the orphan test.
- **T2** probe + measure (CG + CDO, both modes, with and without updaters); price packing.
- **T3** packing, if T2 shows it is worth building.
- **T4** audit, CHANGELOG, spec status.

T1 landed as `e5f3d207`. Discrimination (each break made with Edit, run, reverted; observed in
the session's test logs, not saved): keeping the FIRST entry of a same-id run fails
`dep_meta_answers_as_the_map_it_replaces` (the existing sibling-app test
`tier_decl_surface_and_recovered_files_match_the_all_units_build` does NOT fail, since its
duplicated entries are identical: a stated limit of that test); dropping an unmatched entry
instead of keeping it in `orphans` fails the same test. After the audit the test also covers a
row without metadata (an ABI routine: `get` is `None`). CDO: both stats JSON files
(`--program-call-graph-stats`, `--dependency-bodies-stats --sites`) were `cmp`-identical after
`e5f3d207` to the S10.1b binary's, so their SHA-256 are the ones recorded in the S10.2 plan (the
files were not kept); no golden moved.

## Result (T2, 2026-10-08; `tools/census-probe/runs-s10-3/` vs `runs-s10-2/`, at `e5f3d207`)

Counted heap, MiB; same matrix. The probe now sizes `dep_meta` as a `RoutineMeta` column plus a
`u32` per meta and no longer counts a key per entry (the key is the tier row's id, counted under
`dep.routine`); its shape line reports orphans: 0 in every run.

| Cell (file) | S10.2 | S10.3 |
|---|---:|---:|
| CG 7 roots live (`cg-embedded-base`) | 156.6 | 144.7 |
| CG 7 roots idle with updaters (`cg-embedded-updaters`) | 186.0 | 174.1 |
| CG root 1 `3.dep_layer` in-phase peak (`cg-embedded-base`) | 274.8 | 269.8 |
| CG roots 2-7 build peak (`cg-embedded-base`) | 19.2-19.6 | 19.2-19.7 |
| CDO retained / idle with updater (`cdo-embedded-base` / `-updaters`) | 231.7 / 243.0 | 190.2 / 201.5 |
| CDO `3.dep_layer` in-phase peak (`cdo-embedded-base`) | 363.6 | 339.9 |
| `symbols` mode, every cell (the control: no metadata) | — | ±0.1 |

The `3.dep_layer` peak repeats exactly between runs of the same code (both S10.2 runs read 274.8
and 363.6, both S10.3 runs 269.8 and 339.9). The overall build peak can now be set by the
`2.parse` phase instead, whose peak varies between runs of identical parse code (CG
268.0-275.4, CDO 333.8-349.1; the CDO updaters run's build peak of 349.1 is a parse peak), so a
further dep-layer saving no longer carries 1:1 into the process build peak.

Where it comes from: the drop steps for the tier's routines and `dep_meta` together (the column
holds the tier's routine rows, so they now free in the `dep_meta` step and the `routines` step
reads 0.00): CG 26.84 + 43.72 = 70.56 → 58.61 (−11.95), CDO 29.02 + 75.79 = 104.81 → 63.34
(−41.47); these match the idle drops. The column holds 18.74 MiB of metas plus 0.43 of row
indices (`key_row`) on CG (CDO 20.32 + 0.46), against 26.40 (CDO 28.64) of key-and-value
elements in the map before, which also held empty slots. By arithmetic, matched by the drop
steps: hashbrown sizes a table to a power of two at 7/8 load, so CG's 111,637 entries sat in
131,072 slots (85 % used) and CDO's 121,075 in 262,144 (46 % used), which is why CDO gains more;
old table (slots × 248 B plus one control byte per slot) minus the new column predicts
−11.96 MiB on CG and −41.47 on CDO, against −11.95 and −41.47 measured. RSS (one run, context only): peak working set
CG 430.3 → 389.5, CDO 496.6 → 487.3 (`*-embedded-updaters`). Residual after the roots and the
cache drop: 0.14-0.22 MiB.

**Packing, priced (not built).** `RoutineMeta`'s two `Origin`s are 96 of its 176 B (each a 16 B
`&'static str` kind, a 16 B byte range, two 8 B points); a packed form (kind as a `u16`, ranges
and points as `u32`, 26-32 B) saves 16-20 B each, 32-40 B per meta: about 3.4-4.3 MiB on CG,
3.7-4.6 on CDO, by arithmetic. `Origin` is the IR type every reader uses, so it needs a separate
dependency-tier meta type (or a change to the IR type). For `RoutineNode` itself nothing was
priced beyond one item: a boxed slice instead of a `Vec` for `event_subscribers` is 8 B less per
row, by arithmetic about 0.85 MiB on CG and 0.92 on CDO (whether `size_of::<RoutineNode>()`
actually drops was not checked). Owner's call.
