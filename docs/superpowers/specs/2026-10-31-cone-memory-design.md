# Capability-cone memory: compact inherited facts (design)

Status: **revision 2, draft for review** (measure + spec only; no engine code changed).
Branch: `perf/snapshot-cone-memory`, from master `7ce7811c`.
Dates: revision 1 2026-10-31 (`aa33ed62`); revision 2 2026-10-31.

## Review history

| Round | Who | Outcome |
|---|---|---|
| Revision 1 | spec author | Candidates A–D measured; recommended C (a 16-byte `(Arc, HopIx)` pair in the cone, then a `FactTable` + borrowed `FactView` in the snapshot). |
| Fable r1 | Claude (Fable) | Independent design: 8-byte `(u32, u32)` refs into shared tables, borrowed reader views, a subject-split sort, an owned snapshot handle. |
| astra round 1 | GPT (astra) | Approve with changes; eight findings (row size, circular oracle, recursive selection described wrongly, sort tests, estimates, kind needs, missed readers, policy test cases). All accepted. |
| Fable r2 | Claude | 16-byte snapshot handle; frozen old-emission oracle; first-seen BFS stated; hop interning by content; DEFER on kind needs; readers and empties added. |
| astra round 2 | GPT | Approve with changes; seven findings (subject-construction contract, frozen `candidate_wins`, hop mutation, retained-rep measurement, step-0 label, `literal_reach` and mutation-style tests, pd arena in the CDO proof). All accepted. Step 0 may start. |
| Fable r3 = this revision | Claude | Subject contract, retained-rep census (27,477 entries, 18 MB), oracle audit, CDO pd-arena comparison. |

## 0. Summary

`compose_snapshot` over the cross-app CDO model peaks at **46.4 GB RSS** (37.6 GiB of
live heap). Almost all of it is one thing stored twice: every routine's **inherited
capability facts** (the effects a routine gets from the code it calls). There are
**17.0 million** of them. The cone stores each as a full owned copy of the callee's fact
(713 bytes on average), and the snapshot then makes a second, larger owned copy
(883 bytes), plus a sort-key string and a flatten copy. Everything that is not an
inherited fact costs 3.0 GiB live (4.85 GB RSS).

Each inherited fact is "callee fact X, reached through my first-hop edge Y". There are
only 27,477 distinct callee facts that win anywhere and 38,081 distinct first hops. This
revision stores an inherited fact once, as **two `u32` indexes (8 bytes)** into two
small shared tables, and lets every reader see the full fact through a borrowed view
whose accessors apply `retag`'s rules. The snapshot keeps `Vec<SnapshotCapabilityFact>`,
but a row becomes a **16-byte handle** (one `Arc` to the snapshot's tables, two indexes)
with accessor methods. The snapshot sort is split by subject (exact when no emitted
subject contains `|`, which is checked; a global exact sort otherwise), sorted per
subject in parallel inside one pre-sized vector. Every output stays byte-identical; no
golden is regenerated.

Targets (kept until measured): cross-app full-model `compose_snapshot` **≤ 6 GB RSS**
(from 46.4), cross-app `alsem policy check` **≤ 3.5 GB** (from 9.49), single-app no
regression. Step 1 also fixes a **live** cost: `policy check` holds the same owned
copies for its 5,479 primary routines (7.0 GB of its 9.49 GB peak).

## 1. Terms

- **Direct fact**: an effect a routine's own body has (a table write, an HTTP send, an
  event publish). Type `CapabilityFact` (`src/engine/l4/capability_cone.rs:118`),
  416 bytes plus its strings.
- **Inherited fact**: a direct fact of some routine the subject reaches through calls,
  re-labelled for the subject. `retag` (`capability_cone.rs:1782`) builds it: a deep
  clone of the callee's fact with a new `subject`, `provenance = "inherited"`, and four
  fields taken from the **first-hop edge** (the subject's own out-edge the path starts
  with): `via`, `witness_callsite_id`, `witness_first_hop_operation_id`,
  `witness_first_hop_event_id`. `witness_operation_id` and every other field stay the
  callee's.
- **Rep (representative)**: the callee's direct fact an inherited fact is built from.
- **Raw**: the per-routine inherited facts the cone emits (`ConeOutcome::raw`,
  `R3a3SourceBase::raw`). Which routines get raw is chosen by `WitnessScope`
  (`src/engine/l4/cone_derived.rs:243`): `None`, `All`, or `Subjects(..)`.
- **Cut model / full model**: the cross-app model as production builds it
  (`build_cross_app_base_from`, `capability_cone.rs:4065`) moves the dependency routines'
  own calls out of `model.calls`, so a snapshot over it sees only workspace callers (the
  "cut"). `program_calls::assemble_and_resolve_cross_app_program` returns the model
  **before** that split (the "full" graph). The 46 GB peak needs the full model.

## 2. The measured problem

### 2.1 How it was measured

A throwaway probe (`cone-probe`, a separate crate in the session scratchpad, depending
on this worktree; not committed). It installs a **counting global allocator** (live heap
and peak, by requested size, as `tools/census-probe` does) and reads `VmRSS`/`VmHWM`.
It builds the model with the same call the i264 census test uses, then runs
`build_r3a3_source_only_base` and `compose_snapshot_from_base` as separate phases
(together they are exactly `compose_snapshot`), and frees the parts one by one to read
their sizes. Built with the `release-fast` settings (thin LTO, 4 codegen units). Linux,
22 cores, 93 GB. Temporary marks inside `derive_capability_facts` (reverted) split the
snapshot phase. Two further temporary censuses (revision 2, reverted) counted the
distinct representatives and first hops the raw emission makes.

Commands (workspace `CDO_WS = /home/sshadows/Projects/DO-cdo-baseline/Cloud`, `bc3ccb18`):

