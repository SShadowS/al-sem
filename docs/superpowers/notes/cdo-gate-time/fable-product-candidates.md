# TLS-16 round 2: product speed candidates (Fable)

Read-only, 2026-11-11. Nothing was built, run or traced. Each claim is marked:
**[code]** I read it in the source, **[log]** it is in a gate log or a doc, **[inference]**
I reasoned it from those and it needs the stated measurement.

## The numbers used as pointers

From `speed.log` of run `20261111-003611` (quiet, load 10; the 66-83 s in the brief are
the busy run `052939`):

| call (resident, release-fast) | wall | CPU | CPU / wall |
|---|---|---|---|
| first ask (`r0 build`: model, fold, first explain) | 35-41 s | 154-173 s | 4.4 |
| test universe (`warm-tests`) | 18-19 s | 133-136 s | 7.3 |
| `prove may-commit`, first call on a routine | 12.7-15.8 s | same | 1.0 |
| `impact`, one routine, warm | 1.0-1.5 s | same | 1.0 |

One-shot `alsem impact`, alone, `cdo` profile (`tests/cli/cli_impact.rs:573`,
`cli_impact_base.rs:1150`): one routine 65 s; the 699-root diff 263-275 s; `--base`
428 s.

**[inference] Most of the cold start is one thread.** If the parallel part scaled
perfectly on 22 cores, `serial + (CPU - serial) / 22 = wall` gives about 30 s of serial
wall inside the 36 s first ask, and about 12 s inside the 18 s test universe. Real
scaling is worse than perfect, so these are upper bounds, but they say where to look:
the serial stages, not the parallel ones.

## What was already done (not proposed again)

**[log]** `docs/superpowers/plans/2026-11-03-context-server-remaining.md` and the
CHANGELOG: witnesses memoized per root (W); `pd.solve` in parallel (L2, 3.3 s to
0.8 s); direct facts in parallel (L5); lazy raw emission (L6, cold fold 11-13 s to
5 s); explain's indexes kept per revision; per-view tree work (L4a); L4 row digests in
parallel; the declaration pack measured and left (ceiling 0.8-1.0 s); card render
measured and left serial (28.6 ms); the allocator swap (mimalloc). **[code]**
Already parallel: file resolution (`resolve/full.rs:1917`), dependency-body resolution
(`full.rs:2519`), parsing (`snapshot/parse.rs`), digest's witnesses per root
(`engine/l5/digest.rs:4866`).

## Ranked candidates

### 1. The literal-reach pass and the per-root finish run one root after another

- **Path [code]:** `engine/l5/digest_cli.rs:2135-2166` (`digest_effects_chunks`). For
  each batch of 32 roots (`impact/jobs.rs:80`, `ROOT_BATCH`): the witnesses run on rayon
  (`cx.run`), then `pass.pass(&mut entries)` (`:2164`) and `finish.finish` (`:2165`) run
  on one thread. The pass is `literal_reach_entries` (`:2358`): `for entry in entries`
  (`:2370`), and per root `reach.live_tree_of` (the guarded search), `crossing_entries`
  per crossing, the candidate operations and the trap rulings (`judge_traps`, `:2267`).
  After that, impact's own loop is serial too: `impact/jobs.rs:441-456`
  (`RoutineRollup::build`, `entry_analysis`, `effect_evidence`, `summarize` per entry).
  The unstreamed read (`digest_effects_each`, digest and prove on several roots) calls
  the same pass.
- **Why slow:** serial over N roots. **[inference]** This is the ~200 s the 699-root
  diff adds over the 65 s floor (about 0.3 s per root), and it fits the gate script's
  note that these processes use about 3 cores. `--base` pays it twice (base side and
  working tree: 428 s against 275 s).
- **Change:** run the pass, the finish and the rollup over a batch's entries on rayon,
  results collected in root order. `LiteralReach` is already shared by `&` and has a
  `Mutex` flow cache (`literal_reach.rs:1587`). The one `&mut` is the `SuffixCache`
  (`digest_cli.rs:2361`): make it a concurrent memo (or one per worker, merged never).
  Raise or drop the 32-root batch width where memory allows; today it also caps the
  witness stage at 32 tasks.
- **Expected:** the diff's 200 s falls toward 200 / (cores that scale); **[inference]**
  30-50 s on 22 cores. Every multi-root `impact`, `digest` and `prove` gains; a PR
  review of a real diff is the headline use.
- **Risk:** determinism: each entry is mutated only by its own task **[code]**, so the
  order of results is the only thing to pin; the suffix memo must be a pure function of
  its key (it is described as shared by every root, `:2222`). Memory: more live
  witnesses at once (5.6-7.9 MiB per root, `jobs.rs:75`); keep the batch bound. Lock
  contention on `flow_cache` could eat the gain.
