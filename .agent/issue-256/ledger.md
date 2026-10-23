# Issue #256 — d35: an unknown (opaque-callee) answer is reported at high severity

## Claim

```json
{"run_id":"20261023-204620-399a7d","issue":256,"attempt":1,"session":"claude-code-session:57653578-e111-4b15-8684-017371f816fa","branch":"issue/256-d35-an-unknown-opaque-callee-ans-a1","worktree":"/work/al-sem-issue-256-a1","body_hash":"e64b46c1398d8fc95cbb2dfc166a515caf9546911c4f4b15a0e3c74d9b9b65bb","title":"d35: an unknown (opaque-callee) answer is reported as a high-severity finding, indistinguishable from a found Commit","reconciled":[]}
```

Issue body hash (claim `body_hash`): `e64b46c1398d8fc95cbb2dfc166a515caf9546911c4f4b15a0e3c74d9b9b65bb`.
Base (`make-worktree`): `adde813c5a356f05b47cd2200add529a710cf4fb`.
Reviewers (operator instruction, 2026-10-23, confirmed again for this queue): `gpt-6.1-sol` in the astra slot, Opus in the flash slot; attest with `--substitute astra=sol --substitute flash=opus`. Marks stored as `reviews: {sol: …, opus: …}`.
Operator: queue #256 → #250 → #252 (from alsem-main), loop budget reset; claimed with `--loop-max 3`.

## Classification

**Bounded.** One detector (`d35`), one decision from the issue's three options, fixtures and a CDO before/after. No engine resolution change.

## Assumption probes

| probe | result | evidence |
|-------|--------|----------|
| P1 d35 has three answer arms, all `high` | holds | `src/engine/l5/detectors/d35.rs:11-13` (module doc), `:193` `severity: "high"` for every arm; unknown arm title "Commit may be reachable from event subscriber (incomplete call coverage)" |
| P2 `summary.bySeverity` counts the finding's `severity` string only | holds | `src/engine/gate/format_json.rs:304-318` |
| P3 a detector may pick severity per finding | holds | `d34.rs:198` (`critical` at depth ≥ 2 else `high`); d16/d33/d38/d40/d43/d47/d51 also compute it |
| P4 CDO before (pinned `Cloud` @ `bc3ccb18`, dirty 0, al-sem 2.0.2 at `adde813c`, release-fast; `cdo-measure` run → `/work/bench/256/measure/before/`) | unknown dominates d35 | `--scope primary`: direct 1 (high, likely), via callee 10 (high, possible), **unknown 41** (high, possible) of 52; `--scope all`: 3 / 76 / **83** of 162. d35-unknown is 41 of 577 primary high findings (7.1%), 83 of 2776 all (3.0%). Unknown reasons (primary): {ambiguous-overload, external-target, interface-open-world} 35, {interface-open-world} 6; `confidence.cappedBy` opaque-callee 6, dynamic-dispatch 1, none 34 |
| P5 CentralGauge's run tree is on this host | no | alsem-main: their session runs on the operator's Windows machine |
| P6 CentralGauge's standalone reproduction reproduces here (al-sem at `adde813c`) | yes | `/work/bench/256/repro-{ext,closed,direct,direct-closed}` (sonnet probe): no `.alpackages` needed (only `DIAG-dependencies` warnings, exit 0); each variant gives 1 × d3 + 1 × d35, d35 = unknown arm, high, possible, `cappedBy` [opaque-callee] (+ dynamic-dispatch when the call is in the subscriber), reason `interface-open-world`. **`Extensible = false` changes nothing**: the `Strategy.NextKm` edge reads `dispatch_kind: Interface, resolution: Maybe, candidates: None, dispatch_meta {total_impls: 1, unresolved_impls: [], enum_implementers: [Enum 50102]}` in all four (`aldump --switch-dump`). For `Extensible = true` open-world is honest; for `false` with the value taken from that enum's field the set is closed, so open-world there is a resolver precision gap (discovery, below), not this issue's rule |

## Design

Spec = this section (bounded issue).

