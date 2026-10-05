//! L3 workspace symbol table — Rust port of al-sem's
//! `src/resolve/symbol-table.ts` (`buildSymbolTable`).
//!
//! A read-only lookup index over the assembled L3 workspace model. All name
//! lookups are case-insensitive (AL identifiers are case-insensitive).
//!
//! COLLISION RESOLUTION IS ORDER-DEPENDENT (critical): every name/number index
//! uses `HashMap::insert` (LAST-wins), iterating in the assembled order — so the
//! LAST object/table with a colliding key wins, matching al-sem's `Map.set`.
//! Build this over a workspace assembled in al-sem's deterministic ingestion
//! order (POSIX-path-sorted files → per-file document order) or collisions
//! resolve differently.
//!
//! Routines are keyed `${objectId}::${name.toLowerCase()}` with overload lists
//! pre-sorted by routine id (byte-order). R2b's overload resolution relies on
//! this exact key + sort — locked here in R2a.

use super::workspace::{L3Object, L3PageControl, L3Routine, L3Table};
use al_syntax::IdentifierFoldExt;
use std::collections::HashMap;

/// Strip surrounding double-quotes from an interface name for case/quote-
/// insensitive matching (mirrors `normalizeInterfaceName`).
fn normalize_interface_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.len() > 1 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_lowercase()
    } else {
        trimmed.to_lowercase()
    }
}

/// A read-only lookup index over a workspace L3 model.
///
/// BORROWS the assembled workspace slices — it never owns a copy. Every public
/// accessor already hands back a reference (`&L3Object` / `&L3Table` /
/// `&L3Routine`), so ownership was never part of the contract; the index maps
/// hold `usize`/`String` keys only. Deep-cloning the three slices instead cost
/// **~1.7 GB per table** on an 8020-file corpus, retained for as long as the
/// table lived — and the `analyze` path builds three of them.
///
/// Consequence for callers: the workspace must outlive the table, and it cannot
/// be mutated while a table over it is alive. Both already held everywhere
/// except `l3_workspace::resolve`, which mutates `workspace.routines` — see
/// [`SymbolTable::build_without_routines`].
pub struct SymbolTable<'a> {
    /// `${objectType_lc}/${objectNumber}` → object index.
    by_type_number: HashMap<String, usize>,
    /// `${objectType_lc}/${name_lc}` → object index.
    by_type_name: HashMap<String, usize>,
    /// `${objectId}` → object index (exact-id lookup; LAST-wins like the others).
    by_id: HashMap<String, usize>,
    objects: &'a [L3Object],

    /// `${name_lc}` → table index.
    tables_by_name: HashMap<String, usize>,
    /// `${tableId}` → table index.
    tables_by_id: HashMap<String, usize>,
    tables: &'a [L3Table],

    /// `${objectId}::${name_lc}` → routine index (single, LAST-wins).
    routine_by_key: HashMap<String, usize>,
    /// `${objectId}::${name_lc}` → ALL overloads, sorted by id.
    routines_by_object_and_name: HashMap<String, Vec<usize>>,
    /// `${objectId}` → all routine indices in that object (document order).
    routines_by_object: HashMap<String, Vec<usize>>,
    routines: &'a [L3Routine],

    /// `${extends_target_lc}` → object ids of every `TableExtension` extending that
    /// base table. The key is the raw `extends` target lowercased — a NAME (native
    /// source) or a NUMBER string (dep symbols) — so a base table is looked up by
    /// BOTH its name and its number. Used so a non-builtin method on a Record
    /// resolves against procedures added by ALL extensions of its table (AL makes a
    /// `TableExtension` procedure globally callable on the base record).
    table_extensions_by_base: HashMap<String, Vec<String>>,

    /// Interface name (normalized) → codeunit implementer indices, sorted by id.
    codeunit_implementers: HashMap<String, Vec<usize>>,
    /// Interface name (normalized) → enum implementer indices, sorted by id.
    enum_implementers: HashMap<String, Vec<usize>>,

    impls_knowledge: ImplsKnowledge,

    /// `true` for a table built by [`build`](Self::build), `false` for one built
    /// by [`build_without_routines`](Self::build_without_routines). Every routine
    /// accessor `debug_assert!`s this — a table built without routines fails
    /// *open* (its routine indexes are simply empty, so a query silently returns
    /// `None`/`[]` rather than an error); this flag turns that into a loud panic
    /// in debug/test builds instead of a silent under-resolve. See
    /// `build_without_routines`'s doc for the one caller allowed to set it `false`.
    routines_indexed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImplsKnowledge {
    Complete,
    Partial,
}

