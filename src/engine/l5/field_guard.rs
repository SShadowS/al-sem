//! G36: a RecordRef write behind `if not R.FieldExist(N) then exit;` writes a
//! table that has field `N`.
//!
//! Continia DO's `CDO Handled Fields Management.UpdateHandledFields(var
//! FilterRecord: RecordRef)` modifies whatever table the RecordRef holds, so
//! `prove … writes-table:T` had an `unknown-table-write` for every `T`. But it
//! returns first unless `FilterRecord.FieldExist(6175274)`: the write reaches
//! only a table with field 6175274, which only DO's own table extensions add
//! (to Sales Header and the other documents). `CDO E-Mail Log` has no such
//! field, so it is not written there.
//!
//! The guard must be a statement of the routine's own top-level block (so it
//! runs before every later statement), and `R` must not be rebound after it
//! (`GetTable`, `Open`, `Copy`, `Close`, passing `R` on as an argument, an
//! assignment; `R := InputRecord.Duplicate()` before it is fine). A write it guards may write a table `T` exactly when `T`
//! has field `N`, in its own declaration or a loaded extension of it. Ruling a
//! table out needs every extension of it to be in the model: true for a table
//! the workspace declares (a loaded dependency cannot extend it), so only such
//! a table is ruled out; any other keeps the `unknown-table-write`.

use crate::engine::l5::digest_cli::DigestEntryFull;
use crate::program::body::features::{PCFNNode, PCallee};
use crate::program::model::workspace::{Model, ModelRoutine};
use std::collections::{HashMap, HashSet};

/// The field number each guarded RecordRef write of `r` (by operation id)
/// is guarded with.
fn guarded_operations(r: &ModelRoutine) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let Some(tree) = r.statement_tree.as_ref() else {
        return out;
    };
    let lc = |s: &str| s.trim().trim_matches('"').to_ascii_lowercase();
    let recordrefs: HashSet<String> = r
        .variables
        .iter()
        .filter(|v| v.declared_type.trim().eq_ignore_ascii_case("recordref"))
        .map(|v| lc(&v.name))
        .collect();
    if recordrefs.is_empty() {
        return out;
    }
    // Where each RecordRef is assigned (`R := X.Duplicate()`): a guard holds
    // only when every assignment comes before it.
    let mut assigned_at: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    for a in &r.var_assignments {
        let name = lc(&a.lhs_name);
        if recordrefs.contains(&name) {
            assigned_at
                .entry(name)
                .or_default()
                .push((a.source_anchor.start_line, a.source_anchor.start_column));
        }
    }
    // The call sites (by id) that may rebind a RecordRef, with the one they may.
    let mut rebinds: HashMap<&str, HashSet<String>> = HashMap::new();
    for cs in &r.call_sites {
        let mut names: HashSet<String> = cs
            .argument_texts
            .iter()
            .map(|a| lc(a))
            .filter(|a| recordrefs.contains(a))
            .collect();
        if let PCallee::Member { receiver, method } = &cs.callee
            && matches!(
                method.to_ascii_lowercase().as_str(),
                "gettable" | "open" | "copy" | "close"
            )
            && recordrefs.contains(&lc(receiver))
        {
            names.insert(lc(receiver));
        }
        if !names.is_empty() {
            rebinds.insert(cs.id.as_str(), names);
        }
    }
    let writes: HashMap<&str, String> = r
        .record_operations
        .iter()
        .filter(|op| WRITE_OPS.contains(&op.op.to_ascii_lowercase().as_str()))
        .map(|op| (op.id.as_str(), lc(&op.record_variable_name)))
        .collect();
    let top: &[PCFNNode] = match tree.kind.as_str() {
        "block" => tree.children.as_deref().unwrap_or_default(),
        _ => std::slice::from_ref(tree),
    };
    let mut active: HashMap<String, i64> = HashMap::new();
    for stmt in top {
        if let Some((rec, n)) = &stmt.field_exit_guard
            && recordrefs.contains(rec)
        {
            let Some((line, col, ..)) = stmt.source_range else {
                continue;
            };
            if assigned_at
                .get(rec)
                .into_iter()
                .flatten()
                .all(|&at| at < (line, col))
            {
                active.insert(rec.clone(), *n);
            }
            continue;
        }
        let mut ops = Vec::new();
        let mut sites = Vec::new();
        collect(stmt, &mut ops, &mut sites);
        // A rebinding anywhere in the statement ends the guard for all of it.
        for cs in sites {
            for name in rebinds.get(cs).into_iter().flatten() {
                active.remove(name);
            }
        }
        for op in ops {
            if let Some(rec) = writes.get(op)
                && let Some(&n) = active.get(rec)
            {
                out.insert(op.to_string(), n);
            }
        }
    }
    out
}