**Rule (the issue's option (a)).** The UNKNOWN arm gets severity **`medium`**. Exactly: no direct Commit in the subscriber's own body (`operation_sites` has no `commit`) AND no proven Commit in its cone — the commit answer is UNKNOWN because `may_commit_derived` returns `Unknown` or the subscriber has no effect summary (`d35.rs:62-69`). The direct arm and the proven (`yes`, "via callee") arm stay **`high`**, unchanged in every field. The severity is carried in the arm tuple of the existing `match (direct_commit, summary_commits)` (`d35.rs:100-106`), never computed from `summary_commits` alone: a DIRECT Commit whose cone is also unknown has `summary_commits == "unknown"` and must stay `high`/`likely` (pinned by a lib test, below). Nothing else on the unknown finding changes: same id (`d35/{routine}/transitive`), same rootCauseKey, title "Commit may be reachable from event subscriber (incomplete call coverage)", confidence `possible` with its `cappedBy`, root cause, note and fix hint. The `"no"` skip is unchanged.

**Why (a), not (b) or (c).** (a) puts the difference exactly where consumers already look: the finding's `severity` and `summary.bySeverity` (which counts that string, P2), with no schema change and no new detector id, so `byDetector` keeps working, and baselines and suppressions keep matching: the finding fingerprint hashes detector, object, routine name, tables and the stabilised rootCauseKey (`l5/fingerprint.rs:224-231`), and the loose fingerprint hashes the same parts with the routine name blanked and the anchor routine stripped from the rootCauseKey (`l5/fingerprint.rs:165-181`) — neither hashes severity; the d35 id and rootCauseKey carry no severity (`d35.rs:176-185`); baselines match on fingerprint (`gate/baseline.rs:75-86`); inline suppressions on detector, file and line (`gate/inline_suppression.rs:239-264`). `confidence` cannot separate the arms (yes and unknown are both `possible`), so severity is the field that can. Severity per finding is established practice (P3). (b) a new detector id would split one rule across two ids, break existing suppressions and baselines keyed on d35, and needs a registry/triage-wave cycle. (c) a separate summary count leaves `bySeverity.high` mixed, which is the reported defect (CentralGauge counts high). `medium` (not `low`/`info`): a possibly-committing subscriber is still a real risk that deserves review; it is just not a found defect.

**Surfaces that move (all keyed on the severity string, for a d35-unknown finding only).** analyze JSON `severity` and `summary.bySeverity` (`format_json.rs:304-318`); SARIF level `error → warning` (`format_sarif.rs:24-32`; GitHub code scanning treats the two differently); the PR summary's severity group and counts (`format_pr_summary.rs:28-29, 54-95`); terminal grouping/sort, where a rolled group keeps its highest contributor's severity (`format_terminal.rs:41-42, 98-130`); HTML colour, counts and grouping (`format_html.rs:49-50, 799-894`); `analyze --fail-on high` no longer fails on it (`exit_code.rs:29-54`, fed from `run.rs:733`), `--fail-on medium` still does; `analyze --min-severity high` no longer shows it (`filter.rs:32-39`).

**Not moved, stated so nobody chases them.** `alsem policy check` builds its own findings with the policy rule's severity and does not read d35 (`policy/pipeline.rs:180-188, 337-357`, `policy_engine.rs:441-458`, bundled `policy-default.yaml:7-14`); `alsem diff` severities are semantic-diff findings (`diff/mod.rs:45, 97, 176-183`); summary caches hold summaries, not findings (no `CACHE_VERSION_*` bump).

**Goldens.** No committed golden holds a d35 unknown finding: `grep -rl "may be reachable from event subscriber" tests` matches only the inline tests `tests/cli/cli_d35_unknown_commit.rs` and `tests/cli/cli_step5_coverage_fold.rs`; every d35 finding in the goldens (`ws-d35`, `ws-d8-commit-in-tx`, gate SARIF/PR summary) is the direct or via-callee arm. **The expected golden diff is zero lines**; `scripts/check-goldens` (no `--regen`) must stay green, and any moved golden is a discovery. Those unchanged goldens are also acceptance item 3's byte-for-byte proof.

**Tests (no new r0-corpus fixture: it would move r4/r2c/l2 goldens).** In `tests/cli/cli_d35_unknown_commit.rs` (inline temp workspaces, no golden family): (1) the existing unknown-arm assertion (`severity == "high"`, `:205`) flips to `medium`, with wording and confidence assertions kept — restoring `high` is the discrimination proof; the yes-arm assertion (`:237`) stays `high`; (2) its `analyze(ws, Some(D35))` helper (`:139-166`) returns the whole report, and V1 asserts `payload.summary.bySeverity == {"medium": 1}` EXACTLY (no `high` key); (3) a new case: an event subscriber whose ONLY call is to a symbol-only dependency method (package from `tests/common/symbol_app.rs:7-62`), no Commit in any supplied source → exactly one d35 finding, the unknown arm, `medium`, `possible`. Pre-stated fallback: if that run gives NO d35 finding (the cone answers `"no"` for a symbol-only callee), the case uses an unresolved call instead (a Variant member call, `dynamic-target`), and the "symbol-only callee proven commit-free" result is filed as a discovery (an opaque body cannot prove the absence of a Commit); (4) a new case matching CDO's population (P4: every unknown carries `interface-open-world`): the subscriber's only call goes through an interface variable whose dispatch is not closed → unknown, `medium`, reasons name `interface-open-world`; if a single-app inline workspace cannot produce that reason, the case uses the ambiguous-overload shape and `cli_step5_coverage_fold.rs`'s `step5_t7` (`:388-404`) gains a `severity == "medium"` assertion; (5) `--fail-on high` → exit 0 and `--fail-on medium` → exit 1 on an unknown-only d35 run; (6) **CentralGauge's reproduction** (issue comment 6374601820: 6 AL files + app.json, no dependencies, no `.alpackages` needed — probe P6) as an inline temp workspace, files verbatim, run without `require_dependencies` (the inline workspace has no packages): exactly one d35 finding on `ReleaseOnRegistered`, the unknown arm, `severity == "medium"`, `confidence == {"level": "possible", "cappedBy": ["opaque-callee"]}`, fix hint naming `interface-open-world; partly analysed: IsDue (Codeunit 50103)`; and under the DEFAULT detector selection `summary.bySeverity == {"medium": 2}` exactly (d3 + d35; today `{"high": 1, "medium": 1}`, CentralGauge's complaint — a second restore-`high` proof); (7) the same with the interface call made directly in the subscriber (`Strategy := DemoItem.Kind; if Strategy.NextKm(DemoItem.Km) > 0 then exit;`), the case CentralGauge did not test: unknown arm, `medium`, `cappedBy == ["dynamic-dispatch", "opaque-callee"]`, fix hint naming `partly analysed: ReleaseOnRegistered (Codeunit 50104)`, default-selection `bySeverity == {"medium": 2}`. Cases (6)/(7) replace (4) if they already produce `interface-open-world` (they do, P6), so (4) is not built separately. In `d35.rs`'s lib tests: flip `:344` to `medium`, and add **direct Commit in the body + Unknown/absent summary → `high`/`likely`** (proof: key severity on `summary_commits` → red).

**CHANGELOG** (`### Changed`), plain English: "d35 (Commit in an event subscriber) now reports an unknown answer — no Commit found in the subscriber or proven in its callees, but call coverage incomplete — at medium severity instead of high. A Commit found directly or via a callee stays high. Confidence, ids, fingerprints and loose fingerprints are unchanged, so baselines and suppressions keep matching. Such findings no longer trip `analyze --fail-on high` or show under `--min-severity high`; SARIF reports them as warnings. `alsem policy check` is unchanged. On the pinned CDO workspace, 41 of 52 d35 findings (primary scope; 83 of 162 at --scope all) move to medium; primary `bySeverity.high` drops from 577 to 536. This supersedes #148's note that the unknown answer keeps severity high." Plus the module doc (`d35.rs:13`, `:15-17`) is updated; the historical #148 entry is left as written.

**Not changed.** The d35 unknown arm's other fields, the direct/yes arms, d35's candidate set and skip logic, every other detector.

## Acceptance matrix

| # | Acceptance | Proof |
|---|------------|-------|
| 1 | unknown distinguishable from found in the finding and `summary.bySeverity`; rule in CHANGELOG | unit/fixture test asserting severity `medium` + unchanged title/confidence on the unknown arm; CLI analyze JSON test asserting the d35-unknown finding counts under `bySeverity.medium`, not `high`; CHANGELOG entry |
| 2 | fixture: subscriber whose only callee is opaque, no Commit → new shape, with a discrimination proof (restore `high` → fails) | §Tests (3) symbol-only-callee case, (6) CentralGauge's reproduction and (7) its direct-call variant in `cli_d35_unknown_commit.rs`; proof: restore `high` |
| 3 | real Commit direct and via callee → high, unchanged | the issue's wording says "the `likely` high finding", but in the code direct is `high`/`likely` (`d35.rs:126`) and via-callee `high`/`possible` (`d35.rs:141`); "unchanged" means exactly that. Proof: the existing `ws-d35` goldens (cli-a json/html/terminal, r4, gate SARIF/PR summary) unchanged byte for byte + the direct-plus-unknown-cone lib test |
| 4 | CDO before/after per answer kind; direct and via-callee unchanged | `cdo-measure` after-run + `comp`: direct 1/3 and via-callee 10/76 unchanged in count and every field; unknown 41/83 change only in `severity`; ordinary and loose fingerprint churn 0; `bySeverity` totals before/after |
| 5 | CentralGauge run tree as a regression fixture | CentralGauge posted a standalone reproduction (P6); it reproduces outside CDO on the current binary; §Tests (6) pins it verbatim (and (7) the direct-call variant), with the restore-`high` proof |

**Measurement plan.** The CDO after-run is the item-4 proof (the before-run is P4). No timing: the change is one branch.

## Base/Head SHAs

- Claim base (`make-worktree`): `adde813c5a356f05b47cd2200add529a710cf4fb`.
- After the step-11 rebase: **B** = `0b0b95c9c43db63038485a4ab6c20209a79172fd` (`origin/master`), **H** = `7e6879fc436e2c5b657ccf60d75fe6ec8d4fe8dc`.

## Timeline

- 20:46 claim #256 attempt 1 (`--loop-max 3`), worktree at `adde813c`.
- CDO baseline probe of d35 answer kinds dispatched (sonnet; subagents 59).
- Acceptance item 5 (CentralGauge's run tree): alsem-main reports the CentralGauge session runs on the operator's Windows machine and its trees are not on this host; it has asked the operator for the tree and will forward a path if one arrives before the final panel. Until then: pinned on our own fixture, item recorded under Not delivered.
- Spec round 1 (`spec_rounds` remaining 4; pi_calls 19; subagents 58): sol approve-with-fixes (10), opus approve-with-fixes (10). Design revised.
- Spec round 2 (remaining 3; pi_calls 18): sol 19/20 (S6 re-raised: loose-fingerprint wording); opus 20/20 + O11 (line cite; symbol-only fallback).
- Spec round 3 (remaining 2; pi_calls 17): both approve 7/7 (S1 S4 S6 O4 O5 O8 O11). Converged at 21 entries.
- alsem-main forwarded CentralGauge's standalone reproduction (issue comment 6374601820, saved to `/work/bench/256/cg-repro.md`): subscriber → `Demo Publisher.IsDue` → `Strategy.NextKm()` on an interface variable assigned from an enum field; the enum has one value and one implementation but **`Extensible = true`**; d35 high/possible, `cappedBy` opaque-callee, reason `interface-open-world`. An extensible enum can gain a committing implementation in another app, so the open-world answer is honest; the severity change is the remedy for this shape. Probe of four variants (as posted / `Extensible = false` / direct interface call from the subscriber / both) dispatched (sonnet; subagents 57) to see whether the closed variants still read open-world (a resolver precision question, not this issue's rule).
- Probe P6: all four variants identical (closed enum still open-world → discovery). Design amended: Tests (6) verbatim repro, (7) direct-call variant; item 5 delivered.
- Spec round 4 (remaining 1; pi_calls 16): ids S4 S8 S9 O4 O5 O9 O11 + A1 (alsem-main's repro request). sol approve 8/8; opus 8/8 + O12 (stale row-2 cite), O13 (assert exactly what the probe saw).
- Spec round 5 (remaining 0; pi_calls 15): S4 S8 O4 O5 O9 O11 O12 O13 — both approve 8/8. **Converged**: 24 entries, all accepted by both, none open; blocking entries all fixed. Implementer note (opus r5): assert (7)'s whole confidence object too.
- Plan `docs/superpowers/plans/2026-10-23-issue-256-d35-unknown-medium.md`, 1 task (`plan_tasks` 11 left). Bounded + 1 task → no per-task reviewer.
- Steps 6+8 by one opus agent (subagents 56; task_attempts T1): Phase A red — lib `detectors::d35` 2 passed / 5 failed, CLI `cli_d35_unknown_commit::` 4 passed / 5 failed, every failure `left "high" right "medium"` (or `--fail-on high` exit `left 1 right 0`); the direct/yes assertions passed. Phase B: severity is the 6th member of the arm tuple (direct high, yes high, unknown medium). Commits `0cfe1416` (plan doc), `2386ad89` (d35.rs, cli_d35_unknown_commit.rs, CHANGELOG.md). Tests (1)(2)(3)(5)(6)(7) + lib `direct_commit_with_unknown_cone_stays_high`; (4) folded into (6)/(7); Tests (3) needed no fallback (symbol-only callee → unknown arm). task-affected `3911 passed, 8 skipped`; check-goldens `1028 passed, 4 skipped`, zero files changed; check-diff `{"reasons": []}`.
- Task gate green (31.7 s); `push-branch` → `2386ad89`; draft PR **#262**.
- Final round 1 (`final_rounds` remaining 4; pi_calls 14; subagents 55): built-in `code-review` at high (forked skill, 5 candidates), sol approve (no findings), opus approve (3 minors). CDO after-run (bench) recorded under Gate results. Fixes: `86daac67` (FR1 symbol-only precondition, FR6 comment), by the same implementer (task_attempts F1). FR2 closed by the measurement; FR3 deferred to a discovery; FR4/FR5/FR7 refuted. Task gate green (44.7 s).
- Final round 2 (remaining 3; pi_calls 13): FR1–FR7. sol approve 7/7.
- Step 11 rebase: old base `adde813c`, old head `86daac67`, `origin/master` `0b0b95c9` (#253 `b3686bde`, `Bump version to 2.0.3`). Clean, no conflict. **Range-diff: all 3 commits `=`** (`0cfe1416→4f38649a`, `2386ad89→0fe408d5`, `86daac67→7e6879fc`) — no extra panel round. Register re-pointed (hashes kept). Full gates started on `7e6879fc`.

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|------|----------|-------------|-------------|--------|
| 5 lib unknown tests + CLI V1, fail-on, symbol-only, cg, cg-direct | unknown arm `"medium"` → `"high"` | lib 5 FAILED at d35.rs:354 `left "high"`; CLI 5 FAILED (:236, :293 `left 1 right 0`, :347, :503 ×2) | lib 7/7, CLI 9/9 ok | 2386ad89 |
| V1 `bySeverity {"medium":1}`, cg and cg-direct `{"medium":2}` | same, with the per-finding severity asserts dropped for one run so the map checks are reached | V1 :282 `left {"high":1}`; cg/cg-direct :504 `left {"high":1,"medium":1} right {"medium":2}` | asserts restored, 9/9 ok | 2386ad89 |
| direct_commit_with_unknown_cone_stays_high | severity keyed on `summary_commits == "unknown"` instead of the arm | d35.rs:374 `left "medium" right "high"` | 7/7 ok | 2386ad89 |
| v1_unknown_only_passes_fail_on_high_and_fails_fail_on_medium | unknown arm back to `"high"` | :291/:293 `left 1 right 0` | ok | 2386ad89 |
| symbol_only_callee_is_unknown_at_medium | `write_symbol_app` wrapped in `if false { … }` | :351 left `"… (external-target; partly analysed: OnBeforeDeleteOrder (Codeunit 50101)) …"` right `"… (opaque-dependency; partly analysed: Work (Codeunit 50300)) …"` | ok | 86daac67 |

## Acceptance result

All five acceptance items are met: 1 lib `assert_unknown_with` (`medium`, 6 unknown tests) + CLI `assert_unknown_finding` + V1 `bySeverity == {"medium": 1}` + the CHANGELOG `### Changed` entry; 2 `symbol_only_callee_is_unknown_at_medium` (precondition pinned by the exact fix hint) + the CentralGauge cases, restore-`high` proofs recorded; 3 `direct_commit_with_unknown_cone_stays_high` + V2/V5 yes-arm `high` + the unchanged `ws-d35` goldens (check-goldens zero files changed); 4 the CDO after-run (Gate results); 5 `centralgauge_repro_is_unknown_at_medium` (verbatim) + `centralgauge_repro_direct_interface_call_is_unknown_at_medium`. Both final reviewers marked items 1–5 met.

## Findings register summary

31 entries: spec panel S1–S10, O1–O13, A1 (5 rounds); final panel FR1–FR7 (2 rounds; the built-in `code-review` at high ran as the specified step). 27 `fixed`, 3 `refuted` (FR4 default-selection map is spec decision O13; FR5 ledger committed at Freeze by design; FR7 generic severity code already tested), 1 `deferred` (FR3, minor, filed as a discovery). Every entry accepted by both reviewers (`sol` = gpt-6.1-sol in the astra slot, `opus` = Opus in the flash slot); no `open`, no blocking entry deferred.

## Gate results

**Acceptance item 4 — CDO after-run** (`cdo-measure`, through the supervisor as `bench`, 117.3 s, log `.agent/runs/20261023-204620-399a7d/logs/bench/20261023T212415-0b64b934.log`; binaries `/work/bench/256/{before,after}`, runs in `/work/bench/256/measure/{before,after}`). Both sides: the pinned CDO workspace @ `bc3ccb18eb68`, dirty 0, every command exit 0. `comp`: analyze-primary 2323 → 2323 findings, fingerprints 0 added / 0 removed / 2316 kept; analyze-all 30800 → 30800, 0 / 0 / 30772 kept.

| scope | answer kind | before | after |
|-------|-------------|--------|-------|
| primary | direct | 1 high/likely | 1 high/likely |
| primary | via callee | 10 high/possible | 10 high/possible |
| primary | unknown | 41 high/possible | 41 **medium**/possible |
| all | direct | 3 high/likely | 3 high/likely |
| all | via callee | 76 high/possible | 76 high/possible |
| all | unknown | 83 high/possible | 83 **medium**/possible |

`bySeverity` primary: high 577 → 536, medium 861 → 902 (critical 181, info 679, low 25 unchanged); all: high 2776 → 2693, medium 8429 → 8512 (others unchanged). `jq -S` over every d35 finding with `severity` deleted: 0 diff lines (both scopes); over every non-d35 finding: 0 diff lines. So exactly the unknown findings' `severity` moved; the CHANGELOG's numbers are measured, not derived.

**Attested gate set on H = `7e6879fc`** (all supervised, queued 0 s):

| gate | exit | seconds | log |
|------|------|---------|-----|
| ci-steps-all | 0 | 448.1 | `.agent/runs/20261023-204620-399a7d/logs/ci-steps-all/20261023T213146-98d5dcbf.log` |
| check-goldens-coverage | 0 | 0.0 | `.agent/runs/20261023-204620-399a7d/logs/check-goldens-coverage/20261023T213914-9a7daac8.log` |
| check-goldens | 0 | 0.1 (tree-stamp trust of the green `ci-steps test` on this exact tree) | `.agent/runs/20261023-204620-399a7d/logs/check-goldens/20261023T213915-71a461c4.log` |
| cdo-gate | 0 | 320.8 | `.agent/runs/20261023-204620-399a7d/logs/cdo-gate/20261023T213915-1697e13a.log` |

`git status --porcelain -- . ':!.agent'` empty after the gates. cdo-gate exit 0: its ratchets held (the change touches no resolution). Earlier task gates: 31.7 s and 44.7 s, both 0.

## Discoveries

```json
[
  {
    "subsystem": "resolver / interface dispatch (enum-sourced values)",
    "locator": "src/engine/l4/combined_graph.rs:358-381 (every resolved interface site gets an interface-open-world uncertainty); the call edge carries dispatch_meta.enum_implementers but resolution Maybe; src/engine/l5/snapshot.rs:2186-2215 (every interface site polymorphic, openWorld true)",
    "symptom": "An interface call whose receiver is assigned from a field of a NON-extensible enum (Extensible = false) is still reported open-world, although the possible implementations are exactly the enum's Implementation (plus DefaultImplementation/UnknownValueImplementation) codeunits. Downstream this makes cones 'unknown' (d35 reports a may-commit finding on a subscriber with no Commit anywhere) and digest --tests marks the traversal partial.",
    "kind": "enhancement",
    "origin_issue": 256,
    "reproducer": "CentralGauge's #256 reproduction (issue comment 6374601820) with `Extensible = false` on enum 50102: `alsem analyze . --format json` still gives the d35 unknown finding with reason interface-open-world; `aldump --switch-dump` shows the Strategy.NextKm edge as resolution Maybe, candidates None, enum_implementers [Enum 50102], total_impls 1.",
    "pre_existing": true,
    "capability": "interface dispatch whose receiver value provably comes from an enum that is not `Extensible = true` (the property set to false or absent — to be confirmed against the AL default) (assignment from a field, variable or parameter of that enum type) is classified closed over the enum's implementations (conditionalResolved / a closed candidate set), with no interface-open-world uncertainty",
    "acceptance": "On the reproduction with Extensible = false, the NextKm edge has the enum's implementation(s) as candidates and no open-world uncertainty, and d35 reports no finding (the cone is commit-free); with Extensible = true it stays open-world; an interface variable from a parameter or a codeunit assignment is unchanged; CDO's unknown/ambiguous ratchets and goldens move only where such enum-sourced dispatch exists, each triaged."
  },
  {
    "subsystem": "l5 / d35 message (coverage reasons)",
    "locator": "src/engine/l5/detectors/d35.rs coverage_reasons (~:254, unknown_targets includes the subscriber itself)",
    "symptom": "When the subscriber's OWN call is the one that cannot be resolved (an interface call directly in the subscriber), the unknown finding's root cause and fix hint say 'partly analysed: <the subscriber itself>' — pointing the reader back at the routine the finding is already on, instead of at the call that could not be resolved.",
    "kind": "enhancement",
    "origin_issue": 256,
    "reproducer": "tests/cli/cli_d35_unknown_commit.rs centralgauge_repro_direct_interface_call_is_unknown_at_medium: the fix hint reads 'Check what could not be analysed (interface-open-world; partly analysed: ReleaseOnRegistered (Codeunit 50104))'.",
    "pre_existing": true,
    "capability": "the unknown answer names the unresolved call site(s) (callee text and line) when the gap is in the subscriber's own body, and lists only OTHER routines under 'partly analysed'",
    "acceptance": "For the direct-call reproduction the message names the Strategy.NextKm call (and its line) and not ReleaseOnRegistered under 'partly analysed'; the IsDue-path reproduction still names IsDue; the test that pins today's text is updated deliberately."
  }
]
```

## Not delivered

- Nothing from the acceptance. Out of scope and filed as a discovery: closing interface dispatch over a non-extensible enum (P6).
