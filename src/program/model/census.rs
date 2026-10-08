//! Population census: does every detector-model row correspond to exactly one
//! program-graph physical row? (Engine-switch S2b.3; spec G5/G6.)
//!
//! The model ([`ModelEntities`]) and the program graph's physical rows
//! ([`PhysicalIndex`]) are both built from the same syntax trees, by two different
//! walks. S2b.4 will mint the model FROM the rows; that is only safe once this
//! census shows the walks agree. The join key is the declaration's file and span:
//! the model's `source_anchor` and the row's span both come from the same
//! `Origin` (the model's columns are byte columns too, see `Utf16Cols`).
//!
//! Rows with no model counterpart are expected: the model skips declarations the
//! program graph keeps (objects of a kind the model does not index, and their
//! routines). A model row with no physical row, or with several, is the failure
//! this census exists to catch.

use std::collections::HashMap;

use super::workspace::ModelEntities;
use crate::program::physical::{PhysicalIndex, Pos};

/// The census result. `model_*` lists name model ids; `rows_unmatched` names
/// physical rows as `file#object_ix[.routine_ix]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhysicalCensus {
    pub model_objects: usize,
    pub object_rows: usize,
    pub objects_matched: usize,
    pub objects_model_unmatched: Vec<String>,
    pub objects_model_multi: Vec<String>,
    pub object_rows_unmatched: Vec<String>,
    pub model_routines: usize,
    pub routine_rows: usize,
    pub routines_matched: usize,
    pub routines_model_unmatched: Vec<String>,
    pub routines_model_multi: Vec<String>,
    pub routine_rows_unmatched: Vec<String>,
}

impl PhysicalCensus {
    /// True when every model row has exactly one physical row.
    pub fn model_fully_mapped(&self) -> bool {
        self.objects_model_unmatched.is_empty()
            && self.objects_model_multi.is_empty()
            && self.routines_model_unmatched.is_empty()
            && self.routines_model_multi.is_empty()
    }

    /// One line per fact, for the switch harness dump.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!(
                "objects model={} rows={} matched={} model_unmatched={} model_multi={} rows_unmatched={}",
                self.model_objects,
                self.object_rows,
                self.objects_matched,
                self.objects_model_unmatched.len(),
                self.objects_model_multi.len(),
                self.object_rows_unmatched.len()
            ),
            format!(
                "routines model={} rows={} matched={} model_unmatched={} model_multi={} rows_unmatched={}",
                self.model_routines,
                self.routine_rows,
                self.routines_matched,
                self.routines_model_unmatched.len(),
                self.routines_model_multi.len(),
                self.routine_rows_unmatched.len()
            ),
            format!(
                "routine rows unexplained={}",
                self.unexplained_routine_rows()
            ),
        ];
        for (tag, list) in [
            ("object-model-unmatched", &self.objects_model_unmatched),
            ("object-model-multi", &self.objects_model_multi),
            ("object-row-unmatched", &self.object_rows_unmatched),
            ("routine-model-unmatched", &self.routines_model_unmatched),
            ("routine-model-multi", &self.routines_model_multi),
            ("routine-row-unmatched", &self.routine_rows_unmatched),
        ] {
            v.extend(list.iter().map(|x| format!("{tag}\t{x}")));
        }
        v
    }
}

pub(crate) type Key = (String, Pos, Pos);

/// A model anchor as a physical-row key: `(file, start, end)`.
pub(crate) fn anchor_key(a: &crate::program::body::features::PAnchor) -> Key {
    let file = a
        .source_unit_id
        .strip_prefix("ws:")
        .unwrap_or(&a.source_unit_id)
        .to_string();
    (
        file,
        Pos {
            row: a.start_line,
            col: a.start_column,
        },
        Pos {
            row: a.end_line,
            col: a.end_column,
        },
    )
}