- **Measure:** `ALSEM_TRACE=1 alsem impact <CDO> --diff -` (the gate's diff): the sum of
  `prove.reach_effects` and of `digest.stream_batch` minus `digest.roots`. If that sum
  is under ~60 s the candidate is dead. Proof after: `streamed_effects_equal_one_read`
  and the CDO impact ratchets (699 roots, 12,395 lines, `+681 routines`).

### 2. A production body edit rebuilds the whole test universe

- **Path [log]:** CHANGELOG `:5471-5476` (measured 2026-11-05): `alsem_tests` after a
  body edit of a production file takes 13.2-14.3 s, "the test universe sees the injected
  file as a `Package`-class change and rebuilds". The inputs are `inj:sources`
  (`engine/resident/manifest.rs:157-164`). **I did not re-read the classification code
  itself, and I found no later entry that changes it.**
- **Why slow:** recomputed instead of kept. The production engine refreshes the same
  edit incrementally (a body edit is ~3 s there), but the test engine treats production
  source as a package and starts over: 18 s of wall and 135 s of CPU today
  (`warm-tests`).
- **Change:** classify a changed file under an injected source folder as a B, S or F
  change of that unit and reuse `program::session`'s per-file parses and resolutions.
  The test universe is graph-only (**[log]** CHANGELOG `:5817-5823`), so only the
  program and the sited graph refresh.
- **Expected:** `alsem_tests` and `alsem_impact` after an edit: 13-18 s to a few
  seconds. This is the agent's inner loop (edit, then ask which tests to run), so every
  resident user gains on every edit.
- **Risk:** correctness, the highest on this list: an incremental refresh must equal a
  cold build. The session tests already compare every change type with cold for the
  workspace; the same oracle must run for a change inside an injected unit. Memory: the
  test engine would keep per-file state (it holds 2.2 GB today; budget in CLAUDE.md).
- **Measure:** on a CDO copy through `alsem mcp`: `alsem_tests`, edit one production
  body, `alsem_tests` again; read the second call's wall time and `session/status`
  (which change class it reports). If the second call is already under ~3 s, dead.

### 3. A one-shot `impact` or `tests` builds the production program once per part

- **Path [code]:** `bin/alsem.rs:2649-2658` and `:2697-2717`: each part is its own
  `parse_job` / `plan` / `make_jobs` with a new `Slot` and `standalone_output`, so
  nothing built by one part reaches the next. `engine/impact/run.rs:153-166` runs, in a
  row: the main part (production model), the up part (production sited graph), one
  select part per test universe (`test_select/run.rs:170`), then a probe run per test
  universe for roots no test universe has (`run.rs:283`), then the base. `alsem tests`
  does the same with two parts (`test_select/run.rs:113,170`). The module doc says so:
  "The CLI runs each part in this process".
- **Why slow:** repeated per part. The production workspace and its dependencies are
  snapshotted, parsed and resolved by the main part, again by the up part, and again
  inside each test universe run (production is a source dependency there); the probe
  run builds the test universe a second time.
- **Change:** run a one-shot's parts over one in-process session: the main and up parts
  off one program (the resident path already lends the program to the sited graph,
  CHANGELOG `:5800-5803`), and the select and probe parts off one test universe.
- **Expected [inference]:** the 65 s one-routine floor loses one production build and,
  when probes run, one test-universe build: perhaps 65 s to 35-45 s on the `cdo`
  profile. Every CLI and CI user of `impact` and `tests` gains; MCP users already share.
- **Risk:** low for answers (the MCP path already answers from shared state, and
  contract tests compare MCP with CLI bytes). Peak memory rises: the program stays
  alive across parts (single-app budget 2 GiB, cross-app 8 GiB).
- **Measure:** `ALSEM_TRACE=1 alsem impact <CDO> --routine "CDO Telemetry::UpdateLastUpload"`:
  count how many times `resolve.files` and `crossapp.base_total` appear, and sum them.

### 4. `prove`'s first call on a routine is 13-16 s on one thread

- **Path:** **[log]** rounds in `speed.log`: prove 12.7-15.8 s, CPU equal to wall;
  the plan measured 5.9 s for `IssueReminder` on 2026-11-03 and it is 12.9 s today.
  **[code]** The work is `digest.roots` for one root (`digest.rs:5212`, a serial loop
  over the root's facts at `:5336`), then `LiteralPass::new` (`prove.literal_reach`,
  `prove.group_paths`, `digest_cli.rs:2207-2216`) and `prove.reach_effects`
  (`:2365`). `engine/l5/literal_reach.rs` (4,457 lines) has no rayon use.
- **Why slow:** serial inside one root, and **[inference]** the growth since
  2026-11-03 is the literal-route and trap work of output rounds 2-4, not the witnesses.
- **Change, by what the trace shows:**
  (a) if `prove.reach_effects` leads: run the root's crossings (`crossing_entries`,
  `:2389`) and its rows' candidate and trap rulings on rayon; they read one finished
  `LiveTree`;
  (b) if `prove.literal_reach` (the constructor) leads: it is a function of the tree,
  not the root, so keep it per revision (as explain's indexes are);
  (c) if `digest.roots` leads: the per-fact loop shares `hop_cache`, `dag_cache` and an
  order-dependent `identity_seen` (`:5305-5309`), so it does not split cleanly; leave it.
- **Expected:** unknown until the trace; (b) could remove seconds from every first
  prove on a routine, (a) scales with cores.
- **Risk:** determinism for (a) (row order, first-witness choices); memory for (b).
- **Measure:** `ALSEM_TRACE=1 alsem engine-ask -- prove <CDO> --routine "CDO E-Mail Handler::RunReportID" --question may-commit --deterministic`
  (or `census-probe`): the four spans above. This one run decides (a), (b) or drop.

### 5. A second ask about the same large change recomputes every root

- **Path [code]:** `impact/jobs.rs:82-98`: the witness memo covers asks up to
  `WITNESS_MEMO_MAX_ROOTS`; above it the read is streamed and each entry is dropped
  after its totals are taken (`:429-456`). Nothing is kept.
- **Why slow:** recomputed instead of kept. Human then JSON, tests on then off, or the
  same PR asked again after an unrelated edit all pay the full root pass (item 1's
  200 s today).
- **Change:** keep each root's small result (totals, analysis, evidence, summary: a few
  KB of JSON, not the 5.6-7.9 MiB witnesses) as a revision artifact keyed by root.
