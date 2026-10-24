# Issue #237 — one-hop rename normalization and `rename-overlay-collision`

Spec: the `## Design` in `.agent/issue-237/ledger.md`. One task.

## Task 1 — state one hop, add the collision diagnostic, pin both

Principle: **no rename-table entry may put two live old routines on one key.**

1. Tests first (red):
   - `tests/cli/cli_b_diff_differential.rs`: A1–A5 and D1a–D1d, each through the real
     `alsem diff` binary (`CARGO_BIN_EXE_alsem`) on `.cbor` snapshots written with
     `gate::cbor::encode`, no coverage records.
   - `src/engine/gate/diff/renames.rs` unit tests: `overlay_collision_diagnostic`
     (`{X→T}` old `{X,T}` → one collision; `{X→T, T→U}` → none) and
     `overlay_stale_key_is_not_a_collision` (O9: `{Y→T}`, old `{T}` → only a
     `not-in-old` stale for `Y`).
   - A1–A5 and O9 pin today's behaviour, so they pass at once; their value is their
     breaks.
2. `validate_overlay_against_snapshots`: after each entry's stale checks, push
   `rename-overlay-collision {kind, from, to}` when the old id and the target are both
   live old ids and the target is not a table key.
3. Docs: `normalize` (`indexes.rs`) is one hop by design; `pair_signature_changes`
   states the principle and why the exclusions and the old-side user-target inclusion
   follow from it.
4. CHANGELOG `[Unreleased]`: Added (the diagnostic), Changed (one hop is the rule).
5. Discrimination proofs (a)–(d) and O9, made with Edit and checked applied.
6. Gates: `scripts/check-goldens` (no golden should move, P18), `scripts/ci-steps task`.
