# Step 2: what the idle updater's resolver index holds, by field (2026-10-04)

## Conventions

Same as `docs/2026-10-04-step0-server-census.md`.

- Bytes are heap, requested size, from the probe's counting `#[global_allocator]` (hash-map table capacity and `Vec` spare capacity are included; allocator overhead is not). OS RSS is never a result.
- MiB = 1,048,576 bytes. "Retained" = live heap once it has stopped moving for 0.9 s.
- Engine: branch `feat/step2-lighter-root` with Tasks 1 and 2 applied (`dccf397a` object map holds workspace objects only, `819ea98e` LSP snapshot drops route-less event links), plus the census split of this commit. Probe: `tools/census-probe/`, new mode `--index-split`. Release build, thin LTO, `CARGO_TARGET_DIR=C:/lpt`.
- Raw output: `tools/census-probe/runs-index-split/{cg,cdo}-{embedded,symbols}-{split,updaters}.txt`. One run per cell. CG = 7 roots, 36 `.al` files; CDO = `bc3ccb18`.
- How the split is taken: after the roots are built (as in step 0), the probe builds, from each root's own graph and with the same calls `Rung1Context::build` makes, the `ResolveIndex`, `workspace_object_map` and `DeclSurface` (local part, over the shared `dep_meta`) one at a time, settling between them, and records the live-heap delta each adds. Then `ResolveIndex::census_parts()` hands back every field, and the probe drops them one at a time and records the live-heap and allocation drop of each. The structures are built by the probe, not read out of a running updater. To check that they are the same thing, a `--with-updaters` run on the same binary measures the real idle updater per root (a no-op `on_swap`; the server passes a real closure, but idle never calls it, so the idle heap does not depend on it).

## Per-root updater cost: the pieces add up to the measured total

| Corpus | Mode | Idle updater, per root (measured) | ResolveIndex | object map | DeclSurface local | Sum of the three | Residual |
|---|---|--:|--:|--:|--:|--:|--:|
| CG, root 1 (Core) | embedded | 56.81 | 56.80 | 0.00 | 0.01 | 56.81 | 0.00 |
| CG, root 1 (Core) | symbols | 31.82 | 31.81 | 0.00 | 0.01 | 31.82 | 0.00 |
| CDO | embedded | 65.74 | 62.63 | 0.04 | 3.06 | 65.74 | 0.00 |
| CDO | symbols | 44.21 | 41.11 | 0.04 | 3.06 | 44.21 | 0.00 |

MiB. The seven CG roots are within 0.03 MiB of each other in every column (index total 56.79 to 56.81 embedded, 31.80 to 31.82 symbols; updater 56.80 to 56.82 and 31.81 to 31.83). So the per-root updater cost is, to within 0.01 MiB, `ResolveIndex` + object map + `DeclSurface`'s local part. There is no residual to attribute. The three-way sum (not its parts) is also what the per-root number in the step 0 report (57.45 CG embedded, 66.3 CDO embedded) was, minus 0.64 MiB (CG) and 0.61 MiB (CDO). That drop fits Task 1's object map (0.00 and 0.04 MiB now); the probe did not isolate it.

## ResolveIndex by field (per root, MiB and live allocations)

CG embedded, root 1 (the other six roots match to 0.01 MiB; their sum is in `cg-embedded-split.txt`):

| Field | MiB | Allocations | Share of idle updater (56.81) |
|---|--:|--:|--:|
| `routines_by_obj_name` | 50.03 | 321,320 | **88.1%** |
| `objects_by_name` | 2.29 | 19,715 | 4.0% |
| `subscribers_map` | 1.84 | 8,505 | 3.2% |
| `objects_by_id` | 1.81 | 9,552 | 3.2% |
| `objs_by_number` | 0.77 | 2 | 1.4% |
| `page_extensions` | 0.03 | 260 | 0.05% |
| `table_extensions` | 0.02 | 160 | 0.04% |
| `implementers` | 0.02 | 148 | 0.04% |
| `report_extensions`, `ambiguous_subscriptions`, `orphaned_subscriptions` | under 0.01 each | 24, 1, 8 | under 0.01% |
| sum of fields | 56.80 | 359,695 | |
| `ResolveIndex` total (built) | 56.80 | 359,684 | |

The fields add up to the total within 11 allocations and under 0.001 MiB. Graph shape for this root: 9,795 objects (6 own), 112,182 routines (embedded mode; 547 in the own tier, of which 12 are workspace routines, from the symbols run of the same root).

Same table for the other three corpora (root 1 for CG symbols):

