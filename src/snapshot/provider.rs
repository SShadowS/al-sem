//! Source providers: acquire per-app source by the best available means.

use crate::dependencies::AppFileStamp;
use crate::program::dep_cache::DepCache;
use crate::snapshot::cache::cached_source;
use crate::snapshot::embedded::SourceFile;
use crate::snapshot::identity::{AppId, TrustTier};
use crate::snapshot::verify::{IdentityCheck, verify_local_source};
use crate::source_text::{NestedApps, discover_al_files, read_al_source};
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

/// A resolved set of source files for one app, with its trust tier + hash.
#[derive(Clone, Debug)]
pub struct SourceRoot {
    /// Behind one `Arc` so roots can share the texts and a cache can hold
    /// them weakly through this small owner (see `DepCache::source`).
    pub files: Arc<Vec<SourceFile>>,
    pub tier: TrustTier,
    pub content_hash: String,
}

/// Acquires source for an app. Returns `Ok(None)` when this provider cannot
/// serve the app (caller falls through to the next provider).
pub trait SourceProvider {
    fn try_provide(&self, app: &AppId) -> Result<Option<SourceRoot>>;
}

/// Read `root`'s `.al` source (the shared walk, `crate::source_text`, nested
/// apps included), sorted + content-hashed for determinism. `Ok(None)` if no
/// `.al` files.
fn walk_al_source(root: &std::path::Path, tier: TrustTier) -> Result<Option<SourceRoot>> {
    let found = discover_al_files(root, NestedApps::Walk)
        .with_context(|| format!("walking source {}", root.display()))?;
    let mut files: Vec<SourceFile> = Vec::with_capacity(found.len());
    for f in found {
        let text = read_al_source(&f.abs_path)
            .with_context(|| format!("reading source {}", f.abs_path.display()))?;
        files.push(SourceFile {
            virtual_path: f.rel_posix,
            text: text.into(),
        });
    }
    if files.is_empty() {
        return Ok(None);
    }
    // Hash over sorted file texts (the walk sorts) for determinism.
    let mut hasher = blake3::Hasher::new();
    for f in &files {
        hasher.update(f.text.as_bytes());
    }
    let content_hash = hasher.finalize().to_hex().to_string();
    Ok(Some(SourceRoot {
        files: Arc::new(files),
        tier,
        content_hash,
    }))
}

/// The app under development — source on disk is truth.
pub struct WorkspaceProvider {
    pub root: PathBuf,
}

impl SourceProvider for WorkspaceProvider {
    fn try_provide(&self, _app: &AppId) -> Result<Option<SourceRoot>> {
        walk_al_source(&self.root, TrustTier::Workspace)
    }
}

/// Embedded ShowMyCode source inside a dependency `.app`.
pub struct EmbeddedAppProvider<'a> {
    pub app_path: PathBuf,
    /// The stamp taken BEFORE the `.app` was read (never a fresh stat).
    pub stamp: Option<AppFileStamp>,
    /// Shares the extracted texts with every other root loading this `.app`.
    pub cache: &'a DepCache,
    /// Answer a known source without its text (see [`DepCache::source`]).
    pub defer_text: bool,
}

impl SourceProvider for EmbeddedAppProvider<'_> {
    fn try_provide(&self, _app: &AppId) -> Result<Option<SourceRoot>> {
        self.cache
            .source(&self.app_path, self.stamp, self.defer_text, || {
                #[cfg(test)]
                extract_log::record(&self.app_path);
                let (files, content_hash) = cached_source(&self.app_path)?;
                if files.is_empty() {
                    return Ok(None); // symbol-only app
                }
                Ok(Some(SourceRoot {
                    files: Arc::new(files),
                    tier: TrustTier::EmbeddedSource,
                    content_hash,
                }))
            })
    }
}

/// Every embedded-source extraction (`cached_source` call), by `.app` path.
#[cfg(test)]
pub(crate) mod extract_log {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

    static LOG: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(crate) fn record(path: &Path) {
        LOG.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(path.to_path_buf());
    }

    /// How many extractions so far were of an `.app` under (or at) `path`
    /// (tests use their own temp dir, so parallel tests do not see each other).
    pub(crate) fn extractions_under(path: &Path) -> usize {
        LOG.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|p| p.starts_with(path))
            .count()
    }
}

/// No source available — marks the app symbol-only (honest boundary).
pub struct SymbolOnlyProvider;

impl SourceProvider for SymbolOnlyProvider {
    fn try_provide(&self, _app: &AppId) -> Result<Option<SourceRoot>> {
        Ok(None)
    }
}

/// A local source repository configured to represent a specific app.
///
/// Walks `root` for `.al` files (same rules as `WorkspaceProvider`) but
/// applies identity verification: if the configured `app` identity does not
/// match the requested app, fails closed and returns `Ok(None)`.
pub struct LocalRepoProvider {
    /// The identity this local repo claims to represent.
    pub app: AppId,
    /// Root directory of the local source checkout.
    pub root: PathBuf,
}