```
/usr/bin/time -v cone-probe single $CDO_WS --stats
/usr/bin/time -v cone-probe cross  $CDO_WS --stats
TMP_CONE_NO_RAW=1 /usr/bin/time -v cone-probe cross $CDO_WS   # temporary env switch: cone raw scope None
TMP_CONE_PTR_CENSUS=1 cone-probe cross $CDO_WS                 # temporary census: rep and hop identities
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

The production analyze run confirms the 45 GB case is latent: the ordering cell
(d47/d49/d51) does call `compose_snapshot` on the cross-app model, but on the cut, where
it is cheap.

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

So at the peak the heap holds: model 0.9 + base without raw 1.1 + **raw 11.3** +
**snapshot facts in per-routine Vecs ~18 (with growth slack)** + **flat Vec being
filled ~5**. The cone's own working set is small: with raw off, the base phase peaks
only 0.25 GiB above what it keeps. RSS is 1.2× the live heap here (45.3 GB against
37.6 GiB): glibc keeps freed pages.

### 2.4 The facts themselves

From `cone-probe cross --stats`:

- **Per routine**: 33,285 routines; 17,536 (53 %) inherit nothing; p50 0, p90 68,
  **p99 9,402, p99.9 9,675, max 10,027**; mean 511. The graph has a giant core: a routine
  that reaches it inherits about 9,400 facts.
- **Sharing**: 25,048 distinct callee facts (the fact without subject, provenance and
  the four first-hop fields) and 38,081 distinct first hops (subject, via, call site,
  op, event). Every one of the 17.0 M facts is a distinct (callee fact, first hop) pair.
- **Strings**: the top-level strings alone are 264 B/fact (4.2 GiB); the rest is the
  416-byte struct and the strings inside `extra` and `resource_arg_source`.
- **Kinds**: **78 % are event `publish` facts** (13.3 M); then table read 1.55 M, insert
  0.87 M, delete 0.52 M, modify 0.47 M, lock 0.18 M; everything else under 0.01 M each.
- **Roots**: the 5,479 reportable roots hold 9.78 M facts, **57.4 %**.
- **Serialized**: `snapshot.capability_facts` as compact JSON is **8.8 GiB** (544 B/fact).
- **Single-app** for scale: 162,651 inherited facts, raw 114 MiB (737 B/fact), snapshot
  facts 160 MiB (1,000 B/fact).
- Direct facts: 39,429. Snapshot rows: 17,057,037.

### 2.5 The same representation in a live command

`alsem policy check` cross-app on CDO composes raw for every primary routine
(`WitnessRequest::primary`, `src/engine/session.rs:166`). The census prints
`witness.members=5479 witness.raw_bytes=6966269866` (7.0 GB, a lower bound) against a
9.49 GB process peak. That is the 9.78 M root facts above in the same 713-byte form.

### 2.6 Layout sizes (revision 2; `rustc -O`, x86-64)

| Type | Bytes |
|---|--:|
| `Arc<str>` | 16 |
| `Arc<String>`, `Arc<T>`, `Option<Arc<T>>` | 8 |
| `String`, `Option<String>` | 24 |
| a row of three `Arc`s with an `Arc<str>` subject | 32 |
| **a row `{ Arc<SnapTables>, u32, u32 }`** | **16** |
| `InheritedRef { u32, u32 }` | 8 |
| `CapabilityFact` | 416 |
| today's `SnapshotCapabilityFact` | 456 |

### 2.7 First-hop identities (revision 2)

Over the 17,017,608 emitted facts: 38,846 first hops distinct by edge address (the same
with the subject added, since each edge is owned by one caller's outgoing vector,
`capability_cone.rs:1359-1372`) against **38,081** by content (subject, via, call site,
op id, event id). The 765 extra edges differ only in their target, which `retag` never
reads. So hops are interned by content, never by address.

### 2.8 Retained representatives (revision 2)

A census at the two emission sites recorded every winning rep as (address, hash of its
full content including its own subject), hashing on every record so a reused address
with new content counts as a new identity, and classified a rep as a **producer
variant** when its address was seen in a producer-solve (`own`) loop.

| Count | Value |
|---|--:|
| retained identities (address, content) | 27,868 |
| direct reps by address (today the owned `direct` copy and the cone's `Arc` copy count apart) | 25,450 |
| **direct reps by content** (what the design keeps once `dedup_direct` mints the one `Arc`) | **25,059**, 16.2 MB with strings |
| **producer variants** `(producer, TempState)` that win somewhere | **2,418**, 1.8 MB |
| distinct addresses / address reuses observed | 22,466 / 5,402 |
| producer-solve `Arc`s seen (winning or not) | 4,069 |

The design's rep table holds **27,477 entries, about 18 MB**. Address reuse is real,
so the table must hold a strong `Arc` per entry (§5).

## 3. Who reads inherited facts, and which fields

### 3.1 How an inherited fact is chosen today

`compose_inherited_cones` (`capability_cone.rs:2707`) walks SCCs callee-first. A
non-root SCC's fact cone is a `BTreeMap<Arc<str>, ConeFactEntry>` keyed by
`inherited_fact_key` (`op|resourceKind|resourceId|confidence|kt/nt`), each entry holding
`rep: Arc<CapabilityFact>`, `dist` and an optional guard. Cones are refcount-freed once
their last predecessor has run.

**Singleton (non-recursive) subject** (`inherited_facts_for_singleton`, `:1867`): every
entry of each out-edge target's cone (plus guard-substituted entries) is a candidate at
`dist + 1`; per key the winner is `candidate_wins` (`:1844`): smaller distance, then
smaller `edge.sort_key`. The subject's own producer-solve entries
(`pd_solve::ResolvedEntry`) compete under the same rule. Winners emit in key order.

**Recursive subject** (`inherited_facts_by_bfs`, `:2173`): a breadth-first walk from the
subject. A sibling member contributes its deduped direct facts; a downstream routine
contributes its SCC's whole cone and is not expanded. For both, a key is kept at its
**first encounter** (`:2214-2217`, `:2258-2261`); later candidates are not compared on
distance. Only the subject's own producer-solve entries compete afterwards with
`candidate_wins`, **replacing the pick in place** (`:2266-2280`); emission order is pick
order. This design preserves that algorithm exactly; only what a pick is stored as
changes.

Carriers: `Best { rep: &CapabilityFact, dist, edge: &TypedOutEdge }` (`:1881`) and
`picked: Vec<(&CapabilityFact, usize, &TypedOutEdge)>` (`:2184`). Rep owners: cone
entries (`Arc`, minted by `fact_cone_for_scc` as `Arc::new(f.clone())` from the owned
`direct` map), the owned `direct` map itself (BFS sibling path), and
`pd_solve::ResolvedEntry.rep` (one `Arc` per `(producer, TempState)`,
`pd_solve.rs:402-406`; `TempState` is `Known(bool) | ParameterDependent(u32) | Unknown`).
First hops are `&TypedOutEdge` into `g.outgoing` for all three paths (`pd_solve::solve`
borrows the graph's edges and clones them into `arena.edges` only afterwards, `:417`).

`raw_out.insert(m, inherited)` runs for every member in scope, **empty Vec included**
(`:3066-3068`); `SubjectRaw::new` intersects the scope with the map's keys
(`snapshot.rs:1473-1486`); policy panics on a missing entry for a routine with a summary
and treats an empty one as "no inherited facts" (`policy_engine.rs:362-374`). "Absent"
and "empty" are different values and must stay so.

### 3.2 Readers of raw inherited facts

| Reader | Reads | Subjects |
|---|---|---|
| `derive_capability_facts` (`snapshot.rs:1812`) | every field; the row's subject is the **emitting routine's** stable id (`:1840-1855`), never the rep's | all emitted routines |
| `derive_operation_evidence` / `derive_callsite_evidence` (`:2298`, `:2425`) | `witness_operation_id` / `witness_callsite_id`; the callsite index also takes every typed-edge call site (`:2452-2456`) | all |
| `snapshot_full::derive_permissions` (`snapshot_full.rs:998`) | table/execute facts: op, kind, resource id, temp class; `make_required_perm`: op, call-site witness, both first-hop ids | all |
| `policy_engine` (`policy_engine.rs:362`) | `select_facts` → sort by `op|kind|rid|wop|conf|prov|via|wcs` → `FieldEvalContext { fact }`; rule fields op, resourceKind, resource.table.name, resource.event.name, resource.http.method, resource.ui.kind, confidence, origin, via; `emit_match_finding` copies both witnesses into evidence (`:676-677`) | primary routines |
| `project_r3a3` / `project_r3a5_*` (`WitnessScope::All`) | every field → `PCapabilityFact` goldens | fixtures |
| `FullRoutineSummary::with_inherited` | `cone_census.rs:260`, `l5/test_support.rs:252` only | census + tests |
| census `witness.raw_bytes`, `cone_census::facts_tally` | byte counts | diagnostic |

### 3.3 Readers of the snapshot table `capability_facts`

| Reader | Uses |
|---|---|
| `digest::build_fingerprint_indexes` (`digest.rs:279`) | `facts_by_routine: HashMap<String, Vec<&Fact>>` in table order; `direct_facts_by_op_kind`; `type Fact` in ~30 signatures |
| `idx.capability_facts[st.producer_fact]` (`digest.rs:1051`, `:2063`) | index of a **direct** fact |
| `derive_pd_arena` (`snapshot.rs:2101`) | first index per (subject, op) among `provenance == "direct"` rows |
| `ordering_kept_roots` (`digest.rs:4825`) | `effect_type_of`, `subject`, whole table |
| `ordering_engine.rs:205-218` | whole-table scan: `http_method_by_callsite_id` (last-wins, so order matters), `background_kickoff_callsite_ids` |
| `literal_reach.rs:420-439` | borrows `subject`, `resource_id`, witness ids as `&str` from direct rows for the slice's lifetime |
| `fact_first_hop` (`digest.rs:414`), `effect_fact_loop_identity` (`:3137`), `fact_equivalent` (`:858`) | first-hop ids, `resource_arg_source`, `extra`, every field |
| `FingerprintIndexesPub` (`digest.rs:5343`) | owned clones per subject |
| `capability_facts_to_cbor`, inventory, `diff` (`snapshot_full.rs:247`, `:344`) | every field, table order |
| `snapshot_full/key_classes.rs:784` | `core.capability_facts.retain(|f| is_primary(&f.subject))` |
| `fingerprint_query`, `test_coverage::CoverageGraph::from_snapshot` | iterate and filter |
| r4f goldens (`project_r4f_snapshot`, `snapshot.rs:3152-3175`) | JSON of every row in table order; custom `Serialize`, dynamic key order; the document does **not** include `pd_solution` or `boundary` |
| ~60 test sites | rows as struct literals; some clone a row and edit its fields (`digest_rule_b_tests.rs:320-323`, `snapshot.rs:3572`) |

Consequences:

1. The table **order** (`capability_fact_sort_key`, a `|`-joined string of
   `subject|op|resourceKind|resourceId|confidence|provenance|witnessCallsiteId|witnessOperationId`,
   stable over emission order: direct facts in `direct_full` order, then raw in cone
   order; one emission per internal id at its first row, `snapshot.rs:1828-1835`) and the
   **index of each direct fact** (the pd arena's `producer_fact`) are part of the contract.
2. Every field of a fact is read by someone, so no field can be dropped. A fix may
   change how a fact is **stored**, not what it **says**.
3. A snapshot subject is `stable_routine_id(&r.id, map)` (`snapshot.rs:1065-1069`): the
   stable id `<guid>:<Type>:<key>#<sha256hex>` (`<key>` a number or `@`+`[a-z0-9%]`), **or
   the internal id passed through when unmapped**. Production maps every routine, so a
   production subject never contains `|`; a hand-built or unmapped one might.
