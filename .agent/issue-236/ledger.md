# Issue #236 — `alsem diff`: appending a parameter to a local/internal event publisher is reported as critical

## Claim

```json
{"run_id":"20261023-121852-e02c43","issue":236,"attempt":1,"session":"claude-code-session:e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c","branch":"issue/236-agent-discovery-gate-diff-event-a1","worktree":"/work/al-sem-issue-236-a1","body_hash":"4963a0f151bbb7be444f122c178ff77b53fa63188f29348139fe119e85897e07","title":"…","reconciled":[]}
```

Issue body hash: `4963a0f151bbb7be444f122c178ff77b53fa63188f29348139fe119e85897e07` (claim `body_hash`).
Operator: v2.0.2 before the CentralGauge demo on 2026-10-27. Ponytail: the smallest complete fix.

## Classification

**bounded.** One rule (which publisher signature changes are breaking) in `src/engine/gate/diff/passes.rs`, plus the one snapshot field it needs.

## Assumption probes

Run 2026-10-23 at base `6a2b7c8b`, worktree `/work/al-sem-issue-236-a1`.

| probe | result | evidence |
|---|---|---|
| P1 where severity is decided | `abi_severity` (`passes.rs:87-100`) maps `ProcedureSignatureChanged` to Critical for every kind. `diff_abi` (`passes.rs:133`) emits it when `signatureFingerprint` differs (`passes.rs:241-259`), for any fact `is_abi_surface` accepts (`passes.rs:114-131`). `event_severity` (`passes.rs:510-519`) maps `EventPublisherSignatureChanged` and `EventContractChangedWithAffectedSubscribers` to Critical; `diff_events` emits them when the publisher's `eventId` changed (`passes.rs:639-657`, `:712-748`). `pair_signature_changes` (`renames.rs:158-189`) only pairs the old and new routine ids so the two passes see one routine; it decides no severity | code read |
| P2 what the snapshot carries | A routine contract fact holds `kind, stableId, visibility, signatureFingerprint, attributes, routineKind` (`src/engine/l5/snapshot_full.rs:672-713`). The fingerprint is `sha256(canonical_routine_signature)` (`src/engine/ids.rs:185-202`: lowercased name, `var`+type per parameter, return type; **no parameter names**). An event declaration's `eventId` is `object::name::signatureHash` (`src/engine/l5/snapshot.rs:2526-2552`). So **no snapshot field lists the parameters**: "appended" cannot be decided today without a new field | code read |
| P3 E2E-2 on master | `tests/fixtures/ws-diff-abi-surface` (`a`→`b`: `local [IntegrationEvent(false,false)] OnBeforeCalc(var Handled: Boolean)` gains `; Qty: Decimal`). `issue156_e2e_abi_surface_triggers_publishers_subscribers` (`tests/cli/cli_b_diff_differential.rs:1700`) pins TWO findings: `abi / procedure-signature-changed / OnBeforeCalc` and `events / event-contract-changed-with-affected-subscribers / OnBeforeCalc`. Both kinds map to Critical (P1). So the issue's "critical" is two findings, not one | see Gate results for the run |
| P4 AS0025 | See verdict below | https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/analyzers/appsourcecop-as0025 (ms.date 2026-10-01) |
| P5 #156 access rule | `is_abi_surface` (`passes.rs:114-131`): triggers are not ABI; `local`/`internal` routines are not ABI **unless** they carry `IntegrationEvent`, `BusinessEvent` or `ExternalBusinessEvent` (`EXTERNAL_EVENT_ATTRIBUTES`, `passes.rs:104-105`), because dependents can subscribe to them. So the E2E-2 publisher is ABI, and every signature change on it is critical | code read |
| P6 who counts | CentralGauge runs `alsem diff … --format json` and counts **every `abi` finding** as `abi_changes`, whatever its severity (`.agent/issue-156/ledger.md:12`). So lowering the severity of the `abi` finding alone does not fix the count | ledger read |

