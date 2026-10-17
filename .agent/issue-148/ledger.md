# Issue #148 ledger

## Claim

```json
{"run_id":"20261017-173945-101799","issue":148,"attempt":1,"session":"claude-code-session:cf05ae92-26c8-4a5d-a439-ddec4928220c","branch":"issue/148-d35-runmodal-commit-point-a1","worktree":"U:\\Git\\al-sem-issue-148-a1","body_hash":"1bf1dc2527df15797996f43d6959f13295aff63c3889337ecb626a666c991b79"}
```

Issue body hash: `1bf1dc2527df15797996f43d6959f13295aff63c3889337ecb626a666c991b79`.

**Operator override (2026-10-17).** The owner prioritized #148 and force-approved it. `fetch` excluded it with reason `author`: the author, gkaupp, is not yet a collaborator; the invite is pending. `claim` does not re-check eligibility. The conductor read the issue text, which is a question with a repro and no instructions, and claimed it on the owner's explicit approval. The override is recorded in this run's `ranking.json`.

Reviewer roster: the operator told us (2026-10-15) to use `gpt-6.1-sol` and `claude-opus-5.5` in place of `gpt-6-astra` and `gemini-3.8-flash`. Their marks are recorded as `reviews.sol` and `reviews.opus`, and we attest with `--substitute astra=sol --substitute flash=opus`.

## Classification

**bounded.** The issue asks whether `Page.RunModal` counts as a commit point, and offers two remedies: "a message that says so" or a false-positive fixture.
- **The answer:** `RunModal` is not a commit, either in the engine or in Business Central. The finding is an "unknown coverage" result reported in the wording of a real commit.
- **The fix:** one detector (d35) reports that unknown case honestly, and the reporter's repro becomes a pinned test.
- **Out of scope:** the root cause behind the unknown. Object runs reach only the entry trigger, and an empty run is treated as unresolved. Fixing that is feature-sized; it becomes a discovery (see Design D3).

## Assumption probes

All probes ran on master `786f45f2`. Details are in `.agent/issue-148/investigation.md`.

| probe | result | evidence |
|---|---|---|
| the issue reproduces | confirmed: 1 d35 finding, `possible`, "Commit reachable from event subscriber (via callee)" | investigation §1, variant A |
| the engine maps RunModal to a commit | **falsified.** The commit set is only a bare `Commit()` (`ir_walk.rs:976-980`). RunModal gives `ui-window-open`, and no commit fact (`capability_cone.rs:995-1030`). The keyword form `Page.RunModal(...)` also gives a modal `execute` dispatch (`688-691`); the variable form does not. | investigation §2 |
| why d35 fires | `may_commit` is UNKNOWN, not YES (evidence note `mayCommit(summary) == "unknown"`, against `"yes"` for a real `Commit()`). The run into a workspace page that declares no `OnOpenPage` is an empty Multicast. The analyze adapter maps it to `Opaque` (`program_calls.rs:1198-1227`, a deliberate parity shape from S9.0d). That produces an `object-run-unresolved` edge, so the cone is `partial`, so the result is unknown. d35 treats unknown exactly like yes (`d35.rs:50-62, 112-141`). | investigation §2 |
| RunModal itself is irrelevant | confirmed. Adding an empty `OnOpenPage` to the page gives 0 findings and a `complete` cone, with the RunModal call unchanged (variant E). | investigation §1 |
| BC semantics | RunModal does not commit (Microsoft Learn, Page.RunModal method). Commit ends a write transaction (Database.Commit). The documented implicit commit is a checked `Codeunit.Run`. Historically, a modal page in a write transaction is a runtime ERROR, not a commit. "UI in a write transaction" is already detector d49, opt-in. | investigation §3 |
| CDO impact | 11 d35 findings on CDO: 1 direct and 10 transitive `yes`, **0 unknown**. A change to the unknown arm moves NO CDO finding. | investigation §5 |
| goldens with an unknown d35 | none. Every d35 note in `tests/` is `== "yes"`. | investigation §5 |

## Design