4. The sort trap: with resource ids `a` and `ab`, the tuple puts `a` first, but the joined
   keys compare `"a|…"` with `"ab|…"` and `'|'` (0x7C) sorts after `'b'` (0x62), so `ab`
   comes first. Fields are never compared as a tuple.

## 4. Design

### 4.1 The cone's output: `InheritedFacts`

```rust
pub type RepIx = u32;  pub type HopIx = u32;  pub type SubjIx = u32;
pub const NO_HOP: HopIx = u32::MAX;      // reserved: marks a direct snapshot row; never a table index

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct InheritedRef { pub rep: RepIx, pub hop: HopIx }   // 8 bytes

/// What `retag` reads from a first-hop edge, plus the emitting subject. One row per
/// distinct (subject, via, callsite, operation_id, event_id); `Option`s stay distinct
/// (`None` is not `Some("")`).
pub struct FirstHop {
    pub subject: SubjIx,
    pub via: &'static str,
    pub callsite: Option<String>,
    pub operation_id: Option<String>,
    pub event_id: Option<String>,
}

pub struct InheritedTables {
    pub subjects: Vec<String>,            // internal routine ids of the emitting members, interned
    pub reps: Vec<Arc<CapabilityFact>>,   // interned by Arc identity; a strong Arc per entry
    pub hops: Vec<FirstHop>,
}

/// Per-subject inherited facts, in `sort_inherited` order. One entry per member the
/// scope's `raw` contained, empty or not. Owned keys; the tables are shared.
pub struct InheritedFacts {
    tables: Arc<InheritedTables>,
    by_subject: HashMap<String, Vec<InheritedRef>>,
}
```

