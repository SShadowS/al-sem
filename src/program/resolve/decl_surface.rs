//! `DeclSurface`: OWNED per-routine decl metadata, indexed by `RoutineNodeId`.
//!
//! Replaces the retired borrowed `BodyMap<'a>` (see the owned-decl-surface
//! design spec). Two tiers: `local` (workspace, rebuilt per rung) and
//! `frozen` (dependencies, built once at startup/rung-3 and `Arc`-forwarded
//! across rungs 1/2 — sound because `AppRef`s are stable across those rungs:
//! the `DepLayer`'s `AppRegistry` is cloned into every assembled graph).
//! Lookup is local-first, so a workspace entry always shadows a frozen one.
//!
//! `RoutineMeta` holds EXACTLY the fields resolution reads (audited): never
//! the routine body — dropping the dep parse arenas is the whole point.

use std::collections::HashMap;
use std::sync::Arc;

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{Origin, RoutineDecl};
use serde::{Deserialize, Serialize};

use crate::program::graph::ProgramGraph;
use crate::program::node::{AppRef, ObjKey, ObjectNodeId, RoutineNodeId};
use crate::program::node_extract::RoutineNode;
use crate::program::sig_fp::source_routine_node_id;
use crate::program::str_pool::{ShareStrings, SharedStr, StrPool};
use crate::snapshot::ParsedUnit;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamMeta {
    /// The declared parameter name (engine-switch S2b.5, G15a).
    pub name: SharedStr,
    pub ty: Option<SharedStr>,
    pub by_ref: bool,
}

impl ParamMeta {
    /// Declared `temporary`: the lowerer keeps the parameter's whole type node as
    /// `ty`, keyword included (`Record Customer temporary`). `None` without a type.
    pub fn is_temporary(&self) -> Option<bool> {
        self.ty.as_deref().map(|t| {
            t.split_whitespace()
                .any(|w| w.eq_ignore_ascii_case("temporary"))
        })
    }
}

/// Serde is here because `RoutineMeta` is PERSISTED in a dependency pack
/// (`crate::program::pack`) — it is not derivable on a pack hit, since
/// [`RoutineMeta::from_decl`] consumes a `RoutineDecl` and the `ParsedUnit`s
/// those come from are exactly what a hit avoids building.
///
/// The two `Origin`s route through `pack::origin_wire` rather than deriving:
/// `Origin::kind_text` is a `&'static str` (load-bearing for anchor
/// `syntax_kind` parity) and cannot `Deserialize`. On a BINARY format the wire
/// carries it as a `RawKind` discriminant, so decoding an `Origin` allocates
/// nothing; on a human-readable one it carries the grammar kind string, which
/// is stable across grammar revisions where the positional index is not — see
/// [`crate::program::pack::PackedOrigin`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineMeta {
    pub name: SharedStr,
    /// Name half of `RoutineDecl::enclosing_member` (origin half unused).
    pub enclosing_member: Option<SharedStr>,
    pub parse_incomplete: bool,
    pub params: Vec<ParamMeta>,
    #[serde(with = "crate::program::pack::origin_wire")]
    pub origin: Origin,
    #[serde(with = "crate::program::pack::origin_wire")]
    pub name_origin: Origin,
    pub virtual_path: SharedStr,
}

impl RoutineMeta {
    pub fn from_decl(decl: &RoutineDecl, virtual_path: impl Into<SharedStr>) -> Self {
        RoutineMeta {
            name: decl.name.as_str().into(),
            enclosing_member: decl
                .enclosing_member
                .as_ref()
                .map(|(n, _)| n.as_str().into()),
            parse_incomplete: decl.parse_incomplete,
            params: decl
                .params
                .iter()
                .map(|p| ParamMeta {
                    name: p.name.as_str().into(),
                    ty: p.ty.as_deref().map(SharedStr::from),
                    by_ref: p.by_ref,
                })
                .collect(),
            origin: decl.origin.clone(),
            name_origin: decl.name_origin.clone(),
            virtual_path: virtual_path.into(),
        }
    }
}