**AS0025, as read from the source.** "Parameters cannot be modified, renamed, or removed from Business type and Integration type events, because it will break dependent extensions." Subscribers bind to parameters **by name** and need not use all of them. Note: dependents can subscribe even to `local`/`internal` events; the access modifier only limits who may **raise** the event. Not triggering: "For `local` or `internal` events, it is allowed to add parameters to the event." So:
- `local`/`internal` `[IntegrationEvent]`/`[BusinessEvent]`: adding a parameter is non-breaking; removing, retyping (including `var` on/off, a modification) or renaming one is breaking. Our pipeline sees a rename only when it comes with a fingerprint change (another parameter change); a pure rename changes neither `signatureFingerprint` nor the `eventId` hash, so no pass reaches the rule (D1, O5).
- `public` (or `protected`) publishers: dependents can also raise (call) them, so AS0023/AS0024/AS0026 (public method signature rules) apply and adding a parameter breaks callers. Stays critical.
- `ExternalBusinessEvent`: not named by AS0025; out of scope, stays critical.
- Insert vs append: the rule allows adding anywhere (binding is by name). Our facts must carry names to see that; we accept only "old list is a prefix of new list" (append), the documented example's shape. So the rule is **stricter than AS0025 on purpose** (O4): a middle insertion and a reorder are non-breaking under AS0025 but stay critical here, because the issue's Capability says "reordered → critical". The predicate's comment and the CHANGELOG say so.

**Verdicts.** P1 holds (two places, one per pass). P2 refutes the issue's implied "just change severity": the facts carry no parameter list, so a new field is needed. P3 widens scope: the `events` finding is critical too, so the fix must reach `diff_events`. P5 holds. P6: the `abi` finding must go, not just drop severity.

## Design

**Rule.** A signature change is a *non-breaking append* when both facts are routines, the publisher is `local` or `internal` on BOTH sides, carries `IntegrationEvent` or `BusinessEvent` on both sides, and the old parameter list is a strict prefix of the new one, comparing each parameter as its lowercased `name` plus the exact text `canonical_routine_signature` renders for it (`var ` prefix, `trim().to_lowercase()` type; O8) (so a rename or retype of an existing parameter, a removal, a reorder or a `var` flip are not appends). Then: `diff_abi` emits **no** `procedure-signature-changed` (not an ABI break per AS0025, and CentralGauge counts every `abi` finding); `diff_events` emits its `event-publisher-signature-changed` / `event-contract-changed-with-affected-subscribers` at **Low** instead of Critical, so the change stays visible once. Everything else stays as it is (critical). A fact without the new field (an older snapshot, or no pairing) is never an append: the safe side.

**Where.** One function `is_non_breaking_event_append(old_fact, new_fact) -> bool` in `passes.rs`, next to `is_abi_surface`, reusing its attribute check (split `EXTERNAL_EVENT_ATTRIBUTES` matching into a helper taking the attribute list). `diff_abi` calls it at the signature-change site (`passes.rs:241`); `diff_events` looks up the two contract facts by the NEW publisher decl's `routine` (both `old_contracts_by_subject` and `new_contracts_by_subject` are keyed by the new id once `pair_signature_changes` paired them) and picks the severity per finding. The data: `derive_contracts` (`snapshot_full.rs:672`) adds `"parameters": ["var handled: boolean", "qty: decimal"]` to a routine fact **only when it carries `IntegrationEvent` or `BusinessEvent`** (keeps golden churn to publisher fixtures); `key_classes.rs` lists `parameters` as a non-reference field of `contractFacts`. The per-parameter text comes from one function pulled out of `canonical_routine_signature` (`ids.rs:191-194`) and used by both the fingerprint and `parameters` (prefixed `name: `), so the two never disagree (O8). The predicate's doc has one line pointing at `is_abi_surface`'s `internalsVisibleTo` shortcut (`passes.rs:110-112`): a friend app that raises an `internal` event is broken by an append; we accept that, as AS0025 does (O9).

