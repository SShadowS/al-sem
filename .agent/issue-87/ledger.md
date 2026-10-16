# Issue #87 ledger

## Claim

```json
{"run_id":"20261016-184408-697474","issue":87,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/87-agent-discovery-gate-sarif-form-a1","worktree":"U:\\Git\\al-sem-issue-87-a1","body_hash":"590fe8248e73114a7f0fccf9145ec02b06a893d896f18c648c30e5d46aba6be6"}
```

Issue body hash: `590fe8248e73114a7f0fccf9145ec02b06a893d896f18c648c30e5d46aba6be6`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` instead of `gpt-6-astra` and `gemini-3.8-flash`. Marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** One formatter's rule catalog is a hand-kept list that drifted from the registry. The fix moves the rule metadata into the registry, the one source, and derives the catalog from it.

## Design

**Defect.** `format_sarif::rules()` (`src/engine/gate/format_sarif.rs:56`) is a hand-written list of 21 `(id, name, shortDescription)` triples, and a unit test pins its length at 21 (`:464`). The registry has 54 detectors (`registered_detectors()` = `default_detectors()` + `opt_in_detectors()`, `src/engine/l5/detectors/mod.rs:1223-~1573`). A SARIF `result.ruleId` for any of the other 33 (e.g. `d19-unused-parameter`, `d34-commit-in-loop`) names a rule absent from `tool.driver.rules`. SARIF viewers then show it without a name or description.

**Fix.**
1. `Detector` (`src/engine/l5/registry.rs:315`) gains two required fields:
   - `rule_name: &'static str`: the PascalCase SARIF rule name;
   - `short_description: &'static str`: a one-line summary.
   Being required, a detector cannot be registered without them, so the catalog can never drift again.
   - All 54 registry entries fill them. The existing 21 keep their CURRENT text byte-for-byte; their `rule_name` cannot be derived from the id slug, as `d3`'s `MissingSetLoadFields` shows.
   - The 33 new ones get a name and a one-line description written from each detector module's `//!` header doc.
   - The 9 test-only `Detector` literals in `registry.rs` get short placeholder values.
   - `gate/presets.rs:76-80` is PRODUCTION code, not a test literal (spec round 1, both reviewers). `select_detectors()` used to rebuild a selected registry entry field by field for `--detector`/`--preset`. Since CR2 it pushes `d.clone()` (`#[derive(Clone)]` on `Detector`, `registry.rs:315`), so `rule_name` and `short_description` are carried with every other field.
2. `format_sarif::rules()` becomes a projection of `registered_detectors()`, sorted by detector number with the existing natural-order comparator (`registry.rs` `compare_natural`: `d2` < `d10`). The registry order is not numeric (the opt-in list runs `d40, d46…d51, d17, d8, d45, d61…`), so the sort is explicit.
   - The existing 21 (`format_sarif.rs:56-164`) are already in ascending number order, so they keep their relative order, and the 33 are inserted where their numbers fall.
   - `SarifRule.id` becomes an owned `String` (the registry name is a `String`).
   - The hand-written list and its `rule()` helper are deleted, `assert_eq!(rules().len(), 21)` (`:464`) is replaced (see the tests), and the stale module and section docs ("ported verbatim (21 rules)", `:9`, `:30-32`) are rewritten.
   - Results carry only `ruleId` and no `ruleIndex`, so reordering cannot break an index.
3. **Scope.** The SARIF `rules` catalog lists EVERY registered detector, opt-in included, not only those that ran. SARIF consumers resolve a `ruleId` against it. Listing all of them matches the current static form, which already lists 8 opt-in detectors (`d8`, `d17`, `d46`-`d51`). No formatter other than SARIF reads the new fields. The README's detector counts (`README.md:88`, 43 default / 11 opt-in, against the registry's 40/14) are stale independently of this issue, and are corrected in the same change because they describe the same registry.
4. **Golden movement.** Only `alsem analyze`'s SARIF documents use `format_sarif::rules()`. `diff` builds its own catalog (`src/engine/gate/diff/format.rs:377`, `abi.*` rule ids), and so does `policy` (`src/engine/gate/policy/format_policy.rs:360`). Their goldens (`tests/cli-b-goldens/diff/*.sarif`, `tests/cli-c-policy-goldens/*.sarif`) MUST NOT move. Every analyze SARIF golden (under `tests/gate-goldens/` and any cli-a SARIF golden) gains the 33 added `rules` entries and nothing else.
   - **Triage compares PARSED documents**, because a text diff also shows the separator after the old last rule. The 21 old descriptors must stay identical and in order, exactly the 33 new ones are added, and everything outside `tool.driver.rules` must be unchanged. Done before any regen.
   - **Regen trap** (spec round 1, sol): the full-baseline SARIF test (`tests/cli/gate_suppress_baseline_differential.rs:308-318`) has no regeneration branch, and its golden carries the catalog even with empty results. The implementation adds the same guarded `REGEN_TEMP_GOLDENS` branch the neighbouring partial-baseline test uses, so `check-goldens --regen` rewrites it and no golden is hand-edited.