- **Expected:** a repeat ask on a resident server falls to the render. After item 1
  this matters less; do item 1 first.
- **Risk:** low: the value is a pure function of the root and the revision **[code]**
  (`digest_cli.rs:2099-2104` states an entry is a function of the root over any tree
  holding it). Memory: ~699 small values.
- **Measure:** the same `alsem_impact` diff twice on one `alsem mcp`; the second call's
  wall time.

### 6. The cold start's serial stages (find them before choosing)

- **Path [code], no rayon use at all:** `program/model/program_calls.rs`
  (`crossapp.attach_program_calls`, `:1200`; `attach.settle_dead_marks`,
  `attach.settle_rules`, `attach.event_param_temps`, `:619-631`),
  `program/resolve/demand.rs` (`crossapp.demand`, `program_calls.rs:1062`),
  `program/build.rs` (`graph.extract`, `graph.layer_index`, `:479-553`),
  `program/impact_graph/*` (`graph.sited`; **[log]** 0.84-1.19 s on one thread).
- **Why:** **[inference]** from the CPU/wall ratio above, up to ~30 s of the 36 s first
  ask is single-threaded. The plan's table predicted ~25 s after L2 and L6; today's log
  says 35-41 s at load 10. Either the load, or something grew back.
- **Change:** none proposed yet. Rows per routine in the attach step look independent,
  but I did not read enough to claim it.
- **Risk:** n/a until chosen. Determinism is the usual one for any order-preserving
  parallel collect.
- **Measure:** one `ALSEM_TRACE=1` cold first ask on a quiet host (or
  `aldump --session-refresh <copy> --verify`), spans sorted by self time. Anything
  serial over 1 s is a candidate; compare the total with the plan's table to see what
  grew.

### 7. Probably not worth it: a parser per worker thread

**[code]** `al_syntax::parse` makes a new `tree_sitter::Parser` per file (CLAUDE.md
calls it open and unmeasured). Creating a parser is tiny beside parsing a file;
**[inference]** well under 1% of the parse. Kill it with the existing `engine_stages`
bench rather than build it.

## What this changes in round 1

- **Round 1 item 2 was half wrong.** I wrote that impact's per-root work is serial
  because `src/engine/impact/` has no rayon use. The witnesses are parallel per root
  (`digest.rs:4866`). The serial part is what follows them (candidate 1), and the
  one-shot floor is repeated builds (candidate 3). The saving estimate stands; the fix
  is these two, not "parallelize the roots".
- **Round 1 item 4** (ask the shared server instead of one-shot processes in
  `output_cdo`) is stronger: a resident server shares one model across impact's parts,
  a one-shot does not.
- Candidates 1 and 3 together would also bring `cdo_impact_ratchets` and
  `cdo_impact_base_ratchets` under the other `output_cdo` tests, which makes round 1's
  item 1 (the separate impact lane) unnecessary once they land.

## Not checked

- No trace was run, so every share of time above is inferred from totals.
- Candidate 2 rests on a CHANGELOG line of 2026-11-05; I did not read the manifest's
  classification code to confirm it is still so.
- I did not read `literal_reach.rs` beyond its shared state, so candidate 4's options
  are sketches until the trace picks one.