**Acceptance matrix.**
| Acceptance item | Test |
|---|---|
| E2E-2's added-parameter case is not critical | `issue156_e2e_abi_surface_triggers_publishers_subscribers` rebaselined: only `events / event-contract-changed-with-affected-subscribers` remains, asserted at severity Low; no `abi` row |
| a removed-parameter case stays critical | new E2E `issue236_removed_event_param_stays_critical`: the REVERSED E2E-2, `engine(compose_ws(b), compose_ws(a))` on `ws-diff-abi-surface`, no new fixture (O3). Reversed, `OnBeforeCalc` loses `Qty: Decimal`; the removed triggers are not ABI (`passes.rs:118-120,140-142`) and the subscriber is `local` and publishes nothing. Expects exactly `abi / procedure-signature-changed / OnBeforeCalc` Critical and `events / event-contract-changed-with-affected-subscribers / OnBeforeCalc` Critical |

**Tests and their break → fail.**
1. Unit `non_breaking_event_append_matrix` (passes.rs): hand-built facts; append on local/internal Integration and Business → true; public, ExternalBusinessEvent, plain local routine, removed, inserted-in-the-middle, reordered, missing `parameters` → false. The rename, retype and `var`-flip rows each ALSO append a parameter (`[a]` → `[a', b]`), so the length check alone cannot reject them (S1). Breaks, one at a time: drop the name compare → the rename+append row fails; drop the type compare → the retype+append row fails; drop the `var` compare → the `var`+append row fails; drop the visibility check → the public row fails.
2. E2E-2 rebaselined (above), with subscriber. Breaks, one per call site (O7): remove the call in `diff_abi` only → the `abi` row reappears → fails; remove it in the `diff_events` contract branch only → Critical → fails.
3. E2E removed-parameter (above). Break: make the predicate return `true` → no `abi` row and Low → fails.
4. No-subscriber append (S2, O6): a hand-built snapshot pair in `cli_b_diff_differential.rs` (the existing snapshot builders near `:716-920`): a `local [IntegrationEvent]` publisher `[a]` → `[a, b]`, no subscriber. Expects `events / event-publisher-signature-changed` at Low and no `abi` row. Break: remove the call in the generic `EventPublisherSignatureChanged` branch only → Critical → fails.
5. Key class (S3, O1): `closure_fields_are_classified_and_closed` sees only empty `parameters` (DD's publisher is a dependency row; R5B's `OnHostStarted()` has none; `leaves()` yields nothing for `[]`). So hand-state `parameters: ["x: integer"]` on a kept R5B contract fact, as F28 hand-states `resourceArgSource` (`cli_snapshot_schema4.rs:1395-1419`), with a precondition assert that the leaf exists. Break: omit the `key_classes` entry → fails.

**Failure shapes.** A missed append (stays critical): pairing failed or old snapshot without `parameters`; safe. A wrong pass (a break shown as Low/no abi): only if the prefix compare ignores names/`var`/types; test 1 pins each.

**Pre-stated fallbacks.**
- A golden moves outside the predicted set: stop, diagnose why with the `golden-diff-triager`, fix the cause; never widen the field to make churn look expected (S5).
- The events-pass lookup misses: that only happens when pairing failed (`indexes.rs:151-155` already normalises paired ids), so the finding stays Critical, the safe side; no extra normalisation (S5).
- Test 4's builders cannot express a publisher fact with `parameters`: add a tiny fixture pair under `tests/fixtures/ws-diff-event-noSub/{a,b}` and run it through `engine` like E2E-2.
- Test 5's hand-stated leaf does not reach the closure check: find out why and fix the test's injection; never widen the field (S5).

**Tasks (2).** T1: `parameters` field on publisher contract facts (shared per-parameter formatter in `ids.rs`) + key class + test 5 + golden regen. T2: the rule, its three call sites, tests 1-4, CHANGELOG `Fixed`.