## Assumption probes

| probe | result | evidence |
|---|---|---|
| rules() lists 21 detectors, a test pins 21 | confirmed | `format_sarif.rs:56-164`, `:464` |
| the registry holds 54 (40 default + 14 opt-in) | confirmed: 54 `Detector {` entries in `detectors/mod.rs`; the module's own test checks 40/54 | `detectors/mod.rs:1223-~1573` |
| the existing rules are in ascending detector-number order | confirmed (`d1 … d17, d46 … d51`) | `format_sarif.rs:56-164` |
| rule names are NOT derivable from the id slug | confirmed: `d3-missing-setloadfields` → `MissingSetLoadFields` (the slug gives `MissingSetloadfields`); likewise `setloadfields`, `ishandled`, `tryfunction` in new ids | script over `rules()` |
| every detector module has a `//!` header summary to source descriptions from | confirmed on a sample. Some are one line (d19, d52), others span several (d34 `:1-2`, d60 `:1-4`), so each description is condensed by hand to one line. | `detectors/d*.rs:1-4` |
| only analyze emits `format_sarif`'s catalog | confirmed: `format_sarif(` is called only at `src/engine/gate/run.rs:645` and `:791` (analyze, plus its empty-output path), and in its own unit test | grep `format_sarif(` over `src/` |
| which tests read SARIF goldens, and their regen paths | `.sarif` goldens are read only by `tests/cli/gate_sarif_differential.rs` (regenerates through `maybe_regen`), `tests/cli/gate_suppress_baseline_differential.rs` (every SARIF read regenerates except the full-baseline test, which is S2) `tests/cli/cli_b_diff_differential.rs` (diff's own catalog, which does not move) and `tests/cli/cli_c_policy_differential.rs:53-75,154-182` (policy's own catalog via `format_policy_sarif`, which does not move; it regenerates through `check_or_regen`). The other test files mentioning `sarif` assert in memory and read no SARIF golden. | `grep -rln sarif tests`; per-file read |
| `check-goldens --regen` rewrites goldens | it runs the targets with `REGEN_TEMP_GOLDENS=1` (`tests/common/regen.rs`); the new full-baseline regen branch must gate on that same `regen_mode()` | `scripts/check-goldens:182-183` (the `--regen` block) |
| `Detector` literals outside the registry | 9 in `registry.rs` tests; 1 PRODUCTION literal in `gate/presets.rs:76-80` (`select_detectors`, which copied a registry entry) — pre-CR2; now `out.push(d.clone())` at `presets.rs:76` | grep `Detector {` |

## Base/Head SHAs

- B = `8fd7e7509217dc7b0080497bbbc2d54479691e80` (origin/master)
- H = `9668582a7e7fdd460a1ea5002c294ba84d413901` (approved code head; the evidence commit follows it)

## Timeline

- Worktree created at master `8fd7e750`.
- Classified as bounded; design written.
- Spec panel (sol + opus, the operator roster), 4 rounds (spec_rounds 4; pi_calls 8):
  - r1: both confirmed the registry-derived approach. Important: `presets.rs` is a production literal (copy the fields), and the full-baseline golden has no regen branch. Minors: owned id, parsed-document triage, docs and README, `compare_natural`.
  - r2: sol accepted all; opus re-raised S6 (a stale line range) and raised N1-N5 (evidence rows).
  - r3: both re-raised N3 (the policy SARIF reader was missing from the probe); N6 (citation).
  - r4: both accepted all 12; no new findings. CONVERGED.
- Implementation (opus subagent, TDD; task_attempts T1 = 2): T1-T4 red then green; 4 discrimination proofs. Golden triage + regen (exit 0). Sonnet review: none. Commit `64553966`.
- `code-review` skill at high (Claude Code built-in, run as given): 6 findings CR1-CR6. CR3 refuted, the other 5 fixed by a sonnet review-fix subagent, commit `9668582a`; no golden moved.
- Final panel (sol + opus; final_rounds 3; pi_calls 14):
  - r1: both accepted all 18; both raised F1 (ledger still cited the deleted T4).
  - r2: sol accepted all; opus re-raised N4 (an undocumented hash convention) and raised F2 (stale probe row, proof order).
  - r3: both accepted all 20; no new findings. CONVERGED.
- Rebase check: `origin/master` = `8fd7e750` = the branch base; no rebase needed.

## Acceptance matrix

