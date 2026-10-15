//! The detector model: the objects, tables and routines (with their body facts)
//! that `alsem analyze`'s detectors read, the passes that assemble and enrich it,
//! and the call-resolution shape it carries.
//!
//! Moved out of `engine::l3` in engine-switch S2b.2 (spec
//! `docs/superpowers/specs/2026-10-06-engine-switch-design.md`). The types keep
//! their `L3*` names until the rename in S9; `engine::l3` re-exports these modules
//! under their old names meanwhile. S9.4 moved the program-call adapter and model
//! builders (`program_calls`) and the event-parameter temp proof here; S9.6 moved
//! coverage, deleted L3, and made the model's calls and events mandatory.

pub mod abi_rows;
pub mod calls;
pub mod census;
pub mod coverage;
pub mod event_param_temp;
pub mod events;
pub mod extension_fields;
pub mod program_calls;
pub mod record_types;
pub mod site_links;
pub mod symbol_table;
pub mod taxonomy;
pub mod workspace;

use crate::program::graph::ProgramGraph;
use crate::program::node::{ObjKey, ObjectNodeId, RoutineNodeId};

/// The model's object id for a program object (`encode_object_id`'s
/// `"{app guid}/{type}/{object key}"`), the same string the model writes for it.
///
/// A name key is the FOLDED name: on a source object the folded raw name (inner
/// `""` kept), so it is unescaped here to the folded logical name the model keys
/// on; on an ABI object already the folded logical name (see the #83 spec's Limits
/// for a logical name that itself holds `""`).
pub fn model_object_id(graph: &ProgramGraph, id: &ObjectNodeId) -> String {
    use crate::engine::ids::object_key;
    let guid = &graph.apps.resolve(id.app).guid;
    let ty = crate::program::body::ir_walk::ir_object_type(&id.kind).unwrap_or("Unknown");
    let key = match &id.key {
        ObjKey::Id(n) => object_key(Some(*n), ""),
        ObjKey::Name(n) => object_key(
            None,
            &crate::program::body::scope::unescape_al_identifier(n),
        ),
    };
    crate::engine::ids::encode_object_id(guid, ty, &key)
}

/// A routine outside the model, named in the model's terms:
/// `"{model object id}::{routine name, folded}/{arity}"`, where a field trigger's
/// name is `"{field, folded}::{trigger}"`. Used for dependency call targets
/// (`ExternalTargetRef::target`) and dependency or platform event publishers.
pub fn model_routine_key(graph: &ProgramGraph, id: &RoutineNodeId) -> String {
    let name = match &id.enclosing_member_lc {
        Some(member) => format!("{member}::{}", id.name_lc),
        None => id.name_lc.to_string(),
    };
    format!(
        "{}::{name}/{}",
        model_object_id(graph, &id.object),
        id.params_count
    )
}
