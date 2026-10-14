# Issue #68 ledger — DEFAULT_DETECTOR_NAMES: 20 of 34 names match no Rust detector

## Claim

```json
{"run_id":"20261014-122439-d9008f","issue":68,"attempt":1,"session":"claude-code-local:bb3fcebb-4fcc-42e9-82de-a5b63c6b6cfa","branch":"issue/68-default-detector-names-20-of-34-a1","worktree":"U:\\Git\\al-sem-issue-68-a1","body_hash":"e512ec19e7d32f3fd263ae3ce6f0b45b83950806bb8adff8061d765da2aa7bf9"}
```

## Classification

- **Kind:** bug (a silent wrong selection; no crash, no wrong output today — see P8).
- **Area:** `src/engine/l5` (registry, digest/prove/fingerprint envelope) and `src/engine/gate`
  (events, policy, presets). No resolver, IR or grammar change.
- **Size:** small/medium. One registry split, one shared function signature change,
  five call sites, two deleted name lists, one new integration test, unit tests.
- **Golden risk:** predicted ZERO bytes moved in every golden family (P8, P9). The issue
  expected goldens to move; that expectation is falsified by measurement (see Discoveries
  of fact, F2).
- **Docs-only?** No. CHANGELOG `Fixed` entry required.

## Assumption probes

Scratch: `C:/Users/SShadowS/AppData/Local/Temp/claude/U--Git-al-call-hierarchy/bb3fcebb-4fcc-42e9-82de-a5b63c6b6cfa/scratchpad/68/`
(`registered.txt`, `digest_list.txt`, `presets_default.txt`, `ts_default.txt`, `proxy/`, `cdo/`, `trace/`).
Binary: `target/release-fast/alsem.exe` built in this worktree at `35d13ba4` (3 m 05 s).