/// Join `model` against `rows` (see the module doc).
pub fn physical_census(model: &ModelEntities, rows: &PhysicalIndex) -> PhysicalCensus {
    let mut c = PhysicalCensus {
        model_objects: model.objects.len(),
        object_rows: rows.objects.len(),
        model_routines: model.routines.len(),
        routine_rows: rows.routines.len(),
        ..Default::default()
    };

    let mut obj_rows: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, r) in rows.objects.iter().enumerate() {
        obj_rows
            .entry((r.file.clone(), r.start, r.end))
            .or_default()
            .push(i);
    }
    let mut obj_used = vec![false; rows.objects.len()];
    for o in &model.objects {
        let Some(anchor) = &o.source_anchor else {
            c.objects_model_unmatched
                .push(format!("{} (no anchor)", o.id));
            continue;
        };
        match obj_rows.get(&anchor_key(anchor)).map(Vec::as_slice) {
            None | Some([]) => c.objects_model_unmatched.push(o.id.clone()),
            Some([i]) => {
                c.objects_matched += 1;
                obj_used[*i] = true;
            }
            Some(_) => c.objects_model_multi.push(o.id.clone()),
        }
    }
    for (r, used) in rows.objects.iter().zip(&obj_used) {
        if !used {
            c.object_rows_unmatched
                .push(format!("{}#{}", r.file, r.object_ix));
        }
    }

    let mut rt_rows: HashMap<Key, Vec<usize>> = HashMap::new();
    for (i, r) in rows.routines.iter().enumerate() {
        rt_rows
            .entry((r.file.clone(), r.start, r.end))
            .or_default()
            .push(i);
    }
    let mut rt_used = vec![false; rows.routines.len()];
    for r in &model.routines {
        match rt_rows
            .get(&anchor_key(&r.source_anchor))
            .map(Vec::as_slice)
        {
            None | Some([]) => c.routines_model_unmatched.push(r.id.clone()),
            Some([i]) => {
                c.routines_matched += 1;
                rt_used[*i] = true;
            }
            Some(_) => c.routines_model_multi.push(r.id.clone()),
        }
    }
    for (r, used) in rows.routines.iter().zip(&rt_used) {
        if !used {
            c.routine_rows_unmatched.push(format!(
                "{}\t{}#{}.{}",
                skip_reason(r),
                r.file,
                r.object_ix,
                r.routine_ix
            ));
        }
    }
    c
}

/// Why the model has no row for a physical routine row: the model's own skip rules
/// (`workspace.rs`'s `project_ir`), or `unexplained` — the census's real failure.
pub fn skip_reason(r: &crate::program::physical::PhysicalRoutineRow) -> &'static str {
    use al_syntax::ir::ObjectKind;
    match r.node.object.kind {
        ObjectKind::Interface | ObjectKind::ControlAddIn => "signature-only-object",
        _ if r.node.name_lc.is_empty() => "nameless",
        _ => "unexplained",
    }
}

impl PhysicalCensus {
    /// Unmatched routine rows the model's skip rules do not explain.
    pub fn unexplained_routine_rows(&self) -> usize {
        self.routine_rows_unmatched
            .iter()
            .filter(|l| l.starts_with("unexplained\t"))
            .count()
    }
}

