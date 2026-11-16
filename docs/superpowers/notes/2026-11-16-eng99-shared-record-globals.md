# ENG-99: an object's record-variable globals, stored once

Status: design, before code. Census: ENG-96 (`tools/census-probe/runs-eng96/`).

## Why

`ModelRoutine.record_variables` holds the routine's own record variables (params, a named return
value, locals, the implicit `Rec`, report dataitem names) followed by every object GLOBAL record
variable the routine does not shadow, copied into each routine and re-keyed
`{routine_id}/rv/{name}` (workspace.rs, "Task 3 PROMOTION + SHADOWING"). On CDO 80% of the
564,998 rows are such copies (~45 MiB of 104-byte rows, plus the dependency rows' unshared ids,
30.9 MiB). Scalar globals already avoid this: `RoutineVariables` shares the object's
`Arc<[ModelVariable]>` and keeps a per-routine shadow set.

## What the consumers need (consumer map, 2026-11-16)

- **Readers find by name** almost everywhere; nothing outside one test relies on "own rows first";
  the list is name-unique (shadowed globals are left out) and code relies on that.
- **The per-routine id of a promoted global** reaches output: record_types.rs pass 2a writes it into
  `op.record_variable_id`, which flows into `DbEffect`, `CapabilityExtra::Table.record_variable_id`
  (the capability repKey and the snapshot JSON `recordVariableId`); the G-8 backfill writes it into
  `binding.source_record_variable_id`. Every consumer compares these ids WITHIN one routine
  (op side, binding side, row), never across routines.
- **One per-routine mutation of a global:** the G-2 entry guard (`if not X.IsTemporary then
  Error`) sets `temp_state = Known(true)` on the guarded variable in that routine only, globals
  included on purpose.
- **Resolution writes rows in place** (`resolve_routine_record_types`: table_id, the TableType
  override, the entry guard), and the session row cache resolves a clone of its raw rows.
- `Model::project()` emits every visible row per routine (the r2a `l3rt` goldens): globals must
  still appear per routine, shadowing applied (order is sorted away).

## Decision (mine; mantra 1, 2, 13, 14, 22)

1. **A promoted global keeps its object-level id**, `{object_id}/grv/{name}` (built already by
   `ir_object_global_record_vars`, thrown away at promotion today). A global is one variable; the
   per-routine id was an artefact of copying. Ids are only compared within a routine, so every
   comparison still agrees; the output change is the `recordVariableId` (and repKey) of facts on a
   global, which the goldens and `cdo-before-after` will show and which I will triage line by line.
   Rejected: an accessor that synthesizes the per-routine id on each read (138 call sites expect a
   `&ModelRecordVariable` with an `id` field; a fresh `String` per read; mantra 13/14).
2. **Storage, the `RoutineVariables` pattern:** the routine holds its own rows and an
   `Arc<[ModelRecordVariable]>` of the object's globals plus the shadowed names; one accessor,
   `record_variables()`, yields own rows then the unshadowed globals (the order today). Find-by-name
   helpers go through it.
3. **The entry guard stays per routine:** when a routine's guard names a global, that routine
   copies the one global into its OWN rows with `Known(true)` and shadows it in the shared list, the
   way a same-named local shadows it today. The shared row is never written (a shared write would
   leak into sibling routines), and readers keep getting a plain `&ModelRecordVariable`. Same id
   (the object-level one), so op and binding ids still agree.
4. **Resolution of globals runs once per object,** before routines share them: the object's raw
   globals are resolved (table_id, TableType override) into a new `Arc`, and the routines point at
   it. The row cache keeps raw `Arc`s; resolving a clone makes a new resolved `Arc` per object, never
   writes into the cached one.

## Revised after sol's design review

- **Per declaration, not per object id.** Split `#if` object declarations share an id but can
  declare different globals and different source tables: each declaration gets its own shared
  `Arc` (the promotion loop already runs per declaration).