**Golden moves expected (O2, S4).** Exactly 9 files, every `ws-d8-commit-in-tx` golden that serializes `contractFacts`, each gaining one `parameters` key on its `IntegrationEvent` publisher fact (data only): `tests/cli-b-goldens/snapshot/ws-d8-commit-in-tx.{raw.json,envelope.json,cbor,cbor.gz}` (the CBOR pair is rewritten by `cbor_matches_goldens` / `cbor_gz_matches_goldens`, `cli_b_snapshot_differential.rs:227-302`), `tests/cli-b-goldens/snapshot/shards/ws-d8-commit-in-tx/{all,primary}/primary.json`, `tests/cli-b-goldens/fingerprint/shards/ws-d8-commit-in-tx/primary.json`, and `tests/cli-b-goldens/fingerprint/ws-d8-commit-in-tx.{cbor,cbor.gz}` (rewritten by the fingerprint `cbor_matches_goldens` / `cbor_gz_matches_goldens`, `cli_b_fingerprint_differential.rs:162-200`; checked with `grep -a` / `zcat`: each holds the `IntegrationEvent` fact). The three shard `manifest.json` files do NOT move: they hold no `contractFacts`, and their `workspaceFingerprint` is computed from the inputs (`compute_workspace_fingerprint(inputs, driver_version)`, `snapshot_full.rs:169`). Found by `grep -rlE '"name": ?"(IntegrationEvent|BusinessEvent)"' tests/` intersected with the goldens that hold `contractFacts`, plus the binary CBOR siblings, which grep cannot see. `tests/cli-b-goldens/diff/snapshots/*` do NOT move: they are vendored diff inputs read by `load_snap` (`cli_b_diff_differential.rs:137`), never rewritten; they stay as legacy (field-less) inputs on purpose, and tests 2-4 build fresh snapshots. The `r1a`/`al2dump` hits carry no `contractFacts`. Any other move stops the regen.

## Base/Head SHAs

Base `6a2b7c8b`. Head: not yet.

## Timeline

- 2026-10-23 design author: ledger, probes, design.

## Acceptance matrix

See `## Design`.

| # | test | red (base) | green at |
|---|---|---|---|
| 1 | `non_breaking_event_append_matrix` | compile error (no predicate) | T2 `d030fe16` |
| 2 | `issue156_e2e_abi_surface_triggers_publishers_subscribers` (rebaselined) | extra `abi` row | T2 `d030fe16` |
| 3 | `issue236_removed_event_param_stays_critical` | green at base by design (guard) | stays green |
| 4 | `issue236_no_subscriber_append_is_low` | extra `abi` row | T2 `d030fe16` |
| 5 | `closure_fields_are_classified_and_closed` (R5B `parameters` leaf) | `contractFacts.parameters` unclassified | T1 `915ea36e` |

## Discrimination proofs

Each break made with Edit, the test run, reverted with Edit, the test run again. Logs `/work/bench/236/proof-*.log`.

| break | where | test | broken | reverted |
|---|---|---|---|---|
| B0 omit `parameters` from `NON_REFERENCE_FIELDS` | `key_classes.rs` | 5 | FAIL `R5B: fields in neither ...: {"contractFacts.parameters"}` | pass |
| B1 drop the name from `canonical_param_spec` | `ids.rs` | 1 | FAIL `rename+append` | pass |
| B2 drop the type from `canonical_param_spec` | `ids.rs` | 1 | FAIL `retype+append` | pass |
| B3 drop the `var ` prefix from `canonical_param_spec` | `ids.rs` | 1 | FAIL `var+append` | pass |
| B4 drop the visibility check in `is_non_breaking_event_append` | `passes.rs` | 1 | FAIL `public` | pass |
| B5 remove the call in `diff_abi` | `passes.rs` | 2 (and 4) | FAIL `left: [("abi", "procedure-signature-changed", ...), ("events", ...)]` | pass |
| B6 remove the call in the `diff_events` contract branch | `passes.rs` | 2 | FAIL `left: Critical / right: Low` (3, 4 pass) | pass |
| B7 remove the call in the generic `event-publisher-signature-changed` branch | `passes.rs` | 4 | FAIL `left: Critical / right: Low` (2, 3 pass) | pass |
| B8 predicate returns `true` | `passes.rs` | 3 | FAIL `left: [("events", ...)]` / `right: [("abi", ...), ("events", ...)]` | pass |

