//! Physical declaration rows (engine-switch S2b.3; spec G5/G6).
//!
//! The program graph sorts and dedups its workspace nodes (`build.rs`'s
//! `assemble_program_graph`): two source occurrences that mint the same
//! [`RoutineNodeId`] become one node. The detector model, built from the same
//! syntax trees, keeps EVERY occurrence, in ingestion order. To build the model
//! from the program engine (S2b.4) the graph must therefore remember each
//! occurrence before dedup — its file, its position in that file's object and
//! routine lists, its source span — and which node it became. That is a physical
//! row. One node may have several rows (one-to-many); a row has exactly one node.

use crate::program::node::{ObjectNodeId, RoutineNodeId};

/// A source position (zero-based row, byte column), as in `al_syntax::ir::Point`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub row: u32,
    pub col: u32,
}

impl Pos {
    pub fn of(p: al_syntax::ir::Point) -> Self {
        Pos {
            row: p.row,
            col: p.column,
        }
    }
}

/// One object declaration as it occurs in a workspace file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalObjectRow {
    /// The file's virtual path (workspace-relative, POSIX).
    pub file: String,
    /// Index into the file's `objects`, document order.
    pub object_ix: u32,
    /// The object declaration's span (`ObjectDecl::origin`).
    pub start: Pos,
    pub end: Pos,
    pub node: ObjectNodeId,
}

/// One routine declaration as it occurs in a workspace file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalRoutineRow {
    /// The file's virtual path (workspace-relative, POSIX).
    pub file: String,
    /// Index into the file's `objects`, document order.
    pub object_ix: u32,
    /// Index into that object's `routines`, document order.
    pub routine_ix: u32,
    /// The routine declaration's span (`RoutineDecl::origin`).
    pub start: Pos,
    pub end: Pos,
    /// The node this occurrence became (after dedup, possibly shared).
    pub node: RoutineNodeId,
}

/// The workspace's physical rows, in ingestion order: files in the workspace
/// unit's order, then document order within each file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhysicalIndex {
    pub objects: Vec<PhysicalObjectRow>,
    pub routines: Vec<PhysicalRoutineRow>,
}

impl PhysicalIndex {
    /// Record the rows for one file, given the nodes `extract_nodes` just appended
    /// for it (`new_objects` / `new_routines`, in push order).
    ///
    /// # Panics
    /// When the appended counts do not match the file's declarations:
    /// `extract_nodes` pushes exactly one object node per `file.objects` entry and
    /// one routine node per `obj.routines` entry, in order, and the rows are only
    /// correct while that holds. A change there must fail here, not mis-assign rows.
    pub fn record_file(
        &mut self,
        file: &str,
        ir: &al_syntax::ir::AlFile,
        new_objects: &[crate::program::node_extract::ObjectNode],
        new_routines: &[crate::program::node_extract::RoutineNode],
    ) {
        let declared: usize = ir.objects.iter().map(|o| o.routines.len()).sum();
        assert_eq!(
            (new_objects.len(), new_routines.len()),
            (ir.objects.len(), declared),
            "extract_nodes no longer pushes one node per declaration for {file}; \
             physical rows would be mis-assigned"
        );
        let mut nodes = new_routines.iter();
        for (oi, (obj, onode)) in ir.objects.iter().zip(new_objects).enumerate() {
            self.objects.push(PhysicalObjectRow {
                file: file.to_string(),
                object_ix: oi as u32,
                start: Pos::of(obj.origin.start),
                end: Pos::of(obj.origin.end),
                node: onode.id.clone(),
            });
            for (ri, r) in obj.routines.iter().enumerate() {
                let node = nodes.next().expect("count checked above");
                self.routines.push(PhysicalRoutineRow {
                    file: file.to_string(),
                    object_ix: oi as u32,
                    routine_ix: ri as u32,
                    start: Pos::of(r.origin.start),
                    end: Pos::of(r.origin.end),
                    node: node.id.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every physical row names the node minted from ITS OWN declaration, sits at
    /// its own span, and that node survives the dedup into the graph. Re-derives
    /// each row's expectation by walking the parse independently, so a mis-zipped
    /// row (wrong node on the right span) fails here — the census, which joins on
    /// spans, cannot see that.
    #[test]
    fn each_row_names_the_node_of_its_own_declaration() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/r0-corpus/ws-cross-object-chain");
        let (ctx, _report, _) =
            crate::program::resolve::full::build_program_with_coverage(&ws).unwrap();
        let graph = ctx.graph();
        let rows = &graph.workspace_rows;
        let mut checked = 0;
        for pf in ctx.parsed().iter().flat_map(|u| u.files.iter()) {
            for (oi, obj) in pf.file.objects.iter().enumerate() {
                let orow = rows
                    .objects
                    .iter()
                    .find(|r| r.file == pf.virtual_path && r.object_ix == oi as u32)
                    .expect("object row");
                assert_eq!(orow.node.kind, obj.kind);
                assert_eq!(orow.start, Pos::of(obj.origin.start));
                for (ri, r) in obj.routines.iter().enumerate() {
                    let row = rows
                        .routines
                        .iter()
                        .find(|x| {
                            x.file == pf.virtual_path
                                && x.object_ix == oi as u32
                                && x.routine_ix == ri as u32
                        })
                        .expect("routine row");
                    let expect =
                        crate::program::sig_fp::source_routine_node_id(orow.node.clone(), r);
                    assert_eq!(row.node, expect, "{}#{oi}.{ri}", pf.virtual_path);
                    assert_eq!(
                        (row.start, row.end),
                        (Pos::of(r.origin.start), Pos::of(r.origin.end))
                    );
                    assert!(
                        graph.routines.iter().any(|n| n.id == row.node),
                        "row node missing from the graph"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, rows.routines.len());
        assert!(checked > 20, "fixture too small to discriminate: {checked}");
    }
}
