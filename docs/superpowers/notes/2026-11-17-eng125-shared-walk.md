# ENG-125: sharing the work of impact's per-root reverse walks (design note)

Status: revision 3. §1-§4 are the design reviewed by two models (Fable and gpt-6.1-sol,
2026-11-17). §5 holds what was then built and measured; nothing of it has landed. Measured on
server1, the pinned CDO baseline (`bc3ccb18`), master-equivalent binary 534d636d (ENG-116's
code; its walks visit what master's do), and in §5 against master 7fb70573.

## 1. What is measured

**Redundancy.** The gate's 699-root `impact --diff` reports `coverage.states` in its JSON:
- `perRoot` 81,509,939 walk states in total;
- `union` 191,824 states;
- a factor of **424.9**.

The median root walks 122,660 states, and at least 10 % of the roots walk 1. No root hits the
state budget (`limited` is null for all 699). CDO has no multi-node root and no universal hub,
so fixtures must cover those paths.

**The graph** (`census-probe sited-scc`, new in this task; the sited graph as the session builds
it: 134,344 nodes, 304,305 sites). These are NODE-level SCCs of what the 699 roots reach
backwards, under each tier's site rule (`reverse::tier_over` from a Local state). The design
below works on walk STATES, so the table is suggestive, not a bound.

| sites a path may cross | nodes reached | SCCs | largest SCC | others > 1 |
|---|---:|---:|---:|---:|
| all (the walk with `follow_open`) | 23,066 | 1,823 | 21,244 (92 %) | 0 |
| no open site (tier at most ThroughDependency) | 18,263 | 5,211 | 12,934 (71 %) | 32, at most 26 nodes each |
| local only (tier Local) | 2,524 | 2,517 | 5 | 1 (4 nodes) |

So the 699 cones are almost one set: a giant strongly connected component (dependency code,
closed further by the open hubs), plus each root's local prefix. Each root walks that component
again.

**Where the CPU is.**
- In TLS-86's profile, `walk_multi` is 21.5 % of impact's CPU inclusive. 15.72 % of that is under
  `up`'s per-root closure (#3) and 5.77 % under closure #9, which is not yet identified.
- Self times that any exact per-root BFS keeps, about 9.3 % in all:
  - `walk_multi` 3.98 %;
  - `is_background_target` 1.86 %;
  - `step` 1.73 % (it sets `ext`);
  - the level sort 1.2 %;
  - `of_root_kinds` 0.5 %.
- ENG-118 (d3f975c4, not landed) made the visitor's per-state work cheaper and measured
  x0.98-1.01.

## 2. What each root's answer needs

`up.rs`: `summary`, `walk_root`, `Up`'s `ConeVisitor`.

**A. Reachability only.** These depend on which walk states a root reaches and on each node's
best class over them. They do not depend on distance or arrival:
- ancestor counts by tier, and every `onlyOpen` count;
- `byRelation`, `entriesByKind`, `entriesByContext`, `entries[].count`, `programShare`,
  `userOrJobEntries`, `entryBuckets`, `entryNotes`;
- `external[].count`, `externalCode`, `hidden`, `rootEntryKinds` / `rootEntryWord` /
  `rootUserOrJob`;
- `hubs[].at`, the smallest anchor among the qualifying reached sites;
- `states`.

Every one except `states` counts DISTINCT nodes or entries by their best class. A node has about
8 states, and its states with different tier, `crossed_event` or `ext` sit in different
state-SCCs. So no count adds across SCCs; see §3.

**B. Distance, arrival order or the first arrival's site:**
- `maxHops`;
- every `first` list: `Firsts`' order is (only open, hops, arrival), and the class lists are
  (class, tier rank, hops, arrival);
- `firstByKind` and `nearest`, which pick among entries whose final best class is p, so they also
  need each node's final class;
- `nearestAncestors`, the first `keep.ancestors` with a stable id, in arrival order. Its `hops`
  are those of the best-TIER state, not the shortest;
- `action_site`, the first arrival's action-run site, which feeds the captions;
- `eventPublishers.count`, `.onlyOpen` and `.direct`. `Up::reach` counts a publisher only when
  the state's FIRST arrival is over an event-dispatch site, so these depend on arrival order.
  That may be a latent bug, filed as ENG-128.

**Per-root corrections** any shared form needs:
- A root's own nodes leave its ancestors and entries (`root_nodes`); other roots' nodes still
  count.
- Action-run entries belong to the transition, from the reached SOURCE state, not to the target
  state.

## 3. Options

**Option 1: a thin per-root visitor (the hybrid of revision 1).**
- The per-root BFS stays, with its 81.5 M states and the self costs above. Only the visitor's
  bookkeeping changes.
- So it attacks the per-state constant, NOT the repetition, as ENG-118 did.
- A sound shared form for the A fields would be node-major: per node (and per canonical entry),
  take its states in class order, OR their root bitsets, and add each newly set root bit to that
  root's counter. That is about 16 M increments per count family.
- Fable's simpler exact form: the scratch's dense `seen` word per node already holds each node's
  reached (ext, tier, crossed_event) set for the root. So the A counts can be folded once at the
  walk's end over `touched`, with one extra word for guard states. No SCC code is needed, and the
  shared pass would save nothing beyond that, because the BFS stays.
- But the B fields need each node's final class, so `Classed` and `Firsts` (or a per-(root, node)
  class lookup) stay.