/// Engine-switch S5.4 (spec G3/G4): the object facts derived twice from the same
/// IR — by `node_extract` for the resolver's `ObjectNode` and by the model
/// assembly for `ModelObject` — compared object by object. Joined through the
/// physical object rows. One row per disagreement
/// (`{field}\t{model id}\tmodel={…}\tprogram={…}`), then a `compared\t{n}` row.
pub fn object_fact_census(
    model: &ModelEntities,
    graph: &crate::program::graph::ProgramGraph,
) -> Vec<String> {
    use crate::program::node_extract::ObjectRef;
    use al_syntax::IdentifierFoldExt;

    let mut row_node: HashMap<Key, &crate::program::node::ObjectNodeId> = HashMap::new();
    for r in &graph.workspace_rows.objects {
        row_node
            .entry((r.file.clone(), r.start, r.end))
            .or_insert(&r.node);
    }
    let fold_ref = |r: &ObjectRef| match r {
        ObjectRef::Name { normalized_lc, .. } => normalized_lc.clone(),
        ObjectRef::Id(n) => n.to_string(),
    };
    let mut out = Vec::new();
    let mut compared = 0usize;
    for o in &model.objects {
        let Some(node_id) = o
            .source_anchor
            .as_ref()
            .and_then(|a| row_node.get(&anchor_key(a)))
        else {
            continue;
        };
        let Ok(i) = graph.objects.binary_search_by(|n| n.id.cmp(node_id)) else {
            continue;
        };
        let n = &graph.objects[i];
        compared += 1;
        let mut differ = |field: &str, m: String, p: String| {
            if m != p {
                out.push(format!("{field}\t{}\tmodel={m}\tprogram={p}", o.id));
            }
        };
        differ("name", o.name.clone(), n.name.to_string());
        differ(
            "number",
            o.object_number.to_string(),
            n.declared_id.unwrap_or(0).to_string(),
        );
        differ(
            "extends",
            format!(
                "{:?}",
                o.extends_target_name.as_ref().map(|s| s.fold_identifier())
            ),
            format!(
                "{:?}",
                n.extends_target.as_ref().map(|s| s.fold_identifier())
            ),
        );
        if matches!(o.object_type.as_str(), "Page" | "PageExtension") {
            differ(
                "source_table",
                format!(
                    "{:?}",
                    o.source_table_name.as_ref().map(|s| s.fold_identifier())
                ),
                format!("{:?}", n.source_table.as_ref().map(fold_ref)),
            );
        }
        if let Some(model_impl) = &o.implements_interfaces {
            differ(
                "implements",
                format!(
                    "{:?}",
                    model_impl
                        .iter()
                        .map(|s| s.fold_identifier())
                        .collect::<Vec<_>>()
                ),
                format!(
                    "{:?}",
                    n.implements
                        .iter()
                        .map(|s| s.fold_identifier())
                        .collect::<Vec<_>>()
                ),
            );
        }
    }
    out.push(format!("compared\t{compared}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The production analyze model on a fixture with interfaces maps onto the
    /// program graph's physical rows exactly: every model object and routine has
    /// one row, and the only extra rows are the interface signatures the model
    /// skips by rule. Runs `build_analysis_model`, so it pins the rows the
    /// production path carries, not a hand-built index.
    #[test]
    fn analyze_model_maps_one_to_one_onto_physical_rows() {
        let ws =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-cross-object-chain");
        let built = crate::engine::gate::run::build_analysis_model(&ws, true);
        let model = match built.model.expect("fixture builds") {
            crate::engine::gate::run::AnalysisTarget::SingleApp(r) => *r,
            crate::engine::gate::run::AnalysisTarget::CrossApp(_) => {
                unreachable!("single-app build")
            }
        };
        let rows = built
            .physical
            .expect("rows carried past the program context");
        let c = physical_census(&model.workspace, &rows);
        assert!(c.model_fully_mapped(), "{:#?}", c);
        assert_eq!(c.unexplained_routine_rows(), 0, "{:#?}", c);
        assert_eq!((c.model_routines, c.routines_matched), (25, 25));
        assert_eq!(
            c.routine_rows_unmatched,
            vec![
                "signature-only-object\tsrc/ICCBar.Interface.al#0.0".to_string(),
                "signature-only-object\tsrc/ICCFoo.Interface.al#0.0".to_string(),
            ]
        );
    }

    /// Hand-stated: a model routine with no row, and one whose span two rows
    /// share, are both reported; an unmatched row of a plain codeunit is
    /// `unexplained`.
    #[test]
    fn census_reports_missing_ambiguous_and_unexplained() {
        use crate::program::physical::{PhysicalRoutineRow, Pos};
        let ws =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-cross-object-chain");
        let built = crate::engine::gate::run::build_analysis_model(&ws, true);
        let model = match built.model.unwrap() {
            crate::engine::gate::run::AnalysisTarget::SingleApp(r) => r.workspace,
            crate::engine::gate::run::AnalysisTarget::CrossApp(_) => {
                unreachable!("single-app build")
            }
        };
        let mut rows = built.physical.unwrap();

        // Ambiguous: duplicate the first matched row's span.
        let first = rows.routines[0].clone();
        rows.routines.push(first.clone());
        // Missing: move the second row's span away from its model routine.
        rows.routines[1].start = Pos { row: 9999, col: 0 };
        // Unexplained: an extra codeunit row nothing in the model has.
        let mut extra: PhysicalRoutineRow = first.clone();
        extra.start = Pos { row: 8888, col: 0 };
        rows.routines.push(extra);

        let c = physical_census(&model, &rows);
        assert_eq!(c.routines_model_multi.len(), 1, "{:#?}", c);
        assert_eq!(c.routines_model_unmatched.len(), 1, "{:#?}", c);
        assert!(!c.model_fully_mapped());
        // Unexplained: the moved row, the extra row, and BOTH copies of the
        // duplicated span (an ambiguous match claims neither row).
        assert_eq!(c.unexplained_routine_rows(), 4, "{:#?}", c);
    }
}