impl ShareStrings for RoutineMeta {
    fn share_strings(&mut self, pool: &mut StrPool) {
        pool.share(&mut self.name);
        pool.share_opt(&mut self.enclosing_member);
        for p in &mut self.params {
            pool.share(&mut p.name);
            pool.share_opt(&mut p.ty);
        }
        pool.share(&mut self.virtual_path);
    }
}

/// Every routine declared in `file`, keyed as `DeclSurface` keys it. The ONE
/// place that turns a file's declarations into `RoutineMeta`: `build` and the
/// dependency summaries both call it, so they cannot drift.
pub(crate) fn file_routine_meta(
    app: AppRef,
    file: &al_syntax::ir::AlFile,
    virtual_path: &str,
) -> Vec<(RoutineNodeId, RoutineMeta)> {
    let mut out = Vec::new();
    // One text for the file, shared by every routine in it.
    let virtual_path = SharedStr::from(virtual_path);
    for obj in &file.objects {
        let key = match obj.id {
            Some(n) => ObjKey::Id(n),
            None => ObjKey::Name(obj.name.fold_identifier().into()),
        };
        let obj_id = ObjectNodeId {
            app,
            kind: obj.kind,
            key,
        };
        for routine in &obj.routines {
            out.push((
                source_routine_node_id(obj_id.clone(), routine),
                RoutineMeta::from_decl(routine, virtual_path.clone()),
            ));
        }
    }
    out
}

/// The dependency tier's `RoutineMeta`, one per source routine, held as a column
/// beside the tier's own routine rows (engine-switch S10.3) rather than a map
/// keyed by a second copy of each id. Answers exactly what a
/// `HashMap<RoutineNodeId, RoutineMeta>` built from the same entries in the
/// same order would (a same-id collision keeps the LAST entry).
pub struct DepMeta {
    /// The tier's routine rows, sorted by id (shared with the tier, not copied).
    routines: Arc<Vec<RoutineNode>>,
    /// In id order, one per distinct id.
    metas: Vec<RoutineMeta>,
    /// Per meta, the first row carrying its id: `routines[key_row[i]].id` is
    /// meta `i`'s key.
    key_row: Vec<u32>,
    /// A meta whose id matches no row. Expected empty; kept so nothing is
    /// dropped.
    orphans: HashMap<RoutineNodeId, RoutineMeta>,
}

impl DepMeta {
    /// `entries` in parse order; `routines` sorted by id.
    pub fn build(
        routines: Arc<Vec<RoutineNode>>,
        entries: Vec<(RoutineNodeId, RoutineMeta)>,
    ) -> Self {
        debug_assert!(routines.is_sorted_by(|a, b| a.id <= b.id));
        let mut entries = entries;
        // Stable: within one id, parse order survives, so the last is the
        // last parsed (what `HashMap::extend` keeps).
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut metas = Vec::new();
        let mut key_row = Vec::new();
        let mut orphans = HashMap::new();
        let mut it = entries.into_iter().peekable();
        while let Some((id, meta)) = it.next() {
            if it.peek().is_some_and(|(next, _)| *next == id) {
                continue;
            }
            let row = routines.partition_point(|r| r.id < id);
            if routines.get(row).is_some_and(|r| r.id == id) {
                metas.push(meta);
                key_row.push(u32::try_from(row).expect("routine rows fit in u32"));
            } else {
                orphans.insert(id, meta);
            }
        }
        metas.shrink_to_fit();
        key_row.shrink_to_fit();
        DepMeta {
            routines,
            metas,
            key_row,
            orphans,
        }
    }

    /// No metadata (a tier without source routines, or a test).
    pub fn empty() -> Self {
        DepMeta::build(Arc::new(Vec::new()), Vec::new())
    }

    fn key(&self, i: usize) -> &RoutineNodeId {
        &self.routines[self.key_row[i] as usize].id
    }

    pub fn get_key_value(&self, id: &RoutineNodeId) -> Option<(&RoutineNodeId, &RoutineMeta)> {
        let at = partition_point_by(self.metas.len(), |i| self.key(i) < id);
        if at < self.metas.len() && self.key(at) == id {
            return Some((self.key(at), &self.metas[at]));
        }
        self.orphans.get_key_value(id)
    }

    pub fn get(&self, id: &RoutineNodeId) -> Option<&RoutineMeta> {
        self.get_key_value(id).map(|(_, m)| m)
    }

