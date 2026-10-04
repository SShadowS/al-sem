//! Process-level sharing of the dependency tier across workspace roots.
//!
//! The LSP server builds one snapshot per workspace root. Roots that load the
//! same dependency apps used to each hold their own copy of the same
//! dependency nodes; [`DepCache`] lets them hold one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

use crate::app_package::ParsedAppPackage;
use crate::dependencies::AppFileStamp;
use crate::program::graph::AbiIngestError;
use crate::program::node::AppRef;
use crate::program::node_extract::{ObjectNode, RoutineNode};
use crate::program::profile::{BuildProfile, DependencyBodies};
use crate::program::resolve::decl_surface::DepMetaMap;
use crate::snapshot::embedded::SourceFile;
use crate::snapshot::provider::SourceRoot;
use crate::snapshot::{AppId, AppSetSnapshot, TrustTier};

/// Process-level dependency tier, shared by every workspace root that loads
/// the SAME dependency set. Entries are held weakly: one lives exactly as
/// long as some root's snapshot still uses it.
#[derive(Default)]
pub struct DepCache {
    nodes: Mutex<HashMap<DepKey, Weak<DepNodes>>>,
    packages: Mutex<HashMap<(PathBuf, AppFileStamp), Weak<ParsedAppPackage>>>,
    sources: Mutex<HashMap<(PathBuf, AppFileStamp), WeakSource>>,
}

/// A `SourceRoot` whose file list is held weakly (see [`DepCache::source`]).
struct WeakSource {
    files: Weak<Vec<SourceFile>>,
    tier: TrustTier,
    content_hash: String,
}

impl WeakSource {
    fn of(root: &SourceRoot) -> Self {
        WeakSource {
            files: Arc::downgrade(&root.files),
            tier: root.tier,
            content_hash: root.content_hash.clone(),
        }
    }

    fn is_live(&self) -> bool {
        self.files.strong_count() > 0
    }

    fn upgrade(&self) -> Option<SourceRoot> {
        Some(SourceRoot {
            files: self.files.upgrade()?,
            tier: self.tier,
            content_hash: self.content_hash.clone(),
        })
    }
}

/// The shareable part of a `DepLayer` — independent of which root built it
/// (its AppRefs are 1..n in `snap.apps` order; the root's own app is always
/// AppRef(0) and never appears here).
pub struct DepNodes {
    pub objects: Arc<Vec<ObjectNode>>,
    pub routines: Arc<Vec<RoutineNode>>,
    pub abi_ingest_errors: Vec<AbiIngestError>,
    /// The LSP products derived from this tier (set by the first snapshot
    /// that builds them). Both are keyed by this tier's AppRefs, so they are
    /// valid exactly where the tier itself is shared.
    pub lsp: OnceLock<Arc<DepLspTier>>,
}

/// Dependency-derived LSP data, shared with [`DepNodes`].
pub struct DepLspTier {
    pub dep_meta: Arc<DepMetaMap>,
    pub dep_texts: Arc<DepTexts>,
}

/// Dependency file texts by `(app, virtual path)`.
pub(crate) type DepTexts = HashMap<(AppRef, String), Arc<str>>;

