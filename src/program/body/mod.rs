//! L2 structural body-walk + feature projection.
//!
//! The L2 walk is driven entirely by the owned `al-syntax` IR ([`ir_walk`]); the
//! per-file / per-routine projection lives in [`l2_workspace`]. This module retains
//! only the shared nesting-depth metric over the projected loops.

pub mod capability;
pub mod control_context;
pub mod control_flow;
pub mod features;
pub mod ir_walk;
pub mod l2_workspace;
pub mod node_util;
pub mod operation_order;
pub mod record_op;
pub mod scope;

/// Project the L2 `PFeatures` of a single named routine in a one-file source, via
/// the owned IR. Thin wrapper over [`l2_workspace::ir_features_for_named_routine`]
/// returning only the features — the entry point the L2 vector / receiver oracle
/// tests drive. `None` when the routine isn't found.
pub fn features_for_named_routine(
    source: &str,
    routine_name: &str,
    app_guid: &str,
    model_instance_id: &str,
    source_unit_id: &str,
) -> Option<features::PFeatures> {
    l2_workspace::ir_features_for_named_routine(
        source,
        routine_name,
        app_guid,
        model_instance_id,
        source_unit_id,
    )
    .map(|(f, _, _)| f)
}

/// True if loop `outer`'s source range strictly contains loop `inner`'s.
fn loop_strictly_contains(outer: &features::PLoop, inner: &features::PLoop) -> bool {
    if outer.id == inner.id {
        return false;
    }
    let o = &outer.source_anchor;
    let i = &inner.source_anchor;
    let starts_before = o.start_line < i.start_line
        || (o.start_line == i.start_line && o.start_column <= i.start_column);
    let ends_after =
        o.end_line > i.end_line || (o.end_line == i.end_line && o.end_column >= i.end_column);
    starts_before && ends_after
}

pub(crate) fn compute_nesting_depth(loops: &[features::PLoop]) -> u32 {
    let mut max_depth = 0;
    for loop_ in loops {
        let enclosing = loops
            .iter()
            .filter(|other| loop_strictly_contains(other, loop_))
            .count() as u32;
        let depth = 1 + enclosing;
        if depth > max_depth {
            max_depth = depth;
        }
    }
    max_depth
}
