//! Who writes a table field (intent-evidence spec §2, "writer"): the routines
//! that write `T.F` in their own body, by assignment (`R.F := …`, including
//! the compound `+=` forms), by `R.Validate(F, …)` or by `R.ModifyAll(F, …)`.
//! "`Modify` after assignment" is the assignment itself: the field write is
//! the assignment's, and the `Modify` that stores it is a table write the
//! digest effects already carry.
//!
//! `alsem explain --table T --field F` lists these writers (spec §4.4). Step
//! 6's `substrate::RULES` writer index (d65) is meant to be built from the same
//! function, so both answer "who writes this field" one way.
//!
//! Read from the model alone: each routine's `field_accesses` (the access kind
//! of #14 says which are writes) and `record_operations`, with the record
//! variable's resolved table (`record_variables[].table_id`, the operation's
//! `table_id`). Stated limits:
//!
//! - Direct writers only. A routine that calls a writer is not one (its rules
//!   at the write are step 5's inherited rules).
//! - Workspace (primary-app) routines only, as d65 counts them (spec §4.5's
//!   blind spot): a dependency's own writers are not listed.
//! - A write is matched to a field of the record variable's table by name.
//!   A write whose record variable has no resolved table, or whose field name
//!   is no field of that table (a `RecordRef` / `FieldRef` write, say), is not
//!   a write of any listed field.
//! - A bare field assignment in a table's own trigger (`Amount := 0;`, no
//!   `Rec.`) is a write only when the body walk records it as a field access
//!   of `Rec`.
//! - **`ModifyAll` writers are not found yet.** The body walk keeps a record
//!   operation's argument texts only for the operations in
//!   `program::body::record_op::FIELD_ARGS_OPS`, and `ModifyAll` is not one.
//!   Adding it there changes the serialized `fieldArguments` of every
//!   `ModifyAll` (the l2 feature goldens) and the input of the detectors that
//!   read those arguments, so it is left to step 6 (d65 needs it), which must
//!   measure that move. The `ModifyAll` arm below already reads the field once
//!   the argument is kept.

use std::collections::HashMap;

use al_syntax::IdentifierFoldExt;

use crate::program::body::features::{FieldAccessKind, PAnchor};
use crate::program::model::role::Role;
use crate::program::model::workspace::{Model, ModelTable, table_by_id_preferring_real};

/// How a routine writes a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriteKind {
    /// `R.F := …` (or `+=`, `-=`, `*=`, `/=`).
    Assign,
    /// `R.Validate(F, …)`: also runs the field's `OnValidate`.
    Validate,
    /// `R.ModifyAll(F, …)`: a set-based write.
    ModifyAll,
}

impl WriteKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WriteKind::Assign => "assign",
            WriteKind::Validate => "validate",
            WriteKind::ModifyAll => "modify-all",
        }
    }
}

/// One field write site.
#[derive(Debug, Clone)]
pub struct FieldWrite<'m> {
    /// The writing row's position in `model.workspace.routines`.
    pub routine_ix: usize,
    /// The table's internal id.
    pub table_id: &'m str,
    /// The field's name as its table declares it.
    pub field: &'m str,
    pub kind: WriteKind,
    pub anchor: &'m PAnchor,
}

/// A field name as the body walk records it, compared the AL way: surrounding
/// quotes dropped, `""` read as `"`, case folded.
fn field_key(name: &str) -> String {
    let t = name.trim();
    let inner = t
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t);
    inner.replace("\"\"", "\"").fold_identifier()
}

/// Every field write of every primary-app routine, in model order and, per
/// routine, assignments first, then record operations, each in body order.
#[must_use]
pub fn field_writes(model: &Model) -> Vec<FieldWrite<'_>> {
    let tables = table_by_id_preferring_real(&model.workspace.tables);
    // table id -> folded field name -> the declared name.
    let mut fields: HashMap<&str, HashMap<String, &str>> = HashMap::new();
    let mut field_of = |table_id: &str, name: &str| -> Option<&str> {
        let t: &ModelTable = tables.get(table_id)?;
        let by_name = fields.entry(t.id.as_str()).or_insert_with(|| {
            t.fields
                .iter()
                .map(|f| (field_key(&f.name), f.name.as_str()))
                .collect()
        });
        by_name.get(&field_key(name)).copied()
    };
    let mut out = Vec::new();
    for (ix, r) in model.workspace.routines.iter().enumerate() {
        if model.routine_role(r) != Role::Primary {
            continue;
        }
        let table_of_var = |var: &str| {
            let key = field_key(var);
            r.record_variables
                .iter()
                .find(|rv| field_key(&rv.name) == key)
                .and_then(|rv| rv.table_id.as_deref())
        };
        for fa in &r.field_accesses {
            if !matches!(
                fa.access,
                FieldAccessKind::Write | FieldAccessKind::ReadWrite
            ) {
                continue;
            }
            let Some(table_id) = table_of_var(&fa.record_variable_name) else {
                continue;
            };
            let Some(table) = tables.get(table_id) else {
                continue;
            };
            if let Some(field) = field_of(table_id, &fa.field_name) {
                out.push(FieldWrite {
                    routine_ix: ix,
                    table_id: table.id.as_str(),
                    field,
                    kind: WriteKind::Assign,
                    anchor: &fa.source_anchor,
                });
            }
        }
        for op in &r.record_operations {
            let kind = if op.op.eq_ignore_ascii_case("Validate") {
                WriteKind::Validate
            } else if op.op.eq_ignore_ascii_case("ModifyAll") {
                WriteKind::ModifyAll
            } else {
                continue;
            };
            // The body walk keeps a record operation's arguments only for
            // `FIELD_ARGS_OPS`: `Validate` is one, `ModifyAll` is not (yet), so
            // a `ModifyAll` has no field here and is skipped (module doc).
            let (Some(table_id), Some(first)) = (
                op.table_id.as_deref(),
                op.field_arguments.as_ref().and_then(|a| a.first()),
            ) else {
                continue;
            };
            let Some(table) = tables.get(table_id) else {
                continue;
            };
            if let Some(field) = field_of(table_id, first) {
                out.push(FieldWrite {
                    routine_ix: ix,
                    table_id: table.id.as_str(),
                    field,
                    kind,
                    anchor: &op.source_anchor,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::field_key;

    #[test]
    fn field_keys_compare_the_al_way() {
        assert_eq!(field_key("\"Customer No.\""), field_key("customer no."));
        assert_eq!(field_key(" Amount "), field_key("AMOUNT"));
        assert_eq!(field_key("\"A \"\"B\"\"\""), field_key("a \"b\""));
        assert_ne!(field_key("Amount"), field_key("Amount2"));
    }
}
