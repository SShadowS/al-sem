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
