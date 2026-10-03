//! Process-level sharing of the dependency tier across workspace roots.
//!
//! The LSP server builds one snapshot per workspace root. Roots that load the
//! same dependency apps used to each hold their own copy of the same
//! dependency nodes; [`DepCache`] lets them hold one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use crate::app_package::ParsedAppPackage;
use crate::dependencies::AppFileStamp;
use crate::program::graph::AbiIngestError;
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::snapshot::{AppId, AppSetSnapshot};

/// Process-level dependency tier, shared by every workspace root that loads
/// the SAME dependency set. Entries are held weakly: one lives exactly as
/// long as some root's snapshot still uses it.
#[derive(Default)]
pub struct DepCache {
    nodes: Mutex<HashMap<DepKey, Weak<DepNodes>>>,
    packages: Mutex<HashMap<(PathBuf, AppFileStamp), Weak<ParsedAppPackage>>>,
}

/// The shareable part of a `DepLayer` — independent of which root built it
/// (its AppRefs are 1..n in `snap.apps` order; the root's own app is always
/// AppRef(0) and never appears here).
pub struct DepNodes {
    pub objects: Arc<Vec<ObjectNode>>,
    pub routines: Arc<Vec<RoutineNode>>,
    pub abi_ingest_errors: Vec<AbiIngestError>,
}

impl DepCache {
    /// The live entry for `key`, or `build()`'s result (now cached). The lock
    /// is held only for lookup/insert, never while building: two roots
    /// building the same key at once both build, and the second insert keeps
    /// the first live `Arc`.
    pub fn get_or_build(&self, key: DepKey, build: impl FnOnce() -> DepNodes) -> Arc<DepNodes> {
        if let Some(live) = self.lock().get(&key).and_then(Weak::upgrade) {
            return live;
        }
        let built = Arc::new(build());
        let mut map = self.lock();
        map.retain(|_, w| w.strong_count() > 0);
        if let Some(live) = map.get(&key).and_then(Weak::upgrade) {
            return live;
        }
        map.insert(key, Arc::downgrade(&built));
        built
    }

