# Issue #98: event-dispatch first hops in witness reconstruction (plan)

Spec: `.agent/issue-98/ledger.md` (`## Design`, `## Acceptance matrix`, `## Measurement plan`). The spec panel converged at round 2.

## T0: acceptance tests (red)

- **Where.** `tests/cli/cli_event_dispatch_first_hop.rs` (a new member of the `cli` umbrella), modelled on `tests/cli/cli_implicit_trigger_cone.rs`, which uses inline tempdir workspaces.
- **Tests.**
  - A2/A3: digest of the publisher `OnX`, DB_INSERT path and source evidence, scoped diagnostic. Plus a fingerprint witness for OnX rooted by `roots.config.json`.
  - A2b: the parameter-dependent proof path.
  - A2c: two subscribers.
- **Unit tests.**
  - A1 (`retag`) in `capability_cone.rs`;
  - A1b (`is_first_hop` / `fact_first_hop`) in `digest.rs`;
  - A4 (CBOR and permission provenance, projection) in `snapshot_full.rs` / `capability_cone.rs`.

  Unit tests that need the new field cannot compile before T1, so they land with T1.
- **Red.** Each test fails for the stated reason (capture the output). No commit by itself.

## T1: implementation (D1 + D2)

- `CapabilityFact.witness_first_hop_event_id` is set in `retag` from `edge.event_id`. It goes through all the plumbing named in D1, with the projection via `stable_event_id`.
- `FirstHop` + `fact_first_hop`; `is_first_hop`, Case C and the `enumerate_proofs` early return use it. `edge_event_id()` is the accessor.
- The A1/A1b/A4 unit tests. All T0 tests turn green, plus every discrimination proof from the matrix.
- Commit together with the T0 tests.

## T2: goldens, CHANGELOG, measurement

- Golden diff triaged (`golden-diff-triager`), receipt written, regen, and the manifest matrices updated by hand if needed.
- CHANGELOG under `## [Unreleased]` / Fixed.
- CDO cost measurement: master vs branch, `release-fast`.
