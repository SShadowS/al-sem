//! L4 — per-routine effect summaries over the call graph's SCC condensation.
//!
//! R3a-1 (the FIRST L4 sub-gate) ports the GRAPH substrate:
//!   - `combined_graph` — `build_combined_graph` (the resolved call graph + event
//!     graph → `CombinedEdge`s / `UncertaintyEdge`s / typed `GraphEdge`s) + the
//!     R3a-1 stable projection (`project_r3a1`).
//!   - `scc` — `tarjan_scc` (ITERATIVE Tarjan, reverse-topological output,
//!     deterministic member sort, `recursive` flag).
//!
//! The fixed-point summary core (R3a-2+) layers on this SCC condensation.

pub mod analysis;
pub mod capability_cone;
pub mod cfg_walker;
pub mod combined_graph;
// ⟨C1 residual census⟩ The per-fact byte accounting of the capability cone,
// under `ALSEM_RETAINED_CENSUS=1` (#160; it replaced `C1_CONE_CENSUS=1`). See
// its module doc for the full accounting convention. Inert (no allocation, no
// output) when the env var is unset.
pub mod cone_census;
// C1 — the compact derived cone substrate (`ConeDerivedStore` + the fold + the
// `ConeOutput` mode) that REPLACED the per-routine inherited `Vec<CapabilityFact>`
// on the analyze path: since Task 3 that Vec is not built there at all
// (`ConeOutput::DerivedOnly`). Lives beside `capability_cone`, which folds into it
// at its own `retag` sites.
pub mod cone_derived;
pub mod db_effect_solver;
pub mod effect_lattice;
// ⟨Task 6⟩ The db-effect QUERY surface: `effect_query` is the facade over
// `reverse_index`'s transpose (down / up-global / the ancestor-scoped
// up-query), `effect_query_cli` its `alsem query` transport + the
// `RoutineIx -> ModelRoutine` join that turns an index answer into a
// user-facing one.
pub mod effect_query;
pub mod effect_query_cli;
pub mod effect_store;
pub mod effect_universe;
// #160 step 4: the one execution relation a view builds (graph, SCC, the
// dependency half's uncertainty), from one call split.
pub(crate) mod event_dispatch;
pub mod exec_relation;
pub(crate) mod param_guard;
// #69 D1 — the transitive producer solve: parameter-dependent writes resolved
// in every routine's own frame, outside the shared capability cone.
pub mod pd_solve;
pub mod reverse_index;
pub mod routine_interner;
pub mod scc;
pub mod summary;
pub mod summary_runner;
