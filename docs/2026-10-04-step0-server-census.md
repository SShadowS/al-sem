# Step 0: the running server's idle memory (2026-10-04)

## Conventions

- **Bytes are heap, requested size**, from the probe's counting `#[global_allocator]`. Allocator overhead (LFH, about 24 B per allocation) is excluded. OS RSS appears only as one labelled context line per run and feeds no number below.
- **MiB** = 1,048,576 bytes.
- "Retained" = live heap after the build returns and the heap has stopped moving for 0.9 s (settle). "Peak" = highest live heap during the build.
- Probe: `tools/census-probe/` at the commit that adds it (this one). Engine: branch `feat/compact-graph-core` at `02a638ba` (path dependency). Release build, thin LTO, `CARGO_TARGET_DIR=C:/lpt`.
- Raw output: `tools/census-probe/runs/{cg,cdo}-{embedded,symbols}-{base,updaters}.txt`.
- "Snapshots only" is what the probe holds without updaters: per-root `LspSnapshot`, the workspace `ParsedUnit` and the `compute_all` diagnostics map, one shared `DepCache`. "Updaters idle" = the same, with the `ParsedUnit` moved into a real `spawn_updater` per root (as `src/server.rs:597-629` does, no-op `on_swap`), each waited on until idle.
- The updater's `Rung1Context` (`ResolveIndex`, `DeclSurface`, object map) is `pub(crate)`, so the probe cannot split the delta between those three. Only the per-root total delta (updater running vs not) is reported.

## Results (MiB)

| Corpus | Mode | Build peak (root 1) | Snapshots only | Updaters idle | Updaters add |
|---|---|--:|--:|--:|--:|
| CG, 7 roots | embedded | 1,120.7 | 360.5 | **762.7** | 402.2 |
| CG, 7 roots | symbols | 143.6 | 148.3 | **375.5** | 227.2 |
| CDO, 1 root | embedded | 1,324.0 | 382.6 | **448.9** | 66.3 |
| CDO, 1 root | symbols | 200.5 | 155.2 | **200.0** | 44.8 |

Sanity: snapshots-only numbers match the earlier census within 0.2 MiB (CG embedded 360.5 vs 360.4, symbols 148.3 vs 148.2; CDO 382.6 vs 382.6, 155.2 vs 155.1). The build peak does not change with updaters (they start after the builds).

Per-root updater share (allocation count about 360k per root in CG embedded):

| Corpus | Mode | Per root | Roots |
|---|---|--:|---|
| CG | embedded | 57.44 to 57.47 | all 7 within 0.03 MiB of each other |
| CG | symbols | 32.45 to 32.47 | all 7 within 0.02 MiB of each other |
| CDO | embedded | 66.35 | 1 |
| CDO | symbols | 44.81 | 1 |

RSS, context only (one reading per run, taken with the updaters idle): CG embedded 896.8 MiB working set (peak 1,191.2); CG symbols 461.1 (465.3); CDO embedded 569.6 (1,401.9); CDO symbols 263.6 (270.1). RSS is not comparable to the heap figures.

## What this says

The true idle baseline of today's server on the 7-root CG set is **762.7 MiB** of heap in `embedded` mode (375.5 in `symbols`), not the 360.5 MiB the snapshots alone hold: the updaters more than double it. Each root's updater adds almost the same 57.4 MiB (32.5 in symbols) however small the workspace is (CG roots have 5 to 6 source files), so the cost is the whole-graph dependency indexes, held once per root. They are not shared today. The updater indexes for the 6 roots beyond the first account for about 344.7 MiB (6 x 57.45) in `embedded` and 194.8 MiB in `symbols`. A single shared copy would remove those, leaving about 360.5 + 57.5 = 418 MiB idle in `embedded`; this is arithmetic on the per-root delta, an upper bound for one shared copy, because the delta includes a small workspace-specific part the probe cannot separate. The three structures are not split (see Conventions).
