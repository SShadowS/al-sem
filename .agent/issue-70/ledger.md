# Issue #70 — cross-app follow-ups from #66

## Claim

```json
{"run_id":"20261015-134131-70bf6e","issue":70,"attempt":1,"session":"claude-code-session:cc1781f3-fe4e-414a-aea6-4919dc8d314c","branch":"issue/70-cross-app-follow-ups-from-66-pr-a1","worktree":"U:\\Git\\al-sem-issue-70-a1","body_hash":"9746551861139495e893fdff256373e75b78b47965d430146b4f4ee1d23fbfe8"}
```

Issue body sha256 `97465518…bfe8` (from the claim). Worktree created from `master` @
`b77025e4ed43e1c831f4f09f1684524c2add59e9`. Grammar `tree-sitter-al` `main` @ `8682cb5f`, used through
`TREE_SITTER_AL_PATH`.

## Classification

**ARCHITECTURAL.** The probe agent proposed "bounded", because every piece reuses #66's
machinery. The conductor classifies it architectural for three reasons. It changes 3 subsystems
(the events/policy CLI, the snapshot fingerprint, and the cross-app base's leaf predicate, which
`alsem analyze` shares). It moves two golden families. And two of the issue's premises are
falsified, so the Acceptance needs a reviewed restatement. Spec:
`docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-design.md`.

## Assumption probes

All probes, with evidence, are in
`docs/superpowers/specs/2026-10-15-issue-70-cross-app-follow-ups-probes.md` (sections A-E,
committed with the spec). They were run read-only by an opus subagent on `b77025e4` with a
release-fast `alsem` and an embedded-source dependency fixture
(`$SCRATCH/70/tools/mkfx.py`, outputs `$SCRATCH/70/out/`). The key results:

| probe | result | evidence |
|---|---|---|
| A10 events today | single-app `events fanout/chains` reports **0 events** when a workspace routine subscribes to a dependency event | probes A10 |
| C1-C3 prove soundness | cross-app digest reports `coverage: complete`, no effects, on a workspace → dependency CALL chain that commits. A cross-app prove built the same way would answer `may-commit: no` (false), where single-app answers `unknown`. **Falsifies "bring prove onto the cross-app base" as safe today** | probes C |
| D2 fingerprint premise | **partly false**: `workspaceFingerprint` already hashes the packages in the workspace's OWN package cache. It misses only ANCESTOR caches, which the loader also reads | probes D2 |
| D7-D8 goldens | 171 golden files pin a fingerprint value, and none has a dependency input | probes D7, D8 |
| E3-E7 leaf premise | **false as written**: dependency CALL edges are already cut, so a fixed leaf differs from a solved summary only at a dependency event PUBLISHER with subscribers. There, cross-app digest and `query effects` DISAGREE today: digest reports 2 `DB_INSERT`s without `tempState`, query reports nothing | probes E |

## Base/Head SHAs

- Base: `b77025e4ed43e1c831f4f09f1684524c2add59e9` (`master`; = `origin/master` at step 11, no rebase)
- Head (approved diff): `955b843ec550c730e6bdecaf41c962a0ab28f5ea`. Commits: `5b874f17` spec, `e42e03dc` T1,
  `ac85f65e` T2, `7d8300b2` T3, `1c143140` T4, `4efaa593` RF1, `1b0cf26f` RF2, `6593d90f` and
  `955b843e` CHANGELOG.

## Timeline

- 2026-10-15T11:43Z — claimed (attempt 1), worktree created.
- Steps 2-4: an opus probe/spec subagent (`subagents` 59 left) wrote the probes, the spec draft and
  the classification. The conductor placed the spec and probes under `docs/superpowers/specs/` and
  reclassified the issue ARCHITECTURAL.
- Spec round 1 (`spec_rounds` 4 left, `pi_calls` 18): astra 6 important + 1 minor, "not ready";
  flash 1 important + 2 minor, "ready". The two most consequential claims were source-verified
  (`dependencies.rs:356-378` has no own-`.git` check; `full.rs:2160-2167` emits `missing` rows, and
  24 r0-corpus fixtures declare `platform`). All 10 were fixed in the spec: R1-01…R1-08, R1-10,
  R1-11 (`findings.json`). Reviews: `.agent/runs/20261015-134131-70bf6e/spec-r1-{astra,flash}.md`.