`ConeOutcome::raw`, `R3a3SourceBase::raw`, `View::raw()`, `TreeRaw::{Drain, Borrow}`,
`SubjectRaw` and `PolicyModel::raw` carry `InheritedFacts` (or a borrowed filter over
it). `get(id)` returns `None` only for a subject the composition never walked, and
`Some(empty)` for a composed subject with nothing, exactly as the map does today.

**Subject contract.** A ref stored under key `m` has `hops[hop].subject == m` by
construction (the hop was interned while emitting `m`); debug builds assert it when a
slice is read. A view's `subject()` borrows from the tables (the same string), never
from the caller's lookup key. A drain (`take_subjects`) moves whole
`(String, Vec<InheritedRef>)` entries into a new `InheritedFacts` that clones the
`Arc<InheritedTables>`, so drained vectors are always paired with their owned subject
and a table owner until the tree has projected them. Indexes come from
`u32::try_from(len).expect(..)` and additionally reject `u32::MAX`.

Sizes on CDO cross-app: 17.0 M × 8 B = **136 MB** for the refs (from 11.3 GiB); the
tables ~25 MB (§2.7–2.8). Policy's 9.78 M root facts: 78 MB (from 7.0 GB).

### 4.2 How readers see a fact

```rust
#[derive(Clone, Copy)]
pub struct InheritedFactRef<'a> { rep: &'a CapabilityFact, hop: &'a FirstHop, tables: &'a InheritedTables }
impl<'a> InheritedFactRef<'a> {
    pub fn subject(&self) -> &'a str;                 // tables.subjects[hop.subject]
    pub fn op(&self) -> &'static str;                 // rep
    pub fn resource_kind(&self) -> &'static str;      // rep
    pub fn resource_id(&self) -> Option<&'a str>;     // rep
    pub fn resource_arg_source(&self) -> Option<&'a ValueSource>;   // rep
    pub fn confidence(&self) -> &'static str;         // rep
    pub fn provenance(&self) -> &'static str { "inherited" }
    pub fn via(&self) -> &'static str;                // hop
    pub fn witness_operation_id(&self) -> Option<&'a str>;          // rep (retag keeps it)
    pub fn witness_callsite_id(&self) -> Option<&'a str>;           // hop
    pub fn witness_first_hop_operation_id(&self) -> Option<&'a str>;  // hop
    pub fn witness_first_hop_event_id(&self) -> Option<&'a str>;      // hop
    pub fn extra(&self) -> Option<&'a CapabilityExtra>;             // rep
    pub fn rep(&self) -> &'a CapabilityFact;
    pub fn materialize(&self) -> CapabilityFact;      // exactly retag(rep, subject, edge): the ONE builder
}
/// Direct or inherited, same accessor names; policy and `derive_permissions` take it.
pub enum FactRef<'a> { Direct(&'a CapabilityFact), Inherited(InheritedFactRef<'a>) }

impl InheritedFacts {
    pub fn get(&self, id: &str) -> Option<InheritedSlice<'_>>;
    pub fn contains_key(&self, id: &str) -> bool;  pub fn len(&self) -> usize;  pub fn keys(&self) -> impl Iterator<Item = &str>;
    pub fn take_subjects(&mut self, ids: impl Iterator<Item = &str>) -> InheritedFacts;
    pub fn tables(&self) -> &Arc<InheritedTables>;
}
impl<'a> InheritedSlice<'a> {
    pub fn len(&self) -> usize;  pub fn is_empty(&self) -> bool;
    pub fn iter(&self) -> impl ExactSizeIterator<Item = InheritedFactRef<'a>>;
    pub fn materialize(&self) -> Vec<CapabilityFact>;   // the projections and tests only
}
```

`materialize` is today's `retag` body on the view; the accessors restate the same six
rules; test T1 pins that the two agree, with the old `retag` kept as a `#[cfg(test)]`
oracle. Policy's `FieldEvalContext.fact` becomes `Option<FactRef<'a>>`; the predicate
field reads and `emit_match_finding` go through accessors; nothing in policy
materializes (its loop is rule-outer, so per-subject materialization would clone up to
9.78 M facts per rule).

### 4.3 The snapshot row: a 16-byte handle, subjects from the emitting routine

```rust
pub struct SnapTables {
    pub subjects: Vec<String>,   // stable ids of the emitted routines (one per emission group)
    pub reps: Vec<SnapRep>,      // inherited reps projected ONCE per distinct RepIx, then one per direct fact
    pub hops: Vec<SnapHop>,      // { subject: u32 (into subjects), via, callsite, operation_id, event_id }
}
pub struct SnapRep {
    subject: u32,                // set for a direct fact's rep (the emitting routine); unused on an inherited rep
    op, resource_kind, resource_id, resource_arg_source, confidence, provenance, via,
    witness_operation_id, witness_callsite_id, witness_first_hop_operation_id, witness_first_hop_event_id, extra,
}
/// A row of `CapabilitySnapshot::capability_facts`. 16 bytes.
#[derive(Clone)]
pub struct SnapshotCapabilityFact { tables: Arc<SnapTables>, rep: u32, hop: u32 }   // hop == NO_HOP → direct row
impl SnapshotCapabilityFact {
    pub fn subject(&self) -> &str;     // the hop row's subject, else the direct rep's subject
    pub fn op(&self) -> &str;  pub fn resource_kind(&self) -> &str;  pub fn resource_id(&self) -> Option<&str>;
    pub fn resource_arg_source(&self) -> Option<&SnapValueSource>;  pub fn confidence(&self) -> &str;
    pub fn provenance(&self) -> &str;  // "inherited" when hop != NO_HOP, else rep.provenance
    pub fn via(&self) -> &str;         // hop.via, else rep.via
    pub fn witness_operation_id(&self) -> Option<&str>;             // rep
    pub fn witness_callsite_id(&self) -> Option<&str>;              // hop.callsite, else rep
    pub fn witness_first_hop_operation_id(&self) -> Option<&str>;   // hop.operation_id, else rep
    pub fn witness_first_hop_event_id(&self) -> Option<&str>;       // hop.event_id, else rep
    pub fn extra(&self) -> Option<&SnapCapabilityExtra>;
    pub fn is_known_temp(&self) -> bool;     // the same rule, through extra()
    pub fn fields(&self) -> SnapshotFactFields;                     // owned, today's 13-field layout
}
impl From<SnapshotFactFields> for SnapshotCapabilityFact { .. }   // a one-row private table; tests and hand-built rows
impl Serialize for SnapshotCapabilityFact { .. }   // today's code on accessors; key order unchanged
```

**Construction** (`derive_capability_facts`): while walking the emission groups (one per
first row of an internal id, in `ws_routines` order), the group's stable subject
`stable_routine_id(&r.id)` is interned into `SnapTables::subjects`; each direct fact of
the routine becomes a `SnapRep` carrying that subject index and a row with `NO_HOP`;
each `InheritedRef` of the routine maps its `RepIx` to a `SnapRep` (projected once per
distinct `RepIx`, memoised) and its `HopIx` to a `SnapHop` (projected once per raw hop,
memoised), where `SnapHop.subject` is **the group's** subject index; on first projection
of a hop, debug builds assert `routine_to_stable[tables.subjects[raw_hop.subject]] ==
group subject`, and a later group reusing the same raw hop is a bug (asserted). No row
takes its subject from a rep, and the raw tables alone are never the source of a
snapshot subject.

`SnapshotFactFields` is today's struct under a new name. `type Fact` in digest stays
`SnapshotCapabilityFact`; `Vec<&Fact>`, indexing, `clone()` (one refcount bump),
`retain` and the whole-table scans keep working in table order. Mutation-style fixtures
call `fields()`, edit the plain struct, and convert back with `.into()`; shared tables
are never mutated. `literal_reach` borrows its strings from the accessors of rows in the
borrowed slice, which the `Arc` keeps alive.

Why a handle and not a `FactTable` + `FactView<'_>` (revision 1's step 2): the
lifetime-carrying view threads through ~30 digest signatures, the pd arena,
fingerprint, test coverage, CBOR and every test that builds a snapshot; the handle
changes field reads to method calls and nothing else, at the same 16 B per row.