impl DepCache {
    /// The live entry for `key`, or `build()`'s result (now cached). The lock
    /// is held only for lookup/insert, never while building: two roots
    /// building the same key at once both build, and the second insert keeps
    /// the first live `Arc`. A dependency without a stamp (its file could not
    /// be stat'ed) makes the key unsafe to share: it is built fresh and not
    /// cached, like [`Self::package`].
    pub fn get_or_build(&self, key: DepKey, build: impl FnOnce() -> DepNodes) -> Arc<DepNodes> {
        if key.apps.iter().any(|a| a.stamp.is_none()) {
            return Arc::new(build());
        }
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

    /// The live entry for `key`, only if its LSP products (`lsp`) are already
    /// published. Never builds. An entry without them (built by a CLI-style
    /// build, or by a root still inside `from_context`) is a miss: whoever
    /// fills the slot needs the dependency parse trees.
    pub fn get(&self, key: &DepKey) -> Option<Arc<DepNodes>> {
        self.lock()
            .get(key)
            .and_then(Weak::upgrade)
            .filter(|nodes| nodes.lsp.get().is_some())
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

    /// The `.app`'s extracted embedded source, with every file text shared
    /// with any live snapshot that already holds it, or `load()`'s result
    /// (now cached). `stamp` must be the one taken before the file was read;
    /// without one nothing is cached. A failed or empty (`None`) load caches
    /// nothing.
    ///
    /// Representation: the map holds a `Weak` to the source's file list
    /// (`SourceRoot.files: Arc<Vec<SourceFile>>`), never a `Weak` into a
    /// text. A hit shares that same list. When the last `SourceRoot` holding
    /// the list is dropped, the list and every text it owns are freed; the
    /// dead map entry then pins only the list's small `Arc` header until a
    /// later miss purges it. (A `Weak<str>` would pin the whole text: an
    /// `Arc<str>` stores its bytes in the same allocation as its counts.)
    pub fn source(
        &self,
        path: &Path,
        stamp: Option<AppFileStamp>,
        load: impl FnOnce() -> anyhow::Result<Option<SourceRoot>>,
    ) -> anyhow::Result<Option<SourceRoot>> {
        let Some(stamp) = stamp else {
            return load();
        };
        let key = (path.to_path_buf(), stamp);
        let lock = || self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(live) = lock().get(&key).and_then(WeakSource::upgrade) {
            return Ok(Some(live));
        }
        let Some(built) = load()? else {
            return Ok(None);
        };
        let mut map = lock();
        map.retain(|_, w| w.is_live());
        if let Some(live) = map.get(&key).and_then(WeakSource::upgrade) {
            return Ok(Some(live));
        }
        map.insert(key, WeakSource::of(&built));
        Ok(Some(built))
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
    /// A profile that keeps dependency trees builds a different tier.
    keep_bodies: bool,
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
    pub fn of(snap: &AppSetSnapshot, profile: BuildProfile) -> DepKey {
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
        DepKey {
            apps,
            keep_bodies: profile.dependency_bodies == DependencyBodies::Keep,
        }
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

    /// The dependency's embedded source: `Post` (called by the workspace)
    /// raises the `OnAfterRun` event (subscribed to by the workspace).
    const SALES_POST_SRC: &str = "codeunit 80 \"Sales-Post\"\n{\n    procedure Run()\n    begin\n    end;\n\n    procedure Post()\n    begin\n        OnAfterRun();\n    end;\n\n    [IntegrationEvent(false, false)]\n    local procedure OnAfterRun()\n    begin\n    end;\n}\n";

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
            ("src/SalesPost.Codeunit.al", SALES_POST_SRC.as_bytes()),
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
            format!(
                r#"{{"id":"{guid}","name":"{name}","publisher":"probe","version":"1.0.0.0","dependencies":[{{"id":"{DEP_GUID}","name":"Base Application","publisher":"Microsoft","version":"28.0.0.0"}}]}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/X.Codeunit.al"),
            "codeunit 50100 \"X\"\n{\n    procedure Go()\n    var\n        SalesPost: Codeunit \"Sales-Post\";\n    begin\n        SalesPost.Post();\n    end;\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Sub.Codeunit.al"),
            "codeunit 50101 \"Sub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Sales-Post\", 'OnAfterRun', '', false, false)]\n    local procedure HandleAfterRun()\n    begin\n    end;\n}\n",
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
        let old =
            build_context_from_snapshot_cached(snap, BuildProfile::LIGHT, &cache).expect("context");
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

    /// Same path, new stamp: the package map must not serve the old parse.
    #[test]
    fn a_replaced_app_gets_a_fresh_parsed_package() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let abi = |s: &LspSnapshot| {
            s.snap
                .apps
                .iter()
                .find(|u| u.id.guid == DEP_GUID)
                .and_then(|u| u.abi.clone())
                .expect("dependency unit carries its package")
        };
        let old = build(&fx.root_a, DependencySource::Symbols, &cache);
        write_dep_app(
            &fx.alpackages,
            r#",{"Id":81,"Name":"Extra","Methods":[{"Name":"Run","Id":1}]}"#,
        );
        let new = build(&fx.root_a, DependencySource::Symbols, &cache);
        assert!(!Arc::ptr_eq(&abi(&old), &abi(&new)));
    }

    /// A dependency with no stamp is never shared: each build makes its own
    /// tier and nothing is cached.
    #[test]
    fn a_dependency_without_a_stamp_is_not_shared() {
        use crate::program::resolve::full::build_context_from_snapshot_cached;
        use crate::snapshot::SnapshotBuilder;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let snap = || {
            let (mut snap, _) = (SnapshotBuilder {
                workspace_root: fx.root_a.clone(),
                local_providers: vec![],
            })
            .build_with_options(DependencySource::Symbols, &cache)
            .expect("snapshot build");
            let dep = snap
                .apps
                .iter_mut()
                .find(|u| u.id.guid == DEP_GUID)
                .expect("precondition: the dependency loads");
            dep.app_stamp = None;
            snap
        };
        let one = build_context_from_snapshot_cached(snap(), BuildProfile::LIGHT, &cache)
            .expect("context");
        let two = build_context_from_snapshot_cached(snap(), BuildProfile::LIGHT, &cache)
            .expect("context");
        assert!(
            !one.graph().routines.shared().is_empty(),
            "precondition: the dependency tier is built"
        );
        assert!(!Arc::ptr_eq(
            one.graph().routines.shared(),
            two.graph().routines.shared()
        ));
        assert_eq!(cache.live_entries(), 0);
    }

    /// The dependency's embedded source as a snapshot holds it. Returning the
    /// shared file list (not just its texts) keeps it alive, as a live
    /// snapshot would: the cache shares only a list some root still holds.
    fn dep_texts(cache: &DepCache, root: &Path) -> Arc<Vec<SourceFile>> {
        use crate::snapshot::SnapshotBuilder;
        let (snap, _) = (SnapshotBuilder {
            workspace_root: root.to_path_buf(),
            local_providers: vec![],
        })
        .build_with_options(DependencySource::Embedded, cache)
        .expect("snapshot build");
        let dep = snap
            .apps
            .iter()
            .find(|u| u.id.guid == DEP_GUID)
            .and_then(|u| u.source.as_ref())
            .expect("precondition: the dependency ships embedded source");
        assert!(!dep.files.is_empty());
        Arc::clone(&dep.files)
    }

    fn same_allocations(a: &[SourceFile], b: &[SourceFile]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| Arc::ptr_eq(&x.text, &y.text))
    }

    /// Two roots, one cache: the extracted source is held once, but only for
    /// the same `.app` (same stamp). A replaced file, or no cache at all,
    /// extracts afresh.
    #[test]
    fn roots_share_one_extracted_source_per_app() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = dep_texts(&cache, &fx.root_a);
        let b = dep_texts(&cache, &fx.root_b);
        assert!(same_allocations(&a, &b), "same app, same stamp: shared");

        let solo = dep_texts(&DepCache::default(), &fx.root_a);
        assert!(!same_allocations(&a, &solo), "no shared cache: not shared");

        write_dep_app(
            &fx.alpackages,
            r#",{"Id":81,"Name":"Extra","Methods":[{"Name":"Run","Id":1}]}"#,
        );
        let c = dep_texts(&cache, &fx.root_a);
        assert!(!same_allocations(&a, &c), "new stamp: not served stale");
        assert!(c.len() > a.len(), "the new file's source is the new one");
    }

    /// Dropping the last root frees the dependency's texts: the cache keeps
    /// no `Weak` into a text (an `Arc<str>`'s bytes share the allocation with
    /// its counts, so such a `Weak` would pin the whole text).
    #[test]
    fn dependency_texts_are_freed_with_their_last_root() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let text = a
            .snap
            .apps
            .iter()
            .find(|u| u.id.guid == DEP_GUID)
            .and_then(|u| u.source.as_ref())
            .and_then(|s| s.files.first())
            .map(|f| Arc::downgrade(&f.text))
            .expect("precondition: the dependency ships embedded source");
        let live = text.upgrade().expect("held by the snapshot");
        assert!(
            a.dep_texts.values().any(|t| Arc::ptr_eq(t, &live)),
            "precondition: the text is the one the LSP surface serves"
        );
        drop(live);
        // Checked while the text is alive: `Weak::weak_count` reads 0 once
        // the strong count is 0, whoever still holds a `Weak`.
        assert_eq!(
            text.weak_count(),
            1,
            "the cache holds no Weak into the text (only this test does)"
        );
        drop(a);
        // `LspSnapshot::from_context` drops the dependency parse units on a
        // background thread, so the last strong holder goes away shortly
        // after `drop(a)`, not during it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while text.strong_count() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(text.upgrade().is_none(), "no strong holder is left");
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

    /// Roots sharing a dependency tier share its `dep_meta`/`dep_texts`, and
    /// the texts are the shared extracted-source allocations.
    #[test]
    fn roots_share_dep_meta_and_dep_texts() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert!(!a.dep_texts.is_empty(), "precondition: dependency texts");
        assert!(!a.dep_meta.is_empty(), "precondition: dependency decls");
        assert!(Arc::ptr_eq(&a.dep_meta, &b.dep_meta));
        assert!(Arc::ptr_eq(&a.dep_texts, &b.dep_texts));
        let src = dep_texts(&cache, &fx.root_a);
        assert!(
            a.dep_texts
                .values()
                .all(|t| src.iter().any(|s| Arc::ptr_eq(&s.text, t))),
            "dep_texts values are the shared extracted-source Arcs"
        );
    }