- Realistic saving: about 3-8 % of impact's CPU, on both sides of the 5 % bar. The ceiling is
  measurable cheaply (§4, step 1).

**Option 2: a multi-source, bit-parallel BFS** (Then et al., "The More the Merrier: Efficient
Multi-Source Graph Traversal", VLDB 2014).
- **How:** one traversal of the union state graph per BFS level, carrying each state's set of
  roots as 699 bits (11 words): `seen` and `frontier` bitsets. A root's new states at a level
  are `frontier(parent) & !seen(target)`, per transition.
- **This is the only option that removes the x425.** Each transition is crossed once per level
  for all roots, not once per root.
- **Exactness.** Process each level's states in sorted order and their sites in order, as each
  per-root walk does: the per-root walk sorts its level by (root, state). Then every root
  discovers its states in the same relative order as its own walk. So per-root arrival indices
  (`at`), the first arrival's `via` and site (`eventPublishers`, `action_site`), and the hops
  are all reproducible. `step` must be a pure function of (state, site) that never returns
  `None`; `Up::step` is (state it as a contract; `CodeCalls`'s visitor prunes, so this is not
  generic).
- **Remaining cost:** the visitor would still receive one event per (root, state) pair (81.5 M).
  The win needs the visitor to aggregate per (state, root bitset) instead: node-major class
  folds as in Option 1, and witness picks by scanning bitsets.
- **Memory:** about 16 MiB per bitset family over the union states (191,824 x 88 B), so 2-3
  families plus the state index and edges, about 50-70 MiB. Small against impact's 3.5 GB peak.
  It grows with the root count, and a cap is possible by running roots in batches of 64 x k.
- **The state budget:** a root that exceeds it today stops with `limited`. Its bit must stop
  expanding at the same count, and the count is exact per root. Cancellation is checked as now.

**Option 3: change what B reports** (no `maxHops`, bounded witnesses). This is an output change
with no reason beyond speed. Not recommended (principle 17).

## 4. Recommendation: measure the ceilings before building

1. **Ceiling of any visitor-side change** (Option 1). Run the per-root walks with `Up::reach`
   and `entry` as no-ops (`step` kept), discarding the output, and time it against master.
   That's about an hour. If it saves under 5 %, Option 1 is closed.
2. **Ceiling of the traversal-side change** (Option 2). A prototype multi-source BFS that only
   computes per-root state counts. Its CPU against the per-root walks' traversal-only time (step
   1's run) is the share Option 2 can remove.
3. Only then, a design and plan for the chosen option, and review before code.
   - The current per-root walk stays as the test oracle (fixtures, and CDO through the impact
     ratchets), not as a second production path (principle 16).
   - Every A and B field is byte-identical per root.

Principles used: 1 (exact or nothing), 4 (measure the ceilings first; label estimates), 16 (no
dual path), 17 (no output dropped), 28 (do not repeat ENG-118's experiment), 29-30 (prior art,
review before code).

## 5. What was built and measured (2026-11-17)

**Ceilings (§4 steps 1-2).** Over five clean rounds, the per-root pass (default − skip) was a
mean 63.8 s, 15.6 % of impact's CPU. The visitor alone (default − noop) was noisy, −2 % to
8.2 %. Raw: `/work/.al-sem/profiles/2026-11-17-eng125/e125-proto.log`. The plan for Option 2 is
`docs/superpowers/plans/2026-11-17-eng125-multi-source-walk.md` (rev 3.2) on branch
`tools/eng-125-shared-walk`.

**Built** (branch `tools/eng-125-shared-walk`, not landed):
- `a58c28ae`: `multi_walk` (`src/program/impact_graph/multi_walk.rs`). One bit-parallel walk
  of many logical roots, which gives every root the EXACT callback trace of its own
  `walk_multi`, plus its stats. That includes ENG-128's buffered rejoins and the per-root
  state budget.
  - Tested by comparing traces on two real fixtures, up_tests' worlds and the review-focus
    worlds (a table-event guard among them), with more than 64 roots and budgets at the
    boundaries.
  - Nine discrimination breaks, each red, then green on revert.
- `b40eebd9`, `dde7d51b`: a simpler design than the plan's two passes, to try first. Chunks
  of up to 16 roots, one `multi_walk` per chunk, each event handed to that root's own
  unchanged `Up` visitor (`Demux`). Rows are exact by construction, and every impact golden
  is byte-identical. `dde7d51b` made the state index dense per node.

**Measured** (one-shot `impact --diff`, the gate's 699 roots; outputs sha-identical to master's
in every run). Raw: `/work/.al-sem/profiles/2026-11-17-eng125/chunks/`.

| layout | CPU s | wall s |
|---|---:|---:|
| master 7fb70573 (two clean rounds) | 369.5, 361.6 | 103.0, 98.7 |
| chunks of 16, 8 workers | 365.1, 374.5 | 100.7, 103.0 |
| chunks of 64, 8 workers | 362.0, 358.1 | 101.8, 103.3 |
| all 699 in one walk, 1 worker (census run) | 352.8 | 202.7 |

**The records census** (`chunks/census.log`). A level record is (lane, state, from, via,
roots). Exactness keeps provenance, so roots that reach a state at the same level from
different parents keep separate records.

| layout | records | records ÷ the 81.5 M root-states |
|---|---:|---:|
| all 699 in one walk | 3.63 M (≈ 19 per distinct state) | 0.045 |
| 44 chunks of 16 | 40.2 M (≈ 5 per distinct state per chunk) | 0.494 |

**What it means:**
- Small chunks share little: their roots reach the giant component at different distances. Big
  chunks share a lot, because roots converge and then move in step.
- With the per-root visitors unchanged, the most sharing (one walk) saves about 3 % of CPU. The
  rest is the per-root visitor (`reach` 3.6 %, `step` 2.9 %, `entry` 2.4 % in the chunk-16
  profile `chunks/e125-chunk16.json.gz`): 81.5 M events whatever the walk does.
- So the gain needs the plan's visitor folds (P1/P2: root masks per state instead of one
  callback per root) on top of large chunks. Large chunks make the up pass serial, so wall time
  becomes the risk. That is the plan's Task 2 go/no-go.
- **The lesson:** a prototype that counts states without provenance (the 2.2 s msbfs of §4 step
  2) overstates what an exact multi-source walk saves at small chunk sizes. Measure records per
  layout, not distinct states.
