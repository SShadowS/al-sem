# The cone walk's raw half on CDO: measured, not built (TLS-31)

TLS-29 (`docs/superpowers/notes/2026-11-13-parallel-cone-walk.md`) runs the capability-cone walk
in parallel only when no member wants raw facts. `digest`, `impact` and `prove` ask for raw facts
for their roots (`raw = Subjects(roots)`), so their walk stays serial. TLS-31 asked whether a
parallel raw half would pay. It would pay little. The orchestrator stopped the task at the census
(2026-11-13): a 3-6% saving per process is not worth a concurrency change and its review.

## The census

Setup:
- binary: master `2564958b` (TLS-29 landed), release-fast;
- workspace: the pinned CDO baseline;
- one traced run each, with `ALSEM_CONES_CENSUS=1`;
- inputs: the TLS-28 diff (`bc3ccb18~5..bc3ccb18`);
- host: MemAvailable ≥ 35 GiB and load < 10 at the start, under outside load.

| | `impact --diff` | `digest --diff` |
|---|---:|---:|
| wall | 161.0 s | 80.0 s |
| peak RSS | 3,568 MiB | 6,452 MiB |
| `cone.walk` span (one walk; the serial loop) | 7,238 ms (4.5%) | 6,109 ms (7.6%) |
| `context.capability_cones` (with `pd.solve`) | 8,913 ms | 7,552 ms |
| SCC times summed | 7,218 ms | 6,088 ms |
| critical path (the bound for any scheduler) | 1,488 ms | 1,410 ms |
| slowest SCC (704 members) | 1,209 ms | 1,168 ms |
| raw BFS member walks | 30, 821 ms | 30, 812 ms |
| singleton raw scan + emission | 158 + 147 ms | 136 + 122 ms |

Each command composes cones once. The raw half itself is small: about 1.1 s in impact, about
1.07 s in digest.

## The finding: the raw walks sit in one SCC

All 30 raw BFS member walks fall in the 704-member recursive SCC, the roots' cycle in CDO's
call graph. That one SCC therefore takes 1.2 s in these commands, against 122 ms in `analyze`.
It sets the critical path at 1.41-1.49 s, against 0.38 s for analyze.

A parallel walk like TLS-29's would take the walk from about 7.2 s to about 1.7 s at 8 workers.
- That saves about 5.5 s per `impact` (−3.4%) and about 4.5 s per `digest` (−5.6%).
- cdo-gate's impact phase (344 s in TLS-29's landing gate, five `impact` processes) might drop by
  10-20 s.
- Running the big SCC's member walks on several workers would save about another second.

## What would make it worth revisiting

- **A workspace where the raw half dominates:** many raw subjects, as when `prove` or `digest`
  get hundreds of roots, or roots spread over many SCCs. The census line `cone_scc:` …
  `member_walks=` and `walk_total=` shows it.
- **A walk that becomes a large share of the wall:** for example after `digest`'s batches
  (70 s of impact) get faster. The walk's 7 s would then weigh more.
- **The resident engine's walk going parallel** (the TLS-22 rule: `cdo-speed` passes, memory
  within 1.10). Its raw emission would need the same task-local raw sink.

## How it would be built (for the record)

- **Reuse TLS-29's driver:** the ready queue, freeing at the last reader, and merging in SCC
  order.
- **Raw tail:** the raw half ends in one short tail (`emit_picks`, and the singleton scan's tail:
  intern the subject, intern each pick's rep and first hop, sort). With few raw subjects, tasks
  can compute their picks unlocked and intern them into the one `InheritedBuilder` under a short
  lock.
- **Why the output would not change:** each subject's refs are sorted by content (`cmp_joined`
  over the rep's and hop's fields), and a subject is emitted by one task. So a different table
  order changes no output.
- **Also needed:** split the big SCC's member walks across workers, then the guards and the
  review.
