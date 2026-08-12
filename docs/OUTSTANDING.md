# Outstanding items

Living checklist — tick items (`- [x]` + landing commit/date) as they land; add new
items as they surface. Rewritten clean 2026-07-17 (post preflight-fresh-coverage +
outstanding-sweep runs); the full histories of completed arcs live in the Archive at
the bottom, CHANGELOG, and git log. Consolidated again 2026-07-31 (Wave-2/3 entry
re-based on the post-d1-memory/post-uncertainty 8020 numbers; the d1 follow-up
sizings marked pre-arc).

## Open — needs the user

- [x] `git stash drop` leftover stashes — DONE 2026-07-17: user ran `git stash clear`
  (26 accumulated scratch stashes from merged arcs, all superseded; verified 0 remain)
- [x] `/triage-wave` sharing — DONE 2026-07-17 (`79bf189`): `.claude/commands/`
  un-ignored and versioned (project doctrine as tooling); CLAUDE.md worktree note updated
- [x] **d61/d62/d64 validation** — DONE 2026-07-17 (`f3f5c85`). Corpus: Microsoft
  System App + Base App 28.0 embedded source extracted from DO's `.alpackages`
  (9.3k real files). d62: 9 findings triaged (1 real, 8 FP) → structural
  branch-exclusivity class ROOT-CAUSE FIXED via statement_tree (9→4; the
  `if Success then Log else Error` idiom no longer flags); 3 residual semantic FP
  classes documented → stays opt-in. d64: first population (8 API pages) → only FP
  class (SourceTableTemporary) fixed, 2→0 with honest skips → stays opt-in (no TP
  yet). d61: 7,367 real candidates, 0 emissions, guards hold (caveat: sliced corpus
  may hide cross-slice event pairs) → stays opt-in. Promotion wake for all three:
  a triaged true-positive population

## Open — buildable backlog (no blocker, pick up any time)

