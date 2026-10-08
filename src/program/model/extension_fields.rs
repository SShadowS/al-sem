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

use super::workspace::{ModelEntities, ModelField, ModelTable};
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
pub fn merge_extension_fields(workspace: &mut ModelEntities) {
    // Resolve table-name → index and table-id → index up front (LAST-wins, to
    // mirror the symbol table the TS pass queries). We must mutate tables in
    // place, so we resolve indices, then apply.
    //
    // `objects.clone()` is a genuine borrow-conflict clone, not a stylistic
    // choice: the loop below reads `objects` while mutating `workspace.tables`
    // (the `fields.push` a few lines down), so removing it needs a
    // `&mut *workspace` destructure into disjoint field borrows — a real
    // (if small, ~4 MB) edit, not a one-liner.
    let objects = workspace.objects.clone();
    for object in &objects {
        if object.object_type != "TableExtension" {
            continue;
        }
        let Some(extends_target) = &object.extends_target_name else {
            continue;
        };
        let Some(base_idx) = table_index_by_name(&workspace.tables, extends_target) else {
            continue;
        };
        let extension_table_id = encode_table_id(&object.app_guid, object.object_number);
        let Some(ext_idx) = table_index_by_id(&workspace.tables, &extension_table_id) else {
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
            };
            workspace.tables[base_idx].fields.push(merged);
            existing.insert(field.field_number);
        }
    }
}

/// `tableByName` semantics: case-insensitive, LAST-wins on collision.
fn table_index_by_name(tables: &[ModelTable], name: &str) -> Option<usize> {
    let want = name.to_lowercase();
    let mut found = None;
    for (i, t) in tables.iter().enumerate() {
        if t.name.to_lowercase() == want {
            found = Some(i); // LAST-wins
        }
    }
    found
}

/// `tableById` semantics: LAST-wins on collision.
fn table_index_by_id(tables: &[ModelTable], id: &str) -> Option<usize> {
    let mut found = None;
    for (i, t) in tables.iter().enumerate() {
        if t.id == id {
            found = Some(i); // LAST-wins
        }
    }
    found
}