- Spec round 2 (`spec_rounds` 3 left, `pi_calls` 16): flash accepted all 10, approved both
  deviations, "ready". astra re-raised R1-04 and R1-05, approved both deviations (a: shared
  closure entries instead of the literal `closure-v2:` string; b: the restated leaf mutation and
  the indistinguishable single-app store), and raised 4 new important findings (R2-01…R2-04),
  "not ready". All were fixed in the spec: per-mode statement assertions, a real read failure
  in 2f, `fixed_roles` for dependency publishers (new test 3c), hand-stated policy counts, valid
  inherited-fact YAML, and category hashes for unreadable inputs (new test 2h).
- Spec round 3 (`spec_rounds` 2 left): flash all accepted, "ready". astra re-raised R2-02 (one stale
  §1 sentence), and raised R3-01 (important: `fixed_roles` must reach the detector-context solve)
  and R3-02 (minor: 1f mutation). All fixed.
- Spec round 4 (`spec_rounds` 1 left): flash all accepted, "ready". astra re-raised R3-01 (roles are
  not visible through `cross_app_db_effect_bundle`) and raised R4-01 (important: field-specific
  forwarder expectations). Both fixed in row 3c.
- Spec round 5 (`spec_rounds` 0, `pi_calls` 8): **both accepted all 17 entries, approved deviations
  (a) and (b), "ready" — SPEC CONVERGED.** Spec and probes committed as `5b874f17`, `check-diff`
  `{"reasons": []}`.
- Step 6: acceptance-test subagent (opus) dispatched. RED on `5b874f17`: **18 new tests, 17 FAIL,
  1 PASS (2b, the regression pin expected to pass on master)**, each red for its row's reason or a
  stated non-vacuity precondition (`$SCRATCH/70/red.md`, logs `red-cli.log`, `red-unit.log`).
  Adjustments, each stated with a reason in red.md:
  - 1c uses `facts: direct`, plus `routine.name: CallChain` for the inherited rule, so that "exactly
    one finding" is hand-stated.
  - One extra workspace subscriber, used only by the policy tests.
  - 2c, 2f, 2h and the 2g CLI test gained non-vacuity preconditions; 2e asserts hand-stated keys.
  - 3c path (ii) builds `CrossAppInputs` by hand. GREEN must switch it to `base.cross_app_inputs()`.

  Process slip: the 3b test was appended with a shell heredoc, against the edit rule. It is test
  code only and visible in the diff.
- Implementation deviation (wall-clock): Tasks 1+2 go to ONE opus implementer, and Tasks 3+4 to a
  second, each reviewed by sonnet in parallel with the next dispatch. The two pairs touch disjoint
  files except through `cross_app_inputs()` (Task 2 → Tasks 3/4). Plan task boundaries, proofs and
  `check-diff` per commit are unchanged.
- Tasks 1+2 (`$SCRATCH/70/t12.md`): 12 green (2a-2c, 2e-2h, 3a-3c), 7 red (events/policy only).
  - No golden moved, clippy is clean.
  - Implementer deviations: 2e's expected list was reordered (as written it could never pass;
    the assertion is not weakened, proof 1).
  - 2h was flaky (1 red in 5): `tests/common/symbol_app.rs` zipped every fixture `.app` with the
    current time. The conductor fixed the root cause with one line, a fixed `last_modified_time`.
  - The conductor committed with the red events/policy file parked: **`e42e03dc`** (Task 1) and
    **`ac85f65e`** (Task 2), hook goldens OK both times.
  - `check-diff` then refused `forbidden:allow-attribute:tests/common/dep_dispatch_fixture.rs:#![allow(dead_code)]`.
    The conductor removed it (the cli tests build with no dead-code warning); the removal lands
    in the Tasks 3+4 commit.
