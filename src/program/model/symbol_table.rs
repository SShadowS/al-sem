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

use super::workspace::{ModelObject, ModelTable};
use al_syntax::IdentifierFoldExt;
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
    objects: &'a [ModelObject],

    /// `${name_lc}` → table index.
    tables_by_name: HashMap<String, usize>,
    /// ENG-194: `${name_lc}` → every table of the name, in order
    /// ([`Self::table_by_name_from`]).
    tables_named: HashMap<String, Vec<usize>>,
    /// `${tableId}` → table index.
    tables_by_id: HashMap<String, usize>,
    tables: &'a [ModelTable],
    /// ENG-194: the program's view of who sees which table
    /// ([`Self::table_by_name_from`]); `None`: unscoped.
    scope: Option<&'a TableScope>,
}

impl<'a> SymbolTable<'a> {
    /// [`Self::build`] whose name lookups from an app follow `scope` (ENG-194).
    pub fn build_scoped(
        objects: &'a [ModelObject],
        tables: &'a [ModelTable],
        scope: &'a TableScope,
    ) -> SymbolTable<'a> {
        SymbolTable {
            scope: Some(scope),
            ..SymbolTable::build(objects, tables)
        }
    }

    /// Build the symbol table over an assembled workspace's objects and tables.
    /// The slices MUST be in the deterministic ingestion order (collision
    /// resolution depends on it).
    pub fn build(objects: &'a [ModelObject], tables: &'a [ModelTable]) -> SymbolTable<'a> {
        // --- object indexes (LAST-wins) -------------------------------------
        let mut by_type_number = HashMap::new();
        let mut by_type_name = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            // An object without a number (`object_number` 0) is not indexed by
            // number: every such object of a kind would share `type/0` (#83).
            if o.object_number != 0 {
                by_type_number.insert(
                    format!("{}/{}", o.object_type.fold_identifier(), o.object_number),
                    i,
                );
            }
            by_type_name.insert(
                format!(
                    "{}/{}",
                    o.object_type.fold_identifier(),
                    o.name.fold_identifier()
                ),
                i,
            );
        }

        // --- table indexes (LAST-wins, REAL over stub) ----------------------
        // G-5: a `tableextension` stub's id reuses the EXTENSION's own object
        // number (`${appGuid}/table/${extNumber}`), which collides with a real
        // table sharing that number. A real table always wins the collision
        // (by id AND by name); within the same kind LAST-wins is preserved.
        let mut tables_by_name: HashMap<String, usize> = HashMap::new();
        let mut tables_named: HashMap<String, Vec<usize>> = HashMap::new();
        let mut tables_by_id: HashMap<String, usize> = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            let name_key = t.name.fold_identifier();
            tables_named.entry(name_key.clone()).or_default().push(i);
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
            tables_named,
            tables_by_id,
            tables,
            scope: None,
        }
    }

    /// ENG-194: the table `name` names in code of app `from` (an app guid), as
    /// the program's resolver binds it (`ProgramGraph::resolve_object`): only
    /// tables of `from`'s dependency closure count (`from` itself included and
    /// not preferred), a real table beats a tableextension stub, and a table
    /// declared `ObsoleteState = Moved`/`Removed` counts only when no live one
    /// remains. Exactly one table left binds; more than one is a collision this
    /// DECLINES (`None`: the table is not known) rather than taking the last
    /// (Fable on ENG-158: two apps' tables of one name gave every record
    /// variable the last one's id). With no scope for `from` (an unscoped
    /// table, or an app the program does not hold), every table counts.
    pub fn table_by_name_from(&self, from: &str, name: &str) -> Option<&ModelTable> {
        static NONE: std::sync::LazyLock<TableScope> =
            std::sync::LazyLock::new(TableScope::default);
        let scope = self.scope.unwrap_or(&NONE);
        let all = self.tables_named.get(&name.fold_identifier())?;
        let visible = scope.visible.get(from);
        let mut found: Vec<&ModelTable> = all
            .iter()
            .map(|&i| &self.tables[i])
            .filter(|t| visible.is_none_or(|v| v.contains(&t.app_guid)))
            .collect();
        if found.iter().any(|t| !t.is_extension_stub) {
            found.retain(|t| !t.is_extension_stub);
        }
        if found.iter().any(|t| !scope.gone.contains(&t.id)) {
            found.retain(|t| !scope.gone.contains(&t.id));
        }
        // One table can have several rows (an `#if`-split declaration), not
        // always adjacent: the first row of each id stands for it.
        let mut seen = std::collections::HashSet::new();
        found.retain(|t| seen.insert(t.id.as_str()));
        match found.as_slice() {
            [t] => Some(t),
            _ => None,
        }
    }

    pub fn object_by_type_number(
        &self,
        object_type: &str,
        object_number: i64,
    ) -> Option<&ModelObject> {
        let key = format!("{}/{}", object_type.fold_identifier(), object_number);
        self.by_type_number.get(&key).map(|&i| &self.objects[i])
    }

    pub fn object_by_type_name(&self, object_type: &str, name: &str) -> Option<&ModelObject> {
        let key = format!(
            "{}/{}",
            object_type.fold_identifier(),
            name.fold_identifier()
        );
        self.by_type_name.get(&key).map(|&i| &self.objects[i])
    }

    pub fn table_by_name(&self, name: &str) -> Option<&ModelTable> {
        self.tables_by_name
            .get(&name.fold_identifier())
            .map(|&i| &self.tables[i])
    }

    pub fn table_by_id(&self, id: &str) -> Option<&ModelTable> {
        self.tables_by_id.get(id).map(|&i| &self.tables[i])
    }
}

