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
//! (`GetTable`, `Open`, `Get(Id)`, `Close`, `recref_alias::RecRefBind`; passing `R` on as an argument, an
//! assignment; `R := InputRecord.Duplicate()` before it is fine). A write it guards may write a table `T` exactly when `T`
//! has field `N`, in its own declaration or a loaded extension of it. Ruling a
//! table out needs every extension of it to be in the model: true for a table
//! the workspace declares (a loaded dependency cannot extend it), so only such
//! a table is ruled out; any other keeps the `unknown-table-write`.

use crate::engine::l5::digest_cli::DigestEntryFull;
use crate::program::body::features::PCFNNode;
use crate::program::model::workspace::{Model, ModelRoutine};
use al_syntax::IdentifierFoldExt;
use std::collections::{HashMap, HashSet};

/// The field number each guarded RecordRef write of `r` (by operation id)
/// is guarded with. `shadowed`: the record built-ins a table declares as a procedure.
fn guarded_operations(
    r: &ModelRoutine,
    shadowed: &crate::program::model::recref_alias::ShadowedBuiltins,
) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    let Some(tree) = r.statement_tree.as_ref() else {
        return out;
    };
    // A routine opaque to names (a `with`): a `with`-rebound alias of the guarded RecordRef can
    // hide a write the name-based invalidation misses, so no guard is taken (sol r6, ENG-30).
    if crate::program::body::features::names_opaque(Some(tree), r.body_available) {
        return out;
    }
    // ENG-176: every name is resolved by the routine's scope (`HolderScope`: `this.G` is the
    // global, quoted names, a local hiding a global), never keyed by its text.
    use crate::program::model::recref_alias::{AliasClasses, HolderKey, HolderScope, bind_sites};
    let scope = HolderScope::of(r);
    let key_of = |text: &str| scope.text(text).map(|h| h.key);
    let recordrefs: HashSet<HolderKey> = (0..r.variables.own().len())
        .map(|i| HolderKey::Own(i as u32))
        .chain((0..r.variables.globals().len()).map(|i| HolderKey::Global(i as u32)))
        .filter(|&k| {
            scope
                .variable(k)
                .declared_type
                .trim()
                .eq_fold_identifier("recordref")
        })
        .collect();
    if recordrefs.is_empty() {
        return out;
    }
    // Where each RecordRef is assigned (`R := X.Duplicate()`): a guard holds
    // only when every assignment comes before it.
    let mut assigned_at: HashMap<HolderKey, Vec<(u32, u32)>> = HashMap::new();
    for a in &r.var_assignments {
        if let Some(k) = key_of(&a.lhs_text)
            && recordrefs.contains(&k)
        {
            assigned_at
                .entry(k)
                .or_default()
                .push((a.source_anchor.start_line, a.source_anchor.start_column));
        }
    }
    // ENG-145: `A := R` shares one record (a Variant holds the reference too), and a rebinding
    // through any member rebinds every member (measured on BC 28.4), so a rebinding ends the
    // guard of the whole alias class: the routine's own aliases, and every parameter, global and
    // array holder that may share a record already (`recref_alias::AliasClasses::for_routine`).
    let aliases = AliasClasses::for_routine(r, shadowed);
    // The call sites and record operations (by id) that may rebind a RecordRef, with the ones
    // they may: a call it is passed to, and a bind through it (`recref_alias::RecRefBind`, the
    // one list: `R.Get(Id)` is a record operation, ENG-160).
    let mut names_by_site: HashMap<&str, HashSet<HolderKey>> = HashMap::new();
    for cs in &r.call_sites {
        names_by_site.entry(cs.id.as_str()).or_default().extend(
            cs.argument_texts
                .iter()
                // ENG-175 (sol): a leaf of a ternary receiver or argument may be the handle passed
                // or rebound (BC 28.4: the chosen branch's).
                .chain(cs.ternary_holders())
                .filter_map(|a| key_of(a))
                .filter(|&a| aliases.is_holder(a)),
        );
    }
    for site in bind_sites(r) {
        if let Some(key) = key_of(site.receiver)
            && aliases.is_holder(key)
        {
            names_by_site.entry(site.id).or_default().insert(key);
        }
    }
    // sol round 2 on ENG-145 (ENG-159): a place that may run user code may rebind a record
    // retained elsewhere (a SingleInstance store, a caller's alias), so it ends every retainable
    // holder's guard. A call, or a record operation that fires a trigger or a table event
    // (`recref_alias::user_code_points`). A record operation's own trigger cannot move the
    // record that operation writes (BC 28.4: the write then fails, ENG-159's note), so its
    // rebinding ends the guards only AFTER it (`after_by_op`), unless it is in a loop (a later
    // iteration runs after it) or is not a measured write ([`OWN_TRIGGER_KEEPS`]). Each with
    // where it ends.
    let ends_before: HashMap<&str, (bool, (u32, u32))> = r
        .record_operations
        .iter()
        .map(|op| {
            (
                op.id.as_str(),
                (
                    !op.loop_stack.is_empty()
                        || !OWN_TRIGGER_KEEPS.contains(&op.op.fold_identifier().as_str()),
                    (op.source_anchor.end_line, op.source_anchor.end_column),
                ),
            )
        })
        .collect();
    let mut after_by_op: HashMap<&str, HashSet<HolderKey>> = HashMap::new();
    for p in crate::program::model::recref_alias::user_code_points(r, shadowed) {
        let id = p.id;
        let rebound: Vec<HolderKey> = aliases.rebound_at(p.runs).collect();
        if rebound.is_empty() {
            continue;
        }
        match ends_before.get(id) {
            Some((false, _)) => after_by_op.entry(id).or_default().extend(rebound),
            _ => names_by_site.entry(id).or_default().extend(rebound),
        }
    }
    let after_by_op: HashMap<&str, HashSet<HolderKey>> = after_by_op
        .into_iter()
        .map(|(id, names)| (id, names.iter().flat_map(|&n| aliases.members(n)).collect()))
        .collect();
    let rebinds: HashMap<&str, HashSet<HolderKey>> = names_by_site
        .into_iter()
        .map(|(id, names)| (id, names.iter().flat_map(|&n| aliases.members(n)).collect()))
        .filter(|(_, names): &(&str, HashSet<HolderKey>)| !names.is_empty())
        .collect();
    let writes: HashMap<&str, HolderKey> = r
        .record_operations
        .iter()
        .filter(|op| WRITE_OPS.contains(&op.op.fold_identifier().as_str()))
        .filter_map(|op| Some((op.id.as_str(), key_of(&op.record_variable_name)?)))
        .collect();
    let top: &[PCFNNode] = match tree.kind.as_str() {
        "block" => tree.children.as_deref().unwrap_or_default(),
        _ => std::slice::from_ref(tree),
    };
    let mut active: HashMap<HolderKey, i64> = HashMap::new();
    for stmt in top {
        // The guard's receiver is a logical name (the walk strips its quotes).
        if let Some((rec, n)) = stmt.field_exit_guard()
            && let Some(rec) = scope.logical(rec)
            && recordrefs.contains(&rec)
        {
            let Some((line, col, ..)) = stmt.source_range else {
                continue;
            };
            if assigned_at
                .get(&rec)
                .into_iter()
                .flatten()
                .all(|&at| at < (line, col))
            {
                active.insert(rec, *n);
            }
            continue;
        }
        let mut ops = Vec::new();
        let mut sites = Vec::new();
        collect(stmt, &mut ops, &mut sites);
        // A rebinding anywhere in the statement ends the guard for all of it.
        for id in sites.iter().chain(&ops) {
            for name in rebinds.get(id).into_iter().flatten() {
                active.remove(name);
            }
        }
        // The operations in the order they run, by where each ends (`R.Modify(L.Insert(true))`
        // runs `L.Insert` first, Fable on ENG-159): a write checks the guards still active, then
        // its own trigger's rebinding (`after_by_op`) ends them for the operations after it.
        ops.sort_by_key(|op| ends_before.get(op).map(|(_, pos)| *pos));
        for op in ops {
            if let Some(rec) = writes.get(op)
                && let Some(&n) = active.get(rec)
            {
                out.insert(op.to_string(), n);
            }
            for name in after_by_op.get(op).into_iter().flatten() {
                active.remove(name);
            }
        }
    }
    out
}

/// The writes whose own trigger or table event, rebinding the handle they write through (by
/// `GetTable`, `Get(RecordId)` or `Close` + `Open`), was measured not to move the write (BC 28.4,
/// ENG-159's note: it fails, or writes the record it began with). Any other firing operation's
/// user code (`DeleteAll`, `ModifyAll`, `Validate`, …: unmeasured) ends the guards for its whole
/// statement.
const OWN_TRIGGER_KEEPS: &[&str] = &["insert", "modify", "delete", "rename"];

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
    let shadowed =
        crate::program::model::recref_alias::ShadowedBuiltins::of(&model.workspace.routines);
    for r in &model.workspace.routines {
        guarded.extend(guarded_operations(r, &shadowed));
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
                && (t.name.eq_fold_identifier(table.trim())
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
