# Capability-cone memory: compact inherited facts (design)

Status: **draft for review** (measure + spec only; no engine code changed).
Branch: `perf/snapshot-cone-memory`, from master `7ce7811c`.
Date: 2026-10-31.

## 0. Summary

`compose_snapshot` over the cross-app CDO model peaks at **45.3 GB RSS** (37.6 GiB of live
heap). Almost all of it is one thing stored twice: every routine's **inherited capability
facts** (the effects a routine gets from the code it calls). There are **17.0 million** of
them. The cone stores each one as a full, owned copy of the callee's fact (713 bytes on
average), and the snapshot then makes a second, larger owned copy (883 bytes), plus a sort
key string and a flatten copy. Everything that is not an inherited fact costs 3.0 GiB live
(4.85 GB RSS).

Each inherited fact is really just **"callee fact X, reached through first-hop edge Y"**.
There are only 25,048 distinct callee facts and 38,081 distinct first hops. The recommended
fix stores an inherited fact as a pair of references (16 bytes) instead of a copy, in the cone
(step 1) and in the snapshot's fact table (step 2), and builds the full fact only when a
reader asks for it. Target: **≤ 6 GB RSS** for the cross-app CDO snapshot (from 45.3 GB),
and **≤ 3.5 GB** for `alsem policy check` cross-app on CDO (from 9.49 GB). Every output stays
byte-identical.

Step 1 also fixes a **live** cost: `alsem policy check` (cross-app by default) holds the same
owned copies for its 5,479 primary routines (7.0 GB of its 9.49 GB peak).

## 1. Terms

- **Direct fact**: an effect a routine's own body has (a table write, an HTTP send, an event
  publish). Type `CapabilityFact` (`src/engine/l4/capability_cone.rs:118`), 416 bytes plus
  its strings.