/// ENG-194: what a table name may bind to, from the program graph: each app's
/// dependency closure (its own guid included) and the model ids of the tables
/// declared `ObsoleteState = Moved`/`Removed` ([`SymbolTable::table_by_name_from`]).
#[derive(Debug, Clone, Default)]
pub struct TableScope {
    /// app guid → the app guids it can see (itself included).
    visible: HashMap<String, std::collections::HashSet<String>>,
    /// Model ids (`{guid}/table/{n}`) of the moved or removed tables.
    gone: std::collections::HashSet<String>,
}

impl TableScope {
    /// The scope of the program `graph` describes.
    #[must_use]
    pub fn of(graph: &crate::program::graph::ProgramGraph) -> TableScope {
        use crate::program::node::AppRef;
        let mut visible: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        let mut i = 0u32;
        while let Some(app) = graph.apps.try_resolve(AppRef(i)) {
            let seen = graph
                .topology
                .closure(AppRef(i))
                .into_iter()
                .chain(std::iter::once(AppRef(i)))
                .filter_map(|a| graph.apps.try_resolve(a).map(|x| x.guid.clone()))
                .collect();
            visible.insert(app.guid.clone(), seen);
            i += 1;
        }
        let gone = graph
            .objects
            .iter()
            .filter(|o| o.id.kind == al_syntax::ir::ObjectKind::Table && o.obsolete_gone)
            .filter_map(|o| {
                let n = o.declared_id?;
                Some(format!("{}/table/{n}", graph.apps.resolve(o.id.app).guid))
            })
            .collect();
        TableScope { visible, gone }
    }

    /// No scope: every table of a name counts, none is gone (a model built
    /// without a program graph, as tests and single objects are).
    #[must_use]
    pub fn unscoped() -> TableScope {
        TableScope::default()
    }