| Acceptance item | Proof |
|---|---|
| A test fails when a registered detector has no SARIF rule, or a SARIF rule names an unregistered detector | **T1** (unit, `format_sarif.rs` tests): the set of `rules()` ids EQUALS the set of `registered_detectors()` names (both directions), every rule has a non-empty `name` and `shortDescription`, the ids are unique, and they are sorted by detector number. **T2**: the 21 pre-existing rules keep their exact `(id, name, shortDescription)`, pinned as literals so a careless rewrite of an old description is caught. **T3** (cli): `alsem analyze tests/r0-corpus/ws-d34 --format sarif --single-app` (the issue's reproducer) gives a document where every `results[].ruleId` is in `tool.driver.rules[].id`, including `d19-unused-parameter` and `d34-commit-in-loop`. |
| SARIF goldens move by the added rules only | A parsed-document triage of every moved golden: the old 21 descriptors identical and in order, exactly the 33 added, nothing outside `tool.driver.rules` changed; diff and policy SARIF goldens unmoved. `select_detectors` keeps the metadata by construction: since CR2, `Detector` derives `Clone` and `select_detectors` clones the registry entry (`presets.rs`), so no field can be dropped. The earlier **T4** test, which guarded the hand-written copy, was deleted with it; its proof below (iii) is historical. |

## Golden triage

- Before the regen: a receipt (`<worktree>/.agent/golden-triage.md`) names the 44 analyze SARIF goldens under `tests/gate-goldens/` (40 from `gate_sarif_differential`, 4 from `gate_suppress_baseline_differential`). It predicts what the regen must show: each document gains exactly the 33 new descriptors, the 21 old ones stay identical and in order, and nothing outside `tool.driver.rules` changes.
- The regen ran as `agentflow run --name check-goldens-regen`: exit 0, 141.8 s, supervised (log `.agent/runs/20261016-184408-697474/logs/check-goldens-regen/20261016T191343-c0c1c317.log`).
- Verified after the regen, comparing parsed JSON with a script:
  - all 44 moved documents: the old descriptors are identical and in order, `rules` has exactly 33 more entries, all ids are unique, and the document is otherwise equal;
  - text diff: 10164 insertions and 0 deletions (= 44 × 33 × 7 lines);
  - no golden outside `tests/gate-goldens/` moved; the diff (`tests/cli-b-goldens/diff`) and policy (`tests/cli-c-policy-goldens`) SARIF goldens are unchanged.
- The script's first run reported false mismatches: it decoded `git show` output as cp1252. Rerun with UTF-8 decoding: clean.

## Discrimination proofs

The implementer made each break with Edit, ran the tests, then reverted with Edit and touched the file. Details are in `impl.md`.

| # | Break | Failing test and line | After revert |
|---|---|---|---|
| i | `rules()` filters out `d19-unused-parameter` | T1: `SARIF rules and registered detectors differ`; T3: `ruleIds with no tool.driver.rules entry: ["d19-unused-parameter"]` | T1, T3 pass |
| ii | d10 `short_description` changed | T2: `left: "Loop that modifies its own cursor"` / `right: "Self-modifying loop"` | T2 passes |
| iii (historical: T4 and the field copy were deleted for CR2) | `select_detectors` sets `rule_name: "Selected"` | T4: `left: "Selected"` / `right: "UnusedParameter"` | T4 passes |
| iv | the `compare_natural` sort deleted | T1: `rules out of order: d60-… before d40-…` | T1, T2 pass |
| v (CR1) | d19's `DETECTOR` changed to `"d19-unused-parameterX"` | the rules `id` and both result `ruleId`s become `d19-unused-parameterX` together (lines 125, 395, 449 of the SARIF output), so they cannot disagree; T3 fails only on its literal guard `fixture no longer yields a d19-unused-parameter result` | reverted, T3 passes |

T3 was also red against the unchanged binary: `ruleIds with no tool.driver.rules entry: ["d19-unused-parameter", "d34-commit-in-loop"]`.

## Findings register summary

There are 20 entries, and both reviewers accepted all of them at code-diff hash `7f741e19e53a0d35`:
- spec panel: S1-S6 and N1-N6;
- code review: CR1-CR6, where CR3 was refuted and the rest were fixed;
- final panel: F1 and F2.

No entry is open or deferred.

## Gate results

All 4 gates ran under the supervisor on head `9668582a` and returned exit 0. The worktree's `git status --porcelain -- . ':!.agent'` was empty afterwards. Logs are under `.agent/runs/20261016-184408-697474/logs/`.

| gate | exit | seconds | log |
|---|---|---|---|
| ci-steps-all | 0 | 696.7 | `ci-steps-all/20261016T193148-097da657.log` |
| check-goldens-coverage | 0 | 0.3 | `check-goldens-coverage/20261016T194325-ab11ede9.log` |
| check-goldens | 0 | 63.1 | `check-goldens/20261016T194325-a175950c.log` |
| cdo-gate | 0 | 248.8 | `cdo-gate/20261016T194429-5c41e55e.log` |

An earlier gate run, on `64553966`, was stopped before it finished, so that the code-review fixes would be gated; its results were not used.

## Discoveries

None. The one stale fact found next to this issue was README's d8 row, which did not mark d8 as opt-in. It was fixed in this change (CR5).

## Not delivered

Nothing. Every acceptance item is met.