| probe | result | evidence |
|-------|--------|----------|
| P1 Registry size and split | `registered_detectors()` returns **54**: the first **40** are the defaults (d1 … d60, the `--- DEFAULT_DETECTORS` block), the last **14** are opt-in (d40, d46–d51, d17, d8, d45, d61–d64). The block comments are stale: they say "43" and "11" (d17/d8/d45 were demoted in engine-switch S8/S8.5). | `src/engine/l5/detectors/mod.rs:1211-1221` (doc lists 40 + 14), `:1228` ("43"), the `// --- OPT_IN_DETECTORS (11` line; `registered.txt` (54 lines) |
| P2 `digest_cli::DEFAULT_DETECTOR_NAMES` vs registry | 34 names; **20 unregistered**, **14 registered**. The 14 are d1, d2, d3, d4, d5, d7, **d8**, d9, d10, d11, d12, d13, d14, d43. **d8-commit-in-transaction is OPT-IN** in the registry, so today's envelope set is 13 defaults + 1 opt-in, not "14 defaults". | `grep -vxFf registered.txt digest_list.txt` → 20; mod.rs comment "d8: OPT-IN since engine-switch S8 (owner decision 2026-10-07)" |
| P3 The 20 unregistered names: rename or none? | **Neither — they are invented names.** Every one of the 20 numbers has a registered detector with a different name and a different meaning (e.g. `d16-blob-in-loop` vs registered `d16-obsolete-routine-call`). The retired TS `DEFAULT_DETECTORS` (`U:/Git/al-sem-OBOLETE/src/detectors/registry.ts:66-104`, read for provenance only) carries the SAME 34 numbers with the REGISTERED names (`d16-obsolete-routine-call`, `d44-event-multi-subscriber-overlap`, …); `grep -rn blob-in-loop` over the TS `src/` finds nothing. Historical diff: commit `1e5a5f92` (cli-b/b0, 2026-06-10) ADDED the list (`git show 1e5a5f92` line 2230: `+    "d16-blob-in-loop",`, in the new `tests/cli_b_snapshot_differential.rs`), under the doc "al-sem `DEFAULT_DETECTORS` (34) by name — `analyzeWorkspace` runs this set, so the envelope's detector diagnostics … must come from exactly these". At its parent `1e5a5f92^`, `src/engine/l5/detectors/mod.rs` already registered 41 detectors including `d16-obsolete-routine-call`, `d17-min-version-drift`, `d18-constant-filter-in-loop`, `d19-unused-parameter`, `d44-event-multi-subscriber-overlap`. So the copy kept the TS numbers and wrote names that neither the TS registry nor the Rust registry ever carried; its own doc states the intent was the analyze default. Mapping of the 20: **18** map to a registered DEFAULT (d16, d18, d19, d20, d21, d22, d29, d32, d33, d34, d35, d36, d37, d38, d39, d41, d42, d44); **2** map to OPT-IN (d17-min-version-drift, d45-event-transitive-table-exposure). Separately, d8 (opt-in) is one of the 14 that matched (P2). Full table: below. | `git log -S"d16-blob-in-loop"` oldest = `1e5a5f92`; `git show 1e5a5f92^:src/engine/l5/detectors/mod.rs` (41 `name:` entries); `ts_default.txt` (34 lines) |
| P4 What `alsem analyze` runs by default | `run_analyze_with_exit` → `presets::resolve_analyze_detectors(None, None)` → `select_detectors(presets::DEFAULT_DETECTOR_NAMES)`. That list is **identical, in the same order, to the registry's first 40** (`diff` of the two files: no output). Measured: `ALSEM_TRACE=1 alsem analyze <ws>` opens **40** distinct `detector.d*` begin (`"ph":"B"`) events, **each exactly once**, in both `--single-app` and the default cross-app mode, exit 0. | `src/engine/gate/run.rs:452`; `src/engine/gate/presets.rs:16-57,170-187`; `trace/analyze.json`, `trace/analyze_x.json` |
| P5 What digest / prove / fingerprint / events / policy run today (real runs) | **14** distinct `detector.d*` begin events in each, **each exactly once**: digest (cross-app default AND `--single-app`), prove, fingerprint, events fanout, events chains, policy check — the 14 of P2, d8 included. `detector.result` is an INSTANT event (`registry.rs:673`, `pt::instant_lazy`), not a begin event, so a `ph == "B"` filter excludes it. Trace format: a JSON array of Chrome-trace events with `cat`, `name`, `ph`. Spans are emitted at the default `Stages` detail (`registry.rs:641-642`). Exit-0 invocations for T3: digest `--file src/D1MultiCaller.al` on `ws-d1-multi-caller` exits 0 in both modes (the probe's `--routine x` exited 2, unusable). | `trace/{digest,digest1,prove,fp,evf,evc,pol}.json` on `tests/r0-corpus/ws-d1-multi-caller` (prove on `ws-d34 DirectCommitInLoop may-commit`); per-file count script output: every file `{1}` |
| P6 Which set is right for the five commands | **The analyze default (registry first 40).** (a) TS provenance: in al-sem every one of these commands took its diagnostics from `analyzeWorkspace`, whose default is `DEFAULT_DETECTORS` (`registry.ts:137`; `cli/fingerprint.ts:176`, `cli/policy.ts:32`) — the full default, not a "fast subset". (b) The Rust docs say the same: `compute_analyzer_diagnostics` "follows the `run_diagnostics` build in `run_analyze`… so the cap-hit (3) and detector (6) diagnostics agree with `run_analyze`'s" (`gate/run.rs:1035-1041`); events: "Runs the DEFAULT detector set so detector-stage diagnostics … surface exactly as al-sem does" (`gate/events.rs:459-461`); digest cross-app: "the combination `alsem analyze` runs by default" (`digest_cli.rs:101-104`). No source comment, commit or doc claims a deliberate subset; the commit that introduced the list says the opposite (P3: "`analyzeWorkspace` runs this set"). (c) The "subset" today is not even a subset of the default (it contains opt-in d8). Conclusion: the copy is an oversight, not a design. | as cited |
| P7 Second list `gate/presets.rs::DEFAULT_DETECTOR_NAMES` | **A different list.** 40 names, all registered, equal to the registry default in order. Its doc ("mirrors al-sem DEFAULT_DETECTORS (the 39 non-opt-in detectors)") is stale twice: it is 40, and it is no longer the TS list (no d8/d17/d45; adds d52–d60). Readers: `resolve_analyze_detectors` (production, `presets.rs:183`), its own tests (`presets.rs:209,220`), and `tests/cli/gate_suppress_baseline_differential.rs:28,106`. `OPT_IN_DETECTOR_NAMES` (14) equals the registry tail in order; read only by `presets.rs` tests. | `presets_default.txt` vs `head -40 registered.txt`: identical; `OPTIN_SAME` |
| P8 Which outputs depend on the detector set | The five commands **discard findings**; only `analyzer_diagnostics_of_run` reaches output: (1) workspace diagnostics, (3) `summarize_diagnostics`, (5) overlay, (6) `run.diagnostics`. Only (3) and (6) depend on the set. (6): the ONLY detector that emits a diagnostic is **d43** (`detectors/d43.rs:387`, in both sets); `DetectorError::new` has **0** call sites under `detectors/`; a panic becomes a `detect` diagnostic only under unwind (debug tests) and aborts the process in release (`panic = "abort"`, Cargo.toml:245). (3): filled only when `CORE_SUMMARIES` is demanded (`detector_context.rs:1245-1249`), which d1 demands in both sets. Prediction: **no output byte changes**. | as cited |
| P9 Proxy measurement of P8 on every fixture | `alsem analyze <ws> --format json` with `--detector <today's 14>` vs the default 40, both modes (`--single-app` and cross-app), over all 227 `tests/r0-corpus/*` and `tests/fixtures/cli-c-policy/*` workspaces: **454/454 runs: `diagnostics` arrays identical, every exit 0** (no release abort, so no new detector panics on any fixture). Diagnostics seen: d43 warning (54), dependency (114 total), discover (30). CDO: identical too (0 diagnostics both ways). **Evidence caveat:** the per-run JSON files were keyed by workspace basename; 9 basenames exist in both `tests/r0-corpus` and `tests/fixtures/cli-c-policy` (`comm -12` → 9), so for those 9 the r0-corpus JSON files were overwritten by the cli-c-policy runs. The SAME/DIFF verdict was computed and appended to `summary.txt` right after each run, so all 454 verdicts stand; only the 9 overwritten pairs' raw files cannot be re-checked. This is a PROXY: it runs `analyze`, which shares `run_detectors` / `run_detectors_cross_app` and the diagnostic sources (3) and (6) with the five commands, not the commands' own code. | `proxy/summary.txt` (454 lines, all `SAME`, all `exit=0/0`); `cdo/a_{s,c}_{old,new}.json` |
| P10 Goldens that carry these diagnostics | `tests/cli-b-goldens/{digest(4),prove(8),fingerprint(7),snapshot(3)}` envelope `diagnostics` (all d43 `DIAG-detect` or `DIAG-discover`); `tests/cli-c-events-goldens/ws-event-partial-coverage.fanout.{warn,strict,ignore}.stderr.txt` (the d43 line); policy check goldens do NOT capture the diagnostic stderr (only `notfound.explain.stderr.txt`). `tests/cli-a-goldens/json` carries analyze's own diagnostics (not touched). **Predicted move: none.** | `grep -rl '"DIAG-' tests/*goldens*`; `tests/cli/cli_c_events_differential.rs:420-429`; `tests/cli/cli_c_policy_differential.rs:232` |
| P11 Test-side copies | `tests/cli/cli_b_snapshot_differential.rs:51` holds a byte copy of the bad 34-name list and calls `compute_analyzer_diagnostics` with it (the snapshot goldens' source). `tests/cli/cli_a_{html,json,stats,terminal}_differential.rs` hold the **TS** 34 names (all registered, identical to `ts_default.txt`) as the "default" slot passed to `analyze --detector`; out of scope (Discovery D2). | `diff` vs `ts_default.txt`: `CLI_A_EQ_TS` |
| P12 Cost, CDO (the pinned CDO baseline @ `bc3ccb18`, release-fast, `CDO_WS` NOT set, 2 runs each, peak working set sampled every 50 ms) | **Before** — digest (cross-app, `--file src/DocumentTypes/Finance/General/CDOIssueDocument.Codeunit.al`): 23.2 / 26.8 s, 2832 / 2832 MB. fingerprint (json): 7.5 / 7.5 s, 1360 / 1364 MB. events fanout (json): 4.8 / 4.3 s, 575 / 577 MB. **Detector delta** (analyze 14 → 40, same binary): single-app 4.5→4.4 s and 4.0→4.1 s, 575→577 and 576→579 MB; cross-app 19.7→21.8 s and 21.3→21.5 s, 2829→2824 and 2828→2830 MB. Also **before**: prove (`IssueReminder may-commit`): 7.5 / 7.4 s, 2068 / 2071 MB; policy check (json): 4.5 / 4.6 s, 576 / 576 MB; events chains (json): 4.3 / 4.4 s, 573 / 578 MB. **After: an ESTIMATE only** (before + the analyze delta): digest ≈ +0.2..2.1 s, ±5 MB; the single-app commands ≈ +0.1 s, +2..3 MB. All within run-to-run noise (digest varied 3.6 s between identical runs). The real after-numbers are a gate item (Design 9), measured on the changed binary with the same script and invocations. **Bound for larger workspaces (reasoned, not measured):** after the change each command runs `run_detectors` (or `run_detectors_cross_app` for digest) once over the 40 defaults on the model it already builds — the same call `alsem analyze` makes in that mode (P4: one span per detector). So the added cost per command is at most analyze's own detector stage (context build for the newly demanded substrates + the 27 added detectors + their findings, which are built and then discarded, `registry.rs` `RunOutput.findings`) on the same workspace and mode. It is not bounded by "noise" on a large workspace. BC Base App 8020 was NOT measured: the 8020 corpus available to this session has no `.al` files and no `app.json` (reviewer flash tried; snapshot build fails). | `cdo/timings.txt`, `cdo/timings2.txt`, `measure.ps1` |
| P13 Other readers of the deleted names (docs, doctrine) | `.claude/commands/triage-wave.md:31-33` tells operators to demote a detector by moving "the `presets.rs` name from DEFAULT_DETECTOR_NAMES to OPT_IN_DETECTOR_NAMES" — the flow may not edit `.claude/` (CLAUDE.md, autonomous-flow exception), so this is an owner action. `docs/2026-10-04-b3-code-map.md:66` cites `DEFAULT_DETECTOR_NAMES (presets.rs:16)` — in scope. `docs/superpowers/plans/2026-07-16-bcquality-detector-wave.md:19,461,789,2749,3086` name the lists too; these are dated, executed plans (historical records), left unchanged. No skill under `.claude/` other than `triage-wave.md` names them. | `grep -rn "DEFAULT_DETECTOR_NAMES\|OPT_IN_DETECTOR_NAMES" docs .claude CLAUDE.md CHANGELOG.md` |
| P14 Preset list vs `resolve_preset` | `PRESET_NAMES_LIST` (`presets.rs:109`) and the `match` in `resolve_preset` (`:113-124`) are two separate hand lists; read by `alsem.rs:69,1531` (error text) and `presets.rs:122`. A preset added to the `match` but not to the list is invisible to anything that walks the list. | as cited |

P3 mapping (old name → registered detector with the same number → its class):

| old name | registered | class |
|----------|-----------|-------|
| d16-blob-in-loop | d16-obsolete-routine-call | default |
| d17-non-setbased-on-large-table | d17-min-version-drift | opt-in |
| d18-event-subscriber-heavy | d18-constant-filter-in-loop | default |
| d19-flowfield-in-loop | d19-unused-parameter | default |
| d20-unbounded-result-set | d20-unreachable-after-exit | default |
| d21-temp-table-misuse | d21-read-without-load | default |
| d22-deprecated-api-use | d22-flowfield-without-calcfields | default |
| d29-onaftergetrecord-heavy | d29-subscriber-modify-on-event-record | default |
| d32-internal-event-publisher | d32-constant-boolean-parameter | default |
| d33-event-without-subscribers | d33-unfiltered-bulk-write | default |
| d34-page-source-heavy | d34-commit-in-loop | default |
| d35-implicit-transaction-scope | d35-commit-in-event-subscriber | default |
| d36-redundant-calcfields | d36-late-setloadfields | default |
| d37-record-passed-by-value | d37-validate-without-persist | default |
| d38-page-trigger-heavy | d38-subscriber-to-obsolete-event | default |
| d39-codeunit-instantiation-in-loop | d39-record-left-dirty-across-chain | default |
| d41-unindexed-filter | d41-transitive-filter-loss | default |
| d42-locktable-late | d42-cross-call-wrong-setloadfields | default |
| d44-event-recursive-publish | d44-event-multi-subscriber-overlap | default |
| d45-text-encoding-mismatch | d45-event-transitive-table-exposure | opt-in |

## Design

**Rule (what is promised, and what is not):** digest, prove, fingerprint, events and policy
run the **same detector SET** that `alsem analyze` runs when given no `--detector`/`--preset`:
the registry's default set. There is ONE default set, and it is the registry's, not a name list.
**Not promised:** whole-output parity with `analyze`. Each command keeps its current model mode
(digest cross-app by default with `--single-app`; prove, fingerprint, events, policy single-app —
`prove.rs:891`, `fingerprint_cli.rs:353`, `events.rs:366,515`, `policy/pipeline.rs:195`, all
`assemble_and_resolve_workspace_program`), and keeps its current diagnostic sources: the shared
projection has no dependency diagnostics (source (2), `run.rs:1027-1029`), which `analyze`
reports. Changing modes or adding (2) is #70 / out of scope.

1. **Registry owns membership** (`src/engine/l5/detectors/mod.rs`). Split the one `vec!` into
   `pub fn default_detectors() -> Vec<Detector>` (the 40) and `pub fn opt_in_detectors() -> Vec<Detector>`
   (the 14); `registered_detectors()` becomes `default_detectors()` then `opt_in_detectors()`
   chained, so its order (and every `detectorStats` order) is unchanged. No new field on
   `Detector` (54 struct literals for no gain; which function a detector sits in IS the flag).
   Fix the stale "43"/"11" comments (`mod.rs:1228` and the OPT_IN block header).
2. **`gate/presets.rs`:** delete `DEFAULT_DETECTOR_NAMES` and `OPT_IN_DETECTOR_NAMES`.
   `resolve_analyze_detectors`' no-flag branch returns `default_detectors()`; its `--preset` and
   `--detector` branches are unchanged (explicit selection stays analyze-only). Rewrite the
   module doc (it no longer mirrors TS). Keep the two preset bundles, but make them ONE table:
   `pub const PRESETS: &[(&str, &[&str])] = &[("transaction-integrity", PRESET_TRANSACTION_INTEGRITY), ("bcquality", PRESET_BCQUALITY)]`;
   `resolve_preset` looks the name up in `PRESETS` (same `Unknown preset '…'. Known: …` text), and
   `PRESET_NAMES_LIST` is replaced by `pub fn preset_names() -> Vec<&'static str>` derived from
   `PRESETS` (`alsem.rs:69,1531` and `presets.rs:122` switch to it; same joined text). This closes
   P14: a preset cannot exist outside the walked table.
3. **One shared entry point** (`src/engine/gate/run.rs`): `compute_analyzer_diagnostics(ws, resolved)`
   loses its `detectors` parameter and runs `default_detectors()` itself. Add
   `compute_analyzer_diagnostics_cross_app(ws, base)` beside it (moved from
   `digest_cli::build_envelope_diagnostics_json_cross_app`, using
   `run_detectors_cross_app(base, &default_detectors(), true)`). `analyzer_diagnostics_of_run`
   becomes private to `run.rs` (its only outside caller today, `digest_cli.rs:21,111`, moves into
   run.rs). With no detector parameter and no public run-taking projection, code outside `run.rs`
   cannot build these diagnostics from a different detector set. Limit: code INSIDE `run.rs`
   (the body of the two functions) still could; T3 is what pins that.
   `run_analyze_with_exit` keeps its own call (`run.rs:452`); T3's analyze cases pin it.
4. **Call sites:** `digest_cli.rs` (delete `DEFAULT_DETECTOR_NAMES` and the local
   `default_detectors()`; both envelope builders call the run.rs pair), `prove.rs` (via
   `build_envelope_diagnostics_json`, unchanged call), `fingerprint_cli.rs::analyzer_diags_for`,
   `gate/events.rs::analyze_workspace_diagnostic_lines`,
   `gate/policy/pipeline.rs::analyze_workspace_diagnostic_lines`: drop the filter, call the
   shared function. Fix the "34-detector set" comments (`digest_cli.rs:41-44,1430`).
5. **Old names do not carry over.** They were never renames (P3): same numbers, invented
   names, different meanings. The numbers they pointed at are already in the analyze default
   except d8, d17, d45, which the owner demoted to opt-in (S8/S8.5, recorded in mod.rs).
   So the envelope set goes 14 → 40: **+27** (d16, d18–d22, d29, d32–d39, d41, d42, d44,
   d52–d60) and **−1** (d8, opt-in). d8 emits no diagnostic (P8), so its removal moves no byte.
   **Expanded execution, not advisory promotion:** 27 detectors newly RUN automatically inside
   these five commands, but no detector changes class: all 27 are already `analyze` defaults,
   and these commands discard findings (only diagnostics reach output, P8). So no finding
   becomes newly visible anywhere, and CLAUDE.md's "new advisory L5 detector → triage before
   DEFAULT" rule is not engaged. The cost of the expanded execution is P12's bound.
6. **Tests' copies:** `cli_b_snapshot_differential.rs` drops its copy (it no longer compiles
   against the new signature, which is the point); `gate_suppress_baseline_differential.rs`
   builds its list from `default_detectors()` (same 40 names, same order — P7, byte-identical).
   The cli-a "default" slot stays as is (Discovery D2; changing it moves cli-a findings goldens).
7. **CHANGELOG** `Fixed`: the five commands ran 14 detectors (13 of analyze's default set plus
   opt-in d8) because 20 names in a copied list matched nothing; now they run analyze's default
   detector SET from the registry, in their existing modes. Diagnostics output unchanged on every
   golden; the CDO before/after is the gate measurement (item 9). `Changed`: presets'
   `PRESET_NAMES_LIST` const → `preset_names()`; `DEFAULT_DETECTOR_NAMES`/`OPT_IN_DETECTOR_NAMES` removed
   (demote a detector by moving its entry from `default_detectors()` to `opt_in_detectors()`, then
   update T1b's opt-in literal AND its default count, which T1b(4) asserts as 40).
8. **Docs:** update `docs/2026-10-04-b3-code-map.md:66` (`DEFAULT_DETECTOR_NAMES (presets.rs:16)` →
   `default_detectors()` in `src/engine/l5/detectors/mod.rs`). Dated executed plans under
   `docs/superpowers/plans/` stay as historical records. `.claude/commands/triage-wave.md:31-33`
   cannot be edited by the flow: Discovery D4 (owner action) with the new one-step rule.
9. **Gate measurement (after):** re-run the P12 invocations (same `measure.ps1`, same CDO
   baseline, release-fast, 2 runs each) for digest, prove, fingerprint, events fanout, events
   chains and policy check on the changed binary, and record before/after in Gate results.
   Not a pass/fail gate (no threshold exists); a regression beyond P12's noise band is reported
   to the owner, not hidden.
10. **Work order (so the red run is real):** (i) registry split (Design 1) alone — byte-identical,
   all tests green; (ii) add T3, T1, T1b (all compile against step (i) alone: they need only
   `default_detectors()`/`opt_in_detectors()` and the existing `resolve_analyze_detectors`); run T3
   against the OLD call sites → red (14 vs 40 on the five commands; analyze cases green);
   (iii) change presets (Design 2: the `PRESETS` table and `preset_names()`) together with T2,
   which needs them and so cannot be added before this step, then the call sites
   (Design 3-4) → green; (iv) discrimination proofs; (v) goldens, gates, CHANGELOG, docs.

**Golden license (exact; a FORECAST until the changed source runs):** ZERO bytes move in any of the 8 `scripts/check-goldens` targets.
`scripts/check-goldens` must pass WITHOUT `--regen`. Any moved golden is a defect in this change
(or a P8/P9 premise that was wrong) and stops the task for triage — it is NOT blessed under this
issue. Basis: P8 (only d43 emits a diagnostic; d43 is in both sets; the summarize channel is
demanded by d1 in both), P9 (454/454 fixture runs identical, CDO identical). This replaces the
issue's expected license ("only the newly running detectors' diagnostics are added"): the
newly running detectors add no diagnostics, so the triage set is empty.

**Guard test (unknown name fails):** after this change no production DEFAULT name list exists
to guard — the default set is the registry's own vec. The remaining production name lists are the
presets. T2 (`every_preset_member_is_registered`) walks the `PRESETS` table (Design 2, the
only place a preset can be defined) and requires every member name to be in
`registered_detectors()`, so a new preset is covered automatically. The default and opt-in
sets themselves are guarded against silent reclassification and duplicates by T1b.
(Runtime already errors: `select_detectors` returns `Unknown detector: <name>`, exit 3.)

### Acceptance matrix

| Acceptance item | test | mutation that makes it fail |
|-----------------|------|-----------------------------|
| A1 The default set for digest, prove, fingerprint, events and policy is derived from the registry, and equals what `analyze` runs by default | **T3** `tests/cli/cli_envelope_detector_set.rs` (new `cli` umbrella member). For each case it spawns `env!("CARGO_BIN_EXE_alsem")` with `ALSEM_TRACE=1` and `ALSEM_TRACE_FILE=<per-case temp file>` (child-process env only; no `std::env` mutation in the test process). Cases: `analyze <ws> --format json` (default cross-app); `analyze <ws> --format json --single-app`; `digest <ws> --file src/D1MultiCaller.al --deterministic` (cross-app default); the same with `--single-app`; `prove ws-d34 DirectCommitInLoop may-commit --deterministic`; `fingerprint <ws> --format json --deterministic`; `events fanout` and `events chains` (`--format json --deterministic`); `policy check <ws> --format json --deterministic`. `<ws>` = `tests/r0-corpus/ws-d1-multi-caller`. Per case it asserts: (1) the process exits 0 (P5: all of these do today); (2) the trace parses as a JSON array; (3) over ALL events with `ph == "B"` whose `name` starts with `detector.`: every name minus the prefix is a REGISTERED detector (an unknown name fails); (4) each name occurs **exactly once** (a duplicate execution fails); (5) the set of names **equals** `default_detectors()` names. Pins the USE end to end: the observer is the child process's own trace, so a call site, a selector or a run.rs body that runs a different set fails, whichever layer it is in. | (a) `run_analyze_with_exit` (`run.rs:452`): filter d60 out of the resolved detectors → both analyze cases fail naming `d60-upgrade-loop-should-be-datatransfer`; T1 stays green (proves T3, not T1, pins the use). (b) `compute_analyzer_diagnostics` body: run `default_detectors()` minus d60 → prove, fingerprint, events ×2, policy, digest `--single-app` fail; analyze and digest cross-app stay green. (c) `compute_analyzer_diagnostics_cross_app` body: same → only digest cross-app fails. (d) Red run per Design 10 (ii): registry split applied, call sites unchanged → the seven non-analyze cases fail with 14 names vs 40 (missing 27, extra d8); analyze cases pass. |
| A1 (selector) the analyze no-flag selection is the registry default | **T1** `presets::tests::analyze_default_is_registry_default`: names of `resolve_analyze_detectors(None, None)` == names of `default_detectors()`, in order. A helper-level check; the USE is pinned by T3's analyze cases. | No-flag branch returns `default_detectors().into_iter().take(39)` → T1 fails, and T3's analyze cases fail too. |
| A1 (registry) membership is pinned independently and has no duplicates | **T1b** `detectors::tests::default_and_opt_in_membership`: (1) `registered_detectors()` names == `default_detectors()` names ++ `opt_in_detectors()` names, in order; (2) all 54 names are unique (a `HashSet` the same size as the list — catches duplicates inside one set as well as across the two); (3) `opt_in_detectors()` names equal a HAND-STATED literal list of the 14 owner-decided opt-ins (d40, d46, d47, d48, d49, d50, d51, d17, d8, d45, d61, d62, d63, d64 — each carries an `OPT-IN` comment in `mod.rs`; d17, d8 and d45 also state the S8/S8.5 decision). (3) is the independent oracle: a reclassification must edit this literal on purpose (and the CHANGELOG), so a swap cannot pass silently. (4) explicit counts: `default_detectors().len() == 40` and `registered_detectors().len() == 54` (a default detector deleted from the registry outright passes (1)-(3) and T3, which follow `default_detectors()`; only a count catches it — d19 is in no preset, so T2 does not either). | (i) Swap d8 and d60 between the two functions (counts, partition and disjointness all preserved) → (3) fails naming both. (ii) Duplicate d1 inside `default_detectors()` → (2) fails. (iii) Drop the `opt_in_detectors()` half from `registered_detectors()` → (1) fails. (iv) Delete the `d19-unused-parameter` entry from `default_detectors()` → (1)-(3) and T3 stay green, (4) fails (39 vs 40, 53 vs 54). |
| A1 second list reconciled | Deleted (Design 2); compile-enforced — `presets::DEFAULT_DETECTOR_NAMES` and `OPT_IN_DETECTOR_NAMES` no longer exist. T1 and T3's analyze cases cover the behaviour that read the first; T1b(3) replaces the second. | Covered by T1/T3 (a) and T1b (i). |
| A2 A test fails when a listed name is not a registered detector | **T2** `presets::tests::every_preset_member_is_registered`: walks `PRESETS` (the single preset table, Design 2) and asserts each member is in `registered_detectors()`, naming the preset in the message. Replaces `every_default_and_opt_in_name_is_registered` and the bcquality-only check. Also asserts `preset_names()` == `PRESETS` keys and that `resolve_preset(k)` returns `PRESETS[k]` for every key. | Add `"d99-fake"` to `PRESET_TRANSACTION_INTEGRITY` → fails with `d99-fake not registered (preset transaction-integrity)`. Note: the existing `preset_resolves_transaction_integrity` compares the preset with its own constant and stays green under this mutation, which is why T2 is needed. |
| A3 Goldens move under a stated license | `scripts/check-goldens` (no regen) green on the CHANGED source; license = zero bytes moved (a forecast from P8/P9 until that run; the result goes in Gate results). Plus the existing d43 goldens (`cli-b-goldens/*` `DIAG-detect`, `cli-c-events-goldens/*.stderr.txt`) prove the channel is live and still carries d43. | Not a code mutation: the license is falsifiable by any golden diff, which then gets triaged before anything is regenerated. |

## Base/Head SHAs

- Base: `8610adb80ba4f2e53d470bf20408651d3a9b373e` (master; the branch was rebased from `35d13ba4` onto it).
- Head after tasks 1-4: `bd73eb7eefa753040444d404a1f61abb758ec30b` (`73e80706`, `f46fd933`, `c667d15b`, `bd73eb7e`).
- Final-panel fixes: `9f5ff893` (task-4 review), `314fbfbf` (final round 1), `40a3d6c1` (final round 2).
- **B** = `8610adb80ba4f2e53d470bf20408651d3a9b373e` (origin/master at freeze; no rebase needed, it is
  an ancestor of H). **H** = `40a3d6c1bbaabb4ff5e7e867d13f9d5f3576ffd7`: all four gates exit 0,
  supervised, on this exact tree (logs `.agent/runs/20261014-122439-d9008f/logs/*/20261014T1453*`
  to `T1458*`). Final panel converged in round 3 of 5: both reviewers accepted all 32 entries.

## Timeline

| when (UTC) | step |
|------------|------|
| 2026-10-14 ~10:25 | Worktree and claim. Read issue #68 and the six source sites. |
| ~10:30 | P1–P3, P7 name probes; TS provenance read (`al-sem-OBOLETE`, read only). |
| ~10:31–10:34 | release-fast build of `alsem` in the worktree (3 m 05 s). |
| ~10:35 | P9 proxy: 454 analyze runs over all fixtures, 14 vs 40. |
| ~10:36–10:42 | P12 CDO timings (2 runs × 7 commands). |
| ~10:43 | P5 trace probe of all five commands; design and ledger. |
| ~11:00 | Spec panel round 1 (astra: 3 important / 7 minor; flash: 2 important / 7 minor). Source-verified; new probes: per-event span counts, analyze cross-app trace, exit-0 digest invocation, `1e5a5f92` history, CDO prove/policy/events-chains before-numbers, doctrine/doc references (P13), preset table (P14). Design, probes, matrix, Not delivered revised; `findings.json` written. |
| ~11:45 | Spec panel round 2 (spec_rounds 2/5): both accepted 16/16; one new minor each (R2-01 astra T1b count guard, R2-02 flash T2 step order). Fixed. |
| ~12:05 | Spec panel round 3 (3/5): astra accepted 18/18, nothing new; flash accepted 18/18, one new minor (R3-01 demotion rule must name T1b(4)'s count). Conductor fixed it. |
| ~12:15 | Spec panel round 4 (4/5): both accepted 19/19, nothing new, both VERDICT ready. **Converged** (ledger sha256 `9b6ec3a0…7efa`). Reviewers per operator directive: astra = gpt-6.1-sol (pi), flash = Opus 5.5 (Agent). |
| ~12:40 | Step 6: acceptance tests. None compiles at base (all need step (i) or (iii)). Applied step (i) temporarily: T1b, T1 green; T3 red 7/9 (14 vs 40, missing 27, extra d8), analyze cases green. Reverted; tree back at base. Test text moved to the plan (see Acceptance matrix). |
| ~12:50 | Step 7: plan `docs/superpowers/plans/2026-10-14-issue-68-default-detector-set.md`, 4 tasks in Design 10 order. Nothing committed. |
| Task 1 | Registry split + T1b, discrimination (i)-(iv). Commit `73e80706` (rebased onto `8610adb8`; was `50d4cb77`). |
| Task 2 | `PRESETS` table, `preset_names()`, T1 + T2, discrimination. Commit `f46fd933`. |
| Task 3 | Call sites (digest, prove, fingerprint, events, policy) + T3, red run (d) and discrimination (a)-(c). Commit `c667d15b`. |
| Task 4 | Leftover-name check, `scripts/check-goldens` (no regen) green, CHANGELOG, code map, CDO before/after cost (Design 9), `scripts/ci-steps task` exit 0. Commit `bd73eb7e`. |

## Acceptance matrix

See Design → Acceptance matrix (A1 → T3 [use, incl. analyze], T1 [selector], T1b [registry membership]; A2 → T2; A3 → check-goldens, zero-byte license forecast). Discrimination outcomes are pending (recorded in the next section as each proof runs).

**Step 6 (acceptance tests), 2026-10-14.** At base `35d13ba4` NO acceptance test compiles:
T1, T1b and T3 need `default_detectors()`/`opt_in_detectors()` (Design 1, step (i)); T2
needs `PRESETS`/`preset_names()` (Design 2, step (iii)). So none is left in the tree (the
tree is back at base; `git status` shows only `.agent/issue-68/`). Their exact text is in
the plan `docs/superpowers/plans/2026-10-14-issue-68-default-detector-set.md`:

| test | where | compiles after | state on the old call sites |
|------|-------|----------------|-----------------------------|
| T1b `detectors::tests::default_and_opt_in_membership` | `src/engine/l5/detectors/mod.rs` | step (i) (plan Task 1) | green (guard; red only under its mutations) |
| T1 `presets::tests::analyze_default_is_registry_default` | `src/engine/gate/presets.rs` | step (i) (plan Task 2) | green (old list == registry first 40, P7) |
| T2 `presets::tests::every_preset_member_is_registered` | `src/engine/gate/presets.rs` | step (iii) (plan Task 2) | does not compile until then |
| T3 `cli_envelope_detector_set::*` (9 cases) | `tests/cli/cli_envelope_detector_set.rs` + `tests/cli/main.rs` | step (i) (plan Task 3) | **red, 7 of 9, for the stated reason** |

Red run, measured: step (i) was applied TEMPORARILY (the plan's Task 1 split, by Edit),
T1/T1b/T3 added, then all of it reverted by Edit. `cargo test -p al-sem --lib --
default_and_opt_in_membership analyze_default_is_registry_default` → `2 passed`.
`cargo test -p al-sem --test cli cli_envelope_detector_set::` → `2 passed; 7 failed`:
both analyze cases `ok`; digest, digest `--single-app`, prove, fingerprint, events fanout,
events chains and policy check each fail at `cli_envelope_detector_set.rs:90` with

```
[fingerprint] ran 14 detectors, the analyze default set has 40
missing: ["d16-obsolete-routine-call", "d18-constant-filter-in-loop", "d19-unused-parameter", "d20-unreachable-after-exit", "d21-read-without-load", "d22-flowfield-without-calcfields", "d29-subscriber-modify-on-event-record", "d32-constant-boolean-parameter", "d33-unfiltered-bulk-write", "d34-commit-in-loop", "d35-commit-in-event-subscriber", "d36-late-setloadfields", "d37-validate-without-persist", "d38-subscriber-to-obsolete-event", "d39-record-left-dirty-across-chain", "d41-transitive-filter-loss", "d42-cross-call-wrong-setloadfields", "d44-event-multi-subscriber-overlap", "d52-bulk-write-param-no-temp-guard", "d53-ignored-tryfunction-result", "d54-publish-in-tryfunction-cone", "d55-event-publish-in-loop", "d56-clone-before-write-in-loop", "d57-singleinstance-growing-state", "d58-query-filter-after-open", "d59-integrationevent-var-boolean-guard", "d60-upgrade-loop-should-be-datatransfer"]
extra: ["d8-commit-in-transaction"]
```

(the same 27 missing + d8 extra in all seven). This is exactly mutation (d): 14 vs 40,
missing 27, extra d8, analyze green. No exit-code, trace-parse, unknown-name or
duplicate failure occurred. Log: scratch `68/t3_red.log`; diff of the temporary state:
scratch `68/step6.diff`.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| T1b `default_and_opt_in_membership` | (red) test before the split | `E0425 cannot find function default_detectors / opt_in_detectors` | `... ok` after the split | 73e80706 (was 50d4cb77 before the rebase onto 8610adb8) |
| T1b | (i) swap d8 and d60 between the two functions | panicked: `opt-in set changed` (left has d60 in d8's slot) | `... ok` (1 passed) | 73e80706 (was 50d4cb77 before the rebase onto 8610adb8) |
| T1b | (ii) duplicate the d1 entry in `default_detectors()` | panicked: `duplicate detector name in ["d1-db-op-in-loop", "d1-db-op-in-loop", ...` | `... ok` (1 passed) | 73e80706 (was 50d4cb77 before the rebase onto 8610adb8) |
| T1b | (iii) `registered_detectors()` returns `default_detectors()` only | panicked: `registered_detectors() != default ++ opt_in` | `... ok` (1 passed) | 73e80706 (was 50d4cb77 before the rebase onto 8610adb8) |
| T1b | (iv) delete the `d19-unused-parameter` entry | panicked: `default_detectors() count` left 39 right 40 (only assertion 4) | `... ok` (1 passed) | 73e80706 (was 50d4cb77 before the rebase onto 8610adb8) |
| T1/T2 `analyze_default_is_registry_default`, `every_preset_member_is_registered` | (red) tests before `PRESETS`/`preset_names` | `E0425 cannot find value PRESETS` (x3) / `cannot find function preset_names` | `6 passed` after implement | f46fd933 |
| T1 `analyze_default_is_registry_default` | no-flag branch returns `default_detectors().into_iter().take(39).collect()` | FAILED: `assertion left == right failed` (39 vs 40 names); other 5 presets tests ok | `... ok` (6 passed) | f46fd933 |
| T2 `every_preset_member_is_registered` | add `"d99-fake"` to `PRESET_TRANSACTION_INTEGRITY` | FAILED: `d99-fake not registered (preset transaction-integrity)`; `preset_resolves_transaction_integrity` stayed ok | `... ok` (6 passed) | f46fd933 |
| T3 `cli_envelope_detector_set` (9 cases) | (d) red: T3 against the OLD call sites (before run.rs/call-site change) | `2 passed; 7 failed`: digest, digest --single-app, prove, fingerprint, events fanout, events chains, policy check each `ran 14 detectors, the analyze default set has 40`, `extra: ["d8-commit-in-transaction"]`; both analyze cases ok | `9 passed` after the change | c667d15b |
| T3 | (a) `run_analyze_with_exit` filters out d60 after `resolve_analyze_detectors` | `7 passed; 2 failed`: analyze, analyze --single-app `missing: ["d60-upgrade-loop-should-be-datatransfer"]`; T1 `analyze_default_is_registry_default` stayed ok | `9 passed` | c667d15b |
| T3 | (b) `compute_analyzer_diagnostics` runs default minus d60 | `3 passed; 6 failed`: prove, fingerprint, events fanout, events chains, policy check, digest --single-app `missing: [d60]`; analyze x2 and digest cross-app ok | `9 passed` | c667d15b |
| T3 | (c) `compute_analyzer_diagnostics_cross_app` runs default minus d60 | `8 passed; 1 failed`: only digest (cross-app) `missing: [d60]` | `9 passed` | c667d15b |
| T3 | (e) final panel: delete the five `env_remove("ALSEM_TRACE_*")` lines and run with `ALSEM_TRACE_EXIT_AFTER=context.symbols_resolve_calls` in the parent env (an unconditional span in `build_detector_context_with` that closes before any detector runs, in both modes) | `0 passed; 9 failed`: every case `ran 0 detectors, the analyze default set has 40`. (A first try with `detector.d1-db-op-in-loop` failed only 1 of 9: detectors run in parallel, so most finish before d1's span closes; `context.ordering_facts` failed none, it is not opened on this corpus.) | `9 passed` with the lines restored, same parent env | 314fbfbf |

## Findings register summary

Round 1: see `.agent/issue-68/findings.json` (R1-01 … R1-16; 16 distinct after merging 20 reviewer items; 16 fixed, 0 refuted; both reviewers accepted all 16 in round 2). Round 2: R2-01 (T1b count guard, fixed) and R2-02 (Design 10 step order for T2, fixed). All accepted findings fixed in the
ledger; refutations carry source evidence.

## Gate results

- **Leftover names (plan Task 4 Step 1):** `grep -rn "DEFAULT_DETECTOR_NAMES\|OPT_IN_DETECTOR_NAMES\|PRESET_NAMES_LIST" src tests` hits only `tests/cli/cli_a_{html,json,stats,terminal}_differential.rs` (Discovery D2, out of scope). Nothing in `src/`.
- **A3 golden license (zero bytes):** `scripts/check-goldens` (NO `--regen`) on `c667d15b`: exit 0, all 8 targets `test result: ok`, 0 failed; `git status` afterwards shows no tracked file changed. Zero bytes moved; license held. Log: scratch `68/goldens.log`.
- **Task gate:** `scripts/ci-steps task` exit 0 on the Task 4 tree (log scratch `68/task_gate.log`).
- **Design 9 cost (after), CDO** the pinned CDO baseline, release-fast, `CDO_WS` unset, same `measure.ps1`, 2 runs, before/after INTERLEAVED per command. Before = master `8610adb8` (rebuilt: the existing master binary dated 2026-10-13 23:02 predated `8610adb8`); after = `c667d15b` (the source of `bd73eb7e`; Task 4 changes no code). All exit 0. Wall s / peak working set MB:

  | command | before r1 | before r2 | after r1 | after r2 |
  |---------|-----------|-----------|----------|----------|
  | digest (cross-app, `--file …CDOIssueDocument.Codeunit.al`) | 31.3 / 2836 | 24.8 / 2828 | 28.3 / 2830 | 25.0 / 2831 |
  | prove `IssueReminder may-commit` | 7.9 / 2072 | 7.7 / 2067 | 8.0 / 2071 | 7.9 / 2069 |
  | fingerprint (json) | 7.3 / 1360 | 7.3 / 1363 | 7.5 / 1361 | 6.9 / 1360 |
  | events fanout (json) | 4.7 / 577 | 4.6 / 576 | 4.4 / 574 | 5.3 / 576 |
  | events chains (json) | 4.3 / 571 | 4.8 / 576 | 4.4 / 575 | 5.5 / 577 |
  | policy check (json) | 4.5 / 576 | 4.4 / 576 | 4.6 / 578 | 4.6 / 578 |

  Paired time deltas (after − before, run 1 / run 2): digest −3.0 / +0.2 s; prove +0.1 / +0.2 s;
  fingerprint +0.2 / −0.4 s; events fanout −0.3 / +0.7 s; events chains +0.1 / +0.7 s; policy check
  +0.1 / +0.2 s. Two runs per side are too few to call any delta a regression or a gain: the result
  is INCONCLUSIVE, not "noise". For events, after-run 2 was 0.6-1.2 s slower than every before-run
  (fanout 5.3 vs 4.7/4.6, chains 5.5 vs 4.3/4.8); digest's own before-runs differed by 6.5 s. Peak
  memory moved by 6 MB or less. Output: all six commands' r1 output byte-identical before vs after (`cmp`). Artifacts: scratch `68/gate/` (`run.ps1`, `timings.txt`, outputs).

## Discoveries

Facts this probe falsified (not defects to file):

- F1 The issue says the list "copies the retired al-sem TS DEFAULT_DETECTORS". It copies the TS
  NUMBERS only; the TS names were the registered ones. The 20 names were invented at port time (P3).
- F2 The issue says "Expect the digest, prove, fingerprint, events and policy goldens to move".
  They do not: only d43 emits a diagnostic and it already ran (P8, P9).
- F3 The issue says the commands "run only the 14 names that still match" — true, but one of the
  14 (d8) is opt-in, so they ran 13 of 40 defaults plus an opt-in (P2, P5).

Defects outside scope, in array order D1 (SARIF rules catalog), D2 (cli-a "default" slot),
D3 (`diff --strict`), D4 (triage-wave doctrine; caused by #68, owner action), D5 (analyze's
dependency diagnostics missing from the five commands; found by the final panel, F1-03):

```json
[
  {
    "subsystem": "gate / SARIF formatter",
    "locator": "src/engine/gate/format_sarif.rs:56 (fn rules) and its test at :464 (assert_eq!(rules().len(), 21))",
    "symptom": "The SARIF tool.driver.rules catalog describes 21 of the 54 registered detectors; findings from the other 33 (e.g. every d22+ and BCQuality d52-d64 finding) carry a ruleId with no reportingDescriptor, so SARIF viewers show no rule name or description for them. Same root cause family as #68: a hand list instead of the registry.",
    "kind": "enhancement",
    "origin_issue": 68,
    "reproducer": "Measured 2026-10-14 at 35d13ba4: alsem analyze tests/r0-corpus/ws-d34 --format sarif --single-app -> runs[0].tool.driver.rules has 21 ids; results carry ruleIds d19-unused-parameter and d34-commit-in-loop, neither of which is in rules. The registry has 54 detectors.",
    "pre_existing": true,
    "capability": "Every registered detector has a SARIF rule descriptor (id, name, shortDescription), derived from or checked against the registry.",
    "acceptance": "A test fails when a registered detector has no SARIF rule, or a SARIF rule names an unregistered detector; SARIF goldens move by the added rules only."
  },
  {
    "subsystem": "tests / cli-a differentials",
    "locator": "tests/cli/cli_a_{json,html,stats,terminal}_differential.rs DEFAULT_DETECTOR_NAMES (e.g. cli_a_json_differential.rs:39)",
    "symptom": "The goldens labelled 'default' (tests/cli-a-goldens/*/*.default.*) run the retired TS 34-name default (includes opt-in d8, d17, d45; omits d52-d60), not what alsem analyze runs by default (40), so the 'default' slot no longer measures the default.",
    "kind": "enhancement",
    "origin_issue": 68,
    "reproducer": "Compare the 34 names in cli_a_json_differential.rs:39 with registry default_detectors() (after #68) / presets::DEFAULT_DETECTOR_NAMES (before): 3 are opt-in, 9 defaults are missing.",
    "pre_existing": true,
    "capability": "The cli-a 'default' slot runs the registry default set, so its goldens track the real analyze default.",
    "acceptance": "The cli-a default slot is derived from the registry; cli-a goldens move only by the findings of d52-d60 added and d8/d17/d45 removed, each triaged."
  },
  {
    "subsystem": "gate / diff CLI",
    "locator": "src/engine/gate/diff/cli.rs:171-177 (let _ = opts.strict; analyzer_diagnostics = Vec::new())",
    "symptom": "alsem diff --strict in workspace mode is a silent no-op: its help says 'Exit 1 on any error-severity analyzer diagnostic (workspace mode)', but no analyzer diagnostics are collected, so it can never exit 1 for that reason.",
    "kind": "bug",
    "origin_issue": 68,
    "reproducer": "By code inspection (not run): diff/cli.rs:176-177 discards opts.strict and sets analyzer_diagnostics to an empty Vec on every path, and its own comment calls ws-mode collection 'the corpus-invisible follow-up'. Expected repro: a workspace with an error-severity analyzer diagnostic, alsem diff <wsA> <wsB> --strict exits 0 with no analyzer line, while alsem fingerprint <wsB> --strict exits 1.",
    "pre_existing": true,
    "capability": "diff --strict in workspace mode gates on the same analyzer diagnostics the other commands compute (compute_analyzer_diagnostics).",
    "acceptance": "A test with an error-severity diagnostic in workspace mode makes diff --strict exit 1 and print the diagnostic; snapshot-input mode is unchanged."
  },
  {
    "subsystem": "doctrine / .claude commands (owner action; the flow may not edit .claude/)",
    "locator": ".claude/commands/triage-wave.md:31-33",
    "symptom": "Once #68 lands, step 4 'Gate' tells the operator to move 'the presets.rs name from DEFAULT_DETECTOR_NAMES to OPT_IN_DETECTOR_NAMES' — constants #68 deletes. The demotion step then points at code that no longer exists. (Not pre-existing: #68 causes it; it is listed here because only the owner can edit .claude/.)",
    "kind": "enhancement",
    "origin_issue": 68,
    "reproducer": "After #68: grep -n DEFAULT_DETECTOR_NAMES src -r -> no match; .claude/commands/triage-wave.md:32 still names it.",
    "pre_existing": false,
    "capability": "The triage-wave demotion step names the real one-step rule: move the detector's entry from default_detectors() to opt_in_detectors() in src/engine/l5/detectors/mod.rs, update the T1b opt-in literal and its default count (T1b(4) asserts 40) and the CHANGELOG, and regen the detectorStats goldens.",
    "acceptance": "triage-wave.md step 4 names default_detectors()/opt_in_detectors() and the T1b literal; no reference to the deleted constants remains under .claude/."
  },
  {
    "subsystem": "gate / analyzer diagnostics aggregation",
    "locator": "src/engine/gate/run.rs:~505-545 (run_analyze_with_exit inline aggregation) vs run.rs analyzer_diagnostics_of_run",
    "symptom": "alsem analyze aggregates its diagnostics inline and adds the dependency diagnostics (source 2, from the `fresh` dependency ledger); the shared aggregation used by digest, prove, fingerprint, events and policy check leaves them out. On a workspace with a missing or older-than-declared dependency, analyze reports it and those five commands do not. Two hand-kept copies of one aggregation.",
    "kind": "bug",
    "origin_issue": 68,
    "reproducer": "A workspace whose app.json declares a dependency version newer than the .app in .alpackages: `alsem analyze <ws> --format json` lists a dependency diagnostic; `alsem fingerprint <ws> --format json` envelope diagnostics do not.",
    "pre_existing": true,
    "capability": "every command that reports analyzer diagnostics reports the same set analyze reports in the same mode",
    "acceptance": "run_analyze_with_exit calls the shared aggregation; dependency diagnostics appear in all six commands' diagnostics for the reproducer; a test pins it with a discrimination proof."
  }
]
```

## Not delivered

- (Status refreshed after implementation, final panel round 2.) Implemented in 73e80706,
  f46fd933, c667d15b, bd73eb7e, 9f5ff893, 314fbfbf. The zero-byte golden license is confirmed on
  the changed source: `scripts/check-goldens` exit 0, all 8 targets, zero bytes moved (gate logs
  under `.agent/runs/20261014-122439-d9008f/logs/`). The after-numbers are MEASURED on the changed
  binary (Gate results table), not estimated.
- Cost evidence is CDO only, 2 runs each, and inconclusive (see the paired deltas in Gate
  results). Not measured on BC Base App 8020 (no usable 8020 source workspace in this
  session); only P12's reasoned upper bound (analyze's own detector stage on the same workspace
  and mode) applies there. Large-workspace safety is NOT demonstrated.
- Whole-output parity with `analyze` (model mode, dependency diagnostics) — not promised; #70.
- `.claude/commands/triage-wave.md` doctrine update — owner action (Discovery D4); the flow may
  not edit `.claude/`.
- Dated plan docs under `docs/superpowers/plans/` that name the deleted constants are left as
  historical records.
- The cli-a "default" slot (Discovery D2) stays on the retired TS list; T3's analyze cases cover
  analyze's real default instead, without rebaselining cli-a.
