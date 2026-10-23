# Issue #247 plan: `looseFingerprint` on analyze findings

Design: `.agent/issue-247/ledger.md`, section `## Design` (accepted by both reviewers).

**Rule.** `looseFingerprint` = the fingerprint's five parts with `routineName` blanked
and, only when the anchor (`primaryLocation.enclosingRoutineId`) is a known routine,
every occurrence of the anchor's internal or stable id in `rootCauseKey`, plus the one
`/(op|cs|loop|u)<digits>` site-id segment minted on it (ending at `/`, `|` or the key's
end), replaced by `~`. A parameter position `/p{N}` is kept (final review round 1,
CR1/CR2). `fingerprint` bytes do not change.

Acceptance tests (already red): `tests/cli/cli_loose_fingerprint.rs` T1, T2, T4, T5.

## T1: compute and emit the key

- `fingerprint.rs`: split `fingerprint_of` into shared helpers (object part,
  stabilization, hash) so both keys use one copy; add `loose_fingerprint_of` and the
  anchor strip; add `routine(id)`/`object(id)` accessors. Unit tests: T3 (detector
  part), the suffix chain, `/p2x` kept, a callee id kept, the stable-id form.
- `projection.rs`: `ProjectionIndex` holds a `FingerprintIndex` and reads its maps
  through it (`to_location`, `run.rs` `dependency_object_predicate`);
  `FindingSummary.loose_fingerprint` set in `project_finding`.
- `format_json.rs`: insert `"looseFingerprint"`.
- The six test-only `FindingSummary { … }` literals gain the field.
- Pin T4's literal once the value exists.
- Goldens: `scripts/check-goldens --regen`; only `tests/cli-a-goldens/json/*` may
  move, and only by `+ "looseFingerprint"` lines (the Design's diff check).
- Discrimination proofs for T1-T5 (Edit break, run, Edit revert, run).

## T2: docs and measurement

- CHANGELOG `Added` and README analyze section: both keys, the counting rule
  new = Σ max(0, after(k) − before(k)), both-keys-first matching, preconditions, limits.
- CDO measurement (base vs branch): fingerprint multiset, distinct loose keys,
  wall time and peak RSS against the 1 % fallback thresholds.
- Gate: `scripts/ci-steps task`.