- CDO "before" (`master` `b77025e4`, release-fast, `$SCRATCH/70/cdo/before.out`): `events fanout`
  4.5 s (primary) / 4.2 s (all), `chains` 4.2 s, `policy check` 4.2 s, `analyze` 20.0 s (primary) /
  18.3 s (all, sha `c51ce36e…`, the same as #80's baseline), `fingerprint` 5.7 s.
- Tasks 3+4 (opus) and the Tasks 1+2 review (sonnet) dispatched in parallel.
- Tasks 1+2 review: **changes** (`$SCRATCH/70/t12-review.md`). 1 important, verified at
  `digest_cli.rs:524-528`: `compute_tx_context_map` builds a single-app detector context on the
  cross-app model, so that solve gets neither `leaf_summaries` nor `fixed_roles`. Plus doc minors.
  A review-fix (sonnet) was dispatched after Tasks 3+4 had committed.
- Tasks 3+4 (`$SCRATCH/70/t34.md`): all 7 events/policy tests green, each row with a proof.
  Commits **`7d8300b2`** (events) and **`1c143140`** (policy + CHANGELOG with a CDO placeholder).
  Golden triage: 77 files (events 56, policy 21), +661 −0, every line class a (the `dependencies`
  block) or class b (the platform human first line), 0 other; no other family moved. clippy,
  `--test cli`, `--lib` and `check-goldens` all exit 0; `check-diff` `{"reasons": []}`. A Tasks 3+4
  review (sonnet) was dispatched in parallel with the review-fix.
- Tasks 3+4 review: **approve** (`$SCRATCH/70/t34-review.md`), 3 minors, accepted without change:
  the policy primary filter clones workspace routines (cheap); `classify_roots` also classifies
  dependency routines whose results are unused (cheap); and the CHANGELOG Fixed claims, which the
  conductor checked against the Task 1+2 results (both hooks reported goldens OK, no fingerprint
  golden moved).
- Review-fix RF1 (sonnet) committed **`4efaa593`**: digest's transaction spans use the cross-app
  inputs, plus doc fixes. No golden moved.
- CDO after-measurement and policy triage: see Gate results.
- Final panel round 1 (`final_rounds` 4 left): built-in `code-review high` (forked): 8 candidates.
  flash: all 17 accepted, "ready". astra: all 17 accepted, 2 important + 1 minor, "not ready".
  - Register F1-01…F1-11; 3 cost-only minors deferred with reasons (F1-07, F1-09, F1-11).
  - RF2 (opus) fixed the rest, commit **`1b0cf26f`** (proofs in `$SCRATCH/70/rf2.md`).
  - Its CHANGELOG fix for F1-03 did NOT land; the conductor fixed it with the CDO numbers in
    **`6593d90f`**.
  - Gates rerun on `6593d90f`.
- Final panel round 2 (`final_rounds` 3 left): flash all 28 accepted, "ready"; astra all 28
  accepted, "ready", plus 1 minor (F2-01: the CHANGELOG attributed all 7 policy false positives to
  reachability; one is a table-name glob gap). That gate run was STOPPED (no verdict taken; one
  orphaned process killed); the CHANGELOG was fixed, **`955b843e`**, `check-diff`
  `{"reasons": []}`; the gates were rerun on `955b843e`.
- Final panel round 3 (`final_rounds` 2 left, `pi_calls` 4): both accepted all 29, "ready". **FINAL
  PANEL CONVERGED.**
- Step 11: `origin/master` is still `b77025e4`, so there was no rebase. B =
  `b77025e4ed43e1c831f4f09f1684524c2add59e9`, H = `955b843ec550c730e6bdecaf41c962a0ab28f5ea`. Step 7: plan
  `docs/superpowers/plans/2026-10-15-issue-70-cross-app-follow-ups.md`, 4 tasks (`plan_tasks` 8 left).

## Acceptance matrix

The matrix is in the spec (rows 1a-1g, 2a-2h, 3a-3c). Status at H `955b843e`: **every row is met.**
- 1a-1f: `tests/cli/cli_events_policy_dependencies.rs`, all green, with proofs.
- 1g: the CHANGELOG's Changed entry gives the prove/fingerprint/diff reason.
- 2a-2c, 2e-2h: green with proofs. 2b is the regression pin.
- 2d: zero fingerprint goldens moved (each commit's hook passed, and `check-goldens` passed).
- 3a-3c: green with proofs.

Issue Acceptance:
1. prove / fingerprint / diff stay single-app with a written CHANGELOG reason; events and
   policy are cross-app with `--single-app`, a `dependencies` statement and a no-dependency
   contract test.
2. `workspaceFingerprint` changes with the closure (2a, 2f) and not otherwise (2c, 2h).
3. The leaf input is discriminated by the restated mutation (3a, 3b). This restatement and the
   indistinguishable single-app store were approved by both reviewers as deviation (b).

## Discrimination proofs

| test | mutation | fail output | pass output | commit |
|---|---|---|---|---|
| 2a, 2c (precondition), 2e | `derive_inputs` keeps only own-cache entries (filter out `../` keys) | 2e: the own-cache key `a.app` only, where the expected keys are the ancestor `b.app` and the own `a.app` | ok | `e42e03dc` |
| 2c | ascent loop ignores the `.git` boundary | `an .app above the .git boundary` (assertion failed) | ok | `e42e03dc` |
| 2g (unit + CLI) | delete the new `project_root/.git` check | unit: the above-root cache is returned; CLI: the loader reads it | ok | `e42e03dc` |
| 2f | `dependency_closure_entries` clears its entries and returns on the first read error | `a.app swap beside broken.app not seen` | ok | `e42e03dc` |
| 2h | unreadable `contentHash` hashes a location-bearing message | `left: "9bb90d3e…"` / `right: "9a5812ab…"` | ok (after the fixture-timestamp fix, see Timeline) | `e42e03dc` |
| 3a, 3b, 3c(i) | publishers are fixed leaves again (`… && false`) | 3c `(i) OnDepRec has no subscriber Modify row`; 3a `left: []`; 3b tempState absent | ok | `ac85f65e` |
| 3c (a) | the solver writes back roles for `fixed_roles` members | `(ii) OnDepRec: want (Unknown×5), got (No, Known([]), No, No, Full)`; same for `Fwd` and in `(iii)` | ok | `ac85f65e` |
| 3c (b) | empty `fixed_roles` only at the detector-context call | ONLY `(ii)` fails; `(iii)` passes | ok | `ac85f65e` |

| 1a | `run_events_fanout` builds single-app in the default branch | `no fanout entry for the dependency's OnDepEvent` | ok | `7d8300b2` |
| 1b | same in `run_events_chains` | `no chain dispatches the dependency's OnDepEvent` | ok | `7d8300b2` |
| 1c (1st) | `run_policy_check` builds single-app by default | `left: []` / `right: [("policy-dep-customer-insert", "ws:src/Work.al", 26)]` | ok | `1c143140` |
| 1c (2nd) | cross-app model, single-app detector context | `left: []` / `right: [("policy-no-commit-in-event-subscribers", "ws:src/Extra.al", 3)]` | ok | `1c143140` |
| 1c (3rd) | drop `RAW_INHERITED_FACTS` | the binary panics at `full_summary.rs:110` → test fails | ok | `1c143140` |
| 1c (4th) | no primary-routine filter | findings gain two `dep:…:src/Lib.al` anchors | ok | `1c143140` |
| 1d | clap arm renamed `--single-app-x` | `exit 2 … unexpected argument '--single-app'` | ok | `7d8300b2` |
| 1e / 1f | `human_line` prints with zero apps | `without_any_declared_dependency_no_statement_line` fails | ok | `7d8300b2` |
| 1f | omit the missing-row loop | the no-dependency contract test fails (no System line) | ok | `1c143140` |

Tasks 1+2 per-proof details: `$SCRATCH/70/t12.md`. Tasks 3+4: `$SCRATCH/70/t34.md`, logs `proof-*.log`. Each break was asserted applied with `grep -c`.
One break was redone after a refused Edit. 2b passes on master by design (the regression pin).

## Findings register summary

29 entries, **all `accepted` by BOTH reviewers** (`gpt-6-astra`, `gemini-3.8-flash`), against the
code at `955b843e`:
- spec: R1-01…R1-11 (no R1-09), R2-01…R2-04, R3-01…R3-02, R4-01; converged in spec round 5;
- final: F1-01…F1-11, F2-01; converged in final round 3.

Severity: 18 important, 11 minor, 0 critical. Disposition: 26 `fixed`, 3 `deferred` (F1-07, F1-09,
F1-11; all minor and cost-only, so no blocking entry is deferred), 0 `open`. Two deviations from the
issue's literal text were approved by both reviewers: (a) shared closure entries instead of the
literal `closure-v2:` string; (b) the restated leaf mutation.

## Gate results

### Repo gates (step 9) at H = `955b843e`, all through the supervisor (`"supervised": true`)

| gate | exit | seconds | log |
|---|---|---|---|
| `ci-steps-all` | 0 | 520.4 | `.agent/runs/20261015-134131-70bf6e/logs/ci-steps-all/20261015T155343-130874ad.log` |
| `check-goldens-coverage` | 0 | 1.2 | `…/logs/check-goldens-coverage/20261015T160224-57767603.log` |
| `check-goldens` | 0 | 62.9 | `…/logs/check-goldens/20261015T160226-92cf553e.log` |
| `cdo-gate` | 0 | 213.6 | `…/logs/cdo-gate/20261015T160330-c4d291ee.log` |

After the gates, `git status --porcelain -- . ':!.agent'` is empty. Toolchain rustc 1.96.0; grammar
`8682cb5f`; CDO_WS identity hash sha256(`<CDO_WS path>@bc3ccb18`) =
`1e512deb11c28d513ade51d45a7e4cbd2f8a3fb6f145ba94387ec8e5b288bce7`. Two earlier gate runs (on
`4efaa593`, then `6593d90f`) were stopped before any verdict, because fixes were about to change the
tree; no result was taken from them.

### CDO before/after (`$CDO_WS` @ `bc3ccb18`; before = `master` `b77025e4`, after = `4efaa593`; release-fast; `$SCRATCH/70/cdo/{before,after}.out`, every command exit 0)

| measure | before | after |
|---|---|---|
| `events fanout` primary: time / totalEvents / hot / coveragePartial | 4.5 s / 188 / 0 / 0 | **22.8 s / 248 / 3 / 6** (cross-app); `--single-app` 4.0 s, output == before apart from the `dependencies` key |
| `events chains` primary: time / totalRoots | 4.2 s / 188 | 22.2 s / 248; `--single-app` == before |
| `policy check` (default policy): time / findings | 4.2 s / 7 | 24.2 s / **18**; `--single-app` == before. Rule deltas: install-codeunit-no-business-data-writes 3→4, no-commit-in-event-subscribers 4→10, no-http-from-table-triggers 0→4; `routinesEvaluated` 5479 in both (workspace routines only) |
| `alsem analyze` primary / all: findings | 2293 / 26954 | **byte-equal** (`payload.findings`): the leaf predicate and `fixed_roles` move nothing on CDO |
| `alsem fingerprint` | — | equal apart from the alsem version (CDO's closure is its own `.alpackages`) |
| `aldump --program-call-graph-stats` | sha `c032c8db…94ef` | **byte-identical** |

**Policy triage** (`$SCRATCH/70/cdo/policy-triage.md`, sonnet, read-only against the CDO and
dependency source): of the 16 new or changed findings, **9 REAL** (3 weak: the install-codeunit
trio) and **7 FP** (44%; 63% if the weak trio is counted FP). 0 findings anchored in a `dep:` file;
13 of 16 exist only because dependency code is now visible. FP causes:
- 4× MS `Guided Experience Impl.Insert` commits only when `IsPrimarySetup = true`, and the callers
  pass `false`. Path-insensitive reachability (no literal-argument folding).
- 1× an activation-wizard commit that an early exit makes unreachable (flow-insensitive).
- 1× a page started non-modally from a subscriber (the commit is in the page's own lifecycle).
- 1× a feature-flag table that the `* Setup` name glob cannot recognise (not caused by cross-app).

None is a cross-app plumbing defect. They are the same path-insensitivity `alsem analyze` (already
cross-app by default) has. The policy rules are existing rules, not new detectors, so the >30%
new-detector gate does not apply. The path-insensitivity is filed as a discovery.

## Discoveries

```json
[
  {
    "subsystem": "l5 / digest (cross-app snapshot composition)",
    "locator": "src/engine/l4/capability_cone.rs build_cross_app_base (calls cut to workspace callers); src/engine/l5/digest_cli.rs compose_snapshot over base.resolved",
    "symptom": "Cross-app digest reports coverage complete, no effects and nothing unresolved for a workspace routine whose call chain into dependency code inserts and commits: the base cuts dependency call edges and compose_snapshot never sees the injected dependency edges, so the dependency callee looks call-free and complete. Single-app says partial (external-target). A cross-app prove built the same way would answer may-commit: no.",
    "kind": "bug",
    "origin_issue": 70,
    "reproducer": "workspace CallChain() calls dependency Lib.Outer(); Outer calls Inner(); Inner inserts Customer and calls Commit(). alsem digest --routine CallChain (cross-app default) -> effects [], coverage complete; --single-app -> coverage partial [external-target].",
    "pre_existing": true,
    "capability": "the cross-app snapshot follows calls inside dependency code (or marks them partial), so cross-app digest never claims complete coverage over a dependency call chain it did not walk",
    "acceptance": "on the reproducer, cross-app digest of CallChain lists the DB_INSERT and COMMIT (or reports coverage partial); prove CallChain may-commit cross-app does not answer no."
  },
  {
    "subsystem": "l4 / summary solver (event dispatch roles)",
    "locator": "src/engine/l4/combined_graph.rs:332-340 (dispatch edges carry no callsite); src/engine/l4/summary_runner.rs:635-645, 738-774 (roles composed through source callsites and the body walk only)",
    "symptom": "A solved event publisher's parameter roles ignore its subscribers: role composition walks source callsites and the publisher's body, never the synthetic dispatch edges, so a publisher whose subscriber reads or dirties a forwarded var record reports no entry requirement and dirtyAtExit no. #70 keeps DEPENDENCY publishers' roles at the leaf's values; workspace publishers still get the falsely concrete roles.",
    "kind": "bug",
    "origin_issue": 70,
    "reproducer": "workspace [IntegrationEvent] OnRec(var Rec: Record Customer) with a subscriber doing Rec.Modify(): the publisher's summary roles show dirtyAtExit no / no entry requirement.",
    "pre_existing": true,
    "capability": "a publisher's parameter roles compose its subscribers' effects on forwarded var parameters",
    "acceptance": "a summary test: the publisher's roles for Rec reflect the subscriber's Modify (dirty at exit, or unknown), for workspace and dependency publishers alike."
  },
  {
    "subsystem": "claude-code-lsps / al-language-server-go wrapper (package cache discovery)",
    "locator": "claude-code-lsps/al-language-server-go/wrapper/project.go:203 DiscoverPackageCachePaths; mirrors src/dependencies.rs find_all_alpackages_folders",
    "symptom": "#70 made find_all_alpackages_folders stop at a workspace that is itself a git root (the stated contract). The Go wrapper still ascends from the parent without checking the workspace's own .git, so for a repository-root workspace with an .alpackages above it, the AL LSP is given an ancestor cache that al-call-hierarchy and the loader no longer read, and the two indexes disagree on dependency symbols.",
    "kind": "bug",
    "origin_issue": 70,
    "reproducer": "a workspace folder containing .git, with a parent folder whose package cache holds x.app: the wrapper's packageCachePaths includes the parent's cache; al-sem's find_all_alpackages_folders returns only the own cache.",
    "pre_existing": false,
    "capability": "the wrapper and al-sem discover the same package caches",
    "acceptance": "DiscoverPackageCachePaths does not ascend when the workspace folder itself contains .git (directory or file); a wrapper test pins it."
  },
  {
    "subsystem": "l4 / reachability (path-insensitive commits through literal arguments)",
    "locator": "Microsoft System Application GuidedExperienceImpl.Codeunit.al Insert (~:106-121); the no-commit-in-event-subscribers policy rule and the cone's commit facts",
    "symptom": "Cross-app policy (and analyze) report a commit reachable from OnRegisterAssistedSetup / OnRegisterManualSetup subscribers through Guided Experience.InsertAssistedSetup / InsertManualSetup, but Guided Experience Impl.Insert commits only when IsPrimarySetup is true and those callers pass the literal false. 4 of the 7 false positives in #70's CDO policy triage.",
    "kind": "bug",
    "origin_issue": 70,
    "reproducer": "CDO: alsem policy check (cross-app default) -> no-commit-in-event-subscribers findings at CDOSetupManagement.Codeunit.al:72 and :105 and CDOeSealServiceMgt.Codeunit.al:140 and :165.",
    "pre_existing": true,
    "capability": "commit reachability folds literal boolean arguments into the callee's guarded branches (at least for a parameter tested directly in an if)",
    "acceptance": "those four CDO findings disappear while a call passing IsPrimarySetup = true still reports the commit."
  }
]
```

## Not delivered

- `prove`, `fingerprint`, `diff` stay single-app (reason in the CHANGELOG): the cross-app snapshot
  does not follow calls inside dependency code (Discovery 1).
- Workspace event publishers still get dispatch-blind parameter roles (Discovery 2); only
  dependency publishers keep the leaf's roles here.
- The Go wrapper's package discovery does not yet stop at a repository-root workspace (Discovery 3).
- Policy false positives from path-insensitive reachability (Discovery 4).
- Cost-only follow-ups: one shared cross-app context for diagnostics and report (F1-07), a
  borrowed slice instead of the routine clone (F1-09), a single read per package (F1-11).
