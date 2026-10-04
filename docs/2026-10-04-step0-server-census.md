# Step 0: the running server's idle memory (2026-10-04)

## Conventions

- **Bytes are heap, requested size**, from the probe's counting `#[global_allocator]`. Allocator overhead (LFH, about 24 B per allocation) is excluded. OS RSS appears only as one labelled context line per run and feeds no number below.
- **MiB** = 1,048,576 bytes.
- "Retained" = live heap after the build returns and the heap has stopped moving for 0.9 s (settle). "Peak" = highest live heap during the build.
- Probe: `tools/census-probe/` at the commit that adds it (this one). Engine: branch `feat/compact-graph-core` at `02a638ba` (path dependency). Release build, thin LTO, `CARGO_TARGET_DIR=C:/lpt`.
- Raw output: `tools/census-probe/runs/{cg,cdo}-{embedded,symbols}-{base,updaters}.txt`.
- "Snapshots only" is what the probe holds without updaters: per-root `LspSnapshot`, the workspace `ParsedUnit` and the `compute_all` diagnostics map, one shared `DepCache`. "Updaters idle" = the same, with the `ParsedUnit` moved into a real `spawn_updater` per root (as `src/server.rs:597-629` does, no-op `on_swap`), each measured once the heap stopped moving (idleness inferred, not observed).
- The updater's `Rung1Context` (`ResolveIndex`, `DeclSurface`, object map) is `pub(crate)`, so the probe cannot split the delta between those three. Only the per-root total delta (updater running vs not) is reported.

## Results (MiB)

| Corpus | Mode | Build peak (root 1) | Snapshots only | Updaters idle | Updaters add |
|---|---|--:|--:|--:|--:|
| CG, 7 roots | embedded | 1,120.7 | 360.5 | **762.7** | 402.2 |
| CG, 7 roots | symbols | 143.6 | 148.3 | **375.5** | 227.2 |
| CDO, 1 root | embedded | 1,324.0 | 382.6 | **448.9** | 66.3 |
| CDO, 1 root | symbols | 200.5 | 155.2 | **200.0** | 44.8 |

Build peak is taken from the run without updaters (`*-base.txt`). Corpora: CG refapp, 7 roots, 36 `.al` files; CDO at `bc3ccb18` (see `tools/census-probe/README.md`).

Sanity: snapshots-only numbers are compared with the "ALL n ROOTS LIVE" live-counter lines of the earlier census raw runs (scratchpad `census-{cg,cdo}-*.txt`, not tracked): CG embedded 360.5 vs 360.4, symbols 148.3 vs 148.2; CDO 382.6 vs 382.6, 155.2 vs 155.1, so within 0.2 MiB. The earlier report publishes walk totals instead (CDO 382.34 and 154.91), which are 0.26 and 0.29 MiB lower. The tracked artifacts for this report's own numbers are `runs/*-base.txt`. In the probe's order (builds first, updaters started afterwards) the build peak does not change with updaters; the updater-start transient and the process peak in server order (updaters starting between builds) are not measured.

Per-root updater share (allocation count about 360k per root in CG embedded):

| Corpus | Mode | Per root | Roots |
|---|---|--:|---|
| CG | embedded | 57.44 to 57.47 | all 7 within 0.03 MiB of each other |
| CG | symbols | 32.45 to 32.47 | all 7 within 0.02 MiB of each other |
| CDO | embedded | 66.35 | 1 |
| CDO | symbols | 44.81 | 1 |

RSS, context only (one reading per run, taken with the updaters idle): CG embedded 896.8 MiB working set (peak 1,191.2); CG symbols 461.1 (465.3); CDO embedded 569.6 (1,401.9); CDO symbols 263.6 (270.1). RSS is not comparable to the heap figures.

## What this says