- **Keep every pass's order; share only what is per declaration.** Pass 1 (a global's `table_id`
  from its `table_name`) and the `TableType=Temporary` override that follows from it are the same in
  every routine of the declaration: run once on the shared rows. Every per-routine write to a
  global happens, unchanged, at the pass where it happens today, through **copy-on-write**: the
  routine copies that one global into its own rows (shadowing the shared one) and writes the copy.
  That covers pass 3 (an unresolved global named `Rec`/`xRec` takes the object's own table, after
  pass 2a, with its literal receiver match: moving it earlier would turn d33's "unresolved: skip"
  into a finding) and the G-2 entry guard (at its late pass, after the G-8 backfill read the
  unguarded state: copying earlier would make forwarded bindings temporary and change d39).
- No cross-routine merge keyed on the id was found (capability_cone.rs compares subject and
  witness before the extra holding `recordVariableId`); the id-shape audit decides the output
  question.

## Final shape (after Fable's design review)

`ModelRoutine.record_variables: Vec<Arc<ModelRecordVariable>>`. Fable's simpler option, chosen
over the `RoutineVariables` shape because it keeps every reader's `.iter()` and the list's order
exactly, and makes every per-routine write copy-on-write by construction:

- **Assembly:** each object declaration builds its globals once, as `Arc`s with the object-level id;
  each routine's list is its own rows (`Arc::new`) then `Arc::clone` of the unshadowed globals.
- **Resolve:** before a routine's passes, its global rows are swapped for the declaration's
  pass-1-resolved rows (a map from the raw `Arc` pointer to the resolved one, so `#if`-split
  declarations never share, and the row cache's raw rows are never written). Pass 1 then finds the
  value already set and writes nothing. Every other write (pass 3's `Rec` fill, the `TableType`
  override, the entry guard, `event_param_temp`) goes through `Arc::make_mut` at its existing pass:
  it copies that one row for that routine, exactly when and where the write happens today, so every
  earlier reader (the G-8 backfill, pass 2a, the `GetTable` pass) still sees what it sees today.
  Writes only when the value changes, so an unchanged row stays shared.
- **Cost:** a pointer per promoted row stays (~3.6 MiB of ~45 MiB on CDO); rows the `TableType`
  override touches are copied per routine (globals of temporary-type tables, rare).
- **Fingerprint:** `{:#?}` of an `Arc<T>` prints `T`, so `model_fingerprint` is unchanged in form.

**The id in OUTPUT (stated for the orchestrator).** A shared row holds one id, so under this
design a promoted global's id is the **declaration's object-level id**, `{object_id}/grv/{name}`,
in the row and everywhere it is copied: `op.record_variable_id` (pass 2a),
`binding.source_record_variable_id` (the G-8 backfill), `DbEffect`, and the `recordVariableId` of
capability facts and the snapshot JSON. It is no longer the per-routine `{routine_id}/rv/{name}`.
A global copied into a routine by a per-routine write (`make_mut`) keeps the same object-level id.
Locals and parameters keep their per-routine ids unchanged.

The fallback, if the id-shape audit finds a user-kept or outside-facing field: keep the row shared
but write the per-routine id only at the two places that copy it out (pass 2a and the G-8
backfill). Its catch: within one routine the op id, the binding id and the row id would then
disagree, and the readers that match them (`LoadPoint::is_variable`, `binding_source_table`,
d37) would fall back to matching by name, which needs its own proof of equivalence before use.

## Open questions for review

- Does any consumer need a global's id to differ per routine (e.g. a cross-routine dedup that
  would now merge two facts)? The map found none; the CDO before/after will show it.
- Does the accessor's `Known(true)` view (point 3) have a consumer that reads the shared row
  directly and so misses the guard?
- Is resolving per object (point 4) exactly what per-routine resolution produced for a global
  (pass 3 sets an unresolved `rec`/`xrec` global from the object's own table: per object, yes)?