**D1. d35 reports an unknown commit result honestly.** In `src/engine/l5/detectors/d35.rs`, the UNKNOWN arm gets its own wording, confidence evidence and fix hint. That arm is `!is_direct && summary_commits == "unknown"`, and it includes the no-summary case. Precedence stays as it is: a direct `Commit()` (`is_direct`) or `yes` always wins over unknown. The `yes` and direct arms stay byte-identical.
- **Title:** `Commit may be reachable from event subscriber (incomplete call coverage)`.
- **Root cause:** `{name} is an event subscriber whose callees could not all be analysed ({reasons}); a Commit among them cannot be ruled out — the publisher could not roll back committed state if one exists.`
  - `{reasons}` is the subscriber summary's `coverage.reasons`, deduplicated and sorted, joined with `, ` (for example `object-run-unresolved`).
  - When the summary has `coverage == None`, the text is `coverage unknown`.
  - When the coverage is partial with empty reasons, the text is `incomplete coverage`.
  - When no summary exists, the text is `no effect summary`.
- **Confidence (decided).** The level stays `possible`. The subscriber's uncertainties come from the detector context's `UncertaintyView`, the same source other detectors use for `cappedBy`, and pass through `to_confidence(&uncertainties, "possible")`. The finding then carries `cappedBy: ["unresolved-call"]` (or whatever kinds the view yields) and the matching `evidence`. V1 asserts the exact `cappedBy`. If the implementer finds that `UncertaintyView` gives the subscriber no uncertainty for this shape, they STOP and report rather than fall back silently.
  - **Substrate requirement:** d35's declared substrate becomes `SUMMARIES | CORE_SUMMARIES`; today it is `SUMMARIES` only (`detectors/mod.rs:1401-1407`). The uncertainty view's backing map is built only when `CORE_SUMMARIES` is requested (`detector_context.rs:1283-1296, 1502-1510`), so without it a d35-only selection would get an empty view.
  - **Tests:** V1 asserts the confidence BOTH with the default detector selection AND with `--detector d35-commit-in-event-subscriber` alone. Discrimination (5b): drop `CORE_SUMMARIES` from the declaration, and the d35-only V1 assert fails.
  - **Cost:** the measurement plan records the d35-only `analyze` wall time on CDO, master vs branch, because the added substrate has a build cost.
  - **Empty view is allowed.** In general, the unknown arm may get an EMPTY uncertainty set: the cone can be partial for reasons the summarize step does not record as an uncertainty. In that case `to_confidence(&[], "possible")` gives `{possible, []}`, which is allowed. The root cause and the evidence note still name the coverage reasons. The STOP rule applies only to the repro shape, where `unresolved-call` is known to be present (`combined_graph.rs:~345-349`, folded into the caller by `db_effect_solver.rs:~1039-1048`). The `gap_detector_substrate_parity` test must stay green with the new declaration. Today no parity fixture has an unknown d35 (`gap_detector_substrate_parity.rs:36-52`), so that test cannot catch a missing `CORE_SUMMARIES`. The fix therefore adds an inline d35-unknown workspace, the issue's repro, to `every_detector_parity_between_full_and_minimal_ctx`, following the existing inline workspace at `:144-152`. With it, the declaration is guarded beyond the CLI test.
- **Evidence note:** `[EventSubscriber] {name} may commit (mayCommit(summary) == "unknown"; incomplete coverage: {reasons})`. It no longer asserts "transitively commits".
- **Fix hint:** the unconditional "Remove the Commit…" guidance (`d35.rs:~150-157`) becomes conditional for the unknown arm: "Check the callees that could not be analysed ({reasons}); if one commits, move the Commit out of the subscriber or defer it." The yes and direct hints are unchanged.
- **Finding id:** unchanged (`d35/<routine>/transitive`). It names the same site and kind, so existing baselines and suppressions keep matching.
- **Severity and default-on:** unchanged (`high`, default). This is a deliberate asymmetry with d34, which requires `Yes` (`d34.rs:115`):
  - d35's harm is irreversible. A commit inside a subscriber cannot be rolled back by the publisher, so recall is preferred, and an unproven-absent commit is still worth surfacing.
  - The honest wording and the `cappedBy` evidence say that it is unproven.

