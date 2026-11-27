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
    /// ENG-194: `${objectType_lc}/${name_lc}` → every object of the key, in
    /// order ([`Self::object_by_type_name_from`]).
    objects_named: HashMap<String, Vec<usize>>,
    objects: &'a [ModelObject],

    /// `${name_lc}` → table index.
    tables_by_name: HashMap<String, usize>,
    /// ENG-194: `${name_lc}` → every table of the name, in order
    /// ([`Self::table_by_name_from`]).
    tables_named: HashMap<String, Vec<usize>>,
    /// ENG-194: table number → every REAL table of the number, in order
    /// ([`Self::table_by_number_from`]; a stub's number is its extension's).
    tables_numbered: HashMap<i64, Vec<usize>>,
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
        let mut objects_named: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, o) in objects.iter().enumerate() {
            // An object without a number (`object_number` 0) is not indexed by
            // number: every such object of a kind would share `type/0` (#83).
            if o.object_number != 0 {
                by_type_number.insert(
                    format!("{}/{}", o.object_type.fold_identifier(), o.object_number),
                    i,
                );
            }
            let name_key = format!(
                "{}/{}",
                o.object_type.fold_identifier(),
                o.name.fold_identifier()
            );
            objects_named.entry(name_key.clone()).or_default().push(i);
            by_type_name.insert(name_key, i);
        }

        // --- table indexes (LAST-wins, REAL over stub) ----------------------
        // G-5: a `tableextension` stub's id reuses the EXTENSION's own object
        // number (`${appGuid}/table/${extNumber}`), which collides with a real
        // table sharing that number. A real table always wins the collision
        // (by id AND by name); within the same kind LAST-wins is preserved.
        let mut tables_by_name: HashMap<String, usize> = HashMap::new();
        let mut tables_named: HashMap<String, Vec<usize>> = HashMap::new();
        let mut tables_numbered: HashMap<i64, Vec<usize>> = HashMap::new();
        let mut tables_by_id: HashMap<String, usize> = HashMap::new();
        for (i, t) in tables.iter().enumerate() {
            let name_key = t.name.fold_identifier();
            tables_named.entry(name_key.clone()).or_default().push(i);
            if !t.is_extension_stub {
                tables_numbered.entry(t.table_number).or_default().push(i);
            }
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
            objects_named,
            objects,
            tables_by_name,
            tables_named,
            tables_numbered,
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
    /// variable the last one's id). Unscoped (a table built with no program),
    /// every table counts; scoped, an app the program does not hold sees only
    /// its own tables (fail closed, as the resolver: an app outside the closure
    /// never matches; sol r1 on ENG-194).
    pub fn table_by_name_from(&self, from: &str, name: &str) -> Option<&ModelTable> {
        let all = self.tables_named.get(&name.fold_identifier())?;
        // Fable r1 S5: one table of the name (nearly every lookup) needs no
        // filter but visibility, and no allocation.
        if let [i] = all.as_slice() {
            let t = &self.tables[*i];
            return self.sees(from, &t.app_guid).then_some(t);
        }
        let found = all
            .iter()
            .map(|&i| &self.tables[i])
            .filter(|t| self.sees(from, &t.app_guid))
            .collect();
        self.one_live_table(found, None)
    }

    /// ENG-194 (sol r1): the table NUMBER `n` names in code of app `from`
    /// (dependency symbols give a page's `SourceTable` as a number), as the
    /// resolver binds a number (`ProgramGraph::resolve_object_by_number`):
    /// visible tables of that number (a tableextension stub is no table of it:
    /// its id reuses the extension's own number), a moved/removed one only
    /// when no live one remains, `from`'s own table first, else exactly one.
    pub fn table_by_number_from(&self, from: &str, n: i64) -> Option<&ModelTable> {
        let found = self
            .tables_numbered
            .get(&n)?
            .iter()
            .map(|&i| &self.tables[i])
            .filter(|t| self.sees(from, &t.app_guid))
            .collect();
        self.one_live_table(found, Some(from))
    }

    /// ENG-194: every row of an object of `object_type` named `name` app
    /// `from` may mean, as the resolver's `visible_objects`: visible ones only
    /// (sol r1), a moved/removed one only when no live one remains (sol r2),
    /// each `#if` alternative a row of its own.
    pub fn objects_by_type_name_from(
        &self,
        from: &str,
        object_type: &str,
        name: &str,
    ) -> Vec<&ModelObject> {
        let key = format!(
            "{}/{}",
            object_type.fold_identifier(),
            name.fold_identifier()
        );
        let mut found: Vec<&ModelObject> = self
            .objects_named
            .get(&key)
            .into_iter()
            .flatten()
            .map(|&i| &self.objects[i])
            .filter(|o| self.sees(from, &o.app_guid))
            .collect();
        let gone = |o: &ModelObject| self.scope.is_some_and(|s| s.gone.contains(&o.id));
        if found.iter().any(|o| !gone(o)) {
            found.retain(|o| !gone(o));
        }
        found
    }

    /// ENG-194: a page extension's base page as its `Rec`'s table reads it:
    /// [`Self::objects_by_type_name_from`]'s rows, those of one id that agree
    /// on `SourceTable` counting as one (an `#if`-split page whose
    /// alternatives name one table has that table in every build); more than
    /// one left (two pages, or alternatives naming different tables) declines
    /// (sol r3, Fable r4). Its `SourceTableTemporary` is
    /// [`Self::page_temporary_from`]'s, not this row's.
    pub fn object_by_type_name_from(
        &self,
        from: &str,
        object_type: &str,
        name: &str,
    ) -> Option<&ModelObject> {
        let mut found = self.objects_by_type_name_from(from, object_type, name);
        let mut seen = std::collections::HashSet::new();
        found.retain(|o| {
            seen.insert((
                o.id.as_str(),
                o.source_table_name.as_deref().map(|s| s.fold_identifier()),
            ))
        });
        match found.as_slice() {
            [o] => Some(o),
            _ => None,
        }
    }

    /// ENG-194: the base page's `SourceTableTemporary` for a page extension
    /// of app `from`: known only when one page id is meant and every `#if`
    /// alternative of it agrees (absent and `false` both mean not temporary:
    /// sol r4); else `None` (not known, so never forced temporary).
    pub fn page_temporary_from(&self, from: &str, name: &str) -> Option<bool> {
        let found = self.objects_by_type_name_from(from, "Page", name);
        let first = found.first()?;
        let temp = |o: &&ModelObject| o.source_table_temporary.unwrap_or(false);
        found
            .iter()
            .all(|o| o.id == first.id && temp(o) == temp(first))
            .then(|| temp(first))
    }

    /// Whether code of app `from` sees what app `owner` declares: unscoped,
    /// always; scoped, when `owner` is in `from`'s closure, or is `from` itself
    /// for an app the program does not hold.
    fn sees(&self, from: &str, owner: &str) -> bool {
        match self.scope {
            None => true,
            Some(s) => match s.visible.get(from) {
                Some(v) => v.contains(owner),
                None => from == owner,
            },
        }
    }

    /// The resolver's pick among `found` (visible tables of one name or
    /// number): a real table over a tableextension stub, a live one over a
    /// moved/removed one, then (`own`: a number) `from`'s own, else exactly
    /// one table id; more decline.
    fn one_live_table<'t>(
        &self,
        mut found: Vec<&'t ModelTable>,
        own: Option<&str>,
    ) -> Option<&'t ModelTable> {
        let gone = |t: &ModelTable| self.scope.is_some_and(|s| s.gone.contains(&t.id));
        if found.iter().any(|t| !t.is_extension_stub) {
            found.retain(|t| !t.is_extension_stub);
        }
        if found.iter().any(|t| !gone(t)) {
            found.retain(|t| !gone(t));
        }
        // One table can have several rows (an `#if`-split declaration), not
        // always adjacent: the first row of each id stands for it.
        let mut seen = std::collections::HashSet::new();
        found.retain(|t| seen.insert(t.id.as_str()));
        if let Some(from) = own
            && let Some(t) = found.iter().find(|t| t.app_guid == from)
        {
            return Some(t);
        }
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
    /// Model ids of the moved or removed objects: each one's model object id,
    /// and a table's model table id (`{guid}/table/{n}`) too.
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
            // `closure` holds the app itself. Two units of one guid (only an
            // empty guid repeats) see the union of their closures.
            let seen = graph
                .topology
                .closure(AppRef(i))
                .into_iter()
                .filter_map(|a| graph.apps.try_resolve(a).map(|x| x.guid.clone()));
            visible.entry(app.guid.clone()).or_default().extend(seen);
            i += 1;
        }
        let mut gone = std::collections::HashSet::new();
        for o in graph.objects.iter().filter(|o| o.obsolete_gone) {
            // Its model object id (sol r2: a moved page gives way too), and for
            // a table its model table id.
            gone.insert(crate::program::model::model_object_id(graph, &o.id));
            if o.id.kind == al_syntax::ir::ObjectKind::Table
                && let Some(n) = o.declared_id
            {
                gone.insert(format!("{}/table/{n}", graph.apps.resolve(o.id.app).guid));
            }
        }
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
    /// "two live tables decline"; an app the scope does not hold seeing every
    /// table fails "an unknown app is not every app".
    #[test]
    fn a_table_name_binds_as_the_resolver_binds_it() {
        let tables = vec![
            tab("base", 308, "No. Series", false),
            tab("bf", 308, "No. Series", false),
            tab("base", 50, "Shared", false),
            tab("other", 60, "Shared", false),
            tab("ext", 70, "Shared", true),
            tab("other", 61, "Only Other", false),
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
        // sol r1: an app the scope does not hold sees only its own tables
        // (`other`'s lone `Only Other` would bind, failing open).
        assert_eq!(
            id("ws", "Only Other").as_deref(),
            Some("other/table/61"),
            "precondition: the name binds where seen"
        );
        assert_eq!(
            id("ghost", "Only Other"),
            None,
            "an unknown app is not every app"
        );
        // sol r1: a number binds by number: the Moved stub gives way, and two
        // live tables of one number decline unless one is the caller's own.
        let num = |from: &str, n: i64| st.table_by_number_from(from, n).map(|t| t.id.clone());
        assert_eq!(
            num("ws", 308).as_deref(),
            Some("bf/table/308"),
            "by number, live"
        );
        assert_eq!(
            num("lone", 308).as_deref(),
            Some("base/table/308"),
            "lone sees only the Moved stub: with no live one, it binds"
        );
    }

    /// ENG-194 (Fable r1 B1): a real table beats a tableextension stub of its
    /// name, and an `#if`-split table (two rows, one id, not adjacent) is one
    /// table, not a collision.
    /// DISCRIMINATION (2026-11-26, each break then restored, green): no stub
    /// filter fails "real over stub"; no dedup by id fails "split is one".
    #[test]
    fn a_stub_gives_way_and_a_split_table_is_one() {
        let tables = vec![
            tab("a", 50, "Split", false),
            tab("e", 70, "Base", true),
            tab("a", 51, "Base", false),
            tab("a", 50, "Split", false),
        ];
        let scope = TableScope::stated(&[("w", &["w", "a", "e"])], &[]);
        let st = SymbolTable::build_scoped(&[], &tables, &scope);
        let id = |name: &str| st.table_by_name_from("w", name).map(|t| t.id.clone());
        assert_eq!(id("Base").as_deref(), Some("a/table/51"), "real over stub");
        assert_eq!(id("Split").as_deref(), Some("a/table/50"), "split is one");
    }

    /// ENG-194 (sol r1): a number names a table as the resolver binds a
    /// number: `from`'s own table first, else exactly one visible live table.
    /// DISCRIMINATION (2026-11-26, break then restored, green): no own-app
    /// preference fails "own first".
    #[test]
    fn a_table_number_binds_own_first_then_exactly_one() {
        let tables = vec![tab("a", 5992, "X", false), tab("b", 5992, "Y", false)];
        let scope = TableScope::stated(&[("a", &["a", "b"]), ("w", &["w", "a", "b"])], &[]);
        let st = SymbolTable::build_scoped(&[], &tables, &scope);
        assert_eq!(
            st.table_by_number_from("a", 5992).map(|t| t.id.as_str()),
            Some("a/table/5992"),
            "own first"
        );
        assert_eq!(
            st.table_by_number_from("w", 5992).map(|t| t.id.as_str()),
            None,
            "two visible, neither its own: decline"
        );
    }

    /// ENG-194 (sol r1): a page extension's base page, seen from its app.
    /// DISCRIMINATION (2026-11-26, each break then restored, green): no
    /// closure filter fails the `lone` assertion; no gone filter fails "a
    /// moved object gives way".
    #[test]
    fn an_object_name_binds_within_the_apps_closure() {
        let mut a = numberless("Base");
        a.app_guid = "a".into();
        a.id = "a/Interface/@Base".into();
        let mut b = numberless("Base");
        b.app_guid = "b".into();
        b.id = "b/Interface/@Base".into();
        let objects = vec![a, b];
        let scope = TableScope::stated(&[("lone", &["lone", "a"]), ("w", &["w", "a", "b"])], &[]);
        let st = SymbolTable::build_scoped(&objects, &[], &scope);
        let id = |from: &str| {
            st.object_by_type_name_from(from, "Interface", "base")
                .map(|o| o.id.clone())
        };
        assert_eq!(id("lone").as_deref(), Some("a/Interface/@Base"));
        assert_eq!(id("w"), None, "two visible objects decline");
        // sol r2: a moved/removed object gives way to the live one.
        let gone = TableScope::stated(&[("w", &["w", "a", "b"])], &["a/Interface/@Base"]);
        let st = SymbolTable::build_scoped(&objects, &[], &gone);
        assert_eq!(
            st.object_by_type_name_from("w", "Interface", "base")
                .map(|o| o.id.as_str()),
            Some("b/Interface/@Base"),
            "a moved object gives way"
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