    /// A dropped root leaves nothing behind: the next root's tier is fresh
    /// and equals a cache-less build.
    #[test]
    fn dep_lsp_tier_after_the_first_root_is_dropped_is_fresh_and_correct() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let old = Arc::downgrade(&a.dep_texts);
        drop(a);
        assert!(old.upgrade().is_none(), "nothing retains the dropped tier");
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert!(!b.dep_texts.is_empty());
        assert_eq!(b.dep_texts.len(), solo.dep_texts.len());
        for (k, v) in b.dep_texts.iter() {
            assert_eq!(solo.dep_texts.get(k).map(|s| &**s), Some(&**v));
        }
        let keys = |s: &LspSnapshot| {
            let mut k: Vec<_> = s.dep_meta.keys().cloned().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&b), keys(&solo));
    }

    /// Every LSP answer of a snapshot, order-independent, as text.
    fn answers(s: &LspSnapshot) -> String {
        use std::collections::BTreeMap;
        let sorted = |v: &[crate::program::resolve::full::ClassifiedEdge]| {
            let mut v: Vec<_> = v
                .iter()
                .map(|c| (c.obligation_id.clone(), c.edge.clone()))
                .collect();
            v.sort();
            v
        };
        let decls: BTreeMap<_, _> = s
            .decls_by_file
            .iter()
            .map(|(f, d)| (f.clone(), format!("{d:?}")))
            .collect();
        let edges: BTreeMap<_, _> = s
            .edges_by_file
            .iter()
            .map(|(f, e)| (f.clone(), sorted(e)))
            .collect();
        let incoming: BTreeMap<_, _> = s
            .incoming
            .iter()
            .map(|(t, refs)| {
                let mut o: Vec<_> = refs
                    .iter()
                    .map(|r| s.edge(r).obligation_id.clone())
                    .collect();
                o.sort();
                (t.clone(), o)
            })
            .collect();
        let fanout: BTreeMap<_, _> = s.publisher_fanout.iter().collect();
        let by_id: BTreeMap<_, _> = s
            .decl_by_id
            .iter()
            .map(|(k, d)| (k.clone(), format!("{d:?}")))
            .collect();
        let dep_meta: BTreeMap<_, _> = s.dep_meta.iter().collect();
        let mut dep_texts: Vec<_> = s.dep_texts.keys().collect();
        dep_texts.sort();
        format!(
            "{decls:#?}\n{edges:#?}\n{:#?}\n{incoming:#?}\n{fanout:#?}\n{by_id:#?}\n{dep_meta:#?}\n{dep_texts:#?}",
            sorted(&s.event_edges)
        )
    }

    /// The fixture really exercises the dependency tier: a call into it, an
    /// event raised in it and subscribed to by the workspace.
    fn assert_non_trivial(s: &LspSnapshot) {
        assert!(!s.dep_meta.is_empty(), "precondition: dependency decls");
        assert!(!s.dep_texts.is_empty(), "precondition: dependency texts");
        assert!(!s.event_edges.is_empty(), "precondition: event edges");
        assert!(
            s.event_edges.iter().any(|e| !e.edge.routes.is_empty()),
            "precondition: the workspace subscriber is wired {:#?}",
            s.event_edges.iter().map(|e| &e.edge).collect::<Vec<_>>()
        );
        assert!(
            !s.publisher_fanout.is_empty(),
            "precondition: publisher fan-out"
        );
        // The call must hit the dependency routine `Post` (declared only in
        // its embedded source, not in the symbols) and resolve from source,
        // not through the ABI or a builtin.
        assert!(
            s.incoming.iter().any(|(t, refs)| {
                t.object.app != AppRef(0)
                    && t.object.key == crate::program::node::ObjKey::Id(80) // "Sales-Post"
                    && t.name_lc == "post"
                    && refs.iter().any(|r| {
                        s.edge(r).edge.routes.iter().any(|route| {
                            route.evidence == crate::program::resolve::edge::Evidence::Source
                                && matches!(
                                    &route.target,
                                    crate::program::resolve::edge::RouteTarget::Routine(_)
                                )
                        })
                    })
            }),
            "precondition: the workspace calls Sales-Post.Post with Source evidence {:#?}",
            s.edges_by_file
                .values()
                .flat_map(|v| v.iter().map(|c| &c.edge))
                .collect::<Vec<_>>()
        );
    }

    /// Root B built while root A holds the shared tier (a hit) answers
    /// exactly like a cache-less build of root B.
    #[test]
    fn a_shared_tier_hit_answers_like_a_cache_less_build() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert!(
            Arc::ptr_eq(&a.dep_meta, &b.dep_meta),
            "precondition: B was built on the shared tier"
        );
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_non_trivial(&solo);
        assert_eq!(answers(&b), answers(&solo));
    }

    /// On a hit only the workspace unit is parsed; the dependency's source is
    /// not parsed again.
    #[test]
    fn a_shared_tier_hit_parses_only_the_workspace() {
        use crate::program::resolve::full::build_context_with;
        use crate::snapshot::parse::parse_log::parses_under;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let _a = build(&fx.root_a, DependencySource::Embedded, &cache);
        assert_eq!(
            parses_under(fx._dir.path()),
            1,
            "precondition: root A parsed the dependency once"
        );
        let ctx = build_context_with(
            &fx.root_b,
            DependencySource::Embedded,
            BuildProfile::LIGHT,
            &cache,
        )
        .expect("context");
        assert_eq!(parses_under(fx._dir.path()), 1, "B parsed the dependency");
        let apps: Vec<_> = ctx.parsed().iter().map(|u| u.app.guid.clone()).collect();
        assert_eq!(apps, vec![GUID_B.to_string()]);
    }

    /// Profiles that keep different things build different dependency tiers.
    #[test]
    fn different_profiles_do_not_share_a_dep_tier() {
        use crate::program::resolve::full::build_context_with;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let light = build_context_with(
            &fx.root_a,
            DependencySource::Embedded,
            BuildProfile::LIGHT,
            &cache,
        )
        .expect("light");
        let full = build_context_with(
            &fx.root_b,
            DependencySource::Embedded,
            BuildProfile::FULL,
            &cache,
        )
        .expect("full");
        assert_eq!(light.profile(), BuildProfile::LIGHT);
        assert_eq!(full.profile(), BuildProfile::FULL);
        assert!(!Arc::ptr_eq(
            &light.dep_layer.dep_nodes,
            &full.dep_layer.dep_nodes
        ));
    }

    /// A rung-3 rebuild of a root whose dependency set did not change is a
    /// hit, and answers like a cache-less build.
    #[test]
    fn a_rung3_rebuild_on_a_shared_tier_is_a_hit() {
        use crate::lsp::updater::{ChangeEvent, SharedSnapshot, spawn_updater};
        use crate::snapshot::parse::parse_log::parses_under;
        use std::time::{Duration, Instant};
        let fx = two_roots_one_alpackages();
        let cache = Arc::new(DepCache::default());
        let _a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let (base, parsed) = LspSnapshot::build_full_with_parsed_with_cache(
            &fx.root_b,
            DependencySource::Embedded,
            &cache,
        )
        .expect("initial build");
        let base_generation = base.generation;
        let shared = Arc::new(SharedSnapshot::new(Arc::new(base)));
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = spawn_updater(
            Arc::clone(&shared),
            rx,
            fx.root_b.clone(),
            parsed,
            DependencySource::Embedded,
            Arc::clone(&cache),
            |_new, _scope| {},
        );
        tx.send(ChangeEvent::DepsChanged).expect("send");
        let deadline = Instant::now() + Duration::from_secs(30);
        while shared.get().generation == base_generation && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(tx);
        handle.join().expect("updater thread must exit cleanly");
        let rebuilt = shared.get();
        assert!(
            rebuilt.generation > base_generation,
            "precondition: the rung-3 rebuild was published"
        );
        assert_eq!(
            parses_under(fx._dir.path()),
            1,
            "only root A ever parsed the dependency"
        );
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_non_trivial(&solo);
        assert_eq!(answers(&rebuilt), answers(&solo));
    }
}
