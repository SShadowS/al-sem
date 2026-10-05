//! The detector model: the objects, tables and routines (with their body facts)
//! that `alsem analyze`'s detectors read, the passes that assemble and enrich it,
//! and the call-resolution shape it carries.
//!
//! Moved out of `engine::l3` in engine-switch S2b.2 (spec
//! `docs/superpowers/specs/2026-10-06-engine-switch-design.md`). The types keep
//! their `L3*` names until the rename in S9; `engine::l3` re-exports these modules
//! under their old names meanwhile.

pub mod calls;
pub mod extension_fields;
pub mod record_types;
pub mod symbol_table;
pub mod taxonomy;
pub mod workspace;
