//! L5 QUERY SUBSTRATE (R4-0 Task 2a) — the pure-function + struct-def + native
//! oracle layer the L5 detectors (Task 2b) run over. Byte-parity migration of
//! al-sem's L5 shared substrate.
//!
//! This is the QUERY SUBSTRATE only — NOT the detector harness. It ports:
//!   - `full_summary` — the `FullRoutineSummary` composite the query helpers
//!     read (al-sem carries facts + coverage together on `routine.summary`; the
//!     Rust CORE `RoutineSummary` does not, so this struct re-unifies them).
//!   - `reverse_call_graph` — al-sem `src/engine/reverse-call-graph.ts`.
//!   - `entry_points` — al-sem `src/engine/entry-points.ts`.
//!   - `capability_query` — al-sem `src/detectors/capability-query.ts`.
//!   - `transaction_spans` — al-sem `src/engine/transaction-spans.ts`.
//!
//! NONE of: the `Finding` type, the detector registry, `detector_context`,
//! `path_walker`, the pipeline entry, or aldump wiring — those are Task 2b.
//!
//! Determinism: every output collection that flows to a fingerprint or a dump is
//! a sorted `Vec` / `BTreeSet`. No `HashMap` iteration order reaches output.

pub mod capability_query;
// ⟨C1 Task 3⟩ `cone_parity` — the dual-run derived-vs-raw oracle — is GONE. It
// compared each derived predicate against the raw `capability_facts_inherited`
// Vec; Task 3 stops building that Vec on every path that had one, so the oracle's
// raw side would have degraded to direct-only and false-alarmed on every routine.
// Its one Vec-independent test (the G-18 collision pin) was RELOCATED to
// `detector_context`'s test module, not deleted; ⟨T1⟩ then turned it from a pin on
// the collision's degenerate summary into a pin on the REAL summary surviving
// (`colliding_routine_ids_keep_a_real_summary_and_derived_row`), and deleted
// `ConeDerivedStore::forget` along with the degeneracy it existed to mirror.
// G-19 — closed-world temp inference for `local` routines (the proven
// `(routine, param)` set the d1/d3/d10 temp gates consult).
pub mod closed_world_temp;
pub mod entry_points;
pub mod full_summary;
pub mod reverse_call_graph;
pub mod transaction_spans;

// R4 PATH-WALKER SUPPORT SUBSTRATE — the shared pure-function modules the
// d1/d2/d48/d14 path-walker detectors consume (NO detectors yet). Each ports one
// al-sem L5 helper:
//   - `op_classification` — al-sem `src/engine/op-classification.ts`.
//   - `table_display`     — al-sem `src/detectors/table-display.ts`.
//   - `path_merge`        — al-sem `src/detectors/path-merge.ts`.
//   - `actionable_anchor` — al-sem `src/projection/actionable-anchor.ts`.
pub mod actionable_anchor;
pub mod op_classification;
pub mod path_merge;
pub mod table_display;