**D2. The reporter's repro is pinned as a test.** A CLI test (a temp-dir workspace, the existing CLI test pattern; no new r0-corpus fixture, so no new golden family) runs `alsem analyze` on five variants:
- (V1) the issue's repro (`ReasonPage.RunModal()`, the page has no `OnOpenPage`): exactly ONE d35 finding, with the D1 unknown title, root cause naming `object-run-unresolved`, and confidence `possible`;
- (V2) RunModal replaced by `Commit()`: exactly one d35 finding with the UNCHANGED yes title "Commit reachable from event subscriber (via callee)" and the unchanged root cause;
- (V3) RunModal removed: 0 d35 findings;
- (V4) the page gets an empty `OnOpenPage`: 0 d35 findings. This shows that RunModal itself is not treated as a commit.
- (V5) the callee does BOTH `Commit()` and the RunModal on a page with no `OnOpenPage`: exactly one d35 finding, with the YES wording. Yes beats unknown.

There are FIVE variants (V1-V5). Every run uses `alsem analyze <ws> --format json --with-evidence`, because the default JSON leaves `evidencePath` out and V1 asserts the note. V1 asserts all of the following: the title, the root cause naming `object-run-unresolved`, the evidence note text, `confidence == {level: possible, cappedBy: [the UncertaintyView kinds]}`, `severity == high`, and the id `d35/<routine>/transitive`.

A detector-level unit test also pins the no-summary arm (the `no effect summary` text) and the `coverage == None` arm (the `coverage unknown` text). It uses a hand-built context, as other detector unit tests do.

**D3. Out of scope, filed as discoveries:**
- (a) Object runs reach only the run object's entry trigger. A run is not modelled as the page's full trigger lifecycle (`OnInit`, `OnAfterGetRecord`, `OnQueryClosePage`, `OnClosePage`, field and action triggers). Meanwhile the analyze adapter maps an empty run into a workspace object to `Opaque` → `object-run-unresolved`. Together these produce both the unknown here and a false negative: a commit in `OnQueryClosePage` of a page that has an `OnOpenPage` is missed. The fix reuses the impact graph's lifecycle rule (`impact_graph/project.rs:576-620`) in the typed edge graph, and makes the empty run resolved-empty, in that order. This is feature-sized, because many goldens move.
- (b) `Database.Commit()`, the documented spelling, is invisible to the commit set. `ir_walk.rs:976-980` and `extract.rs:341,427` match only a bare `Commit`. So d34, d35, d46, the transaction spans, the pending writes and the digest's COMMIT all miss it.

## Acceptance matrix

| Item | Proof |
|---|---|
| The question is answered (does RunModal count as a commit point?) | The PR body and the issue answer: no. The engine has no RunModal→commit mapping, and BC's RunModal does not commit. The finding comes from incomplete call coverage (an empty page run treated as unresolved). Evidence comes from the investigation. |
| A message that reads accurately | (A1) D2 V1: the exact unknown title, a root cause naming `object-run-unresolved`, and confidence `possible`. |
| The real-commit finding is unchanged | (A2) D2 V2: the title and root cause byte-identical to master. In addition, `scripts/check-goldens` stays green with no golden moving (no committed golden has an unknown d35), and the CDO d35 `payload.findings` arrays are byte-identical on master and the branch (11 findings, 0 unknown). Any `diagnostics` difference is listed and explained. |
| The fixture | (A3) D2 V3 and V4: 0 findings. V4 shows that RunModal itself is not a commit. |
| Discrimination | Each break is made with Edit, run, then reverted:<ul><li>(1) the unknown arm reuses the yes wording: A1 fails;</li><li>(2) the unknown arm is skipped like `no`: A1 fails (0 findings);</li><li>(3) the yes arm takes the unknown wording: A2 fails;</li><li>(4) unknown beats yes: V5 fails;</li><li>(5) `to_confidence(&[], …)` is restored in the unknown arm: the V1 `cappedBy` assert fails;</li><li>(5b) `CORE_SUMMARIES` is dropped from d35's declared substrate: the d35-only V1 `cappedBy` assert fails, and so does the parity case below;</li><li>(6) the no-summary and `coverage == None` fallbacks are dropped: the unit test fails.</li></ul> |

## Measurement plan