The idle baseline before any edit of today's server on the 7-root CG set is **762.7 MiB** of heap in `embedded` mode (375.5 in `symbols`), not the 360.5 MiB the snapshots alone hold: the updaters more than double it. Retained heap after edits has not been measured. Each root's updater adds almost the same 57.4 MiB (32.5 in symbols) however small the workspace is (CG roots have 1 to 9 source files, 36 in total), which is consistent with whole-graph indexes dominated by dependency entries (inferred; not split). They are not shared today. The updater indexes for the 6 roots beyond the first account for about 344.7 MiB (6 x 57.45) in `embedded` and 194.8 MiB in `symbols`. A single shared copy would remove most of those, leaving about 418 MiB idle in `embedded` at best (360.5 + 57.5). That is a floor, not an upper bound: every extra root still keeps its own workspace-specific part, which the probe cannot separate; at an estimated ~0.6 MiB per CG root the figure is about 422 MiB. For `symbols`, at least about 181 MiB. The three structures are not split (see Conventions).

## After step 1 (2026-10-04): parse, summarize, drop

Same probe, same corpora, same four modes as above. Engine: branch `feat/compact-graph-core` at `c75ce97a`, probe at `43cdc72e` (the LSP builds with `BuildProfile::LIGHT`: each dependency file is summarized at parse time and its syntax tree is dropped). Probe: `tools/census-probe/` after the commit that makes it follow `DepNodes` (its measurements and accounting are unchanged). Raw output: `tools/census-probe/runs-after/{cg,cdo}-{embedded,symbols}-{base,updaters}.txt`. One run per cell; the step 0 runs repeat within 0.1 MiB between the `base` and `updaters` runs, so that is the noise seen here. Heap bytes from the counting allocator, never RSS.

| Corpus | Mode | Build peak, root 1 (before / after) | Snapshots only (before / after) | Updaters idle (before / after) |
|---|---|--:|--:|--:|
| CG, 7 roots | embedded | 1,120.7 / **317.8** | 360.5 / 361.4 | 762.7 / 763.6 |
| CG, 7 roots | symbols | 143.6 / **130.0** | 148.3 / 148.3 | 375.5 / 375.4 |
| CDO, 1 root | embedded | 1,324.0 / **449.1** | 382.6 / 383.6 | 448.9 / 449.9 |
| CDO, 1 root | symbols | 200.5 / 200.5 | 155.2 / 155.1 | 200.0 / 199.9 |

What the numbers say:

- **The first root's build peak fell by 802.9 MiB (CG embedded) and 874.9 MiB (CDO embedded).** The `2.parse` phase used to grow by 804.0 (CG) and 915.4 MiB (CDO), mostly dependency syntax trees. It now grows by 128.8 and 178.7 MiB, because the dependency summaries and `dep_meta` are built there instead of later (CDO's `parse` phase ends at 326.7 MiB live, was 1,063.4). The phase table is in the raw runs (`*-embedded-base.txt`), not in this report.
- **Where the peak is.** In embedded mode it is still set in phase 7 (`event_edges`), as it was before: 317.8 MiB on CG, 449.1 on CDO. In `symbols` mode the CG peak is in phase 3 (`dep_layer`), before and after.
- **`symbols` mode, CG fell 13.6 MiB (143.6 to 130.0).** This is not the tree drop, because no dependency source is parsed in this mode. The `dep_layer` in-phase peak fell by about the owned heap of the dependency routines (13.6 vs 13.63 MiB). That fits the routine dedup no longer making a temporary copy of every survivor (it moves them now). This cause is inferred from the code plus the numeric match; no probe isolates it. CDO's `dep_layer` peak fell the same way (198.0 to 183.7), but its build peak is set in phase 7, which did not change, so its build peak stayed at 200.5.
- **Retained heap did not fall.** In `embedded` mode it is 0.9 to 1.0 MiB higher (CG 361.4 vs 360.5, CDO 383.6 vs 382.6) and updaters-idle moves with it. The probe places all of it in the shared tier's `routines` (CG 36.04 to 36.97, CDO 38.99 to 39.93 MiB) with the same allocation count, and the routines' string lengths are unchanged, so it is spare buffer capacity, not new data: the dedup now moves routines instead of cloning them, and a clone allocates exactly the length (mechanism inferred from code; the probe counts length, not capacity). No summary is retained: `dep_meta` is byte-identical and `recovered`/`bodies` are empty. Accepted: a shrink pass would add a transient copy of every routine during the build, which is the peak this work targets, to save under 1 MiB per shared tier. `symbols` mode is unchanged. Allocation counts differ by at most 58, the same order as between two runs of one binary.
- Updater share per root is unchanged (CG embedded 402.2 total, symbols 227.2, CDO 66.3 and 44.8).
- Not measured here: `fresh_coverage`'s peak (the probe builds LSP snapshots, not that path) and the process peak in server order.
- RSS context only (updaters idle, one reading): CG embedded 931.3 MiB working set (peak 940.6), CG symbols 460.4 (464.7), CDO embedded 574.3 (580.6), CDO symbols 258.3 (264.7). Heap figures above are not comparable to these.

