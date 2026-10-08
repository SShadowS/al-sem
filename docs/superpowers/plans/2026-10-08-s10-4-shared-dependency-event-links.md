# S10.4 — Dependency event links shared by every root

Spec: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §8 item 1 (numeric ids),
re-scoped by the owner on 2026-10-08 after re-pricing. Base: `tools/census-probe/runs-s10-3/`.

## Re-priced (CG 7 roots, `embedded`)

- Numeric ids as specced would replace 111,307 `RoutineNodeId` copies outside the canonical
  rows (227,422 copies − 111,637 tier rows − 4,478 workspace rows): 7.6 MiB inline at 72 B each,
  their text already shared since S10.2. (A different basis from the S10 baseline's 22 MiB, which
  counted every copy, canonical rows included, for root 1 alone at the then 96 B; on that
  all-copies basis the 7 roots held 31.04 MiB at S10.2's start and 23.28 after it.)
- Most of those copies sit in each root's event links, which are the same in every root: each
  extra root frees 3.81 MiB of `event_edges`, 0.69 of `incoming` and 0.32 of `publisher_fanout`
  (`runs-s10-3/cg-embedded-base.txt`, the `Fleet` drop deltas), about 4.8 MiB, and the probe
  reports `dep->dep set identical to root 1: Some(true)` for every root.
- **Owner decision:** share the dependency event links first, then re-price numeric ids on what
  is left.

## Design

- **Split** (`split_event_links`): each routed event link goes to the root's
  `ws_event_edges` if its publisher is a workspace routine; else its routes to workspace
  routines form a workspace part and its other routes a dependency part (a link with routes on
  both sides is split in two). The dependency part does not depend on the workspace: dependency
  code cannot see it.
- **Shared** (`DepEventLinks`: the dependency part's edges, `incoming` and fan-out), built once
  per tier into `DepNodes::lsp_events` (a `OnceLock` of its own: unlike `lsp`, it reads no
  dependency text, so S10.1b's deferral is unaffected). Every rung forwards the `Arc`.
- **Readers** go through `LspSnapshot::incoming(id)`, `incoming_count`, `publisher_fanout(id)`,
  `event_edges()` and `edge(r)` (`DEP_EVENT_EDGES_KEY`), which add the two parts. Outgoing reads
  only `ws_event_edges` (a workspace routine's links are never split). `merged_event_edges` and
  `all_incoming` give the report's one-edge-per-publisher shape for comparisons.
- **Answers unchanged:** `incoming` groups by publisher and fan-out sums routes, so a split
  publisher answers as before.
- **Safety net:** in debug builds every rung-2 rebuild, and every root after the first that
  shares a dependency cache, computes its dependency part and asserts it equals the shared one
  (order aside). A first or cache-less root build publishes its own part unchecked: there is
  nothing to compare it with.
- ponytail: a later root still computes the dependency links before dropping them (transient
  CPU and heap); restricting the emission is the upgrade if it shows in a build profile.

## Tasks

- **T1** split, share, readers, tests (one commit). Test
  `dependency_event_links_are_shared_and_split_from_the_workspace_ones`: a dependency publisher
  with a dependency and a workspace subscriber, two roots on one cache; one shared `Arc`, the
  split, fan-out 2, both subscribers see the publisher, answers equal a cache-less build.
- **T2** probe + measure; audit; CHANGELOG; spec status; re-price numeric ids.

T1 landed as `b9ed9782`. Discrimination (each break made with Edit, run, reverted; observed in
the session's test logs, not saved): putting a mixed link wholly into the workspace part fails
the test (`[]` against `["handleindep"]`); giving each root its own copy instead of the shared
one fails it (`the roots share one dependency-link set`). The debug check did not fire in the
task gate (green; log not saved), where it ran on every rung-2 rebuild and every cache-sharing
later root. CDO stats JSON byte-identical (both files, `cmp` against the S10.1b binary's; files
not kept); no golden moved (task gate).

## Result (T2, 2026-10-08; `tools/census-probe/runs-s10-4/` vs `runs-s10-3/`, at `b9ed9782`)

Counted heap, MiB. The probe walks the shared part once as `dep_events.*` and drops it as
`SHARED dep tier: dependency event links`; the per-root labels `event_edges`, `incoming` and
`publisher_fanout` now hold the workspace part only; the event-class line counts the merged
view (one edge per publisher, as before). The shape line's `event_edges N` is now
`event_edges()`, which lists a split publisher once per part, so it is not comparable across the
two runs (CDO 2,380 → 2,404: 24 publishers split).

| Cell (file) | S10.3 | S10.4 |
|---|---:|---:|
| CG roots 2-7 retained after settle (`cg-embedded-base`) | 5.9-6.3 | 0.9-1.6 |
| CG 7 roots live (`cg-embedded-base`) | 144.7 | 115.8 |
| CG 7 roots idle with updaters (`cg-embedded-updaters`) | 174.1 | 145.3 |
| CG root 1 retained / build peak (`cg-embedded-base`) | 108.6 / 269.8 | 108.6 / 269.8 |
| CG live allocations, 7 roots (`cg-embedded-base`) | 595,772 | 523,795 |
| CDO (one root: nothing to share) and `symbols` mode | — | ±0.2 |

Where it comes from (`cg-embedded-base` drop deltas): a root other than the first used to free
3.81 MiB of `event_edges`, 0.69 of `incoming` and 0.32 of `publisher_fanout` (`runs-s10-3`,
`Fleet`); now 0.00 each (`Fleet`: 1.17 MiB in total), and the shared set frees 4.82 MiB once,
with the tier. Root 1 is unchanged: its dependency links moved into the tier at the same size.
The event classes are unchanged (2,230 dependency-only links per root; after the change the
same set in every root by construction, one `Arc`). CG has no mixed publisher
(`dep publisher w/ ws subscriber 0`), so CG never exercises the split; CDO does (81 mixed
publishers, 24 split): its event structures went from 3.97 + 1.54 + 0.32 = 5.83 MiB to
0.13 + 0.84 + 0.01 per root plus 4.92 shared = 5.90 MiB (+0.07, the split parts' duplicated
edge headers), inside the ±0.2 row. RSS (one run, context only): peak working set CG 389.5 → 409.0, CDO 487.3 → 478.2.
Residual after the roots and the cache drop: 0.22 MiB.

**Numeric ids re-priced (the original S10.4).** `RoutineNodeId` copies over the 7 roots fell from
227,422 to 132,982 (all copies 9.13 MiB inline); outside the canonical rows (111,637 tier +
4,478 workspace) 16,867 remain, about 1.2 MiB inline at 72 B each, of which a `u32` would save
about 68/72, roughly 1.1 MiB (by arithmetic). Not worth the rung-2 risk for memory; the
resolver's linear `graph.objects.iter().find` scans (a speed item, spec §8.1) remain open.