Size on CDO cross-app: 17.06 M × 16 B = **273 MB** (from 14.0 GiB), plus `SnapRep`s
(27,477 inherited reps + 39,429 direct facts ≈ 67 k rows ≈ 40 MB with strings),
`SnapHop`s (38,081 × ~150 B ≈ 6 MB), subjects (33 k).

### 4.4 The sort, preserved exactly

**One comparator for every `|`-joined key.**
`engine::joined_key::cmp_joined(a: [&str; N], b: [&str; N]) -> Ordering` compares two
field lists as if each were joined by `|`, byte by byte, without allocating (a cursor
over field bytes that yields a virtual `|` between fields). Equivalent to
`a.join("|").cmp(&b.join("|"))` for every input, fields containing `|` included. Used by
the snapshot sort, `sort_inherited` and policy's `fact_sort_key`; all three stay stable.

**The snapshot sort, split by subject.** When no emitted subject contains `|`,
`subject + "|"` is prefix-free across distinct subjects, so comparing keys equals
comparing `(subject + "|", rest)` lexicographically. Then: (1) each emission group is
one unit; groups whose subject strings are equal (two internal ids with one stable id)
are concatenated in emission order; (2) sort the units by `subject + "|"` (33 k `String`
compares); (3) allocate ONE `Vec<SnapshotCapabilityFact>` of the exact total and fill
each unit's slice in parallel (no per-routine Vecs survive, no flatten); (4) stable-sort
each slice by `cmp_joined` over the seven remaining fields through the accessors; slices
are ≤ 10,027 rows and sort in parallel. Fallback: the emitted subject strings are
scanned first; if any contains `|`, one global stable sort with `cmp_joined` over all
eight fields runs instead.

**The cone sort and the policy sort.** `sort_inherited` stably sorts `Vec<InheritedRef>`
by `cmp_joined` over `(op, resource_kind, resource_id, confidence, via,
witness_callsite_id, witness_operation_id)` read from `(reps[r], hops[h])`, in pick
order. Policy's `sort_by_cached_key(fact_sort_key)` becomes a stable `sort_by(cmp_joined)`
over `FactRef` accessors in its eight-field order.

## 5. Where inheritance is computed and stored

Same place; the selection algorithm and its inputs are untouched; only the winning
output is compacted:

- `dedup_direct` returns `BTreeMap<String, Arc<CapabilityFact>>`; `fact_cone_for_scc`
  reuses that `Arc` instead of `Arc::new(f.clone())`. The BFS sibling path and the cone
  entries then share one allocation per direct rep.
- Carriers hold the owner: `Best { rep: &'a Arc<CapabilityFact>, .. }`,
  `picked: Vec<(&'g Arc<CapabilityFact>, usize, &'g TypedOutEdge)>`;
  `ResolvedEntry.rep` is already an `Arc`. The interner clones the `Arc` into
  `InheritedTables::reps` the first time its address is seen, so every recorded address
  is held and cannot be reused while interned (§2.8 shows reuse is real otherwise).
- Reps intern by **`Arc` identity** (`HashMap<*const CapabilityFact, RepIx>`), the same
  pattern `pd_solve::solve` uses for edges. Hops intern by **content**
  `(subject, via, callsite, operation_id, event_id)` — exactly what `retag` reads, not
  the edge's `to` or `kind`; `Option` values stay distinct. Winner selection keeps the
  original `&TypedOutEdge`s (`sort_key` tie-breaks included).
- `inherited_facts_for_singleton` and `emit_picks` emit `Vec<InheritedRef>` and sort as
  in §4.4; `inherited_facts_by_bfs` keeps its control flow (first-seen keys, in-place
  producer replacement). No `CapabilityFact` is built in the walk.
- `ConeOutcome::raw: InheritedFacts`; `WitnessScope` still decides which subjects get
  raw; the derived rows (`ConeDerivedStore`) are untouched.

Nothing is computed lazily. Keeping every SCC's cone and answering per read (revision 1's
candidate D) stays rejected: the cones are closure-sized, it breaks the walk's refcount
freeing, the snapshot needs a global order and direct-fact indexes anyway, and at 8 B
per fact there is nothing left to save.

**Is per-routine storage right at all?** The subject-specific part of an inherited fact
is the first hop and the per-key pick; the callee fact is shared. That is exactly one
`(rep, hop)` pair per key, the 8-byte row. A per-SCC form cannot hold the first hop (it
differs per member); the derived rows already are the per-routine summary detectors
read. Keep the explicit per-routine list, compact.

## 6. Fact-kind needs: deferred