| Field | CG symbols | CDO embedded | CDO symbols |
|---|--:|--:|--:|
| `routines_by_obj_name` | 26.89 (84.5%) | 55.39 (84.3%) | 35.69 (80.7%) |
| `objects_by_name` | 2.27 | 2.51 | 2.49 |
| `objects_by_id` | 1.82 | 1.98 | 2.00 |
| `subscribers_map` | 0.00 | 1.87 | 0.04 |
| `objs_by_number` | 0.77 | 0.77 | 0.77 |
| extensions + implementers (4 fields) | 0.07 | 0.11 | 0.11 |
| `ResolveIndex` total | 31.81 | 62.63 | 41.11 |
| `DeclSurface` local | 0.01 | 3.06 | 3.06 |
| idle updater (measured) | 31.82 | 65.74 | 44.21 |

CG symbols values are root 1's (`cg-symbols-split.txt`); the seven roots agree to 0.02 MiB. Shares are of the measured idle updater of that row.

Shares of the per-root updater cost for `routines_by_obj_name`: CG embedded 88.1%, CG symbols 84.5%, CDO embedded 84.3%, CDO symbols 80.7%. Everything else in the updater is 6.78 MiB in CG embedded (4.93 in symbols) and 10.35 MiB in CDO embedded (8.52 in symbols). The "sum of the three" column above is the probe's directly printed pieces sum, not the rounded columns.

## Seven CG roots, embedded (sum)

`ResolveIndex` 397.60 MiB in 2,517,703 allocations, of which `routines_by_obj_name` 350.20 MiB (88.1%); object map 0.00 MiB (8 allocations); `DeclSurface` local 0.03 MiB. The real seven idle updaters added 397.6 MiB in 2,518,095 allocations in the `--with-updaters` run (`RETAINED, ALL 7 UPDATERS IDLE`: 673.6 MiB; all roots built 276.0 MiB). Symbols: index 222.67 MiB, of which `routines_by_obj_name` 188.22; the real updaters added 222.7 MiB.

## What this says

- The per-root cost of an idle updater is almost entirely one hash map. `routines_by_obj_name` is 88.1% of it on CG embedded (50.03 of 56.81 MiB), and between 80.7% and 88.1% in the four cells measured. It holds, for the whole graph (dependency routines included, about 112,000 on CG and 127,000 on CDO, embedded mode; symbols mode has 54,284 and 64,740), one entry per `(object, name)` key: an owned key (an `ObjectNodeId` plus a `String`) and a `Vec<RoutineNodeId>` of owned ids. Its 321,320 allocations are 89% of the index's 359,684 on CG embedded. The ids repeat what `graph.routines` already holds; whether a binary search over `graph.routines` could replace the map is Task 5 of `docs/superpowers/plans/2026-10-04-compact-graph-step-2.md`, not tested here. The census counts capacity as well as content: each value `Vec` starts at capacity 4 (4 x 96 B) and usually holds one id, so by arithmetic an estimated 25 to 29 MiB of the 50.03 MiB may be empty capacity. That is an estimate, not measured. The controller decided Task 5 removes the whole map (bytes and slack), so a length-versus-capacity split was not measured.
- The other index fields together are 6.8 MiB on CG embedded (12% of the updater): the two object-name and object-id maps (4.1 MiB), `subscribers_map` (1.8), `objs_by_number` (0.8, only 2 allocations, so a plain large table).
- `DeclSurface`'s local part is 0.01 MiB on CG and 3.06 MiB on CDO (the CDO workspace has 5,520 own routines; the embedded-mode own-tier count of 6,055 includes 535 that are not workspace routines, and `DeclSurface` local is the same 3.06 MiB in symbols mode). The object map is 0.00 to 0.04 MiB after Task 1.
- Not measured: `routines_by_obj_name`'s key/value split inside the map (the map is dropped as one part), and transient peaks while the index is built (the probe measures retained deltas only).

## Test and gate

`census_parts` is `#[doc(hidden)]`, census-only, production never calls it (it consumes the index). Every field is `Send + 'static`, so `Box<dyn Any + Send>` works as specified. `census_parts_names_every_field` reads the field names from the struct's own source text and compares them with the names `census_parts` returns. Discrimination proof (recorded in the commit message): a dummy field added to the struct and the constructor, with the destructure loosened to `..`, fails the test (`left` lacks `dummy_break`, `right` has it); restored, it passes. With the destructure left exhaustive (as committed), a new field is also a compile error, so the census cannot silently drop a field.
