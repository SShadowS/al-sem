//! L3 extension-field merge — Rust port of al-sem's
//! `src/resolve/extension-fields.ts` (`mergeExtensionFields`).
//!
//! Walks `objects` for `objectType === "TableExtension"` IN ASSEMBLED ORDER,
//! resolves the base table by the extension's `extends` target name, finds the
//! extension's own indexed table, and appends each of its fields onto the base
//! table — physically relocated (`id` / `physicalTableId` rekeyed to the base
//! table) but preserving provenance (`declaringObjectId` / `declaringAppId` stay
//! the extension's).
//!
//! FIRST-wins on a duplicate `fieldNumber` (the dedup set seeds from the base
//! table's existing fields and grows as fields are appended). Because objects are
//! walked in al-sem's deterministic ingestion order, two extensions colliding on
//! a field number resolve identically on both sides: the FIRST-ingested wins.

use super::workspace::{ModelEntities, ModelField, ModelKey};
use crate::engine::ids::{encode_field_id, encode_table_id};
use std::collections::HashSet;

/// Merge each TableExtension's fields into its base table's field set, mutating
/// `workspace.tables` in place. Conservative & idempotent (skip when the extends
/// target / base table / extension's own table are absent; dedup by fieldNumber,
/// FIRST-wins).
///
/// Port of the al-sem original `src/resolve/extension-fields.ts`. (Its R2.5a
/// projected-entity twin in `deps::merged_index` was deleted in engine-switch S9.6;
/// this is the only copy.)
pub fn merge_extension_fields(
    workspace: &mut ModelEntities,
    scope: &super::symbol_table::TableScope,
) {
    // ENG-194 (Fable r1): each table extension's base as its own app sees it
    // (`SymbolTable::table_by_name_from`, as the record types bind `Rec`), not
    // the last table of the name. Worked out before the loop mutates the tables.
    // Keyed by the object's ROW (Fable r2: an `#if`-split tableextension has a
    // row per alternative, each with its own target).
    let base_ids: Vec<Option<String>> = {
        let symbols = super::symbol_table::SymbolTable::build_scoped(
            &workspace.objects,
            &workspace.tables,
            scope,
        );
        workspace
            .objects
            .iter()
            .map(|o| {
                let target = o.extends_target_name.as_deref()?;
                if o.object_type != "TableExtension" {
                    return None;
                }
                let base = match target.trim().parse::<i64>() {
                    Ok(n) => symbols.table_by_number_from(&o.app_guid, n),
                    Err(_) => symbols.table_by_name_from(&o.app_guid, target),
                }?;
                Some(base.id.clone())
            })
            .collect()
    };
    // The bases are resolved above; each one's index (and the extension's
    // stub's) is found per extension below. We must mutate tables in place,
    // so we resolve indices, then apply.
    //
    // `objects.clone()` is a genuine borrow-conflict clone, not a stylistic
    // choice: the loop below reads `objects` while mutating `workspace.tables`
    // (the `fields.push` a few lines down), so removing it needs a
    // `&mut *workspace` destructure into disjoint field borrows — a real
    // (if small, ~4 MB) edit, not a one-liner.
    let objects = workspace.objects.clone();
    // Per tableextension row: which alternative of its id it is (0-based),
    // and how many rows the id has.
    let mut rows_of_id: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let alternative: Vec<usize> = objects
        .iter()
        .map(|o| {
            if o.object_type != "TableExtension" {
                return 0;
            }
            let n = rows_of_id.entry(o.id.as_str()).or_insert(0);
            *n += 1;
            *n - 1
        })
        .collect();
    for (row, object) in objects.iter().enumerate() {
        if object.object_type != "TableExtension" {
            continue;
        }
        let Some(base_id) = &base_ids[row] else {
            continue;
        };
        // The LAST real row of the id, the row `SymbolTable::table_by_id` keeps
        // (an `#if`-split table has one per alternative; sol r2 on ENG-194).
        let Some(base_idx) = workspace
            .tables
            .iter()
            .rposition(|t| !t.is_extension_stub && &t.id == base_id)
        else {
            continue;
        };
        // The extension's own STUB (G-5): its id reuses the extension's object
        // number, which a real table of the same app may share. Continia DO's
        // `CDOSalesHeaderExt` (6175275, adds field 6175274 `CDO Handled`) and
        // its table 6175275 `CDO E-Mail Template Merge Field`: the id lookup
        // took the table, so Sales Header got that table's fields and lost the
        // extension's.
        let extension_table_id = encode_table_id(&object.app_guid, object.object_number);
        // An `#if`-split tableextension has an object row and a stub per
        // alternative, both in declaration order: the k-th row of the id takes
        // the k-th stub (sol r3: every row took the LAST stub, so each target
        // got the last alternative's fields). Counts that disagree keep the
        // last stub, as before.
        let stubs: Vec<usize> = workspace
            .tables
            .iter()
            .enumerate()
            .filter(|(_, t)| t.is_extension_stub && t.id == extension_table_id)
            .map(|(i, _)| i)
            .collect();
        let (alt, rows) = (alternative[row], rows_of_id[object.id.as_str()]);
        let Some(&ext_idx) = (if stubs.len() == rows {
            stubs.get(alt)
        } else {
            stubs.last()
        }) else {
            continue;
        };

        let base_table_id = workspace.tables[base_idx].id.clone();
        let mut existing: HashSet<i64> = workspace.tables[base_idx]
            .fields
            .iter()
            .map(|f| f.field_number)
            .collect();

        let ext_fields = workspace.tables[ext_idx].fields.clone();
        for field in ext_fields {
            if existing.contains(&field.field_number) {
                continue;
            }
            let merged = ModelField {
                id: encode_field_id(&base_table_id, field.field_number),
                physical_table_id: base_table_id.clone(),
                declaring_object_id: object.id.clone(),
                declaring_app_id: object.app_guid.clone(),
                field_number: field.field_number,
                name: field.name.clone(),
                field_class: field.field_class.clone(),
                data_type: field.data_type.clone(),
                is_blob_like: field.is_blob_like,
                captions: field.captions.clone(),
                calc_formula: field.calc_formula.clone(),
            };
            workspace.tables[base_idx].fields.push(merged);
            existing.insert(field.field_number);
        }

        // Output validation r3 L1: the extension's keys attach to the table
        // it extends, named by the extension, after the table's own keys (an
        // extension adds secondary keys only, so the base's first key stays
        // the primary key). Their members resolve against the base table's
        // fields, the extension's merged in, and the system fields: a key on
        // a base field (`key(K; "No.", "CDO Handled")`) keeps both.
        let ext_keys = workspace.tables[ext_idx].keys.clone();
        let base = &mut workspace.tables[base_idx];
        let by_name: std::collections::HashMap<String, String> = base
            .fields
            .iter()
            .map(|f| (al_syntax::fold_identifier(&f.name), f.id.clone()))
            .collect();
        for (index, k) in ext_keys.iter().enumerate() {
            let fields = k
                .members
                .iter()
                .filter_map(|name| {
                    by_name.get(name).cloned().or_else(|| {
                        super::workspace::SYSTEM_FIELDS
                            .iter()
                            .find(|(lc, _, _)| lc == name)
                            .map(|(_, _, n)| encode_field_id(&base_table_id, *n))
                    })
                })
                .collect();
            // sol r4: a later `#if` alternative of the extension names its keys
            // apart (`.../{index}/alt{k}`), or two alternatives on one base
            // shared an id and the second's key was dropped. The first (or
            // only) alternative keeps the id it always had.
            let id = if alt == 0 {
                format!(
                    "{base_table_id}/key/ext/{}/{}/{index}",
                    object.app_guid, object.object_number
                )
            } else {
                format!(
                    "{base_table_id}/key/ext/{}/{}/{index}/alt{alt}",
                    object.app_guid, object.object_number
                )
            };
            // Idempotent, as the field merge is; an alternative repeating a
            // key another alternative already added (same members) adds none.
            if base.keys.iter().any(|b| {
                b.id == id
                    || (b.declared_by.as_deref() == Some(object.name.as_str())
                        && b.members == k.members)
            }) {
                continue;
            }
            base.keys.push(ModelKey {
                id,
                fields,
                members: k.members.clone(),
                declared_by: Some(object.name.clone()),
            });
        }
    }
}