Note on B1-B3: the predicate compares each parameter as ONE string (the design's "lowercased name plus the exact text `canonical_routine_signature` renders"), so there is no separate name / type / `var` compare in `passes.rs` to drop. Test 1 builds its entries through the shared `canonical_param_spec`, so the breaks are made there, in the code production uses. B1's first run came back failing on the WRONG row (`inserted`): fixture `c` shared `b`'s type `Decimal`, so with names dropped the insertion looked like an append. Test defect, fixed (`c` is now `Code: Code[20]`) and B1 re-run: fails on `rename+append` as designed.

## Findings register summary

(none yet)

## Gate results

- P3 reproduction at base `6a2b7c8b`: `cargo test --test cli issue156_e2e_abi_surface` → `1 passed` (pins the two critical-kind findings). Log: `/tmp/claude-1000/-work-alsem/e2bf58b3-5d1b-467f-8c3b-1c59a8d42f0c/scratchpad/e2e2.log`.
- Plan `0afbd6aa`: `ci-steps task-affected` exit 0, 3810 passed (`/work/bench/236/gate-plan.log`); check-diff no reasons before and after.
- T1 `915ea36e`: `check-goldens --regen` exit 0, then `check-goldens` exit 0, 982 passed (`t1-regen.log`, `t1-check-goldens.log`); `ci-steps task-affected` exit 0, 3810 passed (`t1-task-affected.log`); check-diff no reasons before and after.
- T2 `d030fe16`: `ci-steps task-affected` exit 0, 3813 passed (`t2-task-affected.log`); check-diff no reasons before and after.
- Final `ci-steps task` at `d030fe16`: exit 0, 3813 passed, 8 skipped (`/work/bench/236/final-task.log`).

## Discoveries

- D1: **fixed in `667840bf` (final r1, F1/G1)**, no longer a discovery. AS0025 says renaming an event parameter breaks subscribers, but `signatureFingerprint` ignores parameter names, so a pure rename (or a same-type reorder) used to give no finding. `signature_change` now compares the publisher `parameters` lists when the fingerprints are equal; tests `issue236_pure_rename_is_critical`, `issue236_same_type_reorder_is_critical` (O5).

## Not delivered

- Middle insertion and reorder of parameters (non-breaking per AS0025, since binding is by name) stay critical: the prefix rule follows the issue's Capability (O4).
- `ExternalBusinessEvent` publishers: AS0025 does not cover them; unchanged.

## Acceptance tests (red)

Run 2026-10-23 at base `6a2b7c8b` with the tests written and no implementation. Logs: `/work/bench/236/red-t1.log`, `/work/bench/236/red-cli.log`.

| # | test | red output |
|---|---|---|
| 1 | `non_breaking_event_append_matrix` (lib, `passes.rs`) | does not compile: `error[E0432]: unresolved import crate::engine::ids::canonical_param_spec` and `error[E0425]: cannot find function is_non_breaking_event_append in this scope` |
| 2 | `issue156_e2e_abi_surface_triggers_publishers_subscribers` (rebaselined) | `left: [("abi", "procedure-signature-changed", "OnBeforeCalc"), ("events", "event-contract-changed-with-affected-subscribers", "OnBeforeCalc")]` / `right: [("events", "event-contract-changed-with-affected-subscribers", "OnBeforeCalc")]` |
| 3 | `issue236_removed_event_param_stays_critical` | **green at base, by design**: it pins the behaviour that must NOT change (a removal stays critical). Its discrimination is the break (predicate returns `true`), recorded under Discrimination proofs |
| 4 | `issue236_no_subscriber_append_is_low` | `left: [("procedure-signature-changed", "appguid-1:Codeunit:1#p2")]` / `right: []` |
| 5 | `closure_fields_are_classified_and_closed` (R5B leaf hand-stated) | `R5B: fields in neither REFERENCES nor NON_REFERENCE_FIELDS: {"contractFacts.parameters"}` |

## Golden-move verification (T1)

`check-goldens --regen` moved exactly the 9 predicted files and nothing else (`git status` after the regen). Checked mechanically against `HEAD`:
- The 5 JSON files (`snapshot/ws-d8-commit-in-tx.{raw,envelope}.json`, `snapshot/shards/ws-d8-commit-in-tx/{all,primary}/primary.json`, `fingerprint/shards/ws-d8-commit-in-tx/primary.json`): with `jq 'walk(del(.parameters))'` old and new are identical; `parameters` count 0 → 1; the one carrier is the `IntegrationEvent` fact, value `["header: record \"sales header\""]`.
- The 4 CBOR files (`snapshot/` and `fingerprint/` `ws-d8-commit-in-tx.cbor` and the unzipped `.cbor.gz`): an asserted byte compare — identical except the publisher fact's map header `0x06 → 0x07` and one appended `parameters` entry (+43 bytes); each `.cbor.gz` unzips to the same bytes as its `.cbor`.
- No pre-stated fallback fired. Triage receipt: `.agent/golden-triage.md` (worktree, uncommitted).

## Final panel round 1 (register round 5)

Reviews: `final-r1-sol.md` (F1-F4), `final-r1-opus.md` (G1-G3), `final-r1-codereview.md` (CR1-CR6). Every claim was checked against the source before fixing. One commit: `667840bf`.

Findings, one line each (register anchors):
- F1: same-type reorder invisible (fingerprint and eventId hold no names). Fixed: `signature_change` reads `parameters`; engine test `issue236_same_type_reorder_is_critical`; the matrix row that also appended is relabelled `reordered+append`.
- F2: production `var` serialization unpinned. Fixed: E2E-2 asserts the emitted list is literally `["var handled: boolean", "qty: decimal"]`.
- F3: both-side eligibility untested. Fixed: matrix rows `local -> public` and `public -> local`, each with an append.
- F4: CHANGELOG over-claimed renames. Fixed: renames and reorders are now detected, and the CHANGELOG says so in a second Fixed entry.
- G1: same as F4 and D1. Fixed with F1/F4.
- G2: append ignored attribute arguments. Fixed with CR1.
- G3: publisher attribute set spelled twice. Fixed with CR3.
- CR1: `is_non_breaking_event_append` now also requires equal `(name, argsHash)` attribute pairs; matrix row `attribute args changed + append`.
- CR2: `diff_events` decides the severity once per publisher key (`publisher_signature_severity`) and passes it into `event_finding`; the two after-the-fact patches are gone.
- CR3: `PARAMETER_EVENT_ATTRIBUTES` in `ids.rs`, read by `derive_contracts` and `passes.rs`.
- CR4: refuted. AS0025 (ms.date 2026-10-01) names "Business type and Integration type events", and its own non-triggering example is a `local [BusinessEvent]` gaining `j: Integer`. Rule kept.
- CR5: refuted. AS0025: "For `local` or `internal` events, it is allowed to add parameters to the event." Rule kept; the friend-app (`internalsVisibleTo`) raise risk is now stated in the predicate's doc comment.
- CR6: `derive_contracts` maps `r.parameters` directly; the `zip` is gone.

Tests, red before the fix (logs `/work/bench/236/r5-red-*.log`):

| test | red | green |
|---|---|---|
| `issue236_same_type_reorder_is_critical` | `left: []` / `right: [("procedure-signature-changed", "appguid-1:Codeunit:1#p1")]` | `667840bf` |
| `issue236_pure_rename_is_critical` | same shape (`left: []`) | `667840bf` |
| `signature_change_reads_the_parameter_list` | does not compile (`unresolved import SignatureChange`) | `667840bf` |
| matrix rows F3, CR1 | lib did not compile; the CR1 row is proved by P3, the F3 rows by P4/P5 | `667840bf` |
| E2E-2 literal (F2) | green at base (production already right); proved by P6 | `667840bf` |

Discrimination proofs (each made with Edit and reverted with Edit; logs `/work/bench/236/r5-p*.log`):

| break | test(s) | broken | reverted |
|---|---|---|---|
| P1 `signature_change` ignores `parameters` (`&& false`) | unit `signature_change_...`; reorder; rename | FAIL `reordered`; reorder and rename FAILED | pass (`r5-p12-revert-*`) |
| P2 `publisher_signature_severity` never reads the facts | reorder, rename, test 2, test 4 | FAIL `left: []`; `Critical` vs `Low` | pass |
| P3 drop the `attribute_keys` compare | matrix | FAIL `attribute args changed + append` | pass |
| P4 drop `eligible(old)` | matrix | FAIL `public -> local` | pass (checked with P5's revert) |
| P5 drop `eligible(new)` | matrix | FAIL `local -> public` | pass |
| P6 `derive_contracts` emits `is_var: false` | E2E-2 | FAIL `left: [["handled: boolean", "qty: decimal"]]` | pass (P7's run: the literal held) |
| P7 the one severity decision maps an append to Critical | test 2, test 4 (CR2's single place) | FAIL `left: Critical / right: Low` | pass |

None came back green. P4's revert was not run on its own: P5 was applied right after, and the shared revert run is green.

Gates at `667840bf`: check-diff no reasons before and after; `ci-steps task-affected` exit 0, 3816 passed (`r5-task-affected.log`); `check-goldens` exit 0, 986 passed, and no golden moved (`r5-check-goldens.log`). Final `ci-steps task` at `667840bf`: exit 0, 3816 passed, 8 skipped (`r5-final-task.log`).

## Final panel round 2 (register round 6)

Both reviewers accepted all 14 round-5 ids (CR4 and CR5 as refutations). Two new findings, one commit `34e0fc18`:
- F5: `attribute_keys` compared names case-sensitively and in order, so an append plus `[IntegrationEvent]` -> `[integrationevent]` was a false Critical. Fixed: case-folded name plus `argsHash`, as a sorted set (`BTreeSet`); matrix row `attribute name case changed + append` (red before: it panicked on that row).
- G4: a pure rename or reorder showed equal old/new hashes and event ids. Checked first: finding ids hash only `category|kind|subject|secondaryKey` (`fingerprint.rs:134-146`), never `details`, so no id moves. Fixed: `parameter_details` adds `oldParameters`/`newParameters` to the `abi` finding and, through the per-publisher decision, to both `events` findings; pinned in `issue236_pure_rename_is_critical` (contract finding) and `issue236_same_type_reorder_is_critical` (generic finding). Red before: `left: None`.

| break | test | broken | reverted |
|---|---|---|---|
| Q1 attribute names compared as written (`to_string`) | matrix | FAIL `attribute name case changed + append` | pass |
| Q2 `diff_abi` drops the parameter details | rename, reorder | FAIL `procedure-signature-changed` (both) | pass (Q3's run: the abi rows held) |
| Q3 generic events branch drops them | reorder | FAIL `event-publisher-signature-changed` | pass (Q4's run) |
| Q4 contract events branch drops them | rename | FAIL `event-contract-changed-with-affected-subscribers` | pass |

None came back green. Logs `/work/bench/236/r6-*.log`. Gates at `34e0fc18`: check-diff no reasons before and after; `ci-steps task-affected` exit 0, 3816 passed (the first run failed dev clippy on `type_complexity`; fixed with a `Details` alias); `check-goldens` exit 0, 986 passed, no golden moved. The first commit attempt left out the CHANGELOG clause for G4 (the Edit did not apply); it was added and the unpushed commit amended, so it is still one commit.
