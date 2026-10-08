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
    `ObjectRef::Name` (`raw`, `normalized_lc`), `FieldNode`, `PageControlNode`,
    `DataitemNode`, `QueryColumnNode`, `RoutineNode` (`name`, `param_sig_key`,
    `return_type`, `return_type_id.0`), `ParsedSubscriberArgs` (string fields),
    `AbiParamRetained` (string fields).
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