    pub fn contains_key(&self, id: &RoutineNodeId) -> bool {
        self.get_key_value(id).is_some()
    }

    /// Every entry: the column in id order, then the orphans (unordered).
    pub fn iter(&self) -> impl Iterator<Item = (&RoutineNodeId, &RoutineMeta)> {
        (0..self.metas.len())
            .map(|i| (self.key(i), &self.metas[i]))
            .chain(self.orphans.iter())
    }

    pub fn keys(&self) -> impl Iterator<Item = &RoutineNodeId> {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &RoutineMeta> {
        self.iter().map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.metas.len() + self.orphans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Metas whose id matches no tier row (expected 0).
    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// The entries as the map this replaces (tests compare the two).
    #[cfg(test)]
    pub(crate) fn to_map(&self) -> HashMap<RoutineNodeId, RoutineMeta> {
        self.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Same entries, whatever the layout.
impl PartialEq for DepMeta {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl std::fmt::Debug for DepMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// The first index in `0..len` for which `pred` is false (`pred` true then false).
fn partition_point_by(len: usize, pred: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0, len);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

pub struct DeclSurface {
    local: HashMap<RoutineNodeId, RoutineMeta>,
    frozen: Option<Arc<DepMeta>>,
}

impl DeclSurface {
    /// Build from a parsed snapshot. Mirrors the retired `BodyMap::build`
    /// EXACTLY: units whose `AppId` is absent from `graph.apps` are silently
    /// skipped (open-world gap); object key is numeric id when present, else
    /// lowercased name; last-write-wins on true same-key collision.
    pub fn build(graph: &ProgramGraph, parsed: &[ParsedUnit]) -> Self {
        let mut local = HashMap::new();
        for unit in parsed {
            let Some(app_ref) = graph.apps.find(&unit.app) else {
                continue;
            };
            for pf in &unit.files {
                for (r_id, meta) in file_routine_meta(app_ref, &pf.file, &pf.virtual_path) {
                    local.insert(r_id, meta);
                }
            }
        }
        DeclSurface {
            local,
            frozen: None,
        }
    }

    /// Attach the frozen dependency tier (the dependency layer's `dep_meta`).
    #[must_use]
    pub fn with_frozen(mut self, frozen: Arc<DepMeta>) -> Self {
        self.frozen = Some(frozen);
        self
    }

    pub fn get(&self, id: &RoutineNodeId) -> Option<&RoutineMeta> {
        self.local
            .get(id)
            .or_else(|| self.frozen.as_ref().and_then(|f| f.get(id)))
    }

    pub fn get_with_path(&self, id: &RoutineNodeId) -> Option<(&RoutineMeta, &str)> {
        self.get(id).map(|m| (m, m.virtual_path.as_str()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::graph::{ObjectIndex, ProgramGraph};
    use crate::program::node::{AppRef, AppRegistry, ObjKey, ObjectNodeId, RoutineNodeId};
    use crate::program::topology::DependencyGraph;
    use crate::snapshot::{AppId, ParsedFile, ParsedUnit, Provenance, TrustTier};
    use al_syntax::ir::ObjectKind;

    /// A frozen tier from a map, with no routine rows (so every entry sits in
    /// `orphans`; `DepMeta`'s own tests cover the column).
    fn frozen_of(map: HashMap<RoutineNodeId, RoutineMeta>) -> Arc<DepMeta> {
        Arc::new(DepMeta::build(
            Arc::new(Vec::new()),
            map.into_iter().collect(),
        ))
    }

    fn make_app_id(name: &str) -> AppId {
        AppId {
            guid: String::new(),
            name: name.into(),
            publisher: "Test".into(),
            version: "1.0.0.0".into(),
        }
    }

    /// Minimal single-app `ProgramGraph` (no objects/routines — DeclSurface
    /// only needs the `apps` registry to resolve `AppId` → `AppRef`).
    fn single_app_graph(app_id: &AppId) -> ProgramGraph {
        let mut apps = AppRegistry::default();
        apps.intern(app_id);
        ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: vec![].into(),
            routines: vec![].into(),
            obj_index: ObjectIndex::build(&[]),
            ..Default::default()
        }
    }

    /// Two-app `ProgramGraph` (primary + one dependency), for the two-tier
    /// freeze/compose tests.
    fn two_app_graph(primary: &AppId, dep: &AppId) -> ProgramGraph {
        let mut apps = AppRegistry::default();
        apps.intern(primary);
        apps.intern(dep);
        ProgramGraph {
            apps,
            topology: DependencyGraph::default(),
            objects: vec![].into(),
            routines: vec![].into(),
            obj_index: ObjectIndex::build(&[]),
            ..Default::default()
        }
    }

    fn make_unit(app_id: AppId, src: &'static str) -> ParsedUnit {
        let file = std::sync::Arc::new(al_syntax::parse(src));
        let provenance = Provenance {
            app: app_id.clone(),
            tier: TrustTier::Workspace,
            content_hash: String::new(),
        };
        ParsedUnit {
            app: app_id,
            files: vec![ParsedFile {
                virtual_path: "Test.al".into(),
                file,
                provenance,
                text: src.into(),
            }],
        }
    }

    /// S10.3: `DepMeta` answers exactly as the `HashMap` it replaces, built
    /// from the same entries in the same order: a same-id collision keeps the
    /// last entry, an id carried by two rows (an aliased overload) is found
    /// once, and an entry whose id matches no row is kept, not dropped.
    #[test]
    fn dep_meta_answers_as_the_map_it_replaces() {
        let app = AppRef(1);
        let entries_of =
            |src: &str, path: &str| file_routine_meta(app, &al_syntax::parse(src), path);
        let mut entries = entries_of(
            "codeunit 1 C { procedure P() begin end; procedure Q() begin end; }",
            "A.al",
        );
        entries.extend(entries_of(
            "codeunit 1 C { procedure P() begin end; }",
            "B.al",
        ));
        entries.extend(entries_of(
            "codeunit 2 D { procedure R() begin end; }",
            "C.al",
        ));
        let id = |name: &str| {
            entries
                .iter()
                .find(|(k, _)| k.name_lc == name)
                .unwrap()
                .0
                .clone()
        };
        let (p, q, r) = (id("p"), id("q"), id("r"));
        let row = |id: &RoutineNodeId| {
            let mut n = crate::program::node_extract::test_fixtures::fully_populated_routine_node();
            n.id = id.clone();
            n
        };
        // R has no row; P has two (an aliased overload pair).
        let mut rows = vec![row(&q), row(&p), row(&p)];
        rows.sort_by(|a, b| a.id.cmp(&b.id));

        let dep = DepMeta::build(Arc::new(rows), entries.clone());
        let mut map = HashMap::new();
        map.extend(entries);

        assert_eq!(dep.to_map(), map);
        assert_eq!(dep.len(), 3);
        assert_eq!(
            dep.get(&p).unwrap().virtual_path,
            "B.al",
            "the last entry wins"
        );
        assert_eq!(dep.get_key_value(&q).unwrap().0, &q);
        assert_eq!(dep.orphan_count(), 1);
        assert!(dep.contains_key(&r), "an unmatched entry is kept");
        assert_eq!(dep.keys().filter(|k| **k == p).count(), 1);
    }

    #[test]
    fn get_returns_correct_routine_meta() {
        let app_id = make_app_id("TestApp");
        let graph = single_app_graph(&app_id);

        let src = r#"
codeunit 50100 "My Codeunit"
{
    procedure DoSomething() begin end;
    procedure DoOther() begin end;
}
"#;
        let unit = make_unit(app_id, src);
        let units = [unit];
        let surface = DeclSurface::build(&graph, &units);

        let obj_id = ObjectNodeId {
            app: AppRef(0),
            kind: ObjectKind::Codeunit,
            key: ObjKey::Id(50100),
        };

        let r1 = RoutineNodeId {
            object: obj_id.clone(),
            name_lc: "dosomething".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let meta = surface.get(&r1).expect("DoSomething must be found");
        assert_eq!(meta.name, "DoSomething");

        let r2 = RoutineNodeId {
            object: obj_id.clone(),
            name_lc: "doother".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let meta2 = surface.get(&r2).expect("DoOther must be found");
        assert_eq!(meta2.name, "DoOther");

        // Absent routine must yield None.
        let absent = RoutineNodeId {
            object: obj_id,
            name_lc: "notexist".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        assert!(
            surface.get(&absent).is_none(),
            "absent routine must be None"
        );
    }

    #[test]
    fn build_is_infallible_for_extension_objects() {
        // Extension objects carry an explicit numeric id — key is `ObjKey::Id(n)`.
        let app_id = make_app_id("TestApp");
        let graph = single_app_graph(&app_id);

        let src = r#"
tableextension 50100 "Customer Ext" extends Customer
{
    procedure ExtraHelper() begin end;
}
"#;
        let unit = make_unit(app_id, src);
        let units = [unit];
        let surface = DeclSurface::build(&graph, &units);

        // The extension has id=50100, so the parser uses ObjKey::Id(50100).
        let obj_id = ObjectNodeId {
            app: AppRef(0),
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };
        let r_id = RoutineNodeId {
            object: obj_id,
            name_lc: "extrahelper".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        // Verify the routine is found and correctly indexed.
        assert!(
            surface.get(&r_id).is_some(),
            "ExtraHelper must be found in extension with id=50100"
        );
    }

    #[test]
    fn skips_unit_whose_app_is_not_in_graph() {
        let registered = make_app_id("TestApp");
        let graph = single_app_graph(&registered);

        let other = make_app_id("OtherApp");
        let src = r#"codeunit 50100 "C" { procedure F() begin end; }"#;
        let unit = make_unit(other, src);
        let units = [unit];
        let surface = DeclSurface::build(&graph, &units);

        // `OtherApp` is not in the graph; the local tier must be empty.
        assert!(
            surface.local.is_empty(),
            "unit from unknown app must be skipped"
        );
    }

    /// Two `OnValidate` field-triggers on DIFFERENT fields in the same
    /// tableextension must produce DISTINCT `RoutineNodeId`s (differing in
    /// `enclosing_member_lc`) so that `DeclSurface` stores both and `get` can
    /// retrieve each independently — no last-write collision.
    #[test]
    fn same_named_field_triggers_are_distinct() {
        let src = r#"
tableextension 50100 "Cust Ext" extends Customer
{
    fields
    {
        field(50100; Foo; Integer) { trigger OnValidate() begin Bar(); end; }
        field(50101; Baz; Integer) { trigger OnValidate() begin Qux(); end; }
    }
}
"#;
        let app_id = make_app_id("TestApp");
        let graph = single_app_graph(&app_id);
        let unit = make_unit(app_id, src);

        // Confirm the IR populates `enclosing_member` for both field triggers.
        // This is the KEY pre-condition for the discriminator to work.
        let parsed_file = &unit.files[0];
        let obj = &parsed_file.file.objects[0];
        let onvalidates: Vec<_> = obj
            .routines
            .iter()
            .filter(|r| r.name.eq_ignore_ascii_case("OnValidate"))
            .collect();
        assert_eq!(
            onvalidates.len(),
            2,
            "IR must expose two OnValidate routines; got {}",
            onvalidates.len()
        );
        let members: Vec<_> = onvalidates
            .iter()
            .map(|r| {
                r.enclosing_member
                    .as_ref()
                    .map(|(n, _)| n.to_ascii_lowercase())
            })
            .collect();
        assert!(
            members.contains(&Some("foo".to_string()))
                && members.contains(&Some("baz".to_string())),
            "IR enclosing_member must be 'Foo'/'Baz' for the two field triggers; got {members:?}"
        );

        // Build the DeclSurface and verify both triggers are stored with distinct ids.
        let units = [unit];
        let surface = DeclSurface::build(&graph, &units);

        let obj_id = ObjectNodeId {
            app: AppRef(0),
            kind: ObjectKind::TableExtension,
            key: ObjKey::Id(50100),
        };

        let foo_id = RoutineNodeId {
            object: obj_id.clone(),
            name_lc: "onvalidate".into(),
            enclosing_member_lc: Some("foo".into()),
            params_count: 0,
            sig_fp: 0,
        };
        let baz_id = RoutineNodeId {
            object: obj_id.clone(),
            name_lc: "onvalidate".into(),
            enclosing_member_lc: Some("baz".into()),
            params_count: 0,
            sig_fp: 0,
        };

        // The two RoutineNodeIds must be distinct (the discriminator must differ).
        assert_ne!(
            foo_id, baz_id,
            "two OnValidate triggers on different fields must have distinct RoutineNodeIds"
        );

        // Both must be retrievable from the DeclSurface.
        let foo_meta = surface
            .get(&foo_id)
            .expect("OnValidate for Foo must be in DeclSurface");
        let baz_meta = surface
            .get(&baz_id)
            .expect("OnValidate for Baz must be in DeclSurface");

        // Sanity: verify each meta references the right field.
        assert_eq!(
            foo_meta
                .enclosing_member
                .as_deref()
                .map(str::to_ascii_lowercase),
            Some("foo".to_string()),
            "foo_meta must reference the Foo field"
        );
        assert_eq!(
            baz_meta
                .enclosing_member
                .as_deref()
                .map(str::to_ascii_lowercase),
            Some("baz".to_string()),
            "baz_meta must reference the Baz field"
        );
    }

    #[test]
    fn with_frozen_composes_a_workspace_only_build_with_a_prior_dep_tier() {
        let primary_id = make_app_id("PrimaryApp");
        let dep_id = make_app_id("DepApp");
        let graph = two_app_graph(&primary_id, &dep_id);

        let ws_src = r#"codeunit 50100 "WS" { procedure WsProc() begin end; }"#;
        let dep_src = r#"codeunit 50200 "Dep" { procedure DepProc() begin end; }"#;

        let dep_unit = make_unit(dep_id, dep_src);
        let ws_unit = make_unit(primary_id, ws_src);

        let primary_ref = AppRef(0);
        let dep_ref = AppRef(1);

        // The dependency tier: the dependency unit's decls, frozen.
        let frozen = frozen_of(DeclSurface::build(&graph, &[dep_unit]).local);

        // Now simulate a rung: build from the WORKSPACE unit only, attach
        // the prior frozen dep tier.
        let ws_units = [ws_unit];
        let surface = DeclSurface::build(&graph, &ws_units).with_frozen(Arc::clone(&frozen));

        let ws_rid = RoutineNodeId {
            object: ObjectNodeId {
                app: primary_ref,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(50100),
            },
            name_lc: "wsproc".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };
        let dep_rid = RoutineNodeId {
            object: ObjectNodeId {
                app: dep_ref,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(50200),
            },
            name_lc: "depproc".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };

        assert!(surface.get(&ws_rid).is_some());
        assert!(surface.get(&dep_rid).is_some()); // served by frozen tier
    }

    #[test]
    fn local_tier_shadows_frozen_on_key_collision() {
        let primary_id = make_app_id("PrimaryApp");
        let graph = single_app_graph(&primary_id);

        let primary_ref = AppRef(0);
        let rid = RoutineNodeId {
            object: ObjectNodeId {
                app: primary_ref,
                kind: ObjectKind::Codeunit,
                key: ObjKey::Id(50100),
            },
            name_lc: "proc".into(),
            enclosing_member_lc: None,
            params_count: 0,
            sig_fp: 0,
        };

        // A frozen tier that (artificially) contains an entry under the SAME
        // key as a local-tier entry we're about to build.
        let stale_src = r#"codeunit 50100 "C" { procedure Proc() begin end; }"#;
        let stale_unit = make_unit(primary_id.clone(), stale_src);
        let stale_units = [stale_unit];
        // Put the stale primary entry into a frozen map.
        let frozen = frozen_of(DeclSurface::build(&graph, &stale_units).local);
        assert!(
            frozen.get(&rid).is_some(),
            "fixture sanity: stale entry present"
        );

        let fresh_src = r#"codeunit 50100 "C" { procedure Proc() begin /* fresh */ end; }"#;
        let fresh_unit = make_unit(primary_id, fresh_src);
        let fresh_units = [fresh_unit];
        let surface = DeclSurface::build(&graph, &fresh_units).with_frozen(frozen);

        let meta = surface.get(&rid).expect("must be found via local tier");
        // The local build must win: get() checks local before frozen, so
        // this resolves to the freshly-built local entry even though the
        // frozen tier holds a stale entry under the same key.
        assert_eq!(meta.name, "Proc");
    }
}