impl SourceProvider for LocalRepoProvider {
    fn try_provide(&self, requested: &AppId) -> Result<Option<SourceRoot>> {
        // Walk first; identity check runs after because verify_local_source is
        // forward-designed to also corroborate against the produced SourceRoot
        // (hash/commit) in a later task.
        let Some(source_root) = walk_al_source(&self.root, TrustTier::LocalSourceApproximate)?
        else {
            return Ok(None);
        };
        // Fail closed on identity mismatch.
        if let IdentityCheck::Mismatch(_) =
            verify_local_source(requested, &source_root, Some(&self.app))
        {
            return Ok(None);
        }
        Ok(Some(source_root))
    }
}

/// First provider (in priority order) that yields source wins.
pub fn select_source(
    app: &AppId,
    providers: &[Box<dyn SourceProvider + '_>],
) -> Result<Option<SourceRoot>> {
    for p in providers {
        if let Some(root) = p.try_provide(app)? {
            return Ok(Some(root));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::identity::{AppId, TrustTier};

    fn dummy_app() -> AppId {
        AppId {
            guid: "g".into(),
            name: "Continia Document Output".into(),
            publisher: "Continia Software".into(),
            version: "29.0.0.0".into(),
        }
    }

    #[test]
    fn embedded_provider_yields_source_with_tier() {
        let Some(app_path) = std::env::var_os("CDO_APP")
            .map(std::path::PathBuf::from)
            .filter(|p| p.exists())
        else {
            return;
        };
        let p = EmbeddedAppProvider {
            app_path,
            stamp: None,
            cache: &DepCache::default(),
            defer_text: false,
        };
        let root = p.try_provide(&dummy_app()).unwrap().expect("source");
        assert_eq!(root.tier, TrustTier::EmbeddedSource);
        assert!(root.files.len() > 100);
        assert_eq!(root.content_hash.len(), 64);
    }

    #[test]
    fn workspace_provider_returns_none_for_empty_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = WorkspaceProvider {
            root: dir.path().to_path_buf(),
        };
        let result = p.try_provide(&dummy_app()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn workspace_provider_finds_al_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Foo.al"), "codeunit 1 Foo { }").unwrap();
        let p = WorkspaceProvider {
            root: dir.path().to_path_buf(),
        };
        let root = p.try_provide(&dummy_app()).unwrap().expect("source");
        assert_eq!(root.tier, TrustTier::Workspace);
        assert_eq!(root.files.len(), 1);
        assert_eq!(root.files[0].virtual_path, "Foo.al");
        assert_eq!(root.content_hash.len(), 64);
    }

    #[test]
    fn workspace_provider_skips_alpackages() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".alpackages")).unwrap();
        std::fs::write(
            dir.path().join(".alpackages").join("Dep.al"),
            "codeunit 2 Dep { }",
        )
        .unwrap();
        let p = WorkspaceProvider {
            root: dir.path().to_path_buf(),
        };
        let result = p.try_provide(&dummy_app()).unwrap();
        assert!(result.is_none(), "should skip .alpackages");
    }

    #[test]
    fn select_source_first_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Foo.al"), "codeunit 1 Foo { }").unwrap();
        let providers: Vec<Box<dyn SourceProvider>> = vec![
            Box::new(SymbolOnlyProvider),
            Box::new(WorkspaceProvider {
                root: dir.path().to_path_buf(),
            }),
        ];
        let result = select_source(&dummy_app(), &providers).unwrap();
        assert!(
            result.is_some(),
            "SymbolOnlyProvider yields None; WorkspaceProvider should win"
        );
        assert_eq!(result.unwrap().tier, TrustTier::Workspace);
    }

    #[test]
    fn local_repo_mismatch_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("Foo.al"), "codeunit 1 Foo { }").unwrap();
        let p = LocalRepoProvider {
            app: AppId {
                guid: "g".into(),
                name: "X".into(),
                publisher: "P".into(),
                version: "29.0.0.0".into(),
            },
            root: dir.path().to_path_buf(),
        };
        // Version mismatch → fail closed.
        let mismatched = AppId {
            guid: "g".into(),
            name: "X".into(),
            publisher: "P".into(),
            version: "28.0.0.0".into(),
        };
        assert!(
            p.try_provide(&mismatched).unwrap().is_none(),
            "version mismatch should return None"
        );
        // Version match → Some with correct tier.
        let matched = AppId {
            guid: "g".into(),
            name: "X".into(),
            publisher: "P".into(),
            version: "29.0.0.0".into(),
        };
        let result = p.try_provide(&matched).unwrap();
        assert!(result.is_some(), "matching version should return Some");
        assert_eq!(result.unwrap().tier, TrustTier::LocalSourceApproximate);
    }
}
