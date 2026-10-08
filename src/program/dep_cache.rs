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
use crate::program::resolve::decl_surface::DepMeta;
use crate::snapshot::embedded::SourceFile;
use crate::snapshot::provider::SourceRoot;
use crate::snapshot::{AppId, AppSetSnapshot, ParsedUnit, TrustTier};

/// Process-level dependency tier, shared by every workspace root that loads
/// the SAME dependency set. Entries are held weakly: one lives exactly as
/// long as some root's snapshot still uses it.
#[derive(Default)]
pub struct DepCache {
    nodes: Mutex<HashMap<DepKey, Weak<DepNodes>>>,
    packages: Mutex<HashMap<(PathBuf, AppFileStamp), Weak<ParsedAppPackage>>>,
    /// One entry per `.app` path: what the last load at that stamp found.
    sources: Mutex<HashMap<PathBuf, (AppFileStamp, KnownSource)>>,
}

/// What a load of one `.app` found (see [`DepCache::source`]).
enum KnownSource {
    /// Embedded source; the entry outlives its text.
    Source(WeakSource),
    /// The `.app` ships no source.
    NoSource,
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

    fn upgrade(&self) -> Option<SourceRoot> {
        Some(SourceRoot {
            files: self.files.upgrade()?,
            tier: self.tier,
            content_hash: self.content_hash.clone(),
        })
    }

    /// The source without its text: an empty file list, the same tier and
    /// content hash. No provider loads `Some` with no files, so an empty list
    /// on a dependency means exactly this.
    fn deferred(&self) -> SourceRoot {
        SourceRoot {
            files: Arc::new(Vec::new()),
            tier: self.tier,
            content_hash: self.content_hash.clone(),
        }
    }
}

/// The shareable part of a `DepLayer` — independent of which root built it
/// (its AppRefs are 1..n in `snap.apps` order; the root's own app is always
/// AppRef(0) and never appears here).
pub struct DepNodes {
    pub objects: Arc<Vec<ObjectNode>>,
    pub routines: Arc<Vec<RoutineNode>>,
    pub abi_ingest_errors: Vec<AbiIngestError>,
    /// The frozen `DeclSurface` tier: every dependency routine's
    /// `RoutineMeta`, built with the nodes. Every consumer reads dependency
    /// metadata from here, never from dependency `ParsedUnit`s.
    pub dep_meta: Arc<DepMeta>,
    /// `"<app name>::<virtual path>"` of every dependency file whose parse
    /// was `Recovered`, sorted. Held here so a shared-tier hit (which does
    /// not parse the dependencies) still reports them.
    pub recovered: Vec<String>,
    /// The dependency `ParsedUnit`s, in `snap.apps` order: `Some` only when
    /// the tier was built from a `Keep` build parse (`DepInput::Built` under
    /// `DependencyBodies::Keep`). A `DepInput::Parsed` input never keeps
    /// bodies, and `build_dep_layer_cached` asserts it is never keyed `Keep`,
    /// so a `Keep` hit always carries them.
    pub bodies: Option<Arc<Vec<ParsedUnit>>>,
    /// The LSP products derived from this tier (set by the first snapshot
    /// that builds them). Keyed by this tier's AppRefs, so they are valid
    /// exactly where the tier itself is shared.
    pub lsp: OnceLock<Arc<DepLspTier>>,
    /// The dependency-only event links of this tier (engine-switch S10.4),
    /// set by the first snapshot that computes them. Separate from `lsp`: they
    /// read no dependency text, so S10.1b's deferral does not wait for them.
    pub lsp_events: OnceLock<Arc<crate::lsp::snapshot::DepEventLinks>>,
}

/// Dependency-derived LSP data, shared with [`DepNodes`].
pub struct DepLspTier {
    pub dep_lines: Arc<DepLines>,
}