impl<'a> SymbolTable<'a> {
    /// Build the symbol table over an assembled workspace. The slices MUST be in
    /// al-sem's deterministic ingestion order (collision resolution depends on it).
    pub fn build(
        objects: &'a [L3Object],
        tables: &'a [L3Table],
        routines: &'a [L3Routine],
    ) -> SymbolTable<'a> {
        Self::build_inner(objects, tables, routines, true)
    }

    fn build_inner(
        objects: &'a [L3Object],
        tables: &'a [L3Table],
        routines: &'a [L3Routine],
        routines_indexed: bool,
    ) -> SymbolTable<'a> {
        // --- object indexes (LAST-wins) -------------------------------------
        let mut by_type_number = HashMap::new();
        let mut by_type_name = HashMap::new();
        let mut by_id: HashMap<String, usize> = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            by_type_number.insert(
                format!("{}/{}", o.object_type.to_lowercase(), o.object_number),
                i,
            );
            by_type_name.insert(
                format!("{}/{}", o.object_type.to_lowercase(), o.name.to_lowercase()),
                i,
            );
            by_id.insert(o.id.clone(), i);
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

        // --- routine indexes ------------------------------------------------
        let mut routine_by_key = HashMap::new();
        let mut routines_by_object: HashMap<String, Vec<usize>> = HashMap::new();
        let mut routines_by_object_and_name: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, r) in routines.iter().enumerate() {
            let key = format!("{}::{}", r.object_id, r.name.to_lowercase());
            routine_by_key.insert(key.clone(), i); // LAST-wins
            routines_by_object
                .entry(r.object_id.clone())
                .or_default()
                .push(i);
            routines_by_object_and_name.entry(key).or_default().push(i);
        }
        // Sort all overload lists by routine id (byte-order).
        for list in routines_by_object_and_name.values_mut() {
            list.sort_by(|&a, &b| routines[a].id.cmp(&routines[b].id));
        }

        // --- table-extension-by-base index ----------------------------------
        // Every TableExtension keyed by its (lowercased) extends target — a NAME
        // (native source) or a NUMBER string (dep symbols). A base table is queried
        // by BOTH its name and its number, so either encoding resolves.
        let mut table_extensions_by_base: HashMap<String, Vec<String>> = HashMap::new();
        for o in objects {
            if o.object_type.eq_ignore_ascii_case("tableextension")
                && let Some(et) = &o.extends_target_name
            {
                table_extensions_by_base
                    .entry(et.to_lowercase())
                    .or_default()
                    .push(o.id.clone());
            }
        }
        for list in table_extensions_by_base.values_mut() {
            list.sort();
            list.dedup();
        }

        // --- interface implementer indexes ----------------------------------
        let mut codeunit_implementers: HashMap<String, Vec<usize>> = HashMap::new();
        let mut enum_implementers: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            let Some(ifaces) = &o.implements_interfaces else {
                continue; // undefined = unknown, skip
            };
            for iface in ifaces {
                let key = normalize_interface_name(iface);
                if o.object_type.to_lowercase() == "enum" {
                    enum_implementers.entry(key).or_default().push(i);
                } else {
                    codeunit_implementers.entry(key).or_default().push(i);
                }
            }
        }
        for list in codeunit_implementers.values_mut() {
            list.sort_by(|&a, &b| objects[a].id.cmp(&objects[b].id));
        }
        for list in enum_implementers.values_mut() {
            list.sort_by(|&a, &b| objects[a].id.cmp(&objects[b].id));
        }

        // --- per-app interface-knowledge detection --------------------------
        // appGuid → hasAnyDefined. "partial" iff at least one app is "unknown".
        let mut app_knowledge: HashMap<String, bool> = HashMap::new();
        for o in objects {
            let has = o.implements_interfaces.is_some();
            app_knowledge
                .entry(o.app_guid.clone())
                .and_modify(|cur| *cur = *cur || has)
                .or_insert(has);
        }
        let impls_knowledge = if app_knowledge.values().any(|&v| !v) {
            ImplsKnowledge::Partial
        } else {
            ImplsKnowledge::Complete
        };

        SymbolTable {
            by_type_number,
            by_type_name,
            by_id,
            objects,
            tables_by_name,
            tables_by_id,
            tables,
            routine_by_key,
            routines_by_object_and_name,
            routines_by_object,
            routines,
            table_extensions_by_base,
            codeunit_implementers,
            enum_implementers,
            impls_knowledge,
            routines_indexed,
        }
    }

    /// Build an OBJECT+TABLE-only index — the routine indexes are empty, so every
    /// routine accessor ([`routine_in_object`](Self::routine_in_object),
    /// [`routines_in_object`](Self::routines_in_object),
    /// [`routines_in_object_by_name`](Self::routines_in_object_by_name),
    /// [`trigger_in_object`](Self::trigger_in_object)) returns nothing.
    ///
    /// EXISTS FOR EXACTLY ONE CALLER: `l3_workspace::resolve`, which walks
    /// `&mut workspace.routines` while consulting the table. Since the table now
    /// borrows rather than clones, it cannot hold `&workspace.routines` across
    /// that mutation — and it does not need to: `resolve`'s only consumer is
    /// `record_types::resolve_routine_record_types`, whose entire use of the
    /// table is `object_by_type_number` / `object_by_type_name` / `table_by_name`
    /// / `table_by_id`. It never asks about a routine. The old cloning build
    /// deep-copied every routine in the workspace here (100,941 of them on an
    /// 8020-file corpus) to populate an index nothing read.
    ///
    /// Making that structural is the point of this constructor: a routine lookup
    /// against a `resolve`-time table is a bug, and naming the constructor says so.
    /// Do NOT use it anywhere else. `pub(crate)` rather than `pub` so that
    /// restriction is enforced for every consumer outside this lib crate
    /// (`aldump`, `alsem`, integration tests) — its sole in-crate caller is
    /// `l3_workspace::resolve`.
    pub(crate) fn build_without_routines(
        objects: &'a [L3Object],
        tables: &'a [L3Table],
    ) -> SymbolTable<'a> {
        Self::build_inner(objects, tables, &[], false)
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

    /// Look up an object by its exact internal id (`${appGuid}/${objectType}/${objectNumber}`).
    pub fn object_by_id(&self, id: &str) -> Option<&L3Object> {
        self.by_id.get(id).map(|&i| &self.objects[i])
    }

    /// Page controls visible to `CurrPage` inside `object_id` — the object's own controls,
    /// plus (for a PageExtension) the extended base page's controls. Returns `[]` for a
    /// non-page object or an unknown id.
    pub fn page_controls_for(&self, object_id: &str) -> Vec<&L3PageControl> {
        let Some(obj) = self.object_by_id(object_id) else {
            return Vec::new();
        };
        let mut out: Vec<&L3PageControl> = obj.page_controls.iter().collect();
        if obj.object_type.eq_ignore_ascii_case("pageextension")
            && let Some(base) = obj
                .extends_target_name
                .as_deref()
                .and_then(|n| self.object_by_type_name("Page", n))
        {
            out.extend(base.page_controls.iter());
        }
        out
    }

    pub fn table_by_name(&self, name: &str) -> Option<&L3Table> {
        self.tables_by_name
            .get(&name.to_lowercase())
            .map(|&i| &self.tables[i])
    }

    pub fn table_by_id(&self, id: &str) -> Option<&L3Table> {
        self.tables_by_id.get(id).map(|&i| &self.tables[i])
    }

    pub fn routine_in_object(&self, object_id: &str, routine_name: &str) -> Option<&L3Routine> {
        debug_assert!(
            self.routines_indexed,
            "routine_in_object called on a table built via SymbolTable::build_without_routines \
             (routine indexes are empty, so this would silently return None) — use \
             SymbolTable::build instead"
        );
        let key = format!("{}::{}", object_id, routine_name.to_lowercase());
        self.routine_by_key.get(&key).map(|&i| &self.routines[i])
    }

    /// Field-aware trigger lookup — the parity counterpart to
    /// `implicit_trigger_route_applicable` (`program/resolve/applicability.rs`).
    ///
    /// Unlike [`routine_in_object`] (name-only, LAST-wins on `${object}::${name}`),
    /// this enumerates ALL overloads with that name and selects by
    /// `enclosing_member`:
    /// - `enclosing_member_lc == None` → an OBJECT-LEVEL trigger only (the routine's
    ///   own `enclosing_member` must be `None`) — OnInsert / OnModify / OnDelete.
    /// - `enclosing_member_lc == Some(field_lc)` → the SPECIFIC field's own trigger
    ///   (a field's OnValidate); the routine's `enclosing_member` must fold-equal
    ///   `field_lc`. A table declares one `OnValidate` routine PER field, all keyed
    ///   on the same `${object}::onvalidate` string — so the name-only lookup
    ///   collapses them to one arbitrary survivor; this selects the right one.
    ///
    /// Case-insensitive on both the trigger name and the field name (the caller
    /// passes `enclosing_member_lc` already stripped/unescaped/folded to match how
    /// `L3Routine.enclosing_member` is stored — RE-3/RE-4 in `l3_workspace.rs`).
    pub fn trigger_in_object(
        &self,
        object_id: &str,
        trigger_name: &str,
        enclosing_member_lc: Option<&str>,
    ) -> Option<&L3Routine> {
        self.routines_in_object_by_name(object_id, trigger_name)
            .into_iter()
            .find(
                |r| match (enclosing_member_lc, r.enclosing_member.as_deref()) {
                    (None, None) => true,
                    (Some(want), Some(have)) => have.eq_fold_identifier(want),
                    _ => false,
                },
            )
    }

    pub fn routines_in_object(&self, object_id: &str) -> Vec<&L3Routine> {
        debug_assert!(
            self.routines_indexed,
            "routines_in_object called on a table built via SymbolTable::build_without_routines \
             (routine indexes are empty, so this would silently return []) — use \
             SymbolTable::build instead"
        );
        self.routines_by_object
            .get(object_id)
            .map(|v| v.iter().map(|&i| &self.routines[i]).collect())
            .unwrap_or_default()
    }

    /// ALL routines in the object with this name (overloads), sorted by id.
    pub fn routines_in_object_by_name(
        &self,
        object_id: &str,
        routine_name: &str,
    ) -> Vec<&L3Routine> {
        debug_assert!(
            self.routines_indexed,
            "routines_in_object_by_name called on a table built via \
             SymbolTable::build_without_routines (routine indexes are empty, so this would \
             silently return []) — use SymbolTable::build instead"
        );
        let key = format!("{}::{}", object_id, routine_name.to_lowercase());
        self.routines_by_object_and_name
            .get(&key)
            .map(|v| v.iter().map(|&i| &self.routines[i]).collect())
            .unwrap_or_default()
    }

    /// Object ids of every `TableExtension` extending the given base table, looked
    /// up by BOTH the table's name and its number (dep symbols encode extends
    /// targets as numbers, native source as names). Used to union a base table's
    /// own procedures with those added by its extensions when resolving a Record
    /// member call — a `TableExtension` procedure is globally callable on the base
    /// record in AL. Returns `[]` when the table has no extensions.
    pub fn table_extension_object_ids(&self, base_name: &str, base_number: i64) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        if let Some(v) = self.table_extensions_by_base.get(&base_name.to_lowercase()) {
            out.extend(v.iter().map(String::as_str));
        }
        if let Some(v) = self.table_extensions_by_base.get(&base_number.to_string()) {
            out.extend(v.iter().map(String::as_str));
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn objects_implementing(&self, interface_name: &str) -> Vec<&L3Object> {
        self.codeunit_implementers
            .get(&normalize_interface_name(interface_name))
            .map(|v| v.iter().map(|&i| &self.objects[i]).collect())
            .unwrap_or_default()
    }

    pub fn enum_implementers(&self, interface_name: &str) -> Vec<&L3Object> {
        self.enum_implementers
            .get(&normalize_interface_name(interface_name))
            .map(|v| v.iter().map(|&i| &self.objects[i]).collect())
            .unwrap_or_default()
    }

    pub fn interface_impls_knowledge(&self) -> ImplsKnowledge {
        self.impls_knowledge
    }
}