## After step 2 (2026-10-04): lighter per-root state

Same probe, corpora and four modes. Engine: branch `feat/step2-lighter-root` at `eee47e2e` (plus this task's doc-only edits). Raw output: `tools/census-probe/runs-step2/{cg,cdo}-{embedded,symbols}-{base,updaters}.txt`, plus `cg-embedded-split.txt` (`--index-split`). One run per cell. "Before" is the After-step-1 table above. Heap bytes from the counting allocator, never RSS. The four changes: `dccf397a` (resolver object map holds workspace objects only), `819ea98e` (LSP snapshot keeps only event links with routes), `8ff2c81c` (subscriber maps leave `ResolveIndex`), `eee47e2e` (`routines_by_obj_name` deleted). Between the two runs, `f3e47d83` (step-1 review minors, an engine change that is not part of step 2) also landed. Phases 1 to 4 match within 0.1 MiB, so it moved nothing measured here.

| Corpus | Mode | Build peak, root 1 (before / after) | Snapshots only (before / after) | Updaters idle (before / after) | One updater (before / after) |
|---|---|--:|--:|--:|--:|
| CG, 7 roots | embedded | 317.8 / **287.1** | 361.4 / **275.9** | 763.6 / **310.6** | 57.45 / **4.94** |
| CG, 7 roots | symbols | 130.0 / 130.0 | 148.3 / **65.7** | 375.4 / **100.2** | 32.46 / **4.93** |
| CDO, 1 root | embedded | 449.1 / **395.3** | 383.6 / **371.7** | 449.9 / **380.2** | 66.35 / **8.48** |
| CDO, 1 root | symbols | 200.5 / **183.6** | 155.1 / **142.9** | 199.9 / **151.4** | 44.81 / **8.48** |

(The "before" snapshots-only and idle columns are the `updaters`-run values of After step 1. The "after" snapshots-only values come from the `base` runs, except CG symbols 65.7, which is the `updaters` run's value (its `base` run reads 65.8); the `base` and `updaters` runs agree within 0.1 MiB, e.g. CG embedded 275.9 vs 276.0. The build peak is from the `base` run.)

What the numbers say, and which task the probe can attribute:

- **One idle updater, CG embedded: 57.45 to 4.94 MiB.** The index census (`docs/2026-10-04-step2-index-census.md`) measured 56.81 MiB for the index plus object map plus local `DeclSurface`, of which `routines_by_obj_name` 50.03 and the subscriber maps 1.84. 56.81 - 50.03 - 1.84 = 4.94, which is the new figure. So `eee47e2e` accounts for 50.03 MiB and `8ff2c81c` for 1.84 (both from the census, which was taken before those commits; this run's `--index-split` shows the sum of the remaining fields is 4.93 MiB: `objects_by_name` 2.29, `objects_by_id` 1.81, `objs_by_number` 0.77, the three extension maps and `implementers` 0.07). `dccf397a`'s share is the 0.64 MiB between step 1's 57.45 (`runs-after`) and the census's 56.81. That interval holds `dccf397a`, `819ea98e` and `f3e47d83`; the size fits `dccf397a`'s object map, and the build's phase 5 end-live fell by about the same 0.7 MiB, but no run isolates it. CDO: 66.35 to 8.48. The census values give the same result (embedded 65.74 - 55.39 - 1.87 = 8.48; symbols 44.21 - 35.69 - 0.04 = 8.48), but CDO was not re-split after the change, so its remaining fields are not confirmed.
- **Snapshots only, CG embedded: 361.4 to 275.9 MiB (-85.5; about 12.2 per root).** Attributed to `819ea98e`: `event_edges` per root fell from 24,527 to 2,033 links (the `shape:` line). The probe's drop-delta for `event_edges` falls from 15.52 to 3.32 MiB per root (-12.20), and no other snapshot structure moves; root 1's retained size went 253.2 to 241.1 MiB. The other three changes live in the index, which the snapshots do not hold. CDO fell 11.9 (383.6 to 371.7), CG symbols 82.6 (148.3 to 65.7).
- **Build peak.** The in-phase peaks of phases 5 and 7 fell (CG embedded phase 5 from 307.7 to 241.6, phase 7 from 317.8 to 259.7), so the build peak is now set in phase 3 (`dep_layer`) in all four cells, at 287.1 (CG embedded), 395.3 (CDO embedded), 130.0 (CG symbols) and 183.6 (CDO symbols). Phase 3 is untouched by step 2: CG embedded 287.1 before and after; CG symbols 130.0 both; CDO symbols 183.7 in the After-step-1 text (183.6 now, run-to-run) (its peak was set later in phase 7 at 200.5). Split by the Task 3 run (`runs-index-split/`), which has `dccf397a` and `819ea98e` only: the event-link filter took the CG embedded peak from 317.8 to 307.0 (CDO 449.1 to 437.6), and the peak moved to phase 5. Removing the index map (with the subscriber maps) then took it to 287.1 (CDO 395.3), where the untouched phase 3 caps it. Phase 5's in-phase peak fell a further 45.5 MiB that no longer shows in the peak. The build's index lived from phase 5 to phase 8, so it was not a brief spike. Phase 7's spike above its starting heap grew from 13.4 to 18.3 MiB on CG after `8ff2c81c` (the subscriber maps are now built inside it; inferred, not isolated). On CDO embedded, phase 7 (385.1) is now 10.2 MiB below the peak (395.3).
- **Residual.** Per root, the idle updater is 4.93 to 4.95 MiB on CG embedded (4.92 to 4.93 symbols) and 8.48 on CDO, roughly what is left of the index (all objects, shared and own, since the index is built over the whole graph) plus `DeclSurface`'s local part (0.01 MiB CG, 3.06 CDO per the index census). Roots 2 to 7 each still hold 5.7 to 6.1 MiB of snapshot (retained after settle, `base` run), over a shared tier that is pointer-equal to root 1's; most of that is dependency data, see the next section.
- Not measured here: `fresh_coverage`'s peak, the process peak in server order. RSS was recorded in the raw `updaters` runs but is not reported.

### What is left per root (input to the sharing decision, 2d)

CG corpus, embedded, 7 roots, updaters idle: 310.6 MiB retained heap (symbols: 100.2). Of that, root 1's snapshot is 241.0 MiB (the shared tier included, held once because roots 2 to 7 share it by pointer), roots 2 to 7's snapshots add 35.0 MiB (5.7 to 6.1 each), and the seven updaters add 34.6 MiB (4.93 to 4.95 each); these three are from the `updaters` run and sum to 310.6. Roots 2 to 7 together cost about 64.6 MiB (35.0 plus six updaters at 4.94); root 1's updater is another 4.94, which a one-root server also pays. Most of each extra root's share is dependency-derived. In the snapshot, on Reporting (root 6: 1 workspace file, no declarations, no edges) the dependency-to-dependency event links (3.32 MiB, which the probe reports as identical to root 1's), the incoming map built from them (0.86), the object index (0.85) and the publisher fan-out (0.46) make up 5.49 of the 5.67 MiB that root frees (`cg-embedded-base.txt` drop-delta). In symbols mode, where there are no dependency event links, an extra root's snapshot is 0.8 to 1.4 MiB. In the updater, the object maps (`objects_by_name` 2.29, `objects_by_id` 1.81, `objs_by_number` 0.77 MiB; 4.87 of 4.94) are built over every object in the graph, 9,790 to 9,798 per root of which 9,789 are shared; own objects per root are 1 to 9. Whether these can be built once and shared was not measured: no probe splits them by tier, and the maps' keys and per-root differences were not compared. The CDO workspace (1 root) holds 380.2 MiB idle, of which 8.48 is the updater; sharing does not change a one-root server.