    /// The live parsed package for the `.app` at `path`, or `load()`'s result
    /// (now cached). `stamp` must be the one taken before the file was read;
    /// without one (file could not be stat'ed) nothing is cached. Same lock
    /// discipline as [`Self::get_or_build`].
    pub fn package(
        &self,
        path: &Path,
        stamp: Option<AppFileStamp>,
        load: impl FnOnce() -> anyhow::Result<ParsedAppPackage>,
    ) -> anyhow::Result<Arc<ParsedAppPackage>> {
        let Some(stamp) = stamp else {
            return load().map(Arc::new);
        };
        let key = (path.to_path_buf(), stamp);
        let lock = || self.packages.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(live) = lock().get(&key).and_then(Weak::upgrade) {
            return Ok(live);
        }
        let built = Arc::new(load()?);
        let mut map = lock();
        map.retain(|_, w| w.strong_count() > 0);
        if let Some(live) = map.get(&key).and_then(Weak::upgrade) {
            return Ok(live);
        }
        map.insert(key, Arc::downgrade(&built));
        Ok(built)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<DepKey, Weak<DepNodes>>> {
        self.nodes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    fn live_entries(&self) -> usize {
        self.lock()
            .values()
            .filter(|w| w.strong_count() > 0)
            .count()
    }
}

/// Everything the dependency nodes depend on: the dependency apps in
/// `snap.apps[1..]` order (that order IS their AppRef numbering), each with
/// its on-disk identity and whether its source was indexed.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DepKey {
    apps: Vec<DepAppKey>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct DepAppKey {
    id: AppId,
    path: Option<PathBuf>,
    /// Taken by `load_all_apps` BEFORE the file's bytes were read — never
    /// re-read here. Stat'ing now could pair a newer file's stamp with nodes
    /// built from the older bytes, and that stale entry would never heal.
    stamp: Option<AppFileStamp>,
    /// Encodes `--dependency-source`: the same `.app` indexed with and
    /// without its embedded source yields different nodes.
    has_source: bool,
}

impl DepKey {
    pub fn of(snap: &AppSetSnapshot) -> DepKey {
        let apps = snap
            .apps
            .iter()
            .skip(1)
            .map(|unit| DepAppKey {
                id: unit.id.clone(),
                path: unit.app_path.clone(),
                stamp: unit.app_stamp,
                has_source: unit.source.is_some(),
            })
            .collect();
        DepKey { apps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::deps::app_package_zip::test_apps;
    use crate::lsp::snapshot::LspSnapshot;
    use crate::snapshot::DependencySource;
    use std::path::Path;

    const DEP_GUID: &str = "437dbf0e-84ff-417a-965d-ed2bb9650972";
    const GUID_A: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const GUID_B: &str = "bbbbbbbb-0000-0000-0000-000000000002";

    struct Fx {
        _dir: tempfile::TempDir,
        alpackages: PathBuf,
        root_a: PathBuf,
        root_b: PathBuf,
    }

    /// Writes the dependency `.app`: codeunit 80 "Sales-Post" (symbols AND
    /// embedded source), plus any `extra` codeunits as symbols only.
    /// A non-empty `extra` (symbols) also ships codeunit 81 "Extra" as
    /// embedded source, so the extra codeunit shows up in both modes.
    fn write_dep_app(alpackages: &Path, extra: &str) {
        let manifest = test_apps::manifest_xml(DEP_GUID, "Base Application");
        let symbols = format!(
            r#"{{"Codeunits":[{{"Id":80,"Name":"Sales-Post","Methods":[{{"Name":"Run","Id":1}}]}}{extra}]}}"#
        );
        let mut entries: Vec<(&str, &[u8])> = vec![
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
            (
                "src/SalesPost.Codeunit.al",
                b"codeunit 80 \"Sales-Post\" { procedure Run() begin end; }",
            ),
        ];
        if !extra.is_empty() {
            entries.push((
                "src/Extra.Codeunit.al",
                b"codeunit 81 \"Extra\" { procedure Run() begin end; }",
            ));
        }
        let app = test_apps::build_app(&entries);
        std::fs::write(alpackages.join("Microsoft_Base Application_28.4.app"), app).unwrap();
    }

    fn write_root(root: &Path, guid: &str, name: &str) {
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("app.json"),
            format!(r#"{{"id":"{guid}","name":"{name}","publisher":"probe","version":"1.0.0.0"}}"#),
        )
        .unwrap();
        std::fs::write(
            root.join("src/X.Codeunit.al"),
            "codeunit 50100 \"X\"\n{\n    procedure Go()\n    var\n        SalesPost: Codeunit \"Sales-Post\";\n    begin\n        SalesPost.Run();\n    end;\n}\n",
        )
        .unwrap();
    }

    /// Two workspace roots side by side under one parent whose `.alpackages`
    /// holds one dependency app.
    fn two_roots_one_alpackages() -> Fx {
        let dir = tempfile::tempdir().expect("tempdir");
        let alpackages = dir.path().join(".alpackages");
        std::fs::create_dir_all(&alpackages).unwrap();
        write_dep_app(&alpackages, "");
        let root_a = dir.path().join("root_a");
        let root_b = dir.path().join("root_b");
        write_root(&root_a, GUID_A, "RootA");
        write_root(&root_b, GUID_B, "RootB");
        Fx {
            _dir: dir,
            alpackages,
            root_a,
            root_b,
        }
    }

    /// A root's compiled `.app` (symbols only), as a build would drop it into
    /// the shared `.alpackages`.
    fn write_compiled_root(alpackages: &Path, guid: &str, name: &str, cu_id: i64) {
        let manifest = test_apps::manifest_xml(guid, name);
        let symbols = format!(
            r#"{{"Codeunits":[{{"Id":{cu_id},"Name":"{name} Lib","Methods":[{{"Name":"Run","Id":1}}]}}]}}"#
        );
        let app = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
        ]);
        std::fs::write(
            alpackages.join(format!("Microsoft_{name}_1.0.0.0.app")),
            app,
        )
        .unwrap();
    }

    fn build(root: &Path, source: DependencySource, cache: &DepCache) -> LspSnapshot {
        LspSnapshot::build_full_with_cache(root, source, cache).expect("snapshot build")
    }

    #[test]
    fn roots_with_the_same_dependencies_share_one_dep_tier() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Symbols, &cache);
        let b = build(&fx.root_b, DependencySource::Symbols, &cache);
        assert!(
            !a.graph.routines.shared().is_empty(),
            "the dependency must load"
        );
        assert!(Arc::ptr_eq(
            a.graph.routines.shared(),
            b.graph.routines.shared()
        ));
        assert!(Arc::ptr_eq(
            a.graph.objects.shared(),
            b.graph.objects.shared()
        ));
    }