// R4-0 Task 2b — the L5 HARNESS (Finding model + stable projection, fingerprint,
// confidence, detector registry, detector context, path walker) and the ported
// detectors (currently d4).
pub mod confidence;
pub mod detector_context;
pub mod detectors;
// d1-reachability redesign Task 1 — the compact filtered graph + terminals +
// seeds `build_d1_graph` extracts for the reachability search. LIVE:
// `detectors::d1::detect_d1` calls it directly.
pub(crate) mod d1_graph;
// d1-reachability redesign Task 2 — the forward param-temp-state vector
// (`root_state`/`cross_hop`/`resolve_terminal`), differentially proven
// equivalent to the backward `path_temp_resolve` resolver. LIVE: threaded
// through `d1_graph`'s compact graph by `d1_dataflow::score_batch_to_sink`.
pub(crate) mod d1_temp;
// d1-reachability redesign Task 3 — the multi-source per-(loop, terminal-op)
// reachability search over `d1_graph`/`d1_temp`. `search_loops_cohorts` is
// the LIVE production entry point (`detectors::d1::detect_d1` calls it); the
// original per-loop label search (`process_group`/`search_loops`) survives
// only as the `#[cfg(test)]` differential oracle it and `d1_dataflow`'s batch
// solvers are checked against.
pub(crate) mod d1_reach;
// d1 dataflow-solver redesign Task D1 — the backward `Need[node]` param-
// liveness fixpoint + the compiled per-edge `ParamTransfer` table the fact
// solvers consume. LIVE: `compute_liveness` is called once per run by
// `d1_reach::search_loops_cohorts`.
pub(crate) mod d1_liveness;
// d1 dataflow-solver redesign Task D2/D3 — the batched fact solver
// (`BatchSolver`/`score_batch_to_sink`): reach/value facts seeded from
// `d1_temp`, propagated level-synchronously over `d1_graph` using
// `d1_liveness`'s compiled `ParamTransfer` table, scored + winner-selected to
// reproduce `d1_reach::process_group`'s six load-bearing components (coverage,
// reachable_verdicts, severity, verdict, depth_bucket, unc) per (loop,
// terminal-op). `score_batch_to_sink` is LIVE — `search_loops_cohorts` calls
// it per batch; the single-group `solve_group` and its batched predecessor
// `solve_batch` are `#[cfg(test)]`-only differential oracles.
pub(crate) mod d1_dataflow;
// d1 cohort redesign Task C1 — the terminal bitmap-cohort SINK (`TerminalSink`,
// `GroupBitmap`, `ContextKey`) that replaces the old per-(loop, terminal)
// witness materialization (`d1_dataflow::emit_lane_aggregates`, now
// `#[cfg(test)]`-only). LIVE — `detect_d1`'s production path
// (`search_loops_cohorts` -> `score_batch_to_sink`) emits into it directly.
pub(crate) mod d1_cohort;
// d1 cohort redesign Task C3 — the bounded REPRESENTATIVE witness
// (`representative_witness`/`WitnessSummary`) for one `(terminal, ContextKey)`
// cohort: `[loop_step, call_step]` (seed via Task C2's O(1) `origin_seed`, no
// chain walk to find it) + first-K/last-M hop steps + `omitted_hops` + the
// terminal step, built from ONE bounded predecessor-chain walk per cohort
// instead of `d1_dataflow::build_transitive_witness`'s (now `#[cfg(test)]`-only)
// per-(loop, terminal) full witness. LIVE — `d1_dataflow::build_cohort_rep`
// calls it to materialize each cohort's witness lazily.
pub(crate) mod d1_witness;
// Shared event-flow substrate (al-sem `src/engine/event-flow.ts`) the d43/d44/d45
// event-flow detectors consume. NO detectors yet — index + query + fan-out +
// chain-walk substrate only.
pub mod event_flow;
pub mod finding;
pub mod fingerprint;
pub mod path_temp_resolve;
pub mod path_walker;
// The writes pending at a Commit (engine-switch S8 gap 3): d8 / d9.
pub(crate) mod pending_writes;
pub mod registry;

// R4-F Stage-2b — the CapabilitySnapshot CONSUMED-CORE port (composeSnapshot's
// ordering-facts subset). Re-projects the R3a source-only base into the snapshot
// shape + the R4-F stable projection. Additive — no detector/gate depends on it.
pub mod snapshot;
pub mod snapshot_full;

// R4-F Stage-3b — the DIGEST witness + effects + occurrence-build path. Reads off
// the Stage-2 CapabilitySnapshot (snapshot.rs) and produces per-root
// DigestEffectResult[] each with a stable occurrenceId (= factId). Additive.
pub mod digest;

// R4-F Stage-4 — the ORDERING ENGINE: per-effect scopedGuarantee derivation for
// the 5 relevant hazard labels. 4a (intra HB), 4b (cross-hop substrate), 4c
// (compute_ordering + the 5 root labels + merge). Additive.
pub mod ordering;
pub mod ordering_engine;
pub mod ordering_inter;

// R4-F Stage-5b — the ordering-facts FACADE (compute_ordering_facts +
// gradeGuarantee) the d47/d49/d51 detectors consume, plus the M5 stable
// projection. Wraps the Stage-4 ordering engine. Additive.
pub mod ordering_facts;

// cli-b/b1 — DIGEST CLI support modules (unresolved-cone BFS, conditionality
// lattice, unified-diff parser, digest CLI pipeline).
pub mod conditionality;
pub mod diff_parser;
pub mod digest_cli;
// Shared deterministic routine-selector resolution (5-form resolveSelector
// cascade). Consumed by BOTH digest_cli (changed-roots selectors) and
// fingerprint_query (routine selectors) — kept as ONE implementation so the two
// call sites cannot drift apart (see module docs for the bug this fixed).
pub mod selector_index;
pub mod unresolved_cone;

// cli-b/b2 — PROVE CLI (tristate absence-safety query + json/human formatters).
pub mod prove;

// cli-b/b3 — FINGERPRINT QUERY + projection + human renderer.
// Reuses B1's witness machinery (digest.rs public exports).
pub mod fingerprint_query;

// cli-b/b3 — FINGERPRINT CLI pipeline (format dispatch, workspace loading).
pub mod fingerprint_cli;

#[cfg(test)]
mod test_support;