    /// A scope stated by hand (tests): `visible` as (app, apps it sees),
    /// `gone` as table model ids.
    #[cfg(test)]
    pub(crate) fn stated(visible: &[(&str, &[&str])], gone: &[&str]) -> TableScope {
        TableScope {
            visible: visible
                .iter()
                .map(|(a, vs)| {
                    (
                        (*a).to_string(),
                        vs.iter().map(|s| (*s).to_string()).collect(),
                    )
                })
                .collect(),
            gone: gone.iter().map(|s| (*s).to_string()).collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A field-less table `name` of `app`, number `n`.
    fn tab(app: &str, n: i64, name: &str, stub: bool) -> ModelTable {
        ModelTable {
            id: format!("{app}/table/{n}"),
            app_guid: app.to_string(),
            table_number: n,
            name: name.to_string(),
            fields: Vec::new(),
            keys: Vec::new(),
            is_temporary: false,
            is_extension_stub: stub,
        }
    }

    /// ENG-194: a table name binds as the program's resolver binds it, seen
    /// from the referencing app. Hand-stated: apps `base` (a `Moved` stub of
    /// `No. Series`, and `Shared`), `bf` (the live `No. Series`), `other` (its
    /// own `Shared`), the workspace `ws` seeing all three and `lone` seeing
    /// `base` only.
    /// DISCRIMINATION (2026-11-26, each break then restored, green): no
    /// visibility filter fails "lone sees only base's"; no `gone` filter fails
    /// "the Moved stub gives way"; taking the last instead of declining fails
    /// "two live tables decline".
    #[test]
    fn a_table_name_binds_as_the_resolver_binds_it() {
        let tables = vec![
            tab("base", 308, "No. Series", false),
            tab("bf", 308, "No. Series", false),
            tab("base", 50, "Shared", false),
            tab("other", 60, "Shared", false),
            tab("ext", 70, "Shared", true),
        ];
        let scope = TableScope::stated(
            &[
                ("ws", &["ws", "base", "bf", "other", "ext"]),
                ("lone", &["lone", "base"]),
            ],
            &["base/table/308"],
        );
        let st = SymbolTable::build_scoped(&[], &tables, &scope);
        let id = |from: &str, name: &str| st.table_by_name_from(from, name).map(|t| t.id.clone());
        assert_eq!(
            id("ws", "no. series").as_deref(),
            Some("bf/table/308"),
            "the Moved stub gives way"
        );
        assert_eq!(id("ws", "Shared"), None, "two live tables decline");
        assert_eq!(
            id("lone", "Shared").as_deref(),
            Some("base/table/50"),
            "lone sees only base's"
        );
        // Precondition of the old bug: the last table of the name wins.
        assert_eq!(
            st.table_by_name("Shared").map(|t| t.id.as_str()),
            Some("other/table/60")
        );
    }

    /// A numberless Interface `ModelObject` named `name` (#83 tests).
    pub(crate) fn numberless(name: &str) -> ModelObject {
        ModelObject {
            id: format!("app/Interface/@{name}"),
            app_guid: "app".to_string(),
            object_type: "Interface".to_string(),
            object_number: 0,
            object_key: format!("@{name}"),
            name: name.to_string(),
            source_table_name: None,
            extends_target_name: None,
            implements_interfaces: Some(Vec::new()),
            interface_bases: Vec::new(),
            interface_members: Vec::new(),
            object_subtype: None,
            page_type: None,
            inherent_commit_behavior: None,
            inherent_commit_behavior_conditional: false,
            source_table_temporary: None,
            page_controls: Vec::new(),
            single_instance: None,
            editable: None,
            insert_allowed: None,
            modify_allowed: None,
            delete_allowed: None,
            source_anchor: None,
            declaration_only_routines: Vec::new(),
            run_actions: None,
            enum_impls: None,
        }
    }

    /// #83: two numberless objects of one kind must not answer a lookup by
    /// number 0 (it used to return whichever came last).
    #[test]
    fn numberless_objects_are_not_found_by_number_zero() {
        let objects = [numberless("ifoo"), numberless("ibar")];
        let symbols = SymbolTable::build(&objects, &[]);
        assert!(symbols.object_by_type_number("Interface", 0).is_none());
        assert_eq!(
            symbols
                .object_by_type_name("Interface", "ifoo")
                .map(|o| o.id.as_str()),
            Some("app/Interface/@ifoo")
        );
    }
}
