//! L3 resolve foundation (R2a Task 2) — the WORKSPACE-level symbol table +
//! record-type unification, ported from al-sem's `src/resolve/`.
//!
//! Layered on R0 (identity encoders) + R1 (the L2 body walk). Where L2 processed
//! per-file/per-object, L3 assembles ALL objects + tables + routines across the
//! workspace together (`l3_workspace`), in al-sem's deterministic ingestion order,
//! and runs the first three resolve sub-steps:
//!   `build_symbol_table` (`symbol_table`) → `resolve_record_types`
//!   (`record_types`) → `merge_extension_fields` (`extension_fields`).
//!
//! R2a scope: record-types ONLY. The call graph (R2b), event graph (R2c), and
//! coverage / gaps (R2d) are LATER gates and intentionally OUT.

pub mod al_attributes;
pub mod al_builtins;
pub mod al_type;
pub mod b3_diff;
pub mod call_graph_projection;
pub mod call_resolver;
pub mod coverage;
pub mod event_graph;
pub mod extension_fields;
pub mod global_builtins;
pub mod implicit_edges;
pub mod l3_workspace;
pub mod member_builtins;
pub mod program_calls;
pub mod receiver;
pub mod receiver_type;
pub mod record_types;
pub mod resolution_class;
pub mod static_arg;
pub mod symbol_table;
pub mod taxonomy;
pub mod type_ref;
pub mod type_rel;