- **Goldens:** none are expected to move. The note-based grep misses goldens without `evidencePath` (cli-a, gate, sarif), so the implementer also greps `tests/` for the d35 title `(via callee)` and checks that each hit is a real-Commit fixture. `scripts/check-goldens` must stay green.
- **CDO:** run `alsem analyze U:/Git/DO-cdo-baseline/Cloud --scope primary --detector d35-commit-in-event-subscriber --with-evidence --format json --deterministic` with master's and the branch's `cdo`-profile binary. The `payload.findings` arrays must be byte-identical (0 unknown d35 findings on CDO). The d35-only run now also runs the summarize step, which can add cap-hit diagnostics, so any difference in `diagnostics` is listed and explained rather than required to be empty. The wall time of both runs is recorded. Then the `cdo-gate`.

## Base/Head SHAs

- B = `786f45f2dfd8f71208b6077c5402fab3ba6e7a43` (origin/master at claim and at freeze)
- H = `54c458a641041732a4f71b730712ccbe339db6e7` (the approved code head; the evidence commit follows it)

## Discoveries

1. **l4 typed edge graph / page run lifecycle (bug, false negative and false positive).**
   - An object run reaches only the run object's entry trigger (`resolver.rs:2010-2018, 2050-2129`). A `Commit` in another trigger of a page run (`OnQueryClosePage`, `OnClosePage`, `OnAfterGetRecord`, action and field triggers) or of its source table is therefore never seen.
   - The analyze adapter maps an EMPTY run into a workspace object to `Opaque`, giving `object-run-unresolved` and a partial cone (`program_calls.rs:1198-1227`). That is what made d35 report "unknown" on #148's repro.
   - The fix:
     - reuse the impact graph's lifecycle rule (`impact_graph/project.rs:576-620`) in the typed edge graph;
     - then treat an empty workspace run as resolved-empty.
2. **commit detection: `Database.Commit()` is invisible (bug).**
   - `ir_walk.rs:976-980` and `resolve/extract.rs:341,427` match only a bare `Commit` identifier.
   - So the documented spelling `Database.Commit()` produces no commit fact. It is missed by d34, d35, d46, the transaction spans, the pending writes and digest `COMMIT`.

## Timeline

- The worktree was created at master `786f45f2`.
- Investigation (opus, read-only; subagents 1): `.agent/issue-148/investigation.md`.
- Classified as bounded. The design above is the spec.
- Spec panel (sol + opus; hash = sha256(ledger.md)[:16]):
  - r1 (spec_rounds 1; pi_calls 2; hash 5785a981256be99a): both reviewers CONFIRMED the root cause and the shape of D1. Findings S1-S9:
    - the confidence plan cannot work (no summary uncertainties; `object-run-unresolved` is not a valid `cappedBy`);
    - the unconditional fix hint and the "transitively commits" note;
    - no no-summary test, and no evidence/id asserts;
    - `coverage == None` and empty-reasons fallbacks;
    - the golden grep missed formats without `evidencePath`;
    - yes-beats-unknown was not pinned;
    - the d34 asymmetry rationale;
    - a nondeterministic CDO compare;
    - the modal-execute wording.

  - r2 (spec_rounds 2; pi_calls 4; hash 61bc304fd8a25153): S2-S9 accepted by both reviewers.
    - S1 was re-raised by both: d35 declares only `SUMMARIES`, so a d35-only run gets an empty `UncertaintyView`.
    - Opus raised new minors: N1 (the CDO compare and cap-hit diagnostics), N2 (five variants, `--with-evidence`), N3 (an empty view in general).
    - Revision 3 makes d35 declare `SUMMARIES | CORE_SUMMARIES`, asserts V1 with the default selection AND with d35 alone (break 5b), allows an empty view outside the repro, compares only `payload.findings` with the diagnostics explained, and requires `--with-evidence`.

  - r3 (spec_rounds 3; pi_calls 6; hash 603a95ea22dece4e): S1-S9, N2 and N3 accepted by both. N1 was re-raised by both, because A2 still demanded byte-identical whole output. New: N4 (minor), break 5b missing from the matrix, and parity cannot catch the declaration. Revision 4 rewords A2 to `payload.findings` with explained diagnostics, adds 5b to the matrix, and adds the d35-unknown parity fixture.

  - r4 (spec_rounds 4; pi_calls 8; hash 6772a7db5b1a8052): both reviewers accepted all 13 entries, with no new findings. CONVERGED.
