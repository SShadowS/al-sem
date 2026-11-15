# ENG-82: where the bytes and the CPU of the CDO model build go

Measured 2026-11-15 on server1 (kraken tentacle), branch `alsem/eng-82-census-profile` from master
`f912aea8`, on the pinned CDO baseline (`/work/src/cdo-baseline/Cloud`, `bc3ccb18`). Every CDO run
went through `scripts/cdo-slot run engine …`. The point is to rank the survey's tasks (ENG-83..87,
TLS-66/67, filed from a read-only survey whose gains were estimates) by measured numbers.

## Headline

1. **CPU: one function is 42% of `build_target`**, the first half of a resident first ask (session
   and target, 18.4 s of its ~36 s wall; the fold and cone half was not profiled). `byte_to_pos`
   (`src/program/resolve/extract.rs:338`) turns a byte offset into a line and column by counting
   the newlines in the whole file prefix, and it is called twice per call expression
   (`extract.rs:800-801`, `840-841`; the same pattern in `extract_min.rs`). The cost grows with
   file size times call count. The survey did not list it. **New task, top of the list.**
2. **Bytes: dependency syntax trees (940 MiB) and the cones (748 MiB) lead.** Then the revision
   store (611 MiB), dependency routine rows (548 MiB), model routines (473 MiB) and dependency
   site facts (385 MiB, not in the survey). The retained edges are 290 MiB. The model rows' anchors
   are about 94 MiB; the survey's 300-600 MiB estimate also covered the dependency rows, whose
   anchors were not counted.
3. **One survey item is too small to matter:** the dependency closure walk (ENG-84's B1, 0.2%).
   The recorder insert itself (B7) is 0.2% too, but the logs' later use may reach ~3%. The
   lowerer's lookups (ENG-83) are 1-5%: small, not nothing.

## How it was measured

The raw outputs (`resident.out`, `probe.out`, `pprof.out`) and the two scratch probes' sources are
kept in `tools/census-probe/runs-eng82/` (the 337 MB folded stack file is not).

- **Bytes:** `tools/census-probe resident <ws>` (counting global allocator, `--features
  mimalloc`): one resident build, then one ask of each kind, then live bytes and allocation counts
  per bucket. The byte-census skill's conventions: lengths, not capacities; MiB; a shared
  allocation counted once; allocator overhead not counted. The probe had drifted from five API
  changes; this branch fixes it (the only code change, in `tools/`, not part of the shipped crate).
- **Counts and `size_of`:** a scratch probe (not committed) with path dependencies on this tree.
  It counts the parse trees, the retained dependency edges and the model's anchors.
- **CPU:** samply needs `perf_event_paranoid` ≤ 1 and the container has 2. So the profile is an
  in-process SIGPROF sampler (the `pprof` crate, 499 Hz, all threads) in a scratch crate around
  the engine child's own build path (`WorkspaceInputs::read` → `WorkspaceSession` →
  `build_target`), `release` with line tables. One build: 18.3 s wall, 12,445 samples. Shares,
  not times; the sampler sees CPU across all threads. pprof is not added to the shipped
  `Cargo.toml`.

## Bytes

Live after one build and one ask of each kind: **5,716 MiB, 37.9 M allocations**. The build alone
leaves 3,346 MiB (26.1 M allocations, 44.8 s under the counting allocator).

| Bucket | MiB | Allocations | What is inside |
|---|---:|---:|---|
| `session.tier.bodies` | 940 | 4.70 M | dependency syntax trees (IR), source text 115.5 MiB included |
| `fold.cones` | 748 | 2.57 M | capability cones |
| `revision.store` | 611 | 4.67 M | the resident revision's artifacts |
| `session.rows.dependency.routines` | 548 | | statement_tree 144, call_sites 122, record_variables 83 |
| `model.routines` | 473 | | call_sites 151, record_variables 100, record_operations 36 |
| `session.dep_resolution.site_facts` | 385 | 0.55 M | per-site facts (`SiteFacts`); composition not measured |
| `artifact.det` | 314 | 2.28 M | detector context |
| `session.dep_resolution.edges` | 290 | 1.74 M | retained dependency call edges |
| `fold.pd.states` | 284 | | |
| `dependency_record` | 114 | | |
| `artifact.l4` | 84 | | |
| `session.rows.workspace` | 81 | | |
| base, the rest | 78 | | |
| `base.dependency_calls` | 73 | | |
| `fold.pd.links` | 73 | | |
| `session.tier.dep_meta` | 65 | | |
| `fold.direct_full`, `base.direct_full` | 56 each | | |

**Counts** (cross-app CDO):

| | Workspace | Dependencies |
|---|---:|---:|
| files | 582 | 10,881 |
| expressions | 122,411 | 2,791,331 |
| statements | 28,424 | 600,787 |
| blocks | 13,063 | 278,478 |
| identifier uses (one owned String each) | 78,054 | 1,838,488 |
| identifier bytes | 0.98 M | 24.4 M |

- Retained dependency edges: 438,823, with 325,407 routes; 306,595 edges have exactly one route.
  Route conditions: **13 in the whole graph**. Inline size: edges 154 MiB, routes 74 MiB.