    #[test]
    fn roots_share_one_parsed_package_per_dependency() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Symbols, &cache);
        let b = build(&fx.root_b, DependencySource::Symbols, &cache);
        let abi = |s: &LspSnapshot| {
            s.snap
                .apps
                .iter()
                .find(|u| u.id.guid == DEP_GUID)
                .and_then(|u| u.abi.clone())
                .expect("dependency unit carries its package")
        };
        assert!(Arc::ptr_eq(&abi(&a), &abi(&b)));
    }

    #[test]
    fn different_dependency_source_does_not_share() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Symbols, &cache);
        assert!(!Arc::ptr_eq(
            a.graph.routines.shared(),
            b.graph.routines.shared()
        ));
    }

    /// Each root's own compiled `.app` sits in the shared `.alpackages`. A root
    /// drops its own (self-dependency guard) and loads its sibling's, so the
    /// two roots hold different dependency sets of the SAME size and shape.
    #[test]
    fn a_sibling_compiled_app_in_alpackages_splits_the_sets() {
        let fx = two_roots_one_alpackages();
        write_compiled_root(&fx.alpackages, GUID_A, "RootA", 50001);
        write_compiled_root(&fx.alpackages, GUID_B, "RootB", 50002);
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Symbols, &cache);
        let b = build(&fx.root_b, DependencySource::Symbols, &cache);
        assert!(!Arc::ptr_eq(
            a.graph.routines.shared(),
            b.graph.routines.shared()
        ));

        for (root, shared) in [(&fx.root_a, &a), (&fx.root_b, &b)] {
            let fresh = build(root, DependencySource::Symbols, &DepCache::default());
            let routines = |s: &LspSnapshot| s.graph.routines.iter().cloned().collect::<Vec<_>>();
            let objects = |s: &LspSnapshot| s.graph.objects.iter().cloned().collect::<Vec<_>>();
            let edges = |s: &LspSnapshot| {
                let mut all: Vec<_> = s
                    .edges_by_file
                    .values()
                    .flat_map(|v| v.iter().map(|c| c.edge.clone()))
                    .collect();
                all.sort();
                all
            };
            assert_eq!(routines(shared), routines(&fresh), "{}", root.display());
            assert_eq!(objects(shared), objects(&fresh), "{}", root.display());
            assert_eq!(edges(shared), edges(&fresh), "{}", root.display());
        }
    }

    /// The `.app` is replaced AFTER the snapshot read it but BEFORE the dep
    /// tier is built from it. The entry must be keyed on the stamp taken
    /// before the read (the OLD file), so the next full build — which stamps
    /// the NEW file — misses and sees the new codeunit.
    ///
    /// Embedded mode on purpose: the dependency's source is read into the
    /// snapshot at build time, so the first context really holds OLD nodes.
    /// (In symbols mode the ABI is re-read from disk while the tier is built,
    /// so those nodes come out NEW under the old stamp — harmless: the next
    /// build misses and rebuilds.)
    #[test]
    fn an_app_replaced_mid_build_is_not_cached_under_the_new_stamp() {
        use crate::program::resolve::full::build_context_from_snapshot_cached;
        use crate::snapshot::SnapshotBuilder;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let (snap, _) = (SnapshotBuilder {
            workspace_root: fx.root_a.clone(),
            local_providers: vec![],
        })
        .build_with_options(DependencySource::Embedded, &cache)
        .expect("snapshot build");
        write_dep_app(
            &fx.alpackages,
            r#",{"Id":81,"Name":"Extra","Methods":[{"Name":"Run","Id":1}]}"#,
        );
        let old = build_context_from_snapshot_cached(snap, &cache).expect("context");
        assert!(
            !old.graph().objects.iter().any(|o| o.name == "Extra"),
            "precondition: the first context was built from the OLD bytes"
        );

        let new = build(&fx.root_a, DependencySource::Embedded, &cache);
        assert!(
            new.graph.objects.iter().any(|o| o.name == "Extra"),
            "the replaced .app's new codeunit must be in the fresh graph"
        );
        assert!(!Arc::ptr_eq(
            old.graph().routines.shared(),
            new.graph.routines.shared()
        ));
    }

    #[test]
    fn a_replaced_app_is_not_served_stale() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let old = build(&fx.root_a, DependencySource::Symbols, &cache);
        write_dep_app(
            &fx.alpackages,
            r#",{"Id":81,"Name":"Extra","Methods":[{"Name":"Run","Id":1}]}"#,
        );
        let new = build(&fx.root_a, DependencySource::Symbols, &cache);
        assert!(!Arc::ptr_eq(
            old.graph.routines.shared(),
            new.graph.routines.shared()
        ));
        assert!(
            new.graph.objects.iter().any(|o| o.name == "Extra"),
            "the rewritten .app's new codeunit must be in the graph"
        );
    }

    #[test]
    fn entries_die_with_their_last_root() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Symbols, &cache);
        assert_eq!(cache.live_entries(), 1);
        drop(a);
        assert_eq!(cache.live_entries(), 0, "no root uses the tier any more");
        let _again = build(&fx.root_a, DependencySource::Symbols, &cache);
        assert_eq!(cache.live_entries(), 1);
    }
}
