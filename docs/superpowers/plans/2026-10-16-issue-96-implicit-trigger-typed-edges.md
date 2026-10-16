# Plan: issue #96, implicit-trigger edges in the typed graph

Spec: `docs/superpowers/specs/2026-10-16-issue-96-implicit-trigger-typed-edges-design.md` (revision 3, ratified by the sol + opus spec panel in 4 rounds). Each task is TDD: red, then green, then discrimination proofs for the tests it makes pass. Per CLAUDE.md, format each file with `rustfmt`. No golden is regenerated before T5's triage.

| Task | Scope (spec section) | Done when |
|---|---|---|
| T0 | Acceptance tests A1-A6 (spec matrix), written against today's code: a new `tests/cli/cli_implicit_trigger_cone.rs` (inline tempdir workspaces, the `temp_state_probe::write_workspace` pattern) plus the A6 cross-app fixture. They compile and fail for the stated reason. They are not committed alone. | the failing output is in the ledger |
| T1 | D1 typed edge: `TypedEdge` gains `operation_id`, `trigger_kind`, `fires`; the `build_typed_edges` branch goes before the callsite lookup; RunTrigger is captured for `Insert`. D3.1 trigger `Rec` becomes PD (`ir_walk.rs`). D3.2 L4 op-receiver binding (`summary_runner.rs`). | lib tests green; A3's L4 assert green |
| T2 | D2: `TypedOutEdge.operation_id` (in its sort key); `PdEdgeKey` gains `operation_id` on both sides. D3.3 cone `bound_site`/`cross_edge` for `implicit-trigger`. D5's `retag` sets `witness_first_hop_operation_id` explicitly. | A2 and A3/A4 (cone) green |
| T3 | D4 snapshot `SnapshotGraphEdge::ImplicitTrigger`, `compute_edge_id`, operation evidence, the no-anchor rule. D5 digest: `WitnessHop`/`HumanHop` variants, Case C and `enumerate_proofs` by op id, the explicit conditionality arm, memo identity. D7 prose, plus the r3 parity oracle twinning by op. | A1-A6 green; breaks (a)-(f) recorded |
| T4 | Goldens: `scripts/check-goldens`, a line-by-line triage against the spec's per-fixture prediction, the receipt, then `check-goldens --regen`. | every moved line explained |
| T5 | Measurement: CDO `--program-call-graph-stats` byte-identical; CDO/DO `alsem analyze` before/after at `--scope primary` and `--scope all` with every finding delta triaged; cost (`ALSEM_CONES_CENSUS` PD counters, analyze/digest/fingerprint time and peak, witness counters) on CDO and DO. CHANGELOG entry. | numbers in the ledger |

Order: T0 → T1 → T2 → T3 → T4 → T5. T1-T3 may be one implementer dispatch per task, each committing when its own tests and the task gate are green. Goldens move only in T4, so commits before T4 skip nothing: the pre-commit golden hook runs on each. If an intermediate commit would leave goldens red, T1-T3 are committed together with T4's regen.