/// A text-free line index of every dependency file, by `(app, virtual path)`:
/// what the LSP needs to turn a dependency position into an editor column. The
/// LSP keeps no dependency text (engine-switch S10.1).
pub(crate) type DepLines =
    HashMap<(AppRef, crate::program::str_pool::SharedStr), crate::lsp::encoding::LineIndex>;

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

    /// The live entry for `key`, if any. Never builds. Any live entry is a
    /// hit: it always carries `dep_meta` and `recovered` (and the bodies when
    /// `key` keeps them), and its LSP products (`dep_lines`) are either built
    /// already or built from the fresh snapshot (which then holds the
    /// dependency text: `build_context_with` extracts it whenever the tier has
    /// no LSP products yet), so a hit never needs to parse the dependencies.
    pub fn get(&self, key: &DepKey) -> Option<Arc<DepNodes>> {
        self.lock().get(key).and_then(Weak::upgrade)
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
    /// (now remembered). `stamp` must be the one taken before the file was
    /// read; without one nothing is remembered. A failed load remembers
    /// nothing.
    ///
    /// What is remembered outlives the text: a `.app` that ships no source
    /// answers `None` again without `load()`, and with `defer_text` a `.app`
    /// whose text no snapshot holds any more answers its source WITHOUT the
    /// text ([`WeakSource::deferred`]) instead of extracting it again. Only a
    /// build that will read no dependency text asks for that (see
    /// `build_context_with`, engine-switch S10.1b). One entry per path: a
    /// load at a new stamp replaces the old one.
    ///
    /// Representation: the map holds a `Weak` to the source's file list
    /// (`SourceRoot.files: Arc<Vec<SourceFile>>`), never a `Weak` into a
    /// text. A hit shares that same list. When the last `SourceRoot` holding
    /// the list is dropped, the list and every text it owns are freed; the
    /// entry then pins only the list's small `Arc` header. (A `Weak<str>`
    /// would pin the whole text: an `Arc<str>` stores its bytes in the same
    /// allocation as its counts.)
    pub fn source(
        &self,
        path: &Path,
        stamp: Option<AppFileStamp>,
        defer_text: bool,
        load: impl FnOnce() -> anyhow::Result<Option<SourceRoot>>,
    ) -> anyhow::Result<Option<SourceRoot>> {
        let Some(stamp) = stamp else {
            return load();
        };
        let lock = || self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        match lock().get(path) {
            Some((s, KnownSource::NoSource)) if *s == stamp => return Ok(None),
            Some((s, KnownSource::Source(w))) if *s == stamp => {
                if let Some(live) = w.upgrade() {
                    return Ok(Some(live));
                }
                if defer_text {
                    return Ok(Some(w.deferred()));
                }
            }
            _ => {}
        }
        let built = load()?;
        let mut map = lock();
        if let Some((s, KnownSource::Source(w))) = map.get(path)
            && *s == stamp
            && let Some(live) = w.upgrade()
        {
            return Ok(Some(live));
        }
        let known = match &built {
            Some(root) => KnownSource::Source(WeakSource::of(root)),
            None => KnownSource::NoSource,
        };
        map.insert(path.to_path_buf(), (stamp, known));
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

/// The shared `CDO_WS` gate (skips without it, panics under
/// `ENFORCE_CDO_WS=1`), included verbatim like the integration tests do.
#[cfg(test)]
#[path = "../../tests/common/cdo.rs"]
mod cdo;

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
    fn dep_source(cache: &DepCache, root: &Path) -> Arc<Vec<SourceFile>> {
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
        let a = dep_source(&cache, &fx.root_a);
        let b = dep_source(&cache, &fx.root_b);
        assert!(same_allocations(&a, &b), "same app, same stamp: shared");

        let solo = dep_source(&DepCache::default(), &fx.root_a);
        assert!(!same_allocations(&a, &solo), "no shared cache: not shared");

        write_dep_app(
            &fx.alpackages,
            r#",{"Id":81,"Name":"Extra","Methods":[{"Name":"Run","Id":1}]}"#,
        );
        let c = dep_source(&cache, &fx.root_a);
        assert!(!same_allocations(&a, &c), "new stamp: not served stale");
        assert!(c.len() > a.len(), "the new file's source is the new one");
    }

    /// The LSP keeps no dependency text (S10.1), stated directly: a snapshot
    /// built first on the same cache holds the extracted texts, and the LSP
    /// root built next shares those very allocations (the cache serves a list
    /// some root still holds). Once that first holder is dropped the texts are
    /// gone, while the LSP root is still alive: neither its tier
    /// (`dep_lines`), nor its retained snapshot, nor the cache (no `Weak` into
    /// a text: an `Arc<str>`'s bytes share the allocation with its counts, so
    /// such a `Weak` would pin the whole text) holds one.
    #[test]
    fn the_lsp_keeps_no_dependency_text() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let src = dep_source(&cache, &fx.root_a);
        let text = Arc::downgrade(&src[0].text);
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        assert!(!a.dep_lines.is_empty(), "precondition: dependency files");
        // Checked while the text is alive: `Weak::weak_count` reads 0 once
        // the strong count is 0, whoever still holds a `Weak`.
        assert_eq!(
            text.weak_count(),
            1,
            "the cache holds no Weak into the text (only this test does)"
        );
        drop(src);
        assert!(
            text.upgrade().is_none(),
            "the live LSP root holds no dependency text"
        );
        drop(a);
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

    /// Roots sharing a dependency tier share its `dep_meta` (held by the
    /// tier's nodes) and `dep_lines`, which index exactly the dependency's
    /// embedded source files.
    #[test]
    fn roots_share_dep_meta_and_dep_lines() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert!(!a.dep_lines.is_empty(), "precondition: dependency files");
        assert!(!a.dep_meta.is_empty(), "precondition: dependency decls");
        assert!(Arc::ptr_eq(&a.dep_meta, &a.dep_layer.dep_nodes.dep_meta));
        assert!(Arc::ptr_eq(
            &a.dep_layer.dep_nodes.dep_meta,
            &b.dep_layer.dep_nodes.dep_meta
        ));
        assert!(Arc::ptr_eq(&a.dep_meta, &b.dep_meta));
        assert!(Arc::ptr_eq(&a.dep_lines, &b.dep_lines));
        let src = dep_source(&cache, &fx.root_a);
        let mut indexed: Vec<&str> = a.dep_lines.keys().map(|(_, vp)| vp.as_str()).collect();
        let mut files: Vec<&str> = src.iter().map(|f| f.virtual_path.as_str()).collect();
        indexed.sort_unstable();
        files.sort_unstable();
        assert_eq!(indexed, files, "one index per embedded source file");
    }

    /// A dropped last root leaves nothing behind: the next root's tier
    /// (`dep_meta`, `recovered`, `dep_lines`) is fresh and equals a
    /// cache-less build.
    #[test]
    fn dep_tier_after_the_last_root_is_dropped_is_fresh_and_correct() {
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let old_lines = Arc::downgrade(&a.dep_lines);
        let old_meta = Arc::downgrade(&a.dep_layer.dep_nodes.dep_meta);
        drop(a);
        assert!(
            old_lines.upgrade().is_none(),
            "nothing retains the dropped line indexes"
        );
        assert!(
            old_meta.upgrade().is_none(),
            "nothing retains the dropped dep_meta"
        );
        assert_eq!(cache.live_entries(), 0, "the dropped tier is not live");
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert!(!b.dep_lines.is_empty());
        assert_eq!(*b.dep_lines, *solo.dep_lines);
        assert!(!b.dep_meta.is_empty(), "precondition: dependency decls");
        assert_eq!(*b.dep_meta, *solo.dep_meta);
        assert_eq!(
            b.dep_layer.dep_nodes.recovered,
            solo.dep_layer.dep_nodes.recovered
        );
    }

    /// Every LSP answer of a snapshot, order-independent, as text. It
    /// compares answers of the light view: `event_edges` holds only links
    /// with routes, on both sides.
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
            .all_incoming()
            .into_keys()
            .map(|t| {
                let mut o: Vec<_> = s
                    .incoming(&t)
                    .map(|r| s.edge(r).obligation_id.clone())
                    .collect();
                o.sort();
                (t, o)
            })
            .collect();
        let fanout: BTreeMap<_, _> = s
            .ws_publisher_fanout
            .keys()
            .chain(s.dep_events.publisher_fanout.keys())
            .map(|p| (p.clone(), s.publisher_fanout(p)))
            .collect();
        let by_id: BTreeMap<_, _> = s
            .decl_by_id
            .iter()
            .map(|(k, d)| (k.clone(), format!("{d:?}")))
            .collect();
        let dep_meta: BTreeMap<_, _> = s.dep_meta.iter().collect();
        let dep_lines: BTreeMap<_, _> = s.dep_lines.iter().collect();
        format!(
            "{decls:#?}\n{edges:#?}\n{:#?}\n{incoming:#?}\n{fanout:#?}\n{by_id:#?}\n{dep_meta:#?}\n{dep_lines:#?}",
            s.merged_event_edges()
        )
    }

    /// The fixture really exercises the dependency tier: a call into it, an
    /// event raised in it and subscribed to by the workspace.
    fn assert_non_trivial(s: &LspSnapshot) {
        assert!(!s.dep_meta.is_empty(), "precondition: dependency decls");
        assert!(!s.dep_lines.is_empty(), "precondition: dependency files");
        assert!(
            s.event_edges().next().is_some(),
            "precondition: event edges"
        );
        assert!(
            s.ws_event_edges.iter().any(|e| !e.edge.routes.is_empty()),
            "precondition: the workspace subscriber is wired {:#?}",
            s.event_edges().map(|e| &e.edge).collect::<Vec<_>>()
        );
        assert!(
            !s.ws_publisher_fanout.is_empty(),
            "precondition: publisher fan-out"
        );
        // The call must hit the dependency routine `Post` (declared only in
        // its embedded source, not in the symbols) and resolve from source,
        // not through the ABI or a builtin.
        assert!(
            s.ws_incoming.iter().any(|(t, refs)| {
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

    /// S10.1b: root B, built while root A holds the shared tier with its LSP
    /// products, extracts no dependency text (no root holds it after S10.1,
    /// and nothing would read it), and still answers like a cache-less build.
    #[test]
    fn a_root_on_a_live_lsp_tier_extracts_no_dependency_text() {
        use crate::snapshot::provider::extract_log::extractions_under;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        assert_eq!(
            extractions_under(&fx.alpackages),
            1,
            "precondition: root A extracted the text"
        );
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert!(
            Arc::ptr_eq(&a.dep_lines, &b.dep_lines),
            "precondition: B was built on A's tier"
        );
        assert_eq!(
            extractions_under(&fx.alpackages),
            1,
            "root B extracted no dependency text"
        );
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_non_trivial(&solo);
        assert_eq!(answers(&b), answers(&solo));
    }

    /// S10.1b: once the tier died, the cache still knows the dependency's
    /// source but no longer its text, and nothing can supply the parse; the
    /// build extracts the text again and answers like a cache-less build.
    #[test]
    fn a_root_after_the_tier_died_extracts_the_text_again() {
        use crate::snapshot::provider::extract_log::extractions_under;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        drop(build(&fx.root_a, DependencySource::Embedded, &cache));
        assert_eq!(cache.live_entries(), 0, "precondition: the tier died");
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert_eq!(
            extractions_under(&fx.alpackages),
            2,
            "root B extracted the text again"
        );
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_non_trivial(&solo);
        assert_eq!(answers(&b), answers(&solo));
    }

    /// A `.app` that ships no source is opened for it once per stamp, not on
    /// every build (here: a second root build, which also rebuilds its
    /// snapshot because the tier died).
    #[test]
    fn a_source_less_app_is_extracted_once() {
        use crate::snapshot::provider::extract_log::extractions_under;
        let fx = two_roots_one_alpackages();
        write_compiled_root(&fx.alpackages, GUID_B, "RootB", 50002);
        let compiled = fx.alpackages.join("Microsoft_RootB_1.0.0.0.app");
        let cache = DepCache::default();
        drop(build(&fx.root_a, DependencySource::Embedded, &cache));
        assert_eq!(
            extractions_under(&compiled),
            1,
            "precondition: root A loads the source-less sibling app"
        );
        let again = build(&fx.root_a, DependencySource::Embedded, &cache);
        assert_eq!(extractions_under(&compiled), 1);
        assert!(
            again.graph.objects.iter().any(|o| o.name == "RootB Lib"),
            "the source-less app is still loaded"
        );
    }

    /// S10.2: every id text in the shared tier is one allocation per distinct
    /// text (routine ids and `dep_meta` keys alike), and a second root's
    /// event links name tier routines with that same allocation.
    #[test]
    fn equal_id_texts_are_one_allocation_across_the_tier_and_roots() {
        use crate::program::str_pool::SharedStr;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        let tier = &a.dep_layer.dep_nodes;
        let mut first: HashMap<String, SharedStr> = HashMap::new();
        let mut same_text = 0;
        let mut seen = |s: &SharedStr| {
            let f = first.entry(s.to_string()).or_insert_with(|| s.clone());
            if !std::ptr::eq(f, s) {
                same_text += 1;
                assert!(SharedStr::ptr_eq(f, s), "{s:?} is a second allocation");
            }
        };
        for r in tier.routines.iter() {
            seen(&r.id.name_lc);
        }
        for id in tier.dep_meta.keys() {
            seen(&id.name_lc);
        }
        assert!(same_text > 0, "precondition: equal texts to share");
        let mut links = 0;
        for e in b.event_edges() {
            if let Some(r) = tier.routines.iter().find(|r| r.id == e.edge.from) {
                assert!(SharedStr::ptr_eq(&r.id.name_lc, &e.edge.from.name_lc));
                links += 1;
            }
        }
        assert!(links > 0, "precondition: root B links a tier publisher");
    }

    /// S10.2: the built tier holds one allocation per distinct text across
    /// every pooled field (ids, `dep_meta` keys, node names, types, subscriber
    /// arguments, ...): a fresh pool over a copy of it merges nothing.
    #[test]
    fn the_tier_holds_one_allocation_per_text() {
        use crate::program::str_pool::SharedStr;
        use crate::program::str_pool::{ShareStrings, StrPool};
        let fx = two_roots_one_alpackages();
        // Repeated texts in both node kinds: `Run` in two codeunits, and one
        // field type in two fields of a table.
        let manifest = test_apps::manifest_xml(DEP_GUID, "Base Application");
        let symbols =
            r#"{"Codeunits":[{"Id":80,"Name":"Sales-Post","Methods":[{"Name":"Run","Id":1}]}]}"#;
        let table = "table 82 \"T\"\n{\n    fields\n    {\n        field(1; \"A\"; Code[20]) { }\n        field(2; \"B\"; Code[20]) { }\n    }\n}\n";
        let app = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
            ("src/SalesPost.Codeunit.al", SALES_POST_SRC.as_bytes()),
            (
                "src/Extra.Codeunit.al",
                b"codeunit 81 \"Extra\" { procedure Run() begin end; }",
            ),
            ("src/T.Table.al", table.as_bytes()),
            (
                "src/S.Codeunit.al",
                b"codeunit 83 \"S\"\n{\n    [EventSubscriber(ObjectType::Table, Database::\"T\", 'OnAfterInsertEvent', '', false, false)]\n    local procedure H(var Rec: Record \"T\")\n    begin\n    end;\n}\n",
            ),
        ]);
        std::fs::write(
            fx.alpackages.join("Microsoft_Base Application_28.4.app"),
            app,
        )
        .unwrap();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let tier = &a.dep_layer.dep_nodes;
        let mut objects = (*tier.objects).clone();
        let mut routines = (*tier.routines).clone();
        let fields = &objects
            .iter()
            .find(|o| o.name == "T")
            .expect("precondition: the table loads")
            .fields;
        assert_eq!(fields.len(), 2, "precondition: two fields");
        assert!(SharedStr::ptr_eq(
            &fields[0].type_text,
            &fields[1].type_text
        ));
        let mut keys: Vec<crate::program::node::RoutineNodeId> =
            tier.dep_meta.keys().cloned().collect();
        let runs: Vec<_> = routines.iter().filter(|r| r.name == "Run").collect();
        assert!(runs.len() >= 2, "precondition: a node name repeats");
        // Stated directly, not through `ShareStrings` (whose field list the
        // pool check below shares with the build).
        assert!(SharedStr::ptr_eq(&runs[0].name, &runs[1].name));
        let meta_runs: Vec<_> = tier.dep_meta.values().filter(|m| m.name == "Run").collect();
        assert!(meta_runs.len() >= 2, "precondition: in two files");
        assert_ne!(meta_runs[0].virtual_path, meta_runs[1].virtual_path);
        assert!(SharedStr::ptr_eq(&meta_runs[0].name, &meta_runs[1].name));
        assert!(SharedStr::ptr_eq(&meta_runs[0].name, &runs[0].name));
        // A synthesized platform-event publisher id (built per root) holds the
        // subscriber's own text, not a copy.
        let sub_event = routines
            .iter()
            .flat_map(|r| &r.event_subscribers)
            .find(|s| s.event_name == "onafterinsertevent")
            .expect("precondition: the dependency subscribes to a platform event")
            .event_name
            .clone();
        let synth = a
            .event_edges()
            .find(|e| e.edge.from.name_lc == "onafterinsertevent")
            .expect("precondition: the platform publisher is linked");
        assert!(SharedStr::ptr_eq(&synth.edge.from.name_lc, &sub_event));
        let mut metas: Vec<_> = tier.dep_meta.values().cloned().collect();
        let mut pool = StrPool::default();
        objects.share_strings(&mut pool);
        routines.share_strings(&mut pool);
        keys.share_strings(&mut pool);
        metas.share_strings(&mut pool);
        let (seen, distinct, merged) = pool.counts();
        assert!(seen > distinct, "precondition: equal texts to share");
        assert_eq!(merged, 0, "a text is held in more than one allocation");
    }

    /// S10.4: a dependency publisher with a dependency subscriber AND a
    /// workspace subscriber is split: the dependency route goes to the
    /// tier's shared links (one `Arc` for every root), the workspace route to
    /// the root's own. Both subscribers still see the publisher as an
    /// incoming caller, its fan-out counts both, and every answer equals a
    /// cache-less build's.
    #[test]
    fn dependency_event_links_are_shared_and_split_from_the_workspace_ones() {
        use crate::program::resolve::edge::RouteTarget;
        let fx = two_roots_one_alpackages();
        let manifest = test_apps::manifest_xml(DEP_GUID, "Base Application");
        let symbols =
            r#"{"Codeunits":[{"Id":80,"Name":"Sales-Post","Methods":[{"Name":"Run","Id":1}]}]}"#;
        let dep_sub = "codeunit 81 \"DepSub\"\n{\n    [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Sales-Post\", 'OnAfterRun', '', false, false)]\n    local procedure HandleInDep()\n    begin\n    end;\n}\n";
        let app = test_apps::build_app(&[
            ("NavxManifest.xml", manifest.as_bytes()),
            ("SymbolReference.json", symbols.as_bytes()),
            ("src/SalesPost.Codeunit.al", SALES_POST_SRC.as_bytes()),
            ("src/DepSub.Codeunit.al", dep_sub.as_bytes()),
        ]);
        std::fs::write(
            fx.alpackages.join("Microsoft_Base Application_28.4.app"),
            app,
        )
        .unwrap();
        let cache = DepCache::default();
        let a = build(&fx.root_a, DependencySource::Embedded, &cache);
        let b = build(&fx.root_b, DependencySource::Embedded, &cache);
        assert!(
            Arc::ptr_eq(&a.dep_events, &b.dep_events),
            "the roots share one dependency-link set"
        );

        let targets = |edges: &[crate::program::resolve::full::ClassifiedEdge]| -> Vec<String> {
            edges
                .iter()
                .filter(|ce| ce.edge.from.name_lc == "onafterrun")
                .flat_map(|ce| &ce.edge.routes)
                .filter_map(|r| match &r.target {
                    RouteTarget::Routine(id) => Some(id.name_lc.to_string()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(targets(&a.dep_events.edges), ["handleindep"]);
        assert_eq!(targets(&a.ws_event_edges), ["handleafterrun"]);

        let publisher = a
            .dep_events
            .edges
            .iter()
            .find(|ce| ce.edge.from.name_lc == "onafterrun")
            .unwrap()
            .edge
            .from
            .clone();
        assert_eq!(a.publisher_fanout(&publisher), 2);
        for sub in ["handleindep", "handleafterrun"] {
            let id = a
                .event_edges()
                .flat_map(|ce| &ce.edge.routes)
                .find_map(|r| match &r.target {
                    RouteTarget::Routine(id) if id.name_lc == sub => Some(id.clone()),
                    _ => None,
                })
                .unwrap();
            assert!(
                a.incoming(&id).any(|r| a.edge(r).edge.from == publisher),
                "{sub} has the publisher as an incoming caller"
            );
        }

        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_eq!(answers(&b), answers(&solo));
    }

    /// A source remembered at one stamp is never served, with or without its
    /// text, for another stamp of the same path.
    #[test]
    fn a_deferred_source_is_served_only_at_its_stamp() {
        let cache = DepCache::default();
        let path = Path::new("x.app");
        let at = |len| {
            Some(AppFileStamp {
                len,
                modified: None,
            })
        };
        let root = |hash: &str| {
            Ok(Some(SourceRoot {
                files: Arc::new(vec![SourceFile {
                    virtual_path: "a.al".into(),
                    text: Arc::from("codeunit 1 A { }"),
                }]),
                tier: TrustTier::EmbeddedSource,
                content_hash: hash.into(),
            }))
        };
        let old = cache.source(path, at(1), true, || root("old")).unwrap();
        drop(old);
        let deferred = cache
            .source(path, at(1), true, || panic!("known at this stamp"))
            .unwrap()
            .expect("source");
        assert!(deferred.files.is_empty(), "served without its text");
        assert_eq!(deferred.content_hash, "old");
        let new = cache.source(path, at(2), true, || root("new")).unwrap();
        assert_eq!(new.expect("source").content_hash, "new");
        let text = cache
            .source(path, at(1), false, || root("reloaded"))
            .unwrap()
            .expect("source");
        assert_eq!(text.content_hash, "reloaded", "the old stamp was replaced");
    }

    /// Any live tier is a hit, even one whose LSP products were never
    /// published (root A here is a plain context, not a snapshot). On a hit
    /// only the workspace unit is parsed, and the snapshot built from it
    /// answers like a cache-less build.
    #[test]
    fn any_live_tier_is_a_hit_that_parses_only_the_workspace() {
        use crate::program::resolve::full::build_context_with;
        use crate::snapshot::parse::parse_log::parses_under;
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let a = build_context_with(
            &fx.root_a,
            DependencySource::Embedded,
            BuildProfile::LIGHT,
            &cache,
        )
        .expect("context");
        assert!(
            a.dep_layer.dep_nodes.lsp.get().is_none(),
            "precondition: the tier's LSP products are not published"
        );
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
        assert!(Arc::ptr_eq(
            &a.dep_layer.dep_nodes,
            &ctx.dep_layer.dep_nodes
        ));

        let (b, _) = LspSnapshot::from_context(ctx, &fx.root_b);
        let solo = build(&fx.root_b, DependencySource::Embedded, &DepCache::default());
        assert_non_trivial(&solo);
        assert_eq!(answers(&b), answers(&solo));
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

    /// Review Focus 2: a `Keep` request is never served a tier without
    /// bodies, and a `Summary` request never gets bodies — whatever was
    /// built first, and when two `Keep` builds race.
    #[test]
    fn a_full_build_always_gets_bodies_whatever_the_cache_holds() {
        use crate::program::resolve::full::{ProgramContext, build_context_with};
        let fx = two_roots_one_alpackages();
        let cache = DepCache::default();
        let ctx = |root: &Path, profile| {
            build_context_with(root, DependencySource::Embedded, profile, &cache).expect("context")
        };
        let bodies = |c: &ProgramContext| {
            let b = c
                .dep_bodies()
                .expect("a FULL context has dependency bodies");
            assert!(
                b.iter().any(|u| !u.files.is_empty()),
                "the bodies hold the dependency's files"
            );
        };

        // LIGHT then FULL.
        let light = ctx(&fx.root_a, BuildProfile::LIGHT);
        assert!(light.dep_bodies().is_none());
        let full = ctx(&fx.root_b, BuildProfile::FULL);
        bodies(&full);

        // FULL then LIGHT (both tiers live): LIGHT shares the LIGHT tier.
        let light_again = ctx(&fx.root_b, BuildProfile::LIGHT);
        assert!(light_again.dep_bodies().is_none());
        assert!(Arc::ptr_eq(
            &light.dep_layer.dep_nodes,
            &light_again.dep_layer.dep_nodes
        ));

        // FULL twice: the second shares the first's tier, bodies included.
        let full_again = ctx(&fx.root_a, BuildProfile::FULL);
        bodies(&full_again);
        assert!(Arc::ptr_eq(
            &full.dep_layer.dep_nodes,
            &full_again.dep_layer.dep_nodes
        ));
        drop((light, full, light_again, full_again));
        assert_eq!(cache.live_entries(), 0);

        // Two FULL builds at once, on an empty cache.
        let (one, two) = std::thread::scope(|s| {
            let a = s.spawn(|| ctx(&fx.root_a, BuildProfile::FULL));
            let b = s.spawn(|| ctx(&fx.root_b, BuildProfile::FULL));
            (a.join().unwrap(), b.join().unwrap())
        });
        bodies(&one);
        bodies(&two);
    }

    /// Review Focus 5: symbols mode has no dependency source, so nothing is
    /// summarized and `dep_meta` is empty; the nodes are the `FULL` build's.
    #[test]
    fn symbols_mode_summarizes_nothing_and_matches_full() {
        use crate::program::dep_summary::parse_for_build;
        use crate::program::resolve::full::build_context_with;
        let fx = two_roots_one_alpackages();
        let light = build_context_with(
            &fx.root_a,
            DependencySource::Symbols,
            BuildProfile::LIGHT,
            &DepCache::default(),
        )
        .expect("light");
        let full = build_context_with(
            &fx.root_a,
            DependencySource::Symbols,
            BuildProfile::FULL,
            &DepCache::default(),
        )
        .expect("full");
        let parse = parse_for_build(&light.snap, BuildProfile::LIGHT, false);
        assert!(parse.workspace.is_some());
        assert!(parse.dep_summaries.is_empty(), "no dependency source");
        assert!(light.dep_layer.dep_nodes.dep_meta.is_empty());
        assert!(
            !light.graph().routines.shared().is_empty(),
            "precondition: the dependency's ABI nodes load"
        );
        let nodes = |c: &crate::program::resolve::full::ProgramContext| {
            (
                c.graph().objects.iter().cloned().collect::<Vec<_>>(),
                c.graph().routines.iter().cloned().collect::<Vec<_>>(),
            )
        };
        assert_eq!(nodes(&light), nodes(&full));
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

    /// Spec §5 check 1 on the real workspace: `LIGHT` (the LSP) and `FULL`
    /// give the same program report and the same LSP answers on CDO, so a
    /// shape only Base Application has cannot break the LIGHT path unseen.
    /// Gated on `CDO_WS`; `scripts/cdo-gate` runs it via `--lib`.
    #[test]
    fn cdo_light_and_full_profiles_give_the_same_report_and_answers() {
        use crate::program::dep_summary::tests::report_text;
        use crate::program::resolve::full::build_context_with;
        let Some(ws) = super::cdo::cdo_ws_or_enforce() else {
            return;
        };
        // One profile at a time: each CDO build is dropped before the next
        // starts, and only its text projection is kept.
        let project = |profile: BuildProfile| {
            let ctx = build_context_with(
                &ws,
                DependencySource::Embedded,
                profile,
                &DepCache::default(),
            )
            .expect("CDO context");
            let has_bodies = ctx
                .dep_bodies()
                .is_some_and(|b| b.iter().any(|u| !u.files.is_empty()));
            let report = report_text(&ctx);
            let (snap, _) = LspSnapshot::from_context(ctx, &ws);
            assert!(
                !snap.dep_meta.is_empty() && !snap.dep_lines.is_empty(),
                "precondition: {profile:?} has a dependency tier"
            );
            (has_bodies, report, answers(&snap))
        };
        let light = project(BuildProfile::LIGHT);
        let full = project(BuildProfile::FULL);
        assert!(!light.0, "precondition: LIGHT keeps no dependency bodies");
        assert!(full.0, "precondition: FULL keeps CDO's dependency bodies");
        assert_same_text("program report", &light.1, &full.1);
        assert_same_text("LSP answers", &light.2, &full.2);
    }

    /// `assert_eq!` on texts this large would print hundreds of MiB; name
    /// the first differing line instead.
    fn assert_same_text(what: &str, light: &str, full: &str) {
        if light == full {
            return;
        }
        let (l, f): (Vec<_>, Vec<_>) = (light.lines().collect(), full.lines().collect());
        let at = l
            .iter()
            .zip(&f)
            .position(|(a, b)| a != b)
            .unwrap_or(l.len().min(f.len()));
        panic!(
            "{what} differ at line {at} (LIGHT {} lines, FULL {} lines)\nLIGHT: {:?}\nFULL:  {:?}",
            l.len(),
            f.len(),
            l.get(at),
            f.get(at)
        );
    }
}