- Model rows (cross-app, what `explain` needs; the probe's own build, not the resident one):
  34,205 routines, 126,814 call and operation sites, 323,588 control-flow nodes, **600,851
  anchors**: their file-id strings 50 MiB, syntax-kind strings 8 MiB, inline 36 MiB. The retained
  dependency rows (28,726 routines) hold a second set, not counted. The survey estimated 3-5 M
  anchors and 300-600 MiB over both; A4 can remove only the strings (58 MiB per copy).

**`size_of`** (bytes): Origin 48, Expr 128, ExprKind 80, Stmt 88, Block 72, PAnchor 64,
PCFNNode 424, PCallSite 432, POperationSite 184, ClassifiedEdge 368, Edge 224, Route 240,
Condition 1, RoutineNodeId 72, ModelRoutine 1,064.

Dependency expressions alone are 2.79 M × 128 B ≈ 341 MiB inline, the largest counted part of the
940 MiB (36%). With identifier bytes (23 MiB), statements (50) and blocks (19) about 433 MiB is
explained; about 390 MiB of the bucket is not broken down yet.

## CPU

Shares of the kept samples in one `build_target` (inclusive = any frame on the stack with that
exact name):

| | Share | Kind |
|---|---:|---|
| `dep_record::resolve_recorded` (dependency body resolution) | 44.4% | inclusive |
| `byte_to_pos`'s newline count, under it | 42.2% | inclusive |
| allocator (mimalloc, memcpy) | 10.7-16.2% | self (strict leaf names to a looser name match) |
| `lower_file` (lowering, including its tree-sitter cursor walks) | 9.1% | inclusive |
| tree-sitter parsing (`ts_parser_parse`) | 8.4% | inclusive |
| SipHash | 7.1% (9.4% inclusive) | self |
| identifier folding (`fold_identifier`, `eq_fold_identifier`) | 2.5-3.6% | inclusive |
| lowerer lookups: kind and field (`from_raw`, `field`) / with `named_children` | 1.1% / 4.6% | inclusive |
| serde_json (package manifests and symbols) | 2.3% | self |
| recorder insert (`note`) | 0.24% | inclusive, a floor (key clones are inlined) |
| dependency closure walk (`closure`) | 0.18% | inclusive |

So almost all of dependency body resolution is `byte_to_pos`. With it fixed (for example from
the tree-sitter `Point` each `Origin` already carries, or a per-file line-start table with a
binary search), `build_target`'s CPU falls by up to about 40%. The wall gain is smaller: it runs
in the parallel dependency-resolution phase (6.5 s wall of the first ask's ~36 s). Every other
share then grows by about 1.7×: SipHash would be about 12%.

**Limits of this profile.** One build, one corpus, one path: CLI `analyze` (no recorder), a
`--low-mem` S refresh (which parses again), the test universe and the fold and cone half of the
first ask were not profiled; a "drop" here holds for cold `build_target` on CDO only. Shares under
1% rest on 20-40 samples (about ±20%). 12,445 samples at 499 Hz is 24.9 CPU-seconds in 18.3 s
wall, which is low for a 22-thread pool: either the parallel work sits on a few big files, or the
process-wide `ITIMER_PROF` lost samples while many threads ran. No CPU total was recorded to
tell them apart; lost parallel samples would make the 42% an under-count. The sampler's
blocklist drops samples inside libc (`memcpy`, system calls).

## The survey's tasks, re-ranked

| Rank | Task | Measured size | Verdict |
|---:|---|---|---|
| 1 | **new: `byte_to_pos`** | ~42% of build CPU | file it; byte-identical positions must be proven |
| 2 | ENG-86 slimmer IR nodes (A3) | dependency trees 940 MiB, ~341 MiB of it inline expressions, 1.84 M identifier strings | keep, the biggest memory item in reach |
| 3 | **dependency site facts** (not filed) | 385 MiB over 0.55 M allocations (~700 B each); no per-field split | split it by field first, then decide |
| 4 | ENG-87 smaller retained edges (A2) | 290 MiB; 70% of edges have exactly one route, 13 route conditions in the whole graph | keep; an inline single route and no empty condition vectors |
| 5 | TLS-66 fast hasher (A1) | SipHash 7-9% now, ~12% after rank 1 | keep, after rank 1 |
| 6 | ENG-85 strings in anchors and rows (A4, A5 step 1) | anchor strings 58 MiB per copy of the rows | shrink: keep the row-string half, the anchor half is small |
| 7 | ENG-84 cheap CPU batch (B1, B2, B7) | B2 folding 2.5-3.6%; B7 insert 0.24%, the logs' later use up to ~3%; B1 0.18% | **drop B1**; keep B2; narrow B7 to what is done with the logs, measured first |
| 8 | ENG-83 lowerer lookup by id (A6) | 1.1% (kind and field) to 4.6% (with `named_children`), about half of lowering | keep, small; after rank 1 it is 2-8% |
| 9 | TLS-67 test selection, parser reuse (B3, B6) | not in the model build; parsing proper 8.4%, of which the parser setup per file is a small part | keep as tools work; measure on test selection, not here |

Not in the survey and large enough to look at later: the cones (748 MiB), the revision store
(611 MiB) and `PCFNNode` at 424 bytes (statement trees in dependency rows are 144 MiB; boxing its
rare fields, the survey's B5).

## Tooling notes

- **samply is not in the kraken image.** It was installed with `cargo install samply`
  (`~/.cache/cargo/bin/samply`, 0.13.1, not on `PATH`); an image rebuild loses it.
- **perf and samply need `perf_event_paranoid` ≤ 1**; the container has 2. If the owner wants
  system profilers in the kraken image, that setting must change on the host. Without it, an
  in-process sampler (pprof in a scratch crate) works unprivileged.
- `ALSEM_RETAINED_CENSUS` prints nothing on the resident path; use `census-probe resident`.
- `release-fast` strips symbols. A profiling build needs `CARGO_PROFILE_RELEASE_FAST_STRIP=false
  CARGO_PROFILE_RELEASE_FAST_DEBUG=line-tables-only`.