Should consumers declare which inherited fact **kinds** they need (78 % are event
`publish` facts)?

| Reader | Kinds it acts on |
|---|---|
| digest / prove / fingerprint over a shared tree | every mapped kind, **including `publish` → `EVENT_PUBLISH`** (`digest.rs:67`) |
| policy rules | any kind a rule names; `capability.resource.event.name` is a rule field (`predicate_fields.rs:275`) |
| the ordering cell (`digest.rs:4754-4762`) | a **private** snapshot on the cut; `ORDERING_RELEVANT` (`:3921-3932`) has no `EVENT_PUBLISH`; `OrderingIndex` reads HTTP and background facts (`ordering_engine.rs:205-218`) |
| evidence indexes | every fact's witness ids; the callsite index also takes typed-edge call sites (`snapshot.rs:2452-2456`) |
| serializers (r4f, B0, diff, inventory, r3a3/r3a5) | every kind, every field |
| `derive_permissions` | table + execute only |

Verdict: **no kind declaration in this change; a kind-scoped build is deferred.**
Declared-needs rule 0 says measure before you gate: after this change an inherited fact
costs 8 B in raw and 16 B in the table, measured first. The shared trees need every
kind, so a declaration cannot narrow them. The ordering cell is the one named candidate:
it builds its own snapshot on the cut and reads some kinds only; a kind-scoped build for
it needs its own dependency audit (HTTP/background facts feed `OrderingIndex`,
`ordering_kept_roots`, the evidence indexes `digest_one_root` reads for witnesses) and
must count snapshot rows and the per-subject indexes, not only 8-byte refs. It is filed
as a follow-up to be measured after step 2 lands, on the production cut. Kind selection
at read time stays nearly free on the store (a per-rep kind byte; a filter over 8-byte
refs is one lookup each), so a reader that wants one kind can ask the store without a
build-level declaration.