- [ ] **Fixture witnesses for the grammar-v4 semantic fixes** (from the 2026-08-12
  v4.0.0 upgrade's golden triage): the two semantically riskiest v4 changes have NO
  witness in `tests/r0-corpus/` — (a) a dangling `else` now binds to the inner `if`
  (the one v4 change that alters program meaning), and (b) the operator-precedence
  repairs (`and`/`or`/`xor` tighter than comparison; `-a * b` = `(-a) * b`; `..` no
  longer an expression operator). Nothing in the corpus pins the engine's IR shape
  for these, so a future grammar regression would be invisible to every golden.
  Add one small fixture per shape (remember: a NEW r0-corpus fixture moves THREE
  golden families and r4 needs a committed seed file — see CLAUDE.md), with
  discrimination proofs.

- [x] **scanner.c MSVC `_Static_assert` guard defect — FIXED UPSTREAM in
  tree-sitter-al v4.0.1** (2026-08-12, grammar commit `3bac021`): the guard is now
  `__STDC_VERSION__ >= 201112L` alone, exactly the suggested condition; default-flags
  MSVC builds work again. The engine keeps `cc.std("c11")` in
  `crates/al-syntax/build.rs` deliberately — under C11 the compiler takes the
  message-printing `_Static_assert` branch instead of the opaque negative-array
  fallback.

- [x] **Golden-gate coverage repair** — DONE 2026-07-25 (task-3 fix wave, review
  I-3). Every "goldens clean" claim in the l3-substrate/C1 arcs rested on a gate
  with real holes; task 3 walked straight through one (it moved
  `tests/r3a3-goldens/` behind a green `scripts/check-goldens`). Fixed, all
  verified by construction:
  - `scripts/check-goldens` went from **5 targets / 17 of 29 golden directories**
    to **9 targets / 29 of 29**. Added `--test r3` (r3a1/r3a2/r3a3/r3a4/r3a5,
    483 files), `--test r25_abi` (7), `--test l4_summary_differential` (18) and
    `--test program_resolve_harness` (`tests/goldens/semantic-edges/`, 2 —
    a 12th uncovered dir the review's count of 11 missed). Proof: corrupting one
    r3a3 golden now FAILS the script (it did not even run before) and `--regen`
    restores it byte-for-byte. r3a3's regen is content-change-gated by design, so
    `--regen` legitimately writes zero r3a3 files when nothing moved.
  - `--no-fail-fast` added — the first failing binary used to hide every later
    target. Proof: with r3a3 stale, `--test r4` still reports after `--test r3`
    fails.
  - `scripts/git-hooks/pre-commit`'s path filter widened from
    `src/engine/{l4,l5}/` + 4 golden dirs to ALL of `src/engine/`,
    `crates/al-syntax/src/`, every `tests/*goldens*/`, `tests/l4-summary-baseline/`,
    every `tests/*-vectors/`, and the fixture roots. It matched **none** of
    task 3's substrate (`src/engine/{ids.rs,l2,l3,deps}`) and none of the four
    dirs that moved; it fired only because two doc-comment edits landed in
    `src/engine/l5/`. Honest cost: ~23s warm-cache on a firing commit (was ~17s
    for the old 5 targets), plus any debug rebuild.
  - `scripts/cdo-gate` gained `--test l4_summary_differential`. The CDO
    whole-program L4 frozen digest was on **no** runner at all — see the next
    entry.
- [x] **CDO L4 frozen digest re-freeze** — DONE 2026-07-25 (task-3 fix wave,
  review I-2). `tests/l4-summary-baseline/cdo-whole-program-digest.txt` moved
  `d3fc4f0e…` → `d9eac0c7…` (3685 → 4842 routines) as a consequence of task 3's
  enclosing-member discriminator. It was NOT blind-regenerated: disabling that one
  conditional key-part append, with the rest of the tree unchanged, reproduces the
  OLD digest byte for byte — an exact single-variable attribution, stronger than
  the masked-diff method the committed goldens needed. Full evidence table
  (population decomposition 3231 unchanged / 454 replaced / 1611 minted, the
  +1157 matching `detector_context.rs`'s independently-measured figure, zero
  shape or value-domain movement) is in
  `tests/l4-summary-baseline/README.md`'s re-freeze log — **add an entry there
  for every future re-freeze; the independent oracle behind this baseline was
  retired, so "regenerated, tests green" proves nothing on its own.** Note the
  review's premise that `scripts/cdo-gate` would fail on this was wrong in a
  worse way than being wrong: that script did not run the test at all. It does now.

- [x] **Engine memory/speed Wave 1 (Track A)** — DONE 2026-07-18 (branch
  `worktree-design-engine-memory-speed`, commits `9c0ee77..708f000`, 10 tasks
  SDD-executed + per-task reviewed, goldens byte-stable throughout). Base App
  8k 3-detector: DNF@90min/35.8GB → **90 s / 6.1 GB**; slice-5400 236s/9.8GB →
  58s/3.4GB; DO unchanged (10.7s, byte-identical). W1.0 demand-driven substrate
  (per-detector requires + full-vs-minimal parity test), W1.1 Jacobi
  (uncertainty index, serde-free change keys w/ equivalence proof, take-based
  snapshot, dirty frontier), W1.2 SpanTemplate, W1.3+A7 move-don't-clone,
  W1.4 parallel L3 parse, W1.5 FingerprintIndex-once, A8 cross-ext hoist,
  A9' parallel diagnostics re-parse. Decision (a): substrate-skipping runs omit
  summarize cap-hit diagnostics (only permitted output change). Wave-1 outcome
  table: findings doc §7b
- [ ] **Engine memory/speed Wave 2/3 (Track B)** — *consolidated 2026-07-31; the
  per-wave narrative that used to live here (Wave-2a/2b/2c, the "8020 full-default
  DNF at 45.2 GB" measurements, the falsified 846-SCC trigger-edge perf hypothesis,
  and the falsified OUTPUT-BOUND attribution) is in
  `docs/superpowers/specs/2026-07-18-wave2-measurements.md` §2–§9 and in the d1
  section at the bottom of this file. It described a corpus state three arcs out of
  date and is not repeated.*

  **Current 8020 state (BC Base App, 100,941 routines, default preset,
  `release-fast`): ~158 s wall / ~5,394 MB process peak** — from DNF@2h/45.2 GB,
  via the d1 cohort redesign (`ee3aa45`), the d1 memory arc (peak 9,675 → 6,411 MB,
  wall 310 → 197 s) and the uncertainty substrate (`93bb9af`, peak 6,409.8 →
  5,394.5 MB). Wall figures for this corpus/probe swing **±80 s** — see the d1
  capstone's qualifier in CHANGELOG; only the memory figures are robust singly.

  **DONE 2026-07-31 — `context.transaction_spans`** (branch `perf/transaction-spans`,
  plan `docs/superpowers/plans/2026-07-31-transaction-spans-interning.md`, ledger
  `docs/2026-07-31-transaction-spans-measurements.md`). It was **58.90 s of a 206.43 s
  run** — the largest single span, and on no list until this arc measured it. Now
  **1.14 s**. `aggregate_span` was resolving each visited routine's whole folded-cone
  window into a fresh `Vec<String>` and dropping it: **261,772,789 strings per 8020 run
  → 1,762,840**. Byte-identical on both corpora (DO `f022f677…`, 8020 `36151bf6…`),
  zero golden movement. **The census falsified two of the three cost centres the plan
  was built on** — the `String`-keyed BFS runs 927 walks over 129,350 steps total, so
  the planned interned-ix rewrite was never built, and the per-op payload clone
  duplicates only 134 of 1,061 spans, so that task was skipped by its own gate. Measure
  the population before building the taxonomy for it — again.

  **Post-fix 8020 profile (warm, `analyze.total` 76.15 s) — the re-ranked lever list:**
  `context.capability_cones` 14.53 s (19.1 %) · `detector.d1`'s `scoring` 10.56 s
  (13.9 %, inside `search_loops_cohorts` 12.35 s / `detector.d1` 13.71 s) ·
  `context.compute_summaries` 8.60 s (11.3 %) · `preflight.fresh_coverage` 4.89 s
  (6.4 % — its 71.81 s in the cold baseline run was file-cache, not real cost) ·
  `detector.d2` 3.46 s. Note this makes the d1 witness/uncertainty follow-up below a
  REAL item again, at a re-measured envelope of ≤10.56 s.

  **MEASURED 2026-07-31 — `context.compute_summaries`** (branch
  `perf/summaries-census`, ledger `docs/2026-07-31-compute-summaries-census.md`,
  probe `ALSEM_SUMMARIES_CENSUS=1`). Attribution only — **no fix built yet**. The
  span was ~10 s of ~75 s on 8020 with zero attribution; the census accounts for
  97 % of it. `scc_loop` 86.1 % › `db_solver` 71.4 % › **`solve_side_facts`
  46.5 %**, which splits evenly between its per-member edge loop (1,773 ms) and
  its final assemble loop (1,724 ms). **The edge loop is not edge-bound**: 150,211
  edges but **4,397,866** `shared.insert(uncertainty_key(u), u.clone())` calls
  (29.3 per edge — every external edge re-folds the settled callee's whole
  `uncertainties` vector), each a fresh key `String` plus a five-`Option<String>`
  deep clone; the assemble loop then re-copies **3,687,409** elements and feeds
  **3,708,222** to `dedupe_uncertainties`. ~8.1 M deep clones + ~4.4 M key strings
  per run, for an output of 27,037 nodes / 19,311 distinct values / 10,112
  distinct sets — the transaction-spans over-materialization shape again.
  Everything else is small: `finish` 9.0 %, `roles` (the surviving JACOBI
  fixpoint) 9.8 %, the whole prologue 4.6 %, `field_index` 0.2 %.
  **Falsified by the census before anything was built:** (a) the workspace-sized
  `settled.clone()` in `solve_scc_db_effects`'s multi-sibling path NEVER runs —
  `multi_eff_sccs=0` on 8020 and on DO; (b) the roles fixpoint is not the cost.
  **DO's shape is different** (span 130 ms; `roles` 42.2 % > `db_solver` 40.1 %,
  `side_facts` 12.6 %) — survive-Base-App item, like B2, and its DO effect must
  not be negative.

  **DONE 2026-07-31 — the `solve_side_facts` fix** (branch `perf/side-facts`,
  Part 2 of the same ledger). `fold_shared` (reused key buffer + compare-don't-
  overwrite), an allocation-free `dedupe_uncertainties` (stable sort on a
  `memcmp`-backed concatenated-key comparator + keep-last), a MOVE of `shared_vec`
  into each effective SCC's last member, and two `get().cloned()` → `remove()`
  sites. **Paired A/B, alternating runs, 3 each, medians**: `phases_total`
  8,877.8 → **6,018.5 ms (−32.2 %)**, `side_facts` 4,230.2 → 2,519.9 (−40.4 %),
  `out_assemble` 458.1 → 17.5 (−96.2 %), with the untouched `roles` control at
  −3.9 % (the noise floor). DO improved too (130.0 → 96.7 ms), so unlike the
  uncertainty substrate this one has no negative small-workspace side. Both gate
  hashes exact. **The dedupe change was a measured REGRESSION in its first form** —
  removing 3.7 M allocations lost to the byte-iterator comparator that replaced
  them; an allocation count alone is not evidence of a win.

  **DONE 2026-07-31 — the profile is FULLY ATTRIBUTED, and a quarter of it was
  never measured** (branch `perf/attribute-unmeasured`, ledger
  `docs/2026-07-31-profile-attribution.md`). Ranking by SELF time (exclusive of
  nested spans) instead of inclusive total put **24.8 % of the run — 18.9 s of a
  76.2 s median — inside two brackets that named none of it**: `analyze.total`
  (15.5 %) and `l4_l5.run_detectors` (9.3 %), more than the largest named lever.
  Five spans added (`gate.model_instance_id`, `gate.teardown`,
  `context.build_total`, `context.ctx_drop`, `l4_l5.role_scope_and_sort`) took
  those two to 2.9 ms and 0.2 ms of self time. **What it was: `gate.teardown`
  13.8 % + `context.ctx_drop` 2.8 % = 16.6 % of the run (14.9 s) is `free()`.**
  Falsified in passing: `gate.model_instance_id` (a SECOND full
  `discover_al_files` walk, the one named suspect) is **51 ms**, not seconds —
  the duplicate walk is a real but tiny redundancy, not a lever; and **d1 is off
  this list** — the "scoring 10.56 s / 13.9 %, third overall" figure this entry
  used to carry was measured before the d1 interning fix and is **1.8 %**.

  **DONE 2026-07-31 — the allocator swap** (ledger
  `docs/2026-07-31-allocator-swap.md`). That `free()` result pointed away from
  every structural lever, so `alsem` now installs `mimalloc` with
  `purge_delay = 0` (8 lines, `#[global_allocator]` scoped to that ONE binary).
  Paired A/B, alternating, 4 pairs on 8020 and 5 on DO: **8020 −41.2 %**
  (per-pair 0.605/0.637/0.571/0.563) with **peak 5,312 → 4,961 MB (−6.6 %)**;
  **DO −23.8 %** (0.733/0.720/0.762/0.794/0.775) with **peak 1,603 → 1,580 MB
  (−1.4 %)**. `gate.teardown` −87.0 %, `context.ctx_drop` −81.6 %.
  `purge_delay = 0` is load-bearing: plain mimalloc is a peak REGRESSION on DO
  (+7.4 %), and immediate purging costs ~7–9 % of the wall win to avoid it.
  **Caveat that now applies to every future entry in this track: a faster
  allocator makes an allocation-churn regression cheaper and therefore HARDER TO
  SEE. Keep counting allocations — the count is the part that stays legible.**
  Scope limits: library / LSP server / `aldump` / benches / tests keep the
  platform default; figures are Windows 11 (the least favourable case), CI is
  ubuntu/glibc and should show less.

  **DONE 2026-07-31 — cone fact keys are `Arc<str>`** (ledger
  `docs/2026-07-31-cone-arc-keys.md`). `merge_cone` took an OWNED `String`, so
  all **6,556,465** merges per 8020 run passed `key.clone()`. Now borrowed, with
  the `Arc` cloned only on insert: **6,435,078 heap key copies gone**, and a cone
  entry copied into N predecessor cones shares one allocation instead of holding
  N. `fact_cone` **1,976.8 → 1,783.0 ms (−9.8 %)**, 3/3 pairs negative against
  untouched controls flat within ±3 %; peak ≈ −17 MB. This is the caveat above in
  action — pre-mimalloc those 6.4 M copies would have been worth much more.

  **The re-ranked lever list (post-allocator, 8020, `release-fast`, ~35–37 s
  run).** Re-measure before picking: this list has now been wrong twice by being
  a profile out of date.

  | region (SELF) | ms | note |
  |---|---:|---|
  | `context.capability_cones` | ~8,200 | ~23 % — the dominant lever, twice the next item |
  | `context.compute_summaries` | ~4,100 | `solve_side_facts` residual below |
  | `preflight.fresh_coverage` | ~3,500 | CENSUSED 2026-07-31 — see below |
  | `gate.workspace_diagnostics` | ~1,330 | never yet censused |
  | `l3.parse_project_parallel` | ~1,310 | |
  | `search_loops_cohorts` (self) | ~1,270 | d1 |
  | `gate.format` | ~1,240 | 143,926 findings serialized |
  | `context.build_total` (self) | ~1,195 | the unspanned stretches of the context build |
  | `gate.teardown` | ~1,100 | was 12,430 |
  | `gate.project_filter_scope_baseline_suppress` | ~1,000 | 143,926 findings projected |
  | `l4_l5.role_scope_and_sort` | ~990 | see below |

  **MEASURED 2026-07-31 — `preflight.fresh_coverage`** (branch
  `perf/preflight-census`, ledger `docs/2026-07-31-preflight-census.md`).
  Attribution only, no fix. `src/program/` had **zero** spans, so the moat's own
  pipeline was one opaque number. **On DO — the real customer workspace — it is
  2,642 ms of a 3,171 ms run: 83.4 %.** `parse_snapshot` 1,139.2 ms (**35.9 % of
  the whole run**) › `resolve_full` 535.6 › `snapshot_build` 458.8 › `ctx_drop`
  248.7 › `dep_layer` 141.9 › `assemble_graph` 116.4. On 8020 it is 10.8 % and the
  ORDER INVERTS: `resolve_full` 1,838.4 leads, `dep_layer` is zero.
  **The two corpora have opposite shapes — this was never one lever.** DO has 551
  primary `.al` files and **11 dependency `.app` packages**; 8020 has 8,020 files
  and none. `parse_snapshot` parses every source-bearing app including deps, and
  BC 24+ `.app`s ship embedded source, so ~**95 % of DO's preflight parse is
  dependency source re-parsed from scratch on every run**.
  **The ceiling is in not redoing the work, not in making it faster** — the
  preflight returns FOUR SCALARS and then destroys the whole model:
  - [x] **DONE 2026-08-02 - `FreshCoverage` verdict cache** (ledger
    `docs/2026-08-02-preflight-cache-measurements.md`, spec
    `docs/superpowers/specs/2026-08-01-preflight-verdict-cache.md`). Paired,
    alternating, 3 pairs: **DO -68.2 %** (3,155/3,133/3,386 -> 1,003/1,031/1,052 ms),
    8020 -7.1 %; `preflight.*` -82 %/-87 %. Byte-identical cold, warm AND
    cache-disabled on both corpora. The two whole-run numbers differ because the
    preflight is 83.4 % of a DO run and 10.8 % of 8020 - the first lever in this
    track worth ~10x more on a real customer workspace than on the synthetic
    corpus every prior arc was tuned against. Original scoping below.
  - ~~**Cache `FreshCoverage` itself** on a workspace+dependency CONTENT hash.~~
    Ceiling on DO: the whole **2.64 s / 83.4 %** per warm hit, minus a deliberate
    `snapshot_build` (~459 ms) floor — the sound key derives FROM the snapshot, and
    a cheaper pre-snapshot key would mean a second discovery implementation that
    can drift from the real one. **Identical-input reruns only** (CI, a second
    `--format`, a no-op re-run): any primary edit is a guaranteed miss by
    construction, so this does NOT help the edit loop.
  - **Dep-parse artifact caching** — the edit-loop lever, and the bigger design.
    SIZED 2026-08-02, `docs/2026-08-02-dep-parse-sizing.md`. Q1/Q2 are settled;
    what remains is a go/no-go measurement. Verified at source that the resolve
    NEVER walks dep bodies (Phase 1 filters to the primary app + `ws_file_set`;
    Phase 2 is event flow over declarations; `DeclSurface::build` reads only
    `RoutineMeta::from_decl`) — so ~11,856 dep files are parsed in full and only
    their declarations are consumed. Saving on a hit ≈ **1,280 ms** on DO
    (`parse_snapshot` 1,139 + `dep_layer` 142), on EVERY run with unchanged deps
    — including the edit loop, which the verdict cache does not help.
    **Population: 11,165 dep objects / 126,640 dep routines** ⇒ a 40–60 MB
    artifact whose `serde_json` load plausibly costs 150–400 ms, a material
    fraction of the saving. **GO/NO-GO: measure a `DepLayer` round-trip first**
    (<~200 ms build it; ~600 ms don't, or switch to a compact binary encoding and
    re-measure). `AppRef` is per-run, so the artifact must store app identity
    symbolically and re-intern regardless.
  - **FALSIFIED 2026-08-02 — "lower declarations only, skip dep bodies".** The
    attractive no-cache version of the above. `al_syntax::parse` splits
    **74.6 % tree-sitter / 25.4 % lowering** on DO (21,293 / 6,867 CPU ms over
    11,856 files), so the ceiling is well under a quarter of `parse_snapshot`,
    against a change to the LOWERER — the only file in the repo that reads raw
    tree-sitter. A cache skips both halves and dominates it. Do not build this.
  - **NEW, cheaper, and exposed by that measurement: a LIGHT snapshot for the
    cache key.** `preflight.snapshot_build` is now the largest preflight item on a
    warm verdict-cache hit (~470 ms of DO's ~1,020 ms run), and most of it is
    `cached_source` loading ~11,856 dep source texts that a warm hit never parses.
    The key needs app identities + `.app` content hashes, not source text. This is
    NOT the "cheaper pre-snapshot key" the spec rejects — that rejection was about
    a SECOND discovery implementation that can drift; this is the same
    `load_all_apps`/`app_content_hash` path with source materialization skipped for
    dep units. Compounds with the shipped verdict cache.
  - Correctness constraint on both: the preflight is the "no silent clean" gate,
    so a cache must fail CLOSED — stale, missing, corrupt or unverifiable means
    recompute, never a reused verdict. Cache `Ok` only: `fresh` is a `Result` and
    its `Err` ("could-not-verify") captures TRANSIENT environment, so caching it
    would laminate an I/O flake into a persistent verdict.
  - **Corrections to this entry's first draft, both verified at source:**
    `AbiCache` (`abi_ingest.rs:121-135`) is a process-level in-memory
    `Mutex<HashMap>`, so constructing it fresh per call costs nothing across runs
    — it is NOT a cross-run lever, and its key is version-based rather than
    content-based, so persisting it as-is would be unsound. And
    `compute_gate_model_instance_id` (`model_instance_id.rs:82-88`) hashes file
    PATHS plus `guid@version`, never file CONTENT — unusable as a cache key; it
    would serve a stale verdict after any edit. The house pattern is
    `src/snapshot/cache.rs`, a live content-addressed cache (blake3 of the whole
    `.app`) that already caches dep source EXTRACTION across runs; what repeats is
    the ~11,600-file PARSE of that text.
  - **Two open questions gate the dep-parse lever** (design pointers in
    `docs/superpowers/notes/2026-08-01-preflight-cache-pointers.md`): (Q1) the
    preflight's `unknown` is primary-scoped but `coverage_holds`/`recovered_files`
    are WHOLE-PROGRAM, so "skip dep bodies" is a deliberate contract change to what
    the gate vouches for, never a silent side effect of caching; (Q2) event-
    subscriber wiring is whole-snapshot-scoped, so dep-derived edges may not be a
    pure function of the dep universe — if a dep subscriber can bind a primary
    publisher, a dep-only-keyed artifact is unsound. Settle both before speccing.
  - **Latent soundness bug found while scoping this, independent of any cache:**
    `SourceRoot.content_hash` (`snapshot/provider.rs:59-64`) folds only
    `f.text.as_bytes()`, concatenated — no `virtual_path`, no length prefix. Two
    workspaces that differ by a file RENAME, or by re-splitting the same bytes
    across different file boundaries, hash identically. The second is
    verdict-changing (the files parse completely differently). Consumers to audit
    before changing it: `snapshot/verify.rs`, `snapshot.rs:154-265`.

  **Still open in this track:**
  - **`l4_l5.role_scope_and_sort` — a comparator that allocates per COMPARISON.**
    Newly named by the attribution. It sorts **143,926 findings** (8020) with
    `sort_by(|a, b| compare_natural(&a.detector, &b.detector).then_with(||
    primary_location_key(a).cmp(&primary_location_key(b))) …)`, and BOTH of those
    build fresh heap values on every call: `compare_natural` runs `tokenize` twice
    (a `Vec<String>` each) and `primary_location_key` is a `format!`. That is
    ~2.5 M comparisons paying ~10–16 allocations apiece where N key builds would
    do. The fix is decorate-sort-undecorate; the detector-name component collapses
    to a `u32` rank over the ~54 distinct names (assign equal ranks to
    natural-EQUAL names so the rank is an exact monotone image of the comparator's
    preorder), and the location key must stay a STRING — `format!("{u}:{l}:{c}")`
    compared as text is NOT the same order as comparing `(u, l, c)` field-wise
    (`"a1:…" < "a:…"` because `'1' < ':'`). **`finding.rs:962` and `:1051` carry
    the identical comparator** on the r4 projection paths. Not built here; sized,
    not measured.
  - **`solve_side_facts`' remaining 2,519.9 ms** — still the largest item in the
    span. Its edge loop's residual is the 4,397,866 folds themselves: no longer
    allocating, but still hashing a key and walking a settled callee's WHOLE
    `uncertainties` vector once per external edge. Killing that needs the callee's
    propagatable set carried as an interned id set instead of re-folded per edge —
    the move `ctx.uncertainties`/`UncertaintyIndex` already made one layer up.
    Needs its own census round before it is sized.
  - **B1 — interned ids + bitsets.** SEQUENCE with the `str::to_lowercase()`
    census below (same call sites, one churn).
  - **The cone singleton walk** — PARTLY DONE 2026-07-31 (branch
    `perf/cone-singleton`, ledger `docs/2026-07-31-cone-singleton-census.md`).
    Census split it 61 % scan / 33 % fold / **0 % raw** (the raw path is dead
    under `ConeOutput::DerivedOnly`). **Not edge-bound**: 100,419 calls, only
    136,952 out-edges (1.36 each), but **12,170,325** cone entries scanned with
    **86.5 % winning** — so 10,522,793 `String` key clones per run. Borrowing the
    keys (`BTreeMap<&'g str, _>`) took `singleton` 4,214.4 → 3,592.6 ms (−14.8 %
    median; both controls drifted +10 % the other way, so read it as ≈ −10 to
    −20 %). **Still open, with populations already counted:** a single-cone fast
    path (calls split 46,373 zero-cone / 30,160 one-cone / 23,886 multi-cone —
    covers 4,158,131 of the 12.17 M entries, since a one-cone `best` is just a
    copy of a `BTreeMap` already in the right key order), and the fold's
    10,030,145 `fold_fact` calls, mostly `interner.intern(rid)` — the re-intern
    shape the uncertainty substrate fixed one layer up. Re-censused 2026-07-31
    post-allocator: `singleton` 3,339.7 ms of a 7,981.7 ms span, split
    scan 2,173.6 / fold 1,041.1 / raw 0.0; populations unchanged. Note the
    single-cone fast path removes the SCAN for those 30,160 calls but NOT the
    fold — killing the fold too needs a per-cone derived digest memoized at the
    cone, since a 1-cone call's derived contribution is a pure function of that
    one cone.
  - **B2 — SCC-shared cones** — RE-SCOPED 2026-07-31 by `ALSEM_CONES_CENSUS=1`
    (`a822da9`). `context.capability_cones` is ~12.9 s of ~66 s on 8020 and
    attributes to exactly TWO costs inside `compose_inherited_cones`
    (11.4 s): the per-routine singleton inherited walks (**4.8 s over 100,419
    calls**) and `fact_cone_for_scc` (**4.3 s over 65,822 non-root SCCs**). BFS
    is 0.9 s over 503 calls, and everything else — graph build, Tarjan, dedup,
    derived fold, coverage cone, record emit, summary assembly — totals ~1.5 s.
    **The SCC-structure framing this entry carried is FALSIFIED for this graph:
    `max_scc_members` is 126.** The 797-member SCC belongs to the COMBINED graph
    (d1's), not the typed-edge graph the cones walk, so there is no giant-SCC
    quadratic here to share cones against. Any fix targets one of the two costs
    above, and either alone caps at ~35 % of the span. DO's whole span is 96 ms,
    so this is a survive-Base-App item. Original text (now historical):
    8.34 M-cardinality summary mass, partially overtaken by C1's
    `ConeDerivedStore`; re-scope against the post-C1 peak
    before building — the largest remaining spans are now all L3-substrate
    (`l3.assemble_resolve` 3,381 MB, `l3.parse_project_parallel` 2,770 MB,
    `context.symbols_resolve_calls` 1,723 MB, `gate.coverage` 1,157 MB).
  - **B3 — single-substrate unification.** Needs a detector-feature parity
    harness first.
  - **d1 typed-receiver §7 guard-tag / flow-insensitive redesign** — a
    precision item, not a perf one, now that d1 no longer sets the peak.

  The change-impact wedge's effects-on-fresh fork still consumes B1/B2's bitset
  cones; the findings doc remains the evidence AGAINST making L3 load-bearing again.
- [x] **tree-sitter-al quirks list** — WAS ALREADY DONE, stale item (live-verified
  2026-07-17 against pinned v3.2.0 `14bd55c`): `statement_block`/`argument_list`/
  `parenthesized_expression` carry ZERO fields (left/operator/right pollution gone,
  fixed 2026-06-27/28 grammar arcs), `case_else_branch` HAS the `body` field
  (asymmetry fixed), member_trigger_name landed, spaced-preproc closed at v3.2.0.
  The grammar has no documented open limitations
- [x] **Multi-root LSP workspaces** — DONE 2026-07-17 (`6470e3e`). Per-root
  `ServerState` map (`Workspace`/`RootState`, each root gets its own `LspSnapshot`/
  updater/watcher/`DiagnosticsState`) + URI→root routing (`route_uri`, longest-prefix)
  for `dispatch_request`/`handle_notification`; `incomingCalls`/`outgoingCalls` route
  via a stamped `CallHierarchyItem.data` root marker instead (required, not cosmetic —
  `RoutineNodeId.AppRef` is a raw per-snapshot index, so the same id value can name a
  different routine in a different root). Single-root byte-identical (no marker/
  warnings ever emitted; the pre-existing dispatch test's assertions untouched). New
  follow-up surfaced by this work: `workspace/didChangeWorkspaceFolders` is NOT
  implemented — safe root removal needs an `AlFileWatcher` cancellation signal that
  doesn't exist yet (see `server.rs`'s module doc); the notification now warns loudly
  instead of being silently swallowed. Report: `.superpowers/sdd/multiroot-report.md`
- [x] **Snapshot-scoped LineTable cache** — DONE 2026-07-17. `ParsedFileEntry` gained
  a `OnceLock<LineTable>`-backed cache (rides the existing Arc-forwarding
  invalidation architecture, no new bookkeeping); `LineTable` moved from
  borrowing `&'t str` to owning `Arc<str>` so it can be stored. `incoming`
  ~5.82ms → ~4.30ms median on the 999-way-fan-in synthetic corpus (noisy
  machine — see `.superpowers/sdd/linetable-cache-report.md`); `dep_texts`
  (dependency-embedded-source) deliberately left uncached (smaller, rarer
  population). All perf_bounds gates still pass.
- [x] **Unicode-fold moat task** — DONE 2026-07-18. New choke point
  `al_syntax::{fold_identifier, eq_fold_identifier, IdentifierFoldExt}`
  (`crates/al-syntax/src/casing.rs`, `is_ascii()`-guarded simple 1:1 Unicode
  fold — byte-identical to `to_ascii_lowercase` for all-ASCII input, never
  `str::to_lowercase()`'s 1:n `İ`→`i̇`). Mechanically swapped every SEMANTIC
  identifier fold across `crates/al-syntax`'s lowerer, `src/program/`
  (production+lookup sides together, one commit), and `src/engine`+`src/lsp`
  — 3 commits, one per layer, each landing green. New fixture
  `tests/r0-corpus/ws-unicode-fold/` proves cross-case non-ASCII identifiers
  (Danish `Løbenr Mgt.`/`LØBENR MGT.`, German `Prüfung`/`PRÜFUNG`) now
  resolve via `Evidence::Source` — verified they would NOT under the old
  ASCII-only fold. North-star SHA guard: **unchanged**
  (`0a3b85bc832ff0a3e77acee118d203edbf62827dc37617c8d9315fe52d5cb7d0`, exactly
  as the investigation predicted — DO's primary source is 100% ASCII).
  Report: `.superpowers/sdd/unicode-fold-report.md`
- [x] **r3a4 source-bearing-dep pin hardening** — DONE 2026-07-17 (`8b5b4ec`):
  closure-membership assert added; the pin can no longer be vacated by a fixture edit

- [ ] **Multi-root follow-ups** (from the 6470e3e review): (a)
  `workspace/didChangeWorkspaceFolders` deferred — safe root REMOVAL needs an
  `AlFileWatcher` cancellation signal that doesn't exist (warns loudly today); (b)
  nested-root diagnostics overlap — two nested AL app roots can both publish
  diagnostics for the same URI (last-write-wins clobber); routing handles nesting,
  the publish side lacks URI-ownership arbitration. Both narrow; build when a real
  client hits them

- [ ] **`str::to_lowercase()` census in the advisory engine** (surfaced by the
  unicode-fold arc): ~364 sites across `src/engine/l2`-`l5` use full Unicode
  `to_lowercase()` (the 1:n-hazard primitive) as their own pre-existing convention —
  inconsistent with the new `fold_identifier` simple-fold choke point. One live
  interaction traced neutral-to-improving; population of divergent inputs is empty
  today. Migrate to `eq_fold_identifier`/`fold_identifier` layer-by-layer for
  consistency (low priority; advisory engine only)

- [ ] **perf_bounds `compute_all_within_bound` CI flake** (seen once, 2026-07-18,
  docs-only push; adjacent runs of the same code passed): magnitude bound lost to
  shared-runner load variance — the exact class the T3 arc fixed for rung bounds via
  interleaved complexity-class assertions. Give compute_all the same load-stable
  treatment if it flakes again

- [x] **L4 db-effect RSS consumer-migration — remove the analyze-path
  materialization shim** — DONE 2026-07-24 (`a0cd348`, Part B B1-migrate). The
  analyze path (`build_detector_context` + the cross-app builder) now consumes
  the LEAN bundle entry points, and the compact `SummaryBundle` rides on
  `DetectorContext.db_effect_bundle` so `db_effects` stays queryable without the
  per-routine `Vec<DbEffect>` expansion. MEASURED (8020, `release-fast`, full
  detector set): `context.compute_summaries` `rss_delta` 24 250 MB → **477 MB**
  (24 GB → 0.47 GB — the sub-GB target), span wall 87 s → 15.7 s, whole-process
  peak 39.9 GB → 18.1 GB, `analyze.total` 620 s → 366 s. The shim itself is
  UNCHANGED and still serves the projection + differential. Original item:
  (`src/engine/l4/summary_runner.rs`
  `compute_summaries_v2_bundle_with_leaves` → `_core`; the follow-up the L4 store
  redesign B1 explicitly deferred). B1 deleted the old Jacobi solver and flipped
  the differential to a frozen baseline, but the shim that re-expands the shared
  `EffectStore` into an owned `Vec<DbEffect>` per routine (so the returned
  `HashMap<String, RoutineSummary>` keeps the legacy shape) STAYS — the projection
  (`summary.rs::project_r3a2`) and the differential still need materialized
  `db_effects`. Measured cost (8020,
  `docs/2026-07-24-l4-dbeffect-store-8020-remeasure.md`): `context.compute_summaries`
  ~87 s + ~24 GB peak RSS is dominated by this shim, and the analyze path never
  READS `RoutineSummary.db_effects` (detectors consume only `.uncertainties` /
  `.parameter_roles` / capability facts — verified grep). Migrate the analyze path
  (`detector_context` / `gate`) to the bundle's borrowing view + the A4
  `ReverseEffectIndex`, keeping a materializing path ONLY for the projection +
  differential. Expected: −24 GB, `compute_summaries` ~87 s → ~13 s. No wake
  condition — buildable now.
- [x] **C1 — `context.capability_cones` base-assembly RSS** — DONE 2026-07-24
  (Tasks 1–4, plan `docs/superpowers/plans/2026-07-24-c1-cone-derived-substrate.md`;
  see the CHANGELOG `Changed` entry for the full shape). Diagnosis:
  `.superpowers/sdd/C1-cones-diagnosis.md`; residual attribution:
  `.superpowers/sdd/c1-residual-census.md`. The compact `ConeDerivedStore`
  replaced the per-routine raw inherited-fact Vec on the analyze path
  (`ConeOutput::DerivedOnly`), and the SCC walk stopped materializing cones no
  predecessor will ever read — it is Task 4's code change (not building a root
  SCC's cone at all) that takes the root-SCC residual to 0; the
  `C1_CONE_CENSUS=1` byte census only MEASURES that it stayed at 0, it did not
  cause it. MEASURED (8020, `release-fast`, `d8-commit-in-transaction`-only
  shape, EXITCODE=0): span `rss_delta` 10 941 MB (pre-C1) → 2 151 MB (Task 3) →
  2 195 MB (Task 4); whole-process peak 17 055 MB → 9 593 MB (Task 3) →
  **7 787 MB (Task 4)**; wall 213 s → 196 s → 127 s. The span's own `rss_delta`
  barely moved Task 3→4 despite the ~1.8 GB peak drop: `rss_delta` is working
  set at span end minus span start, and the root cones were always freed
  inside the span either way (when `compose_inherited_cones` returned) — they
  lived in the PEAK, which is where Task 4's saving shows up, not in the
  delta. Output byte-identical throughout (five golden families + l4
  differential 17/17 + DO `analyze` and `policy check`). Post-Task-4 the
  largest remaining spans are all OUTSIDE this arc — `l3.assemble_resolve`
  3 381 MB, `l3.parse_project_parallel` 2 770 MB, `context.symbols_resolve_calls`
  1 723 MB, `gate.coverage` 1 157 MB — so the cone span is no longer the
  dominant consumer, but the arc did NOT reach sub-GB whole-process: the
  remaining ~7.8 GB peak is L3-substrate work, not reachable by B1 + C1 alone.
- [ ] **A future incremental L4 path over the new `EffectStore`** (a redesign, NOT
  a re-port of the deleted R3b). The reusable design intent — fine-grained Salsa
  query topology, SCC-identity rules (interned sorted-member `SccKey`), fixed-leaf
  successor handling, deterministic sorted member-order, the demand-order /
  DB-provenance / fixpoint-schedule / `RUST_HASH_SEED` nondeterminism invariants,
  and the strict-subset minimal-invalidation fixtures — is preserved in
  `docs/superpowers/notes/2026-07-24-r3b-incremental-l4-design-intent.md`. Wake: a
  real incremental-analyze consumer.
- [ ] **`salsa` derive-only dependency** — after the R3b `incremental/` deletion
  (Task B1), nothing in the crate consumes `#[salsa::db]`/`#[salsa::input]`/
  `#[salsa::tracked]`/`#[salsa::interned]` any more; the only surviving usage is
  `#[derive(salsa::Update)]` on a handful of `l4` types (`summary.rs`,
  `combined_graph.rs`, `scc.rs`, `capability_cone.rs`). A full salsa-ectomy
  (drop the derives + the `Cargo.toml` dep) is a legitimate future cleanup — not
  urgent, no correctness or perf stake, just dead-weight-dependency hygiene.
  Wake: convenient piggyback on an unrelated touch of those types, or a
  dependency-audit pass.
- [ ] **L-3 globals sharing is retained-only — the transient build-then-discard
  allocation cost survives** (`feat/l3-substrate-and-parked-items` Task 5 /
  lever L-3; `task-5-review.md` finding F1). `ir_variables`
  (`src/engine/l2/ir_walk.rs:2189`) still materializes a FRESH copy of every
  object global — 2 heap `String`s plus a `PAnchor` carrying 2 more — for
  EVERY routine (the ~6 M small allocations §3.3/§5 L-3 counted on), and
  `project_file` (`src/engine/l3/l3_workspace.rs:1038-1050`) immediately
  filters them back out and drops them before constructing
  `RoutineVariables`. The RETAINED cost is genuinely gone (Task 5's measured
  ~540 MB peak drop is real, not an artifact, **on the 8020 corpus** — real
  BC customer workspaces replicate far less: CDO/DO measures 16.8x global
  replication against 8020's 56.4x, a ~5.29 MB win there vs. 8020's
  ~434.97 MB, see `scope-l3-substrate.md` §8 and the CHANGELOG's L-3 entry);
  the TRANSIENT churn is not, and is the most likely explanation for the
  scoped ~700 MB vs. measured ~560 MB shortfall (also 8020-scale).
  ⟨final-branch-review-l3.md M-6⟩ **The ~540/~560 MB figures themselves rest on
  one inference, not a Task-5-isolated re-measurement**: the "before" baseline
  used for them already sits downstream of Tasks 3 and 4's id-schema change, so
  attributing the full drop to Task 5 assumes T3/T4 moved the peak by ~0 (both
  are same-length SHA-256-hex substitutions — reasonable, not idle, but still an
  assumption) rather than toggling Task 5 off against an otherwise-identical
  build, the way Task 3's own re-freeze isolates its own change. **Not a
  defect on its own** — L2's `PFeatures.variables`
  is a serialized golden surface (`tests/ir-l2-goldens/l2_features.snapshot`,
  `tests/r1a-vectors/l2-vectors.json`) and must keep its full flat
  params→locals→globals form; L-3 was scoped as "never-build" for the L3
  assembly path specifically, not for L2's own projection. On the
  `analyze`/L3-assembly path, the only remaining consumer of the
  flat-with-globals list is `innermost_scope_by_lc`
  (`src/engine/l3/l3_workspace.rs:1136-1142`), which is derivable first-wins
  straight from `RoutineVariables::iter()` (`own` already precedes
  non-shadowed `globals` in that exact order) without ever building the flat
  L2 form. A globals-free `ir_variables` variant reserved for the L3
  assembly call site (L2's serialized projection keeps calling the existing
  full-globals path, unchanged) would collect the rest of the scoped L-3
  win. **Wake:** the next L3-substrate memory pass (e.g. sizing L-5 for real),
  or opportunistically alongside another `ir_variables`/`ir_walk.rs` touch.

## Parked — deferred WITH evidence; do NOT start without the wake condition

- [x] **`ctx.uncertainties_by_node` — Step 2: intern the ELEMENTS to ids** — DONE
  2026-07-31, branch `perf/d1-followups`, plan
  `docs/superpowers/plans/2026-07-31-uncertainty-identity-substrate.md`. Delivered as a
  consequence of an IDENTITY correction rather than as a memory task: the context-build
  hash-cons pool already computed set identity by content and then discarded it, so d1
  re-derived it per run (2,229,391 interns into a detector-local table, memoized behind a
  RAW POINTER key needing an `Arc` keep-alive to be sound) and `d1_reach` answered
  "does this node carry uncertainty" with a `HashMap<String, _>` lookup.
  `UncertaintyIndex` now mints `UncertaintyId` per distinct value and `UncertaintySetId`
  per distinct set; `uncertainties_by_node` carries set ids; every consumer reads one
  `UncertaintyView`; `UncertaintySetPool`, `d1_cohort::UncertaintyTable` and
  `d1_cohort::UncertaintyId` are all DELETED, and with them the pointer key and its
  soundness argument. Its precondition — the missing d2 golden — was built first
  (`c16c552`) and PROVEN to fail (deleting d2's `dedupe_uncertainties` makes it red with
  4 evidence entries instead of 2, nothing else moving).

  **MEASURED (`ALSEM_UNCERTAINTY_CENSUS=1`, which prices both shapes from one run):**
  8020 **74.5 MiB → 17.4 MiB, −57.2 MiB**, over the same populations this entry recorded
  (27,037 nodes / 19,311 distinct values / 10,112 distinct sets — an independent
  cross-check). **NOT the −88 MiB predicted here**, and the difference is accounting, not
  disagreement: the 102.0 MiB baseline above came from an allocation-tracking census
  (allocator bytes, `Arc` headers, rounding over 10,112 buffers) while the new census is a
  by-hand byte count that models none of that and prices the same data at 74.5 MiB. The
  real saving is therefore larger than −57.2 MiB; −88 MiB is not claimed as delivered.

  **The trade this entry did NOT record: on a small workspace the substrate costs MORE.**
  DO measures 0.4 MiB → 0.8 MiB (**+0.4 MiB**) — 777 distinct sets over 1,808 nodes leaves
  nothing to amortize, so the value table, the precomputed keys/lites and the `by_value`
  reverse map exceed the records they replace. The entry said "survive Base App, not a
  customer-workspace win"; the sign is actually negative there.

  **Next, if this is ever squeezed again:** `by_value` holds a second copy of every value
  (2.8 MiB on 8020) and `by_set` a second copy of every id slice (2.0 MiB). Both are
  interner-BUILD scaffolding, never read after context build.


- [ ] **`FindingConfidence` carries ids, not records (−78 MiB retained in d1).** The
  finding already holds the winner cohort's `Vec<UncertaintyId>`; carrying 4-byte ids and
  materialising `Evidence` at the consumer takes d1's `confidence` bucket from 115.0 MiB
  to ~37 MiB and lands its retained total at the ~110 MiB the scoping band actually
  predicts. Three readers (`project_evidence`, `merge_confidence`,
  `format_policy::finding_to_jv`) plus keeping the `UncertaintyTable` alive to projection.
  Scoped in `.superpowers/sdd/task-3-review.md`; deliberately not built in the d1 arc.
  **Wake: when d1 memory is worth another pass.** Two things changed 2026-07-31 (branch
  `perf/d1-followups`) and both make this EASIER, not done:
  - The blocker is gone. This was awkward because ids were d1-LOCAL and died with the
    detector; they are now run-global (`ctx.uncertainties`), so a `FindingConfidence`
    holding ids no longer needs a table shipped beside it. The three readers named above
    are unchanged.
  - **Its gate is UNEVALUATED, not passed.** The plan gated building this on d1's retained
    `confidence` bucket still being near the ~115 MiB predicted here. That could not be
    checked: the d1-memory arc's retained-`DetectorOutput` census was a one-off probe and
    no longer exists in-tree — the only censuses that survive are
    `ALSEM_UNCERTAINTY_CENSUS` (the ctx substrate), `ALSEM_D1_SCORING_CENSUS` and
    `ALSEM_TXSPAN_CENSUS`, none of which measure it. **Re-measure d1's retained output
    before building; do not code to the ~115 MiB / −78 MiB prediction, which is now two
    arcs old.**

- [ ] **Residual duplicate-id groups: 15 groups / 19 routines on BC Base App (0 on
  DO) still share BOTH the internal and the stable routine id.** ⟨task-4-review.md
  finding M-3, fix wave⟩ Promoted out of the CLOSED `compute_routine_id` entry
  below — a reader scanning this section for open work used to meet a closed item
  first, with the genuinely-open residual buried as one of its sub-paragraphs. The
  member-discriminator fold that entry describes (closing DO's 262 collision
  groups to 0 and 8020's 3,058 to 15) leaves two shapes a FLAT enclosing-member
  string cannot separate: **13 XMLport same-name `fieldelement` members at
  different nesting paths** — a flat member name cannot separate a nested XMLport
  tree — and **2 preproc `#if`/`#else` alternatives** of one member, an artifact of
  the deliberate union-read preproc design rather than of the discriminator defect
  (one of those two is a codeunit-level procedure with no enclosing member at all,
  so no member discriminator could ever separate it).

  **Wake:** a consumer that must tell two same-named XMLport elements apart — at
  which point the fix is a path-qualified member (or a declaration ordinal) in the
  same conditional position, with the same shape constraint the closed entry's id
  schema already established. Until then the fail-closed handling stands:
  `detector_context`'s skip-on-drained-map branch, d1's
  `edge_target_matches_callsite_callee` guard, and the inventory's THREE-key sort
  (`inventory_row_cmp`'s `sort_by` in `build_inventory_doc`: primary
  `stableRoutineId`, secondary `case_insensitive_compare_opt` on `enclosingMember`,
  tertiary `locale_compare` on `originatingObject`) — all three kept deliberately
  and each with a test that states its precondition executably:
  `detector_context::tests::hand_stated_id_collision_keeps_a_real_summary_and_derived_row`,
  `gap_g18_transitive_loop.rs`'s (d)/(e), and — for the inventory sort — BOTH
  `snapshot_full::tests::hand_stated_collision_discriminates_by_member_case_insensitively`
  (secondary key) AND
  `snapshot_full::tests::hand_stated_collision_discriminates_by_originating_object_when_member_also_ties`
  (tertiary key), respectively (⟨task-4-review.md finding I-2⟩, fix wave — the
  first of those did not exist at T4: its predecessor,
  `member_tie_break_is_case_insensitive_and_none_first`, pinned the comparator
  function but not the sort's actual use of it, so deleting the tie-break's
  `.then_with` call from `build_inventory_doc` left every test — and every golden
  — green; ⟨final-branch-review-l3.md finding I-3⟩ then found the tertiary key had
  no coverage of any kind — not function-level, not use-level, no golden — because
  reaching it needs two rows agreeing on BOTH `stableRoutineId` AND
  `enclosingMember`, which the T4 fix wave's test does not construct; the second
  test closes that gap the same way, by hand-stating the collision one key
  further).

- [x] **CLOSED 2026-07-25 (T1 + T3 + T4) — `compute_routine_id`
  member-discriminator gap: colliding same-name triggers shared ONE id.**
  `compute_routine_id` (`src/engine/l2/scope.rs`) keyed
  app/object-type/number/kind/name/signature with NO member
  discriminator, so two same-name same-signature triggers in one object (e.g.
  any page with two actions each declaring `trigger OnAction()` — ordinary in
  real BC) collided on one routine id. This is the SAME collision family as
  `docs/engine-gaps.md`'s **G-18** (which fixed a different symptom — d1's
  cross-body loop misattribution — and correctly remains marked FIXED).
  Kept here in full because the three tasks' measurements are the evidence
  behind the current id schema, and because the honest residual below is a
  live wake condition.

  **MEASURED, 2026-07-25** (`.superpowers/sdd/scope-routine-id-collision.md`),
  correcting this entry's own earlier "handful of routines" framing by three
  orders of magnitude:

  | corpus | routines | collision groups | routines erased by the collapse |
  |---|---:|---:|---:|
  | DO (`DocumentOutput/Cloud`) | 4 842 | 262 | **1 157 (23.9 %)** |
  | 8020 (BC Base App) | 100 941 | 3 058 | **16 906 (16.7 %)** |

  Largest group: 17 routines on one id (DO), 100 (8020). Every colliding
  routine is a member trigger; 98 % of groups carry real call graph.
  `enclosing_member` is already in the model at every `compute_routine_id`
  call site and closes DO to **0** residual groups, 8020 to 15 groups / 19
  routines (preproc `#if` alternatives + XMLport same-name elements at
  different nesting paths).

  **DONE (T1, 2026-07-25): the cone-LOSS symptom is fixed.**
  `build_detector_context` drained its cone maps with `remove()`, so a later
  occurrence of a colliding id wrote a fully degenerate summary over the real
  one and `ConeDerivedStore::forget` dropped the matching derived row — the
  whole cone of an id shared by N routines was erased.
  (`build_detector_context_cross_app` reads with `get()` and never had the
  accident, which is what identified the drain as an accident rather than a
  decision.) The builder now skips the later occurrence instead; `forget` is
  deleted. Measured on DO: **+4 `d8-commit-in-transaction` findings, −1
  `d9-transaction-span-summary`, 20 findings changed in place** — see
  `.superpowers/sdd/task-1-report.md`. Zero golden movement.

  **DONE (T3, 2026-07-25): the INTERNAL id schema is fixed.**
  `CanonicalRoutineKey` gained `enclosing_member: Option<String>` and
  `encode_canonical_routine_key` appends it as a 7th `sha256_of_strings` part
  **only when a member exists**, so procedures, object-level triggers and every
  dependency-ABI routine (the dep projection passes `None`) keep byte-identical
  ids and the cross-app join stays symmetric by construction. The id's SHAPE is
  unchanged — `routine_id_shape_is_two_parts_with_64_hex_regardless_of_member`
  pins it independently of its content. Measured closure: **DO 262 collision
  groups → 0; 8020 3 058 → 15 groups / 19 routines (0.019 %)**, the residual
  being 13 XMLport same-name `fieldelement` members at different nesting paths
  (wake condition for a path-qualified member, if a consumer ever needs one)
  and 2 preproc `#if`/`#else` alternatives (an artifact of the deliberate
  union-read preproc design, not of this defect). 20 goldens moved, all
  id-shaped — 84 lines, 6 distinct ids, each re-derived as (6-part hash) →
  (6-part + lowercased member), every masked run in `{modelInstanceId}/`
  position; no fact content or fingerprint moved. (`git diff --stat -- tests/`
  says 22: it also counts 2 hand-edited `tests/l2_ir/*.rs` sources.) DO
  findings 2 387 → 2 369: −14 d34 / −3 d45 / −3 d8 / −2 d9 (cone shrink, the
  predicted direction — cross-body splices removed) and +3 d1 / +1 d3
  (previously unaddressable sibling BODIES becoming real routines). See
  `.superpowers/sdd/task-3-report.md`.

  **DONE (T4, 2026-07-25): the STABLE id schema is fixed — option (a3), and
  the parked entry closes with it.** `to_stable_routine_id_from_parts` takes
  `enclosing_member: Option<&str>` and, when `Some`, replaces the hash part with
  `sha256_of_strings([normalizedSignatureHash, member_lowercased])` — the same
  ONE canonical normalization (`ir_enclosing_member`) at all five construction
  sites, `None` on the dep-ABI side. `None` reproduces
  `{stableObjectId}#{normalizedSignatureHash}` byte for byte; the SHAPE stays
  `{stableObjectId}#{64 lowercase hex}` (folded into the hash, never appended as
  a segment — an appended `#member` would have defeated `alsem diff`'s join,
  `stable_sub_id`'s two-part split and `DepIdStabilizer`, moving EVERY
  fingerprint in the product), pinned independently of its content by
  `stable_routine_id_shape_is_object_plus_64_hex_regardless_of_member`.
  Measured on DO: **71 of 2 369 findings (3.00 %) get a new fingerprint and
  2 295 of 2 362 baseline fingerprints (97.16 %) still match** — against the
  scoping doc's 81/2 384 (3.4 %) prediction, made before T1/T3 moved the
  population. **Zero findings appeared or disappeared**; every detector count is
  identical. **Duplicate-fingerprint excess 7 → 3**, closing all four
  sibling-`OnAction` groups (including `b2d1b142f0577a38` and
  `47500c86760f3f93`). 50 goldens moved, every one explained order-insensitively
  by the stable-id substitution (42) or by it plus the 4 intended fingerprint
  moves (8); the 6 stable ids and 4 fingerprints were re-derived independently
  in Python; the mask was POSITIONAL and confirmed that zero bare 64-hex values
  (`normalizedSignatureHash` / `signatureFingerprint`) and zero
  `{modelInstanceId}/`-prefixed internal ids changed value. `scripts/cdo-gate`
  with `CDO_WS` set: PASS, and the frozen CDO L4 whole-program digest did **not**
  move. See `.superpowers/sdd/task-4-report.md`.

  **The honest residual, and its wake condition** — promoted to its own open
  entry directly above this one (⟨task-4-review.md finding M-3⟩, fix wave):
  15 groups / 19 routines on BC Base App (0 on DO) still share both the
  internal and the stable id.

  **Historical (superseded by T3 above) — what the collision cost.** With one
  id per N siblings the
  surviving DIRECT facts were still ONE arbitrary (last-sibling-wins) sibling's.
  The surviving INHERITED cone is NOT one sibling's view: the combined graph
  files every sibling's out-edges under the one shared `from` key, so the cone
  walk consumes their union — `(last sibling's direct facts) ∪ (cone over the
  union of ALL siblings' callees)`, an over-approximation. This explained why
  the +4 d8 findings above were genuine rather than accidental (their manager
  qualification rode the union) and predicted the direction of the
  member-discriminator fix's own diff: de-colliding ids SHRINKS each sibling's
  cone back to its own callees, so some of those +4 d8 findings would disappear
  again if a manager's `writes_physical_tables_count_of >= 3` was only met by
  the union — expected movement, not a regression.
  **T3 CONFIRMED that prediction exactly**: 3 of the d8 findings went away,
  including the `CDO Move Logs` one this entry called out by hand (it anchored
  on line 212, `UpdateStatusAction`'s trigger, while its `Commit()` sits at line
  188 in the sibling `StartmovinglogsAction` — `routine_by_id` had resolved the
  last sibling and the op-site lookup fell back to its declaration anchor). The
  d9 twin now anchors correctly on line 142 with 2 tables instead of the
  union's 5, which is precisely why d8's `>= 3` threshold stops being met.
- [ ] **`d55-event-publish-in-loop`'s `rootCauseKey` omits the callsite, so
  several publish sites in ONE loop share a fingerprint** — `d55.rs:73` builds
  `d55/{routine.id}/{loop_info.id}` while the finding `id` additionally carries
  `/cs{n}`. Measured on DO (2026-07-25, after T4 closed every id-collision
  duplicate): this is now the **entire** remaining duplicate-fingerprint
  population — 2 groups / 3 excess, `CDO Payment Link Handler.al:43,51`
  (`/cs9`, `/cs18`) and `CDOEMailJob.Table.al:254,257,464`
  (`/cs6`, `/cs7`, `/cs86`), each group inside a single routine with a single
  internal id. **Not an id defect** — the ids are correct and distinct; the
  detector's key is coarser than its findings. Two coherent fixes: include the
  callsite in the key (one finding per publish site, distinct fingerprints), or
  emit ONE finding per loop (matching the key). The second is arguably what the
  message text already implies. **Wake:** a user reporting that baselining one
  d55 finding silently suppressed a second publish in the same loop, or the next
  deliberate fingerprint-moving change (piggyback — the cost is the same class as
  T4's 3 %).
- [ ] **Preflight shared parse — RE-SIZED 2026-07-31 to ~54 ms; effectively dead.**
  ⟨`docs/2026-07-31-preflight-census.md`⟩ The premise below is CONFIRMED (deps parse
  once, so only the primary app's parse is duplicated) but the sizing was three
  orders out. 8020 parses 8,020 files in 786.4 ms = **0.098 ms/file**, so DO's 551
  primary files are **~54 ms**, not a "sub-second saving". The dep-source parse —
  ~95 % of DO's 1,139 ms `preflight.parse_snapshot` — is NOT duplicated and is not
  reachable by sharing a parse; it is reachable by CACHING one (see the Track B
  preflight entry). Do not build this; build the cache. Original text: measured
  2026-07-17: duplicated work is the PRIMARY
  app's parse only (deps parse once in the fresh pass); on DO that's 407 files of a
  dep-dominated 4.8 s resolve → sub-second saving. Live BOM divergence (DO has 4
  BOM-carrying `.al` files; snapshot keeps BOM, L3 strips) makes naive sharing
  behavior-changing. Investigation: `.superpowers/sdd/shared-parse-investigation.md`.
  **Wake:** analyze latency becomes user-facing pain, dep-parse caching lands, or BOM
  handling gets unified anyway
- [ ] **FreshCoverage ABI-error / missing-dep reporting** (+ serde-default-empty
  exemption hardening) — population-less on DO (0 ingest failures, 0 declared-but-
  missing; real ingest failures already surface as could-not-verify). **Wake:** the
  first real failing-ingest or missing-declared-dep population, or a SymbolReference
  emitter shape change
- [ ] **Number-less object identity collision (engine-wide)** — `o.id.unwrap_or(0)`
  (`src/engine/l2/l2_workspace.rs:355/414/593`) gives every Interface/ControlAddIn in
  an app the id `{guid}/{type}/0`. Harness symptom fixed; harm latent (DO: 5
  interfaces share one id, zero shared routine names → no routine-id collapse
  observed). Fix is a stable-id earthquake (fingerprints/baselines/digests/cache).
  **Wake:** two same-app number-less objects sharing a routine name, a misattributed
  production finding on an interface, or the next planned stable-id break (piggyback)

## Parked — call-graph roadmap (doctrine-deferred, population-less)

- [ ] ProvenAbsent — wake: a real proven-absence population (MemberNotFound is 0)
- [ ] Implicit conversions — wake: nonzero `ambiguousResolved` (currently 0)
- [ ] Full ParseStatus gate — wake: the first absence-claiming consumer
- [ ] Protected `Variables[]` — wake: an extension routine consuming a base protected var
- [ ] Preproc-symbol fidelity — wake: a real consumer
- [ ] Sender param-TYPE drift analysis — wake: a version-drifted-closure corpus

## Product direction (post-1.0 — needs a brainstorm session, not a dispatch)

- [ ] **Change-impact wedge** — the charter's headline product feature ("what breaks
  if I change X" over the zero-unknown whole-program graph). Brainstorm input +
  substrate map + the 8 open design forks:
  `docs/superpowers/notes/2026-07-18-change-impact-wedge-brainstorm-input.md`
  (its file:line substrate map is a `b7da82d` snapshot — re-verify after any refactor;
  the product framing is refactor-independent). Biggest architecture fork: effects-on-
  fresh vs re-consuming the advisory L4 layer

## Separate track

- BC-Brain — its own product backlog (`SShadowS/bc-brain`), never mixed into this list.

---

## Archive — completed (compressed; details in CHANGELOG + git log)

2026-07-17, outstanding-sweep run:
- [x] Push master to origin (113 commits, `e6b1283..d695392`; then continuously)
- [x] Differential-harness identity keying + wrong IEmpty fingerprint golden
  (`fix/outstanding-test-bugs`; "flaky" claim falsified — was deterministic-wrong)
- [x] gate_sarif regen-mode anti-degenerate bypass (`819790d`)
- [x] condition_references consumer audit — CLEAN, no consumer bitten
  (`.superpowers/sdd/condref-audit-report.md`)
- [x] d56 re-promotion OPT-IN → DEFAULT via keyRemappedClone analysis (`752a496`;
  DO: 0 findings, both real key-remap sites verified excluded)
- [x] MERGE-TIME CRLF re-materialization on master (552 files; detection law: use
  `file`/`od`, never grep — MSYS grep strips CR)
- [x] Stale-section corrections: deep-review T0-T4 ALL merged long ago (T2 `542740e`,
  T3 = the LSP-migration arc incl. legacy-pipeline deletion, T4 `d99c65e`); both
  Recovered-parse grammar defects fixed at grammar-defects-and-repin
  (`recoveredFiles` re-measured 0 on CDO)

2026-07-17, preflight-fresh-coverage arc (`d14cf84`):
- [x] §1 preflight fix — analyze preflight re-keyed to the fresh resolver
  (FreshCoverage + could-not-verify state + fail-closed hole + empty-ABI exemption);
  DO warning gone, totalFindings 2307 exact, north-star SHA byte-identical

2026-07-16/17, BCQuality detector wave (`8bb9756`):
- [x] 13 detectors d52–d64 + `bcquality` preset; FP triage on DO; root-cause fixes for
  d53/d56/d60/d63 (only d56 was temporarily opt-in, since re-promoted)

## d1-db-op-in-loop cohort redesign — RESOLVED 2026-07-21 (merged ee3aa45)

The perf-optimization-handoff §4 "open decision" (d1 output semantics / no-caps)
is CLOSED. d1's exhaustive path-enumeration walk — which took ~8h on Base App
8020 and was always killed — was rewritten as an IFDS/reachability-indexing
COHORT DATAFLOW (co-designed with gpt-5.6-sol; see memory `d1-output-bound-falsified`
§2026-07-21 for the full design + the 24-commit arc C1-C9 + cleanup):

- Per-loop reachability computed once as bitmap COHORTS (Terminal -> ContextKey
  -> loop-bitmap), not per-loop re-traversal. Compressed terminal-centric output
  (interned loop-sets + loop catalog + ONE bounded representative witness per
  verdict-class). Reuses d1_graph/d1_liveness/d1_temp; the old walk path is now
  cfg(test) as the differential oracle.
- **8020 now FINISHES** (~26min total, d1 detector ~140-250s, machine-noise-
  dependent; pure compute floor 9.35s). DO (real customer ws) BYTE-IDENTICAL on
  all identity fields at 6-7s. Correctness proven: decompressed cohorts == old
  (loop,terminal,verdict,depth_bucket,unc) tuples + reachable_verdicts (differential
  + regenerated goldens + DO diff). Whole-branch review clean.
- Output shape CHANGED (user-approved): compressed cohorts vs old per-loop contexts;
  pathCount now counts verdict-classes.

### Deferred d1 follow-ups (non-blocking; 8020 already finishes)

> **STATUS 2026-07-31: items 1-3 are DONE** — 1 and 2 on branch
> `perf/d1-followups` (`12f7e95`, `d92df42`), 3 as `286814d`. Items 4 and 5 remain
> open and unchanged. Item 1's premise was FALSIFIED before it was built: a census
> showed its walk is 7.5 hops, never the cost; the cost was 92,054,600 per-run
> string interns, and the fix that followed took `d1.cohort/scoring` from
> 10,568 ms to 1,105 ms. Item 2's whole ceiling turned out to be ~1.2 s and its
> timing is explicitly not claimed. Read the entries below as the pre-arc
> reasoning they were, not as live scoping.
>
> **These five were written 2026-07-21, BEFORE the d1 memory arc (`6e136e2`) and the
> uncertainty substrate (`93bb9af`). Their sizings are pre-arc and several are known
> stale — re-measure before building any of them.** Same-run traced spans after the
> memory arc: `detector.d1` **67.65 → 25.90 s**, `assemble_cohort_findings`
> **6.27 → 1.59 s**; d1 no longer sets the process peak (first exceeded at
> `detector.d19-unused-parameter`), so a saving that removes only a d1 *transient*
> no longer moves the user-visible peak at all.

1. **Witness/uncertainty polish** — `build_cohort_rep`'s full-chain
   `path_uncertainty_ids` walk for UNCERTAIN cohorts (certain cohorts already skipped,
   ee07983). Eliminate by accumulating uncertainty-KIND-SETS in the fixpoint (no walk)
   — output-identical. **The "~130 s residual / targets d1 ~10-30 s" sizing is STALE**:
   it was written when d1's own span was ~140-250 s; the span is 25.90 s now, so the
   whole remaining envelope is smaller than the claimed saving. Re-attribute against a
   current trace first. NEEDS A QUIET MACHINE either way (detached 8020 runs swing
   ±80 s; sub-fixes are unmeasurable against that noise).
2. **`affected_objects` bitmap-partition** (d1.rs ~1807): a `bm.iter()` loop over the
   ~3.2M (loop,terminal) population, same shape C9 bitmap-partitioned for `by_rv`.
3. ~~**`finding.rs` LoopContext/StableLoopContext cleanup**~~ — DONE 2026-07-31
   (`286814d`). `Finding.contexts`, `StableFinding.contexts`, `StableLoopContext`
   and `project_loop_context` deleted; `LoopContext` survives `#[cfg(test)]`-only
   for the shadow oracle, which now returns `(Finding, Vec<LoopContext>)` rather
   than stuffing contexts into the finding — that kept the two tests asserting
   per-loop context ORDER alive instead of deleting them with the field. The
   cutover guard `assert!(f.contexts.is_none(), …)` was deleted WITH its reason
   in place: the field is gone, so the state it guarded is unrepresentable.
   **Scope was 71 sites across 63 files, not the 139 this entry's discussion
   assumed** — that number matched `contexts: None` as a substring and counted
   every `cohort_contexts: None`, the field that stays. Byte-identical on both
   corpora, zero golden movement.
4. **Global-arrival-cohort solver** (the "full" redesign, deferred): only if the
   FIXPOINT ever becomes the bottleneck (currently 9.35s, fine).
5. **Depth-semantics**: the 22,511 reached terminals include deep-chain findings
   (arguably spurious SCC artifacts from no-depth-bound); a depth bound would cut
   count + improve precision, but the user chose no-caps. Revisit only if the deep
   findings prove noise on real triage.

MEASUREMENT GOTCHAS (recorded so a fresh session doesn't re-learn them):
`ALSEM_TRACE_DETAIL=hot` ALONE (not `stages,hot` — parse falls to Stages, gating
off Hot counters). Detached `Start-Process`+sentinel (logs/run-det-d1only.ps1)
survives the harness reaping background bash at ~30-90min. span names are BARE not
cat-prefixed; `serde_json` sorts object keys (grep individual keys).
