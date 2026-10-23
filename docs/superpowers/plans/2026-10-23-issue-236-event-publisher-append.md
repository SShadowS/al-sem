# Issue #236 plan: appending a parameter to a local/internal event publisher

Design: `.agent/issue-236/ledger.md`, section `## Design` (accepted by both reviewers).

**Rule.** A signature change is a non-breaking append when both contract facts are
routines, `local` or `internal` on both sides, carry `IntegrationEvent` or
`BusinessEvent` on both sides, and the old `parameters` list is a strict prefix of the
new one. Then `diff_abi` emits no `procedure-signature-changed`, and `diff_events`
emits its signature finding at Low instead of Critical. Everything else stays Critical.
The rule is stricter than AppSourceCop AS0025 on purpose: a middle insertion or a
reorder stays Critical.

## T1: the `parameters` field

- `ids.rs`: pull the per-parameter text out of `canonical_routine_signature` into
  `canonical_param_spec(p, name)`; the fingerprint calls it with no name, so its bytes
  do not change.
- `snapshot_full.rs` `derive_contracts`: add `parameters` (each entry
  `canonical_param_spec(p, Some(name))`, e.g. `var handled: boolean`) to a routine fact
  only when it carries `IntegrationEvent` or `BusinessEvent`.
- `key_classes.rs`: list `parameters` as a non-reference field of `contractFacts`.
- Turns green: test 5 `closure_fields_are_classified_and_closed` (R5B leaf hand-stated).
- Goldens: `scripts/check-goldens --regen`; exactly the 9 `ws-d8-commit-in-tx` files
  named in the design may move, each by one `parameters` key on its publisher fact.
- Break: drop the `key_classes` entry -> test 5 fails.

## T2: the rule

- `passes.rs`: `is_non_breaking_event_append(old, new)` next to `is_abi_surface`, with
  the attribute match split into a helper both use. Three call sites: `diff_abi`'s
  signature-change site, the `diff_events` contract branch, the generic
  `event-publisher-signature-changed` branch.
- Turns green: test 1 `non_breaking_event_append_matrix`, test 2
  `issue156_e2e_abi_surface_triggers_publishers_subscribers` (rebaselined), test 4
  `issue236_no_subscriber_append_is_low`; test 3 `issue236_removed_event_param_stays_critical`
  stays green.
- Breaks: drop the name / type / `var` part of the shared formatter -> the rename /
  retype / var rows of test 1 fail; drop the visibility check -> the public row fails;
  remove the `diff_abi` call -> test 2; remove the contract-branch call -> test 2;
  remove the generic-branch call -> test 4; predicate returns `true` -> test 3.
- CHANGELOG `## [Unreleased]` / Fixed, stating the rule is stricter than AS0025.
