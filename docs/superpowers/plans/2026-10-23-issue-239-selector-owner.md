# Issue #239 plan: an `<owner>::<routine>` selector never falls back to other owners

Design: the issue flow's ledger for #239 (section `## Design`, accepted by both spec
reviewers; attempt 2 amends it). The ledger is flow state, not part of the tree; this
plan states everything the code change needs.

**Rule** (in the shared cascade, `src/engine/l5/selector_index.rs`). `resolve_selector`
returns `SelectorMatches`, whose ids are private: `narrow_to_owner` is the only way to
read them, so a caller cannot skip the owner check. Only a form-5 (object-qualified)
match is narrowed. The object part is:
- empty (`::Persist`): no narrowing, it is a bare routine name;
- typed (first word an object type word, `Codeunit "Sel A"`): keep the matches whose
  owner display key equals it, with an AL quoted name unquoted (`""` -> `"`);
- otherwise a bare object name (`Sel A`, `"Sel A"`): keep the matches whose owner key
  without its type word equals it.

The routine part is unquoted the same way. When nothing is kept, return nothing: each
caller's existing unmatched path fires (digest `selector-unmatched`, fingerprint
`selector-unresolved`, prove `unknown`, all exit 2). `narrow_to_owner`'s signature stays
(fingerprint_query.rs calls it and is not edited here); no diagnostic or renderer changes.

## T1: the rule, its tests, the CHANGELOG

- `selector_index.rs`: `SelectorMatches`, `OwnerPart`, `unquote`, `owner_part`; form 5
  unquotes the routine part; `narrow_to_owner` without the fallback.
- Tests: U3 (`narrow_to_owner_keeps_the_named_owner_only`, through `resolve_selector`);
  in `tests/cli/cli_object_display.rs` A1 (M1), A2 (M2, M3, quoted routine), A3 (M4,
  M6), A4 (typed part vs bare name), A5 (`::Persist`), each asserting digest,
  fingerprint and prove in one assert. A6, A7 unchanged.
- CHANGELOG `Fixed` under `## [Unreleased]`, superseding the 2.0.1 #78 line that says an
  unmatched owner "matches as before".
- Goldens: none expected to move.
- Breaks (one ledger row each): restore the fallback; typed part also matches bare
  names; keep the owner's quotes; keep the routine's quotes; narrow an empty part; drop
  the bare-name match; keep only the first match; read a quoted bare name as typed; no
  `""` unescape; a caller that skips `narrow_to_owner` (must not compile).
- Gates: `agentflow check-diff`, then `scripts/ci-steps task`.