/// The record operations that write.
const WRITE_OPS: &[&str] = &[
    "insert",
    "modify",
    "delete",
    "modifyall",
    "deleteall",
    "rename",
];

fn collect<'a>(n: &'a PCFNNode, ops: &mut Vec<&'a str>, sites: &mut Vec<&'a str>) {
    if let Some(op) = &n.operation_id {
        ops.push(op);
    }
    if let Some(cs) = &n.callsite_id {
        sites.push(cs);
    }
    for list in [&n.condition_leaves, &n.children, &n.else_children] {
        for c in list.iter().flatten() {
            collect(c, ops, sites);
        }
    }
}

/// G36: for `writes-table:table`, per effect index of `entry`: `true` when the
/// effect is a write whose table is not known but every origin operation is
/// field-guarded and some guard's field is one `table` has (the write may
/// write it), `false` when no guard's field is one `table` has and the
/// workspace declares `table` (it does not). Effects not in the map are
/// unchanged. `table` matches a table's name, case-insensitively.
pub(crate) fn resolve_field_guarded_writes(
    model: &Model,
    primary_guid: &str,
    entry: &DigestEntryFull,
    table: &str,
) -> HashMap<usize, bool> {
    let mut out = HashMap::new();
    let mut guarded: HashMap<String, i64> = HashMap::new();
    for r in &model.workspace.routines {
        guarded.extend(guarded_operations(r));
    }
    if guarded.is_empty() {
        return out;
    }
    let tables = &model.workspace.tables;
    // Every table of that name or number (a name or number can repeat across
    // apps), as `prove`'s own match allows: asked by number, Continia DO's
    // HAND-CORE-002.c5 (`writes-table:6175300`, CDO Send Code) answered
    // `unknown` where the same table by name answered `no`.
    let asked: Vec<&crate::program::model::workspace::ModelTable> = tables
        .iter()
        .filter(|t| {
            !t.is_extension_stub
                && (t.name.eq_ignore_ascii_case(table.trim())
                    || t.id.rsplit('/').next() == Some(table.trim()))
        })
        .collect();
    if asked.is_empty() {
        return out;
    }
    // Field numbers the asked tables have: their own and their loaded
    // extensions' (`merge_extension_fields` moved those onto the table). A
    // stub's own fields are not read: its id may be a real table's (G-5).
    let asked_fields: HashSet<i64> = asked
        .iter()
        .flat_map(|t| &t.fields)
        .map(|f| f.field_number)
        .collect();
    let workspace_declared = !primary_guid.is_empty()
        && asked
            .iter()
            .all(|t| t.app_guid.eq_ignore_ascii_case(primary_guid));
    for (i, eff) in entry.effects.iter().enumerate() {
        if !crate::engine::l5::prove::writes_unknown_table(eff)
            || eff.origin_operation_ids.is_empty()
        {
            continue;
        }
        let Some(fields) = eff
            .origin_operation_ids
            .iter()
            .map(|op| guarded.get(op.as_str()).copied())
            .collect::<Option<Vec<i64>>>()
        else {
            continue;
        };
        if fields.iter().any(|n| asked_fields.contains(n)) {
            out.insert(i, true);
        } else if workspace_declared {
            out.insert(i, false);
        }
    }
    out
}
