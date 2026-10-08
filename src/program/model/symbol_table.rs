//! Workspace symbol table — the object/table half of al-sem's
//! `src/resolve/symbol-table.ts` (`buildSymbolTable`). Its routine and interface
//! indexes served only the legacy L3 call resolver and were deleted with it in
//! engine-switch S9.6; record typing (`record_types`) is the one consumer left.
//!
//! A read-only lookup index over the assembled workspace model. All name
//! lookups are case-insensitive (AL identifiers are case-insensitive).
//!
//! COLLISION RESOLUTION IS ORDER-DEPENDENT (critical): every name/number index
//! uses `HashMap::insert` (LAST-wins), iterating in the assembled order — so the
//! LAST object/table with a colliding key wins, matching al-sem's `Map.set`.
//! Build this over a workspace assembled in the deterministic ingestion order
//! (POSIX-path-sorted files → per-file document order) or collisions resolve
//! differently.

use super::workspace::{L3Object, L3Table};
use std::collections::HashMap;

/// A read-only lookup index over a workspace model's objects and tables.
///
/// BORROWS the assembled workspace slices — it never owns a copy (deep-cloning
/// them cost ~1.7 GB per table on an 8020-file corpus). So the workspace must
/// outlive the table; `workspace::resolve` mutates only `routines` while one is
/// alive, which the disjoint borrow allows.
pub struct SymbolTable<'a> {
    /// `${objectType_lc}/${objectNumber}` → object index.
    by_type_number: HashMap<String, usize>,
    /// `${objectType_lc}/${name_lc}` → object index.
    by_type_name: HashMap<String, usize>,
    objects: &'a [L3Object],

    /// `${name_lc}` → table index.
    tables_by_name: HashMap<String, usize>,
    /// `${tableId}` → table index.
    tables_by_id: HashMap<String, usize>,
    tables: &'a [L3Table],
}

impl<'a> SymbolTable<'a> {
    /// Build the symbol table over an assembled workspace's objects and tables.
    /// The slices MUST be in the deterministic ingestion order (collision
    /// resolution depends on it).
    pub fn build(objects: &'a [L3Object], tables: &'a [L3Table]) -> SymbolTable<'a> {
        // --- object indexes (LAST-wins) -------------------------------------
        let mut by_type_number = HashMap::new();
        let mut by_type_name = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            by_type_number.insert(
                format!("{}/{}", o.object_type.to_lowercase(), o.object_number),
                i,
            );
            by_type_name.insert(
                format!("{}/{}", o.object_type.to_lowercase(), o.name.to_lowercase()),
                i,
            );
        }

        // --- table indexes (LAST-wins, REAL over stub) ----------------------
        // G-5: a `tableextension` stub's id reuses the EXTENSION's own object
        // number (`${appGuid}/table/${extNumber}`), which collides with a real
        // table sharing that number. A real table always wins the collision
        // (by id AND by name); within the same kind LAST-wins is preserved.
        let mut tables_by_name: HashMap<String, usize> = HashMap::new();
        let mut tables_by_id: HashMap<String, usize> = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            let name_key = t.name.to_lowercase();
            let keep_prev_name = tables_by_name
                .get(&name_key)
                .is_some_and(|&p| !tables[p].is_extension_stub && t.is_extension_stub);
            if !keep_prev_name {
                tables_by_name.insert(name_key, i);
            }
            let keep_prev_id = tables_by_id
                .get(&t.id)
                .is_some_and(|&p| !tables[p].is_extension_stub && t.is_extension_stub);
            if !keep_prev_id {
                tables_by_id.insert(t.id.clone(), i);
            }
        }

        SymbolTable {
            by_type_number,
            by_type_name,
            objects,
            tables_by_name,
            tables_by_id,
            tables,
        }
    }

    pub fn object_by_type_number(
        &self,
        object_type: &str,
        object_number: i64,
    ) -> Option<&L3Object> {
        let key = format!("{}/{}", object_type.to_lowercase(), object_number);
        self.by_type_number.get(&key).map(|&i| &self.objects[i])
    }

    pub fn object_by_type_name(&self, object_type: &str, name: &str) -> Option<&L3Object> {
        let key = format!("{}/{}", object_type.to_lowercase(), name.to_lowercase());
        self.by_type_name.get(&key).map(|&i| &self.objects[i])
    }

    pub fn table_by_name(&self, name: &str) -> Option<&L3Table> {
        self.tables_by_name
            .get(&name.to_lowercase())
            .map(|&i| &self.tables[i])
    }

    pub fn table_by_id(&self, id: &str) -> Option<&L3Table> {
        self.tables_by_id.get(id).map(|&i| &self.tables[i])
    }
}