- Plan, 2 tasks (plan_tasks 2): T0/T1 is the tests (red) plus the d35 change; T2 is the CDO compare, the CHANGELOG and the close-out. Both are dispatched to one opus implementer (subagents 2; task_attempts T0T1 = 1).
- T0/T1/T2: PASS. Report: `.agent/issue-148/t1.md`.
  - **Red:** V1 failed on master (the yes title). V2-V5 pin unchanged behaviour. Break 5b was observed before the substrate change.
  - **Results:** lib 2087, gap 217, CLI 5 and the d35 unit tests 3 all green; clippy clean; check-goldens green with no golden moved. The `(via callee)` grep finds only real-Commit fixtures (`ws-d35`) and d54's own title.
  - **Proofs:** breaks 1-6 and 5b each fail, then pass after revert. The first attempt at break 3 did not apply, because a reflow made the Edit's target string miss. It was redone, and its green run is not counted.
  - **CDO:** the d35 output is byte-identical between master and the branch (sha `0be0ce1a…`, 11 findings, 0 diagnostics). Wall time is within noise: 37.8/42.2 s on master, 41.4/40.1 s on the branch.
- Commit `8fec99ef`. Pre-commit goldens OK; `check-diff` `reasons: []`. Gates were started, then stopped for the review fixes.
- `code-review` (high) raised 10 findings, CR1-CR10.
  - **Refuted, with evidence:** CR1 (the root cause, deferred by D3a), CR2 (severity, decided in D1), CR5 (the cost, measured within noise).
  - **Fixed:** CR3 (neutral wording), CR4 (`unanalysed` targets), CR6 (V4 doc limit), CR7 (stale comments), CR8 (unit tests), CR9 (one match), CR10 (the SARIF rule text).
- CR fix (opus; subagents 3; task_attempts CR = 1). Report: `.agent/issue-148/cr-fix.md`.
  - **Tests:** lib 2090, d35 unit tests 6, CLI 5, gap 217 green; clippy clean.
  - **Discrimination:** breaks A, B, C and 1-6, 5b each fail, then pass after revert. Each break is re-read to confirm it applied.
  - **CDO d35 output:** byte-identical.
  - **Goldens:** 44 `tests/gate-goldens` SARIF files carry the old rule text once. Receipt `.agent/golden-triage.md`. Regen exit 0 (175 s). The diff check shows exactly 44 `-`/`+` pairs of that string; the other changed lines are the test file.
- Commit `fb5a5613`. Pre-commit goldens OK; `check-diff` `reasons: []`.
- **Final panel r1** (final_rounds 1; pi_calls 10; hash 75f1f6562b07bbff): both reviewers accepted all 23 entries.
  - Sol had no new findings, but kept one reservation: it had not seen the full golden diff.
  - Opus raised two minors. D1: the `unanalysed` label overclaims, so it becomes `partly analysed`. D2: the V4 test name.
  - Both were fixed in `54c458a6`. The d35 unit tests (6) are green, and `check-diff` gives `reasons: []`.
- **Final panel r2** (final_rounds 2; pi_calls 12; hash 358e8c5033a0fa2d): opus accepted all 25 entries. Sol accepted 24 and re-raised CR10 for verification only.
- **Final panel r3** (sol only; final_rounds 3; pi_calls 13; same hash): Sol was given the full `git diff master..HEAD -- tests/gate-goldens`, which is 44 files, each with one `-`/`+` pair of the rule text. Sol accepted CR10, and its other 24 marks stand. CONVERGED.
- **Gates at `54c458a6`:** all supervised, all exit 0. The logs are under `.agent/runs/20261017-173945-101799/logs/`:
  - ci-steps-all: 601.4 s (`ci-steps-all/20261017T193200-b596fdd0.log`);
  - check-goldens-coverage: 4.7 s (`check-goldens-coverage/20261017T194202-1b8c534f.log`);
  - check-goldens: 97.3 s (`check-goldens/20261017T194207-237afe4a.log`);
  - cdo-gate: 289.6 s (`cdo-gate/20261017T194345-b21da15f.log`).

  The porcelain check came back empty.
- **Rebase:** `origin/master` is `786f45f2`, an ancestor of HEAD, so no rebase was needed.
- **Freeze:** the ledger and the register are committed as the evidence commit.

    Revision 2 decides confidence via `UncertaintyView` (asserted `cappedBy`), makes the note and fix hint conditional, adds the fallbacks, V5 and the unit test, the asymmetry rationale, and deterministic CDO and title greps, and fixes the wording.