- **Inherited fact**: a direct fact of some routine the subject reaches through calls,
  re-labelled for the subject. `retag` (`capability_cone.rs:1782`) builds it: a deep clone of
  the callee's fact with a new `subject`, `provenance = "inherited"`, and four fields taken
  from the **first-hop edge** (the subject's own out-edge the shortest path starts with):
  `via`, `witness_callsite_id`, `witness_first_hop_operation_id`, `witness_first_hop_event_id`.
- **Raw**: the per-routine `Vec<CapabilityFact>` of inherited facts the cone emits
  (`ConeOutcome::raw`, `R3a3SourceBase::raw`). Which routines get raw is chosen by
  `WitnessScope` (`src/engine/l4/cone_derived.rs:243`): `None`, `All`, or `Subjects(..)`.
- **Cut model / full model**: the cross-app model as production builds it
  (`build_cross_app_base_from`, `capability_cone.rs:4065`) moves the dependency routines' own
  calls out of `model.calls`, so a snapshot over it sees only workspace callers (the "cut").
  `program_calls::assemble_and_resolve_cross_app_program` returns the model **before** that
  split, so its `calls` include every call dependency code makes (the "full" graph). The
  45 GB peak needs the full model.

## 2. The measured problem

### 2.1 How it was measured

A throwaway probe (`cone-probe`, a separate crate in the session scratchpad, depending on
this worktree; not committed). It installs a **counting global allocator** (live heap and
peak, by requested size, as `tools/census-probe` does) and reads `VmRSS`/`VmHWM`. It builds
the model with the same call the i264 census test uses, then runs
`build_r3a3_source_only_base` and `compose_snapshot_from_base` as separate phases (together
they are exactly `compose_snapshot`), and frees the parts one by one to read their sizes.
Built with the `release-fast` settings (thin LTO, 4 codegen units). Linux, 22 cores, 93 GB.
Temporary marks inside `derive_capability_facts` (reverted) split the snapshot phase.

Commands (workspace `CDO_WS = /home/sshadows/Projects/DO-cdo-baseline/Cloud`, `bc3ccb18`):

```
/usr/bin/time -v cone-probe single $CDO_WS --stats
/usr/bin/time -v cone-probe cross  $CDO_WS --stats
TMP_CONE_NO_RAW=1 /usr/bin/time -v cone-probe cross $CDO_WS   # temporary env switch: cone raw scope None
/usr/bin/time -v target-cone-mem/release-fast/alsem analyze --preset transaction-integrity --format json $CDO_WS
/usr/bin/time -v target-cone-mem/release-fast/alsem policy check --format json --out p.json $CDO_WS
ALSEM_RETAINED_CENSUS=1 alsem policy check ...                  # for witness.raw_bytes
```

### 2.2 Baseline: whole runs

| Run | Peak RSS | Wall |
|---|--:|--:|
| probe, single-app (model + `compose_snapshot`) | 0.93 GB | 9.0 s |
| probe, cross-app full model (model + `compose_snapshot`) | **46.4 GB** (45.3 GB at the end of the snapshot) | 137 s with `--stats`, 104 s without |
| probe, cross-app full model, cone raw scope `None` (no inherited facts at all) | 4.85 GB | 34 s |
| `alsem analyze --preset transaction-integrity`, cross-app (production: the cut) | 5.18 GB | 66 s |
| `alsem policy check`, cross-app (production) | **9.49 GB** | 41 s |

The production analyze run confirms the 45 GB case is latent: the ordering cell (d47/d49/d51)
does call `compose_snapshot` on the cross-app model, but on the cut, where it is cheap.

### 2.3 Breakdown: cross-app full model (live heap, counting allocator)

| Phase / part | Live heap | Time |
|---|--:|--:|
| Model (`assemble_and_resolve_cross_app_program`, 33,287 routines, 88,716 calls) | 0.86 GiB retained, 2.75 GiB phase peak | 19 s |
| `build_r3a3_source_only_base` (graph + direct facts + cone) | +12.4 GiB retained | 43 s |
| — of which **raw inherited facts** | **11.3 GiB** (17,017,608 facts, **713 B/fact**) | |
| — clone of `ws_routines` / pd solution / graph / calls / direct + coverage / rest | 0.51 / 0.32 / 0.08 / 0.07 / 0.09 / 0.02 GiB | |
| — same base with raw scope `None` | 1.10 GiB retained; peak only 0.25 GiB above it | **7.4 s** |
| `compose_snapshot_from_base` | +14.9 GiB retained, **37.6 GiB live peak** | 32–41 s |
| — `snapshot.capability_facts` | **14.0 GiB** (17,057,037 facts, **883 B/fact**) | |
| — every other snapshot field | 0.86 GiB | ~5 s |

Inside `derive_capability_facts` (`src/engine/l5/snapshot.rs:1812`), live heap at the marks:

| Mark | Live | Peak since last mark |
|---|--:|--:|
| start | 13.3 GiB | |
| per-routine `Vec`s built (parallel) | 31.5 GiB | 31.6 GiB |
| flattened into one `Vec` | 27.3 GiB | **36.7 GiB** (the per-routine Vecs and the growing flat Vec are both alive) |
| sorted (`sort_by_cached_key`) | 27.3 GiB | 33.0 GiB (one key `String` per fact: +5.7 GiB) |

So at the peak the heap holds: model 0.9 + base without raw 1.1 + **raw 11.3** + **snapshot
facts in per-routine Vecs ~18 (with growth slack)** + **flat Vec being filled ~5**. The peak is
in the intermediate forms, not in a serialized form: nothing is serialized. The cone's own
working set is small: with raw off, the base phase peaks only 0.25 GiB above what it keeps.

RSS is 1.2× the live heap here (45.3 GB against 37.6 GiB): glibc keeps freed pages.

### 2.4 The facts themselves

From `cone-probe cross --stats`:

- **Per routine**: 33,285 routines; 17,536 (53 %) inherit nothing; p50 0, p90 68, **p99 9,402,
  p99.9 9,675, max 10,027**; mean 511. The graph has a giant core: a routine that reaches it
  inherits about 9,400 facts.
- **Sharing**: 25,048 distinct callee facts (the fact without subject, provenance and the four
  first-hop fields) and 38,081 distinct first hops (subject, via, call site, op, event). Every
  one of the 17.0 M facts is a distinct (callee fact, first hop) pair, so a pair of references
  loses nothing.
- **Strings**: the top-level strings alone are 264 B/fact (4.2 GiB); the rest is the 416-byte
  struct and the strings inside `extra` and `resource_arg_source`.
- **Kinds**: **78 % are event `publish` facts** (13.3 M); then table read 1.55 M, insert
  0.87 M, delete 0.52 M, modify 0.47 M, lock 0.18 M; everything else is under 0.01 M each.
- **Roots**: the 5,479 reportable roots (primary routines with a parsed body, the routines
  digest, prove, ordering and policy read) hold 9.78 M facts, **57.4 %**. Scoping alone saves
  at most 43 %.
- **Serialized**: `snapshot.capability_facts` as compact JSON is **8.8 GiB** (544 B/fact).
- **Single-app** for scale: 162,651 inherited facts, raw 114 MiB (737 B/fact), snapshot facts
  160 MiB (1,000 B/fact).

### 2.5 The same representation in a live command

`alsem policy check` cross-app on CDO composes raw for every primary routine
(`WitnessRequest::primary`, `src/engine/session.rs:166`). The census prints
`witness.members=5479 witness.raw_bytes=6966269866` (7.0 GB, a lower bound) against a 9.49 GB
process peak. That is the 9.78 M root facts above in the same 713-byte form. The batch spec
measured the same bucket at 6.04 GB on 2026-10-25; it has grown with the dependency code the
model now demands.

## 3. Who reads inherited facts, and which fields

This bounds what a fix may change.

| Reader | Reads | Subjects read | Fields |
|---|---|---|---|
| `compose_snapshot_from_base` / `compose_tree` (`snapshot.rs:1327`, `:1522`) | raw → `SnapshotCapabilityFact` rows; raw's `witness_operation_id` / `witness_callsite_id` for the evidence indexes | all (cut) / the view's subjects (tree) | every field |
| `snapshot_full::derive_permissions` (`snapshot_full.rs:998`) | base raw | all | table facts' fields |
| `policy_engine` (`gate/policy/policy_engine.rs:362`) | `View::raw()`, as `&CapabilityFact` in `FieldEvalContext` | primary routines | any field a rule names |
| `project_r3a3`, `project_r3a5_*`, `FullRoutineSummary::capability_facts_inherited` | cone raw | all | every field (goldens) |
| `capability_query` (`l5/capability_query.rs:69`) | `inherited_raw()` | tests only (raw is `None` in production) | |

Readers of the snapshot's `capability_facts` (`Vec<SnapshotCapabilityFact>`):

| Reader | Uses |
|---|---|
| digest / prove / ordering (`digest.rs:279` `build_fingerprint_indexes`) | per-subject lists, direct facts by (op, kind), and the global **index** of a fact (`idx.capability_facts[i]`) |
| `ordering_kept_roots` (`digest.rs:4825`) | `effect_type_of(f)` and `subject` of every fact |
| `derive_pd_arena` (`snapshot.rs:2168`) | index of each direct fact in the sorted table (the pd arena stores it) |
| `FingerprintIndexesPub` (`digest.rs:5343`) | owned clones per subject |
| fingerprint B0, `diff`, the inventory (`snapshot_full.rs:247`, `key_classes.rs:784`) | CBOR of every fact; `retain` by subject class |
| `fingerprint_query`, `test_coverage::CoverageGraph::from_snapshot` | iterate and filter |
| `project_r4f_snapshot` (r4f goldens) | JSON of every fact, in table order |

Consequences:

1. The **table order** (`capability_fact_sort_key`, a `"|"`-joined string, stable sort) and
   the **index of a fact** are part of the contract: the pd arena and digest store indexes.
2. Every field of a fact is read by someone, so no field can be dropped. A fix may change how
   a fact is **stored**, not what it **says**.
3. Every reader of inherited facts reads one subject at a time, except the serializers and the
   two whole-table scans (`ordering_kept_roots`, `test_coverage`), which only need a borrowed
   view.
4. Serialized whole-snapshot outputs (B0, `diff`, r4f) run single-app only today
   (`SnapshotUniverse::SingleApp` in `fingerprint_cli.rs:740` and `gate/diff/cli.rs:250`). Over
   the cross-app full graph they would be 8.8 GiB of JSON, so no design makes those outputs
   cheap there; this spec does not try.

## 4. Candidate designs

Estimates use §2's numbers. "Floor" = everything except inherited facts: 3.0 GiB live,
4.85 GB RSS, 34 s (the raw-scope-`None` run).

### A. Scope, drain and a cheaper sort (no type change)

- Give `compose_snapshot` raw only for non-dependency rows (`WitnessScope::Subjects`), as the
  view already does for trees.
- Drain raw into the snapshot (`TreeRaw::Drain` exists), so a routine's raw Vec is freed once
  its rows are emitted.
- Build `capability_facts` into one pre-sized Vec (no per-routine Vecs, no flatten copy), and
  sort with an allocation-free comparator instead of one key `String` per fact.

Estimate: the dependency rows' 7.2 M facts are gone; raw (6.6 GiB for roots) and snapshot
facts (8.2 GiB) no longer fully coexist, and the flatten and key transients go. Live peak
about **10–12 GiB, ~13–15 GB RSS**. Time: maybe −30 %.
Cost: output changes for any reader of a dependency row's inherited facts. On the cut there
are none (dependency rows have no out-edges there), and single-app has no dependency rows, so
no golden moves; but the full-model snapshot no longer equals "every routine". Does nothing for
`policy check` (already scoped). It treats the symptom: each fact is still copied twice.

### B. Compact raw: an inherited fact is a pair of references (step 1 of the recommendation)

The cone already holds each callee fact once, as `Arc<CapabilityFact>` (`ConeFactEntry::rep`,
`capability_cone.rs:1587`), and each first hop once, as a `TypedOutEdge` in the graph. `retag`
copies both into a new owned fact. Instead, emit:

```rust
/// One inherited fact: the callee's fact, reached through the subject's first hop.
pub struct InheritedFact {
    rep: Arc<CapabilityFact>,   // shared with the cone and every other subject
    hop: HopIx,                 // u32 into the composition's hop table
}
/// Per composition: the first hops, each stored once.
pub struct HopTable(Vec<Hop>);  // Hop { via: &'static str, callsite, operation_id, event_id: Option<Arc<str>> }
```

`raw` becomes `HashMap<String, Vec<InheritedFact>>` plus the `HopTable`, and `retag` becomes
`InheritedFact::materialize(&self, subject, &HopTable) -> CapabilityFact`, the **one** place
that builds the owned form (it keeps `retag`'s exact field rules, #96 D5 / #98 D1). A reader
that needs `&CapabilityFact` (policy, permissions, the projections) materializes one subject
at a time; a reader that only needs a field reads it through the pair (`rep.op`,
`hop.callsite`). The emission order (`sort_inherited`) is computed from the pair with the same
key, so each Vec's order is unchanged.

Estimate: 17.0 M × 16 B = **0.26 GiB** for raw (from 11.3 GiB). The snapshot side is unchanged,
so the full-model `compose_snapshot` peak drops to about 26 GiB live (~31 GB RSS): not enough
alone. `policy check` cross-app: the 7.0 GB raw becomes ~0.15 GB, so **~9.5 GB → ~2.7 GB RSS**.
Cone time: the 17 M deep clones go (43 s base vs 7.4 s with no raw; the walk itself remains,
its share of the 36 s is not measured).

### C. B plus a compact snapshot fact table (step 2 of the recommendation)

Replace `CapabilitySnapshot::capability_facts: Vec<SnapshotCapabilityFact>` with a table that
stores inherited rows as references:

```rust
pub struct FactTable {
    rows: Vec<Row>,                       // in the final sorted order; index = today's index
    direct: Vec<SnapshotCapabilityFact>,  // ~40 k, owned as today
    reps: Vec<SnapRep>,                   // projected callee facts, one per distinct rep
    hops: Vec<SnapHop>,                   // projected first hops
    subjects: Vec<Arc<str>>,              // stable subject ids
}
enum Row { Direct(u32), Inherited { subject: u32, rep: u32, hop: u32 } } // 16 B
impl FactTable {
    pub fn len(&self) -> usize;
    pub fn get(&self, i: usize) -> FactView<'_>;      // borrowed; same fields as today, as &str
    pub fn iter(&self) -> impl Iterator<Item = FactView<'_>>;
    pub fn retain(&mut self, keep: impl FnMut(FactView<'_>) -> bool);  // key classes
    pub fn from_owned(v: Vec<SnapshotCapabilityFact>) -> Self;        // tests that hand-build facts
}
```

`FactView` carries today's `Serialize` impl (key order unchanged) and `is_known_temp`; CBOR
conversion reads views. Readers switch from `&SnapshotCapabilityFact` to `FactView<'_>` (digest's
`type Fact`), `FingerprintIndexesPub` calls `to_owned()` per subject. The sort is a **stable**
`sort_by` on rows with a comparator that compares the `"|"`-joined key **byte by byte without
building it** (see §6.3: comparing the fields as a tuple gives a different order).
`derive_capability_facts` writes rows straight into one pre-sized Vec (A's flatten fix).

Estimate: rows 17.06 M × 16 B = 0.26 GiB; reps and hops tables ~0.05 GiB; sort buffer
(`sort_by` is a merge sort, n/2 scratch) 0.13 GiB. Live peak ≈ floor 3.0 + raw 0.26 + table
0.3 + sort 0.13 ≈ **3.7 GiB, about 4.5–5.5 GB RSS**. Snapshot derive: today 32–41 s, of which
~5 s is the rest of the snapshot; the 17 M owned-fact builds and key strings go. The comparator
sort of 17 M rows is the unknown (string compares, mostly settled on the subject prefix);
estimate **under 15 s**, to be measured.

### D. Lazy inherited facts from kept cones (rejected)

Keep every SCC's fact cone after the walk and compute a routine's inherited facts on demand.
Rejected: a cone holds the whole closure below it (the cones of the giant core each hold ~9 k
entries), so keeping all of them is closure-sized again (≥ 17 M entries at ~100 B with
`BTreeMap` overhead: ≥ 1.6 GiB, more than B's 0.26 GiB); every read re-runs the singleton/BFS
pick and the guard substitution; and the snapshot still needs a global sorted order and fact
indexes (§3), which force full materialization of the order anyway. It also breaks the
refcount freeing the walk relies on (`compose_inherited_cones`' `remaining_uses`).

### Comparison

| | Full-model snapshot, live / RSS | `policy check` cross-app RSS | Output | Code touched |
|---|--:|--:|---|---|
| today | 37.6 GiB / 45.3 GB | 9.49 GB | | |
| A | ~11 GiB / ~14 GB | 9.49 GB | full-model snapshot drops dependency rows' facts | `snapshot.rs` only |
| B | ~26 GiB / ~31 GB | **~2.7 GB** | identical | cone + its raw readers |
| **C (= B + table)** | **~3.7 GiB / ≤ 6 GB** | **~2.7 GB** | identical | B + every `capability_facts` reader |
| D | ≥ 5 GiB, slower reads | ~3 GB | identical | cone, snapshot, every reader |

## 5. Recommendation

Do **C, in two steps**, each merged on its own:

1. **Step 1 (B): compact raw.** `InheritedFact` + `HopTable` in the cone; `materialize` is the
   only builder of an owned inherited fact; policy, permissions, the projections and
   `FullRoutineSummary` materialize per subject. Pays off at once in production (`policy
   check` cross-app, and every view with a witness request: prove, digest, batch).
2. **Step 2: `FactTable`.** The snapshot keeps inherited rows as references; readers take
   `FactView`; one pre-sized row Vec; allocation-free join-order comparator.

Not chosen: A (it removes facts instead of their copies, and keeps a 14 GB peak), D (§4).

Keep `compose_callsite_resolutions` (ad80fa9c): it is cheaper still for a reader of call-site
rows alone, and its equality test stays a guard.

**Targets** (release-fast, the probe and the commands of §2.1, pinned CDO baseline):

| Run | Today | Target |
|---|--:|--:|
| cross-app full-model `compose_snapshot` (probe) | 45.3 GB RSS / 37.6 GiB live | **≤ 6 GB RSS, ≤ 4.5 GiB live** |
| `alsem policy check` cross-app | 9.49 GB | **≤ 3.5 GB** |
| single-app `compose_snapshot` (probe) | 0.93 GB | no regression (≤ 0.95 GB) |
| `alsem analyze --preset transaction-integrity` cross-app | 5.18 GB, 66 s | no regression |
| full-model `compose_snapshot` wall (probe, no `--stats`) | 104 s | ≤ 60 s (goal; the comparator sort is the unmeasured part) |

### Declared-needs fit

Neither step adds or skips a fact, so no `BuildProfile`, `ViewNeeds`, substrate bit or `Plan`
field changes (declared-needs rule 0: this changes how a fact is stored, not whether it is
built). `WitnessScope` keeps deciding **which** subjects get raw; this changes only the raw
form. The census buckets that walk raw (`witness.raw_bytes`, `cone_census::facts_tally`,
`census_snapshot`'s `snapshot.capability_facts`) are rewritten to count the new form, so their
numbers drop; that is expected and is recorded with the change.

## 6. What must stay byte-identical, and how it is proven

### 6.1 Outputs

Everything. No golden is regenerated in either step. That covers all 24 golden directories
through `scripts/check-goldens` (no `--regen`), in particular r3a3/r3a4/r3a5 (raw facts
projected), r4/r4f (snapshot JSON and digest), `l4-summary-baseline`, every `tests/cli-*`
family (fingerprint, diff, digest, prove, policy, query, explain), `gate-goldens`, and
`tests/goldens/semantic-edges`. A golden that moves is a bug in the change.

### 6.2 Proof on CDO (before/after, same pinned baseline)

- `alsem policy check --deterministic --format json` cross-app and `--single-app`: `cmp`.
- `alsem analyze --preset transaction-integrity --deterministic --format json`, both modes: `cmp`.
- `alsem digest` and `alsem prove` on a fixed root list, cross-app: `cmp`.
- The full-model snapshot itself: the probe streams `compose_snapshot`'s
  `capability_facts` (and the whole `project_r4f_snapshot`-shaped document) through a BLAKE3
  hasher instead of storing it (8.8 GiB of JSON); the hash before and after must match. Done
  once per step, recorded in the change's CHANGELOG entry with the numbers.
- `scripts/cdo-gate` green.

### 6.3 The sort-order trap

`capability_fact_sort_key` (`snapshot.rs:1795`) joins eight fields with `"|"` and compares the
strings. That is **not** the same as comparing the fields as a tuple: with resource ids `"a"`
and `"ab"`, the tuple puts `"a"` first, but the joined keys compare `"a|…"` with `"ab|…"` and
`'|'` (0x7C) sorts after `'b'` (0x62), so `"ab"` comes first. The comparator must walk the
joined bytes (fields chained with `"|"`), and the sort must be stable (`sort_by_cached_key` is
stable: ties keep emission order). The same holds for `inherited_output_sort_key`
(`capability_cone.rs:1800`) in step 1.

## 7. Test plan

Unit and lib tests, each with a recorded discrimination proof (break with Edit, watch it fail,
revert, watch it pass):

- **T1 `materialize` equals `retag`** (step 1). Hand-stated rep and three first hops: a call
  edge, an `implicit-trigger` edge (op id) and an `event-dispatch` edge (event id), plus a rep
  that itself carries a stale first-hop op id. Assert `materialize` gives exactly the old
  `retag` output, field by field (keep the old `retag` as a `#[cfg(test)]` oracle).
  Break: drop `witness_first_hop_event_id` in `materialize` → fails; break 2: let the rep's
  stale op id through (`..rep` without the override) → fails.
- **T2 the joined-key comparator** (step 1 for `inherited_output_sort_key`, step 2 for
  `capability_fact_sort_key`). Hand-stated facts whose tuple order and joined order differ
  (§6.3), plus two facts with equal keys in a known emission order. Break 1: compare as a
  tuple → fails. Break 2: `sort_unstable_by` → the tie case fails (assert the tie pair is
  adjacent and in emission order, so the break cannot pass by luck).
- **T3 whole-composition equality** (both steps). On every `tests/r0-corpus` and
  `tests/fixtures` app, single-app and cross-app **full** model (the unsplit
  `assemble_and_resolve_cross_app_program`, so dependency callers are in), compare the new
  path with the oracle: step 1 compares each subject's materialized raw with the old owned
  raw; step 2 compares the serialized `capability_facts` and the pd arena's fact indexes with
  the old `Vec<SnapshotCapabilityFact>` path. Precondition asserts, in the style of
  `callsite_resolutions_alone_equal_the_snapshot_rows`: > 100 models, some inherited facts,
  some guarded (param-guard) entries, some trigger and event first hops. Break: swap two hop
  indexes when emitting → fails on N models (record N).
- **T4 policy reads the same facts** (step 1). A policy rule with `facts: inherited` over a
  fixture whose match depends on a first-hop field; break: materialize without the hop → the
  finding vanishes → fails.
- **T5 memory guard** (both steps). A standalone test crate (its own `tests/*.rs`, so it can
  install a counting `#[global_allocator]` without touching other tests) builds a synthetic
  corpus: N routines that all reach one shared set of publishers and table writers, so the
  inherited facts grow as N × set size. It asserts the live-heap growth of the composition per
  inherited fact stays ≤ 32 B. Break: emit owned facts again → fails. The CDO numbers
  themselves are checked by the probe run of §6.2, not by a test.

Gates per step: `scripts/ci-steps task` per commit; `scripts/ci-steps all`,
`scripts/check-goldens` (no regen) and `scripts/cdo-gate $CDO_WS` before merge; §6.2's CDO
comparisons and §5's targets, measured on release-fast and recorded.

## 8. Surprising findings

1. **The 45 GB case is not the production cross-app model.** Production splits the calls
   (`build_cross_app_base_from`) before any snapshot; the full graph appears only through
   `assemble_and_resolve_cross_app_program` (tests, the i264 census). The ordering cell does
   run `compose_snapshot` cross-app in production, but on the cut (5.18 GB whole run).
2. **The same representation is a live 7 GB cost** in `alsem policy check` cross-app.
3. **78 % of inherited facts are event `publish` facts** (13.3 M of 17.0 M). Whether every
   routine should inherit every downstream publish is a semantics question this spec does not
   change; it is why the closure is so large.
4. **The snapshot's copy is bigger than the cone's** (883 vs 713 B/fact), and both are alive
   together; the flatten step briefly holds a third partial copy (+5.4 GiB), and the sort keys
   add 5.7 GiB.
5. **The cone walk itself is cheap in memory**: with no raw output the base phase keeps
   1.10 GiB and peaks only 0.25 GiB above it, and takes 7.4 s instead of 43 s.
6. Half the routines inherit nothing and 1 % inherit ~9,400 or more: a giant reachable core.
7. The full-model step-0 measurement (2026-10-18, `docs/measurements/2026-10-18-issue-160-step0.md`)
   counted 14.8 M inherited facts and 31 GB; it is 17.0 M and 46 GB now, as the cross-app model
   holds more dependency code.

## 9. Open questions for the owner

1. **Step 2 scope.** Step 1 alone fixes `policy check` and halves the latent peak (to ~31 GB);
   step 2 changes the type every `capability_facts` reader uses (digest, fingerprint, diff,
   CBOR, key classes, test coverage, many tests). Recommended: both. Is step 2 wanted now, or
   only when a full-graph snapshot gets a production reader (the #161 follow-up that moves the
   ordering cell to the cone graph)?
2. **Should `compose_snapshot` accept the unsplit cross-app model at all?** Either support it
   (this spec makes it affordable) or make it refuse a model whose calls include dependency
   callers (a debug assertion, or a type for the cut model). Both can be done.
3. **Keep the probe?** Commit it as `tools/cone-probe` (like `tools/census-probe`) or as an
   `aldump` mode, so the targets in §5 can be re-measured; it was a scratchpad crate here.
4. **Event publish inheritance** (finding 3): out of scope here; file separately if wanted.
