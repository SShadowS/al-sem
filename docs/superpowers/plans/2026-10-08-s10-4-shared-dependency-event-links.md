# S10.4 — Dependency event links shared by every root

Spec: `docs/superpowers/specs/2026-10-04-compact-graph-core-design.md` §8 item 1 (numeric ids),
re-scoped by the owner on 2026-10-08 after re-pricing. Base: `tools/census-probe/runs-s10-3/`.

## Re-priced (CG 7 roots, `embedded`)

- Numeric ids as specced would replace 111,307 `RoutineNodeId` copies outside the canonical
  rows (227,422 copies − 111,637 tier rows − 4,478 workspace rows): 7.6 MiB inline at 72 B each,
  their text already shared since S10.2. The S10 baseline had priced the item at 22 MiB for
  root 1 alone.
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
- **Safety net:** in debug builds every root and every rung-2 rebuild computes its dependency
  part and asserts it equals the shared one (order aside), so every test run checks the "same in
  every root" claim.
- ponytail: a later root still computes the dependency links before dropping them (transient
  CPU and heap); restricting the emission is the upgrade if it shows in a build profile.

## Tasks

- **T1** split, share, readers, tests (one commit). Test
  `dependency_event_links_are_shared_and_split_from_the_workspace_ones`: a dependency publisher
  with a dependency and a workspace subscriber, two roots on one cache; one shared `Arc`, the
  split, fan-out 2, both subscribers see the publisher, answers equal a cache-less build.
- **T2** probe + measure; audit; CHANGELOG; spec status; re-price numeric ids.
