# Issue #239 plan: an `<owner>::<routine>` selector never falls back to other owners

Design: `.agent/issue-239/ledger.md`, section `## Design` (accepted by both reviewers).

**Rule** (in `narrow_to_owner`, `src/engine/l5/selector_index.rs`; digest, prove and
fingerprint all call it). Only a form-5 (object-qualified) match is narrowed. The object
part is a display key with `"` removed. Keep the matches whose owner key equals it; only
when there are none, keep the matches whose owner key without its type word equals it.
When nothing is kept, return nothing: each caller's existing unmatched path fires
(digest `selector-unmatched`, fingerprint `selector-unresolved`, prove `unknown`, all
exit 2). No signature, diagnostic or renderer changes.

## T1: the rule, its tests, the CHANGELOG

- `narrow_to_owner`: quote removal, full owner then bare name (`strip_type_word_prefix`),
  delete the fallback and its doc-comment sentence.
- Tests (written red first, see the ledger's `## Acceptance tests (red)`):
  U3 rewritten (`narrow_to_owner_keeps_the_named_owner_only`); in
  `tests/cli/cli_object_display.rs` the fixtures `one_owner_workspace`,
  `shared_name_workspace` and A1 (M1), A2 (M2, M3), A3 (M4, M6). A6, A7 unchanged.
- CHANGELOG `Fixed` under `## [Unreleased]`, superseding the 2.0.1 #78 line that says an
  unmatched owner "matches as before".
- Goldens: none expected to move (`scripts/check-goldens`, no regen). A moved golden
  applies the design's pre-stated fallbacks or stops the task.
- Breaks (one ledger row each): restore the fallback; one-pass OR of full owner and bare
  name; drop the `"` removal; drop the bare-name step; keep only the first bare-name
  match; bare-name step keeps another owner's match.
- Gates: `scripts/ci-steps task-affected`, `agentflow check-diff`, then `scripts/ci-steps task`.