Declared-needs fit: neither step adds or skips a fact, so no `BuildProfile`,
`ViewNeeds`, substrate bit or `Plan` field changes (rule 0: this changes how a fact is
stored, not whether it is built). `WitnessScope` keeps deciding **which** subjects get
raw. The census buckets that walk raw (`witness.raw_bytes`, `cone_census::facts_tally`,
`census_snapshot`'s `snapshot.capability_facts`) are rewritten to count the new form;
their numbers drop, and that is recorded with the change.

## 7. Targets and expectations

Targets (commitments, measured with the probe and the commands of §2.1 on release-fast,
pinned CDO baseline):

| Run | Today | Target |
|---|--:|--:|
| cross-app full-model `compose_snapshot` (probe) | 46.4 GB RSS / 37.6 GiB live | **≤ 6 GB RSS, ≤ 4.5 GiB live** |
| `alsem policy check` cross-app | 9.49 GB | **≤ 3.5 GB** |
| single-app `compose_snapshot` (probe) | 0.93 GB | no regression (≤ 0.95 GB) |
| `alsem analyze --preset transaction-integrity` cross-app | 5.18 GB, 66 s | no regression |
| full-model `compose_snapshot` wall (probe, no `--stats`) | 104 s | ≤ 60 s (goal) |

Expectations (not commitments), with their arithmetic:

- Full-model snapshot live heap ≈ floor 3.0 GiB + refs 0.14 + rows 0.27 + `SnapRep`s
  ~0.04 + hops/subjects ~0.01 + `InheritedTables` ~0.02 ≈ **3.5 GiB**; RSS at the floor
  run's ratio (4.85 GB for 3.0 GiB) about 5.0–5.5 GB. Retained heap and peak RSS are
  measured separately.
- Policy check: the 7.0 GB `witness.raw_bytes` is a live-census bucket, not a peak
  component; refs for 9.78 M facts are 78 MB and the tables ~25 MB; expectation about
  2.7–3.0 GB, to be measured.
- Step 0 alone (the sort and the flatten): the **capability-row phase footprint** from
  §2.3's marks is base 13.3 + rows 14.0 = 27.3 GiB; with the other retained snapshot
  fields (0.86 GiB) about **28.2 GiB retained**. Transient allocations and RSS are not
  predicted; they are measured when step 0 lands.

## 8. Migration steps, in order (each merged alone; outputs byte-identical after each)

**Step 0 — the sort and the flatten, no type change.** `cmp_joined` with its tests (T2);
the subject-split, pre-sized, parallel per-slice stable sort on today's owned rows, with
the delimiter fallback and T6; `sort_inherited` and policy's sort on `cmp_joined`.
Removes the per-row key strings and the flatten transient. Measured alone, so the later
steps' effects are not confused with it. May start now.

**Step 1 — compact raw in the cone.** The types of §4.1–4.2; `dedup_direct` mints the
`Arc`; carriers hold `&Arc`; rep interning by identity, hop interning by content with the
subject; `materialize` replaces `retag`. Readers: `derive_capability_facts` (materialize
per subject, then `snapshot_fact`, until step 2), the two evidence derivers,
`derive_permissions` and `make_required_perm`, policy (`FieldEvalContext.fact:
Option<FactRef>`, the field reads, `emit_match_finding`), `project_r3a3`/`project_r3a5_*`
(materialize per subject), the census, `test_support` and `cone_census::with_inherited`
call sites, `SubjectRaw`/`TreeRaw`/`View::raw`. Pays off at once in `policy check`
cross-app and every witnessed view (prove, digest, batch).

**Step 2 — the 16-byte snapshot row.** `SnapTables`, `SnapRep`, `SnapHop`, the handle
with accessors, `SnapshotFactFields` + `From` + `fields()`, `Serialize` on accessors; the
construction of §4.3. Readers switch field reads to accessors: digest, prove,
fingerprint, fingerprint_query, `snapshot_full` CBOR, inventory, `key_classes::retain`,
`ordering_engine`'s scan, `literal_reach`, `test_coverage`, `derive_pd_arena`,
`ordering_kept_roots`; tests: literals gain `.into()`, mutation-style fixtures go through
`fields()`. Measured: the full-model snapshot and analyze cross-app.

**Step 3 — housekeeping.** Census buckets rewritten for the new forms; the
`legacy_raw_oracle` (§9, T3) deleted once the CDO comparisons for steps 1–2 are
recorded; the probe committed as `tools/cone-probe` beside `tools/census-probe` so §7
can be re-measured; the ordering-cell kind-scope follow-up filed.

`compose_snapshot` keeps accepting the unsplit cross-app model (tests and the i264
census use it); with this design it is affordable, so no refusal or new type is added.
`compose_callsite_resolutions` (ad80fa9c) stays: it is cheaper still for a reader of
call-site rows alone.

## 9. What must stay byte-identical, and how it is proven

Everything. No golden is regenerated in any step: all 24 golden directories through
`scripts/check-goldens` (no `--regen`), in particular r3a3/r3a4/r3a5, r4/r4f,
`l4-summary-baseline`, every `tests/cli-*` family, `gate-goldens`, and
`tests/goldens/semantic-edges`. A golden that moves is a bug in the change.

Every test below records a discrimination proof: break with Edit, watch the named
assertion fail, revert with Edit, watch it pass; a scripted break asserts it applied.

- **T0 sizes and sentinel.** `const` asserts `size_of::<InheritedRef>() == 8` and
  `size_of::<SnapshotCapabilityFact>() == 16`; a test that index construction rejects
  `u32::MAX` and that a direct row reports `NO_HOP`.
- **T1 `materialize` == old `retag`; accessors == `materialize`.** Hand-stated rep and
  three hops (a call edge with a call site; `implicit-trigger` with an op id;
  `event-dispatch` with an event id), plus a rep carrying a stale first-hop op id and
  its own call-site witness. Breaks: keep the rep's first-hop op id → fails; return the
  rep's call site from `witness_callsite_id()` → fails.
- **T2 `cmp_joined` == `join("|")`.** The `a`/`ab` trap, a field containing `|`, empty
  fields, and an exhaustive enumeration over fields from `{"", "a", "ab", "|", "a|", "b"}`
  for three fields. Breaks: tuple compare → the prefix case fails; drop the virtual `|` →
  the `"a|"` vs `("a", "")` case fails.
- **T3 raw equality against independent sources.** (a) The byte-compared goldens
  r3a3/r3a5 (raw per subject) and r4f (the table) are frozen pre-change output. (b) A
  `#[cfg(test)]` module `legacy_raw_oracle` holds a verbatim copy of today's
  `inherited_facts_for_singleton` (raw half), `inherited_facts_by_bfs`, `emit_picks`,
  **`candidate_wins`**, `retag`, `capability_via_for_edge_kind`,
  `inherited_output_sort_key` and `sort_inherited`. Its shared calls are audited and
  listed, all unchanged by this design and immutable inputs: the graph `g`,
  `scc_id_by_routine`, the cones, `direct`, the pd `own` entries, `base_key_of`,
  `frame_bound`, `substitute_entry`, `inherited_fact_key`. The oracle must not call the
  new selection, interning or comparison code (a test greps the module for those
  symbols). Under a test flag both run inside `compose_inherited_cones`, and the test
  compares every subject's materialized new raw with the oracle's owned raw, field by
  field, over every `tests/r0-corpus` and `tests/fixtures` app, single-app and cross-app
  **full** model (`assemble_and_resolve_cross_app_program`, so dependency callers are
  in). Preconditions asserted: > 100 models, > 0 inherited facts, > 0 guarded entries,
  > 0 trigger and event first hops, > 0 recursive members with producer replacements.
  Required failures, mutating **production only**: flip production `candidate_wins` to
  prefer the larger distance → T3(b) fails on N models (record N); emit the previous
  pick's hop index → fails. Stated limit: the oracle shares cone construction, so it does
  not verify it; the goldens and the CDO comparisons cover that boundary. (c) Step 2:
  the serialized table, the pd arena's `producer_fact` indexes and `facts_by_routine`
  per subject, against rows built by the old owned path from the oracle's raw.
- **T3b recursive first-seen.** A literal recursive SCC where the first-encountered
  downstream cone holds a key at `hops + dist = 3` and a later cone offers it at 2: the
  first is kept (today's behaviour). Break: compare on distance → fails. Second case:
  an in-place producer replacement keeps the pick's position. Break: push instead of
  replace → the order assertion fails.
- **T4 policy reads the same facts.** T4a normal call hop: the callee's direct fact has
  its own call-site witness; the finding's evidence carries the caller's call site.
  Break: return the rep's call site → fails. T4b trigger hop: call site `None`,
  `witness_operation_id` kept, first-hop op id present in the snapshot row and in
  `make_required_perm`'s output. T4c event hop, likewise with the event id.
- **T5 memory guard.** A standalone test crate with a counting `#[global_allocator]`
  builds a synthetic corpus (N callers of one shared publisher/writer set, so inherited
  facts grow as N × set size) and asserts live-heap growth per inherited fact ≤ 16 B
  (raw, step 1) and ≤ 32 B per snapshot row (step 2: 16 B plus tables and slack). Break:
  materialize at emission → fails.
- **T6 sort split and fallback.** Hand-built snapshots with: rows whose complete 8-field
  keys tie but whose payloads differ, ≥ 3 per group, in a known emission order; subjects
  `a` and `ab` whose `rest` keys make tuple order and joined order disagree; two
  internal ids mapped to one stable subject; a duplicated internal id (only the first
  row emits); a subject containing `|` whose rows interleave with another subject's under
  the joined order (defeats grouping); two direct facts of one subject with the same op
  id whose first index `derive_pd_arena` must pick. Each case equals the old global
  `sort_by_cached_key` order and, for the pd case, the old `producer_fact` index.
  Breaks, each observed: `sort_unstable_by` in the slice → the tie case fails; skip the
  delimiter scan → the `|` case fails; group by bare subject instead of `subject + "|"`
  → the prefix case fails.
- **T7 interning and subjects.** (i) Two subjects reaching one callee fact through
  different hops share a `RepIx` and differ in `HopIx`; a guarded and an unguarded entry
  share the `RepIx`. (ii) Two subjects whose hops have identical (via, callsite, op,
  event) get different `HopIx`, and the **snapshot rows carry the right subjects**.
  Break: intern hops without the subject → the snapshot-subject assertion fails (raw
  equality alone stays green, because views inject the subject). (iii) One subject with
  two edges from one call site to different targets gets ONE `HopIx` (content merge).
  Break: intern by edge address → the merge assertion fails. (iv) `None` and `Some("")`
  call sites get different `HopIx`.
- **T8 ownership and empties.** Production-path tests on a view: overlapping trees then
  policy (borrow, then the rest drained), a partial drain, a borrow, and an explicitly
  composed subject with no inherited facts. `get` is `Some(empty)` after every
  drain/borrow; the tables survive a drain (strong count); the drained map's keys are
  owned and the views' subjects borrow from the tables; the empty subject keeps its
  scoped producer-solve states in the tree's pd arena. Break: drop empty entries at
  emission → policy's "summary but no raw" panic fires in the test.

**CDO proofs** (branch gate; numbers recorded in CHANGELOG): `alsem policy check
--deterministic --format json` cross-app and `--single-app`: `cmp`; `alsem analyze
--preset transaction-integrity --deterministic --format json` both modes: `cmp`;
`alsem digest` and `alsem prove` on a fixed root list cross-app: `cmp`; the probe
streams the whole full-model snapshot **document** (`project_r4f_snapshot` shape: every
serialized field — capability facts, typed edges, operation and callsite indexes,
callsite resolutions, gaps, coverage, event declarations, root classifications, frames)
through BLAKE3 before and after each step, hashes equal; **plus** a deterministic
comparison of the in-memory parts the document omits: `pd_solution` (states as
`(producer_fact, temp_state, routine, seed)`, links as `(callee state, edge index)`,
`by_routine` with sorted members) and `boundary`, serialized to JSON and hashed;
`scripts/cdo-gate` green; `scripts/check-goldens` with no `--regen` green.

## 10. Candidates considered (revision 1) and where revision 2 differs

Revision 1 measured four candidates: **A** scope + drain + cheaper sort (symptom only,
~14 GB), **B** a 16-byte `(Arc<CapabilityFact>, HopIx)` pair in the cone, **C** = B plus
a `FactTable` of 16-byte rows with a borrowed `FactView<'_>` in the snapshot
(recommended), **D** lazy facts from kept cones (rejected). Revision 2 keeps C's shape
and goals and differs in: `(u32, u32)` refs into shared tables (half the bytes, one
projection per rep, no per-fact refcount traffic); a 16-byte owned handle with
accessors instead of a lifetime-carrying view; the subject-split sort with a delimiter
check, filled in place, instead of one global comparator; step 0 first and alone;
borrowed `FactRef` for policy and the raw readers instead of per-subject
materialization; direct reps shared from `dedup_direct` onward; an explicit subject
contract; kind needs answered as deferred with a named candidate; T3 with a frozen
old-emission oracle. Agreed and kept from revision 1: A and D rejected;
`compose_callsite_resolutions` stays; `compose_snapshot` keeps accepting the full model;
no golden regenerated; no declared-needs change; the sort trap and
`inherited_output_sort_key` preserved.

## 11. Risks

- **Identity interning of reps.** Safe because the table holds the `Arc` on first sight
  (§2.8 shows 5,402 address reuses without it); T7 pins the sharing. Hops intern by
  content, so no address assumption there.
- **Subject construction.** The contract of §4.1/§4.3 with its debug assertions, T7(ii)
  and T8.
- **A direct fact that is also a rep.** provenance/via/call-site rules follow the row
  kind; T1 and T3 pin it, including the dynamic serialized key order.
- **First-seen BFS semantics.** The walk's control flow is untouched; T3b and the frozen
  oracle catch a "simplification".
- **Refcount traffic.** 17 M `Arc` clones and drops of the table handle; measured by the
  probe, not assumed.
- **Churn.** ~60 row literals gain `.into()`, a few mutation-style fixtures go through
  `fields()`, ~30 digest signatures keep their type; the oracle copy is deleted in step 3.
- **Census numbers move** (`witness.raw_bytes`, `snapshot.capability_facts`); recorded
  with the change so a later reader does not read the drop as a bug.
- **The `cdo` profile vs `release-fast`.** All timings and memory come from
  `release-fast`.

## 12. Open questions for the owner

1. Step 0 may start now (reviewer's verdict); steps 1–2 after this revision's subject
   contract and oracle plan are accepted.
2. Keep the probe: commit it as `tools/cone-probe` in step 3 so §7 can be re-measured.
3. The ordering-cell kind-scoped build (§6): file after step 2's measurements.

## 13. Re-measuring

The throwaway probe of §2.1 is now a committed `aldump` mode. It replaces the
`tools/cone-probe` plan of §8 step 3 and §12 item 2. It builds the same model (the gate
model-instance id, `AssemblyNeeds::BASE`; cross-app by default, `--single-app` for the
workspace alone). It runs `build_r3a3_source_only_base` and `compose_snapshot_from_base`
as timed phases and prints JSON: phases (wall time, `VmRSS`, `VmHWM`), census marks,
counts (inherited and direct facts, `InheritedTables` and snapshot table sizes, snapshot
rows, pd arena) and row sizes. `--hash` adds BLAKE3 per serialized section, of the whole
document and of the pd arena (the §9 comparison). It reports RSS, not live heap: a
counting allocator would become `aldump`'s global allocator for every mode. Live heap
stays with `tools/census-probe`.

```
cargo build --profile release-fast --bin aldump
/usr/bin/time -v target/release-fast/aldump --snapshot-memory $CDO_WS        # peak RSS
target/release-fast/aldump --snapshot-memory $CDO_WS --hash                  # hashes
target/release-fast/aldump --snapshot-memory $CDO_WS --single-app
```

Reference values (master `93634d11`, after step 3; CDO `bc3ccb18`; release-fast; Linux,
22 cores; 2026-10-31):

| Run | Peak RSS | Wall | Facts |
|---|--:|--:|---|
| cross-app | **5.29 GB** (the step-2 probe: 5.28 GB) | 54 s | 17,017,608 inherited + 39,429 direct = 17,057,037 rows |
| cross-app `--hash` | 5.36 GB (hashing adds ~0.07 GB; take peak RSS from the run without it) | 110 s | same |
| single-app | 0.82 GB | 6.3 s | 162,651 inherited + 4,717 direct |

Cross-app tables: `InheritedTables` 33,285 subjects, 27,477 reps, 38,081 hops; snapshot
tables 33,285, 66,906, 38,081; pd arena 2,822,661 states, 3,928,044 links, 9,049
routines. Hashes, equal to the step-2 probe's:

- document (9,463,055,195 bytes): `3b480fbf53066d4d071422ee4b9390f768da655687600860937e95c2c2a2ddaf`
- pd arena: `76fd29acd662d9bbae6fc899876c94b0ff05e951b288610403e6f52c238b8c98`

The pd-arena hash includes `Debug` forms (`TempStateKind`, ranges, the sorted
`by_routine` map), so compare it only across changes that keep those types.
