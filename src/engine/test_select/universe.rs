//! **Test universes** (agent-tasks design §4.3): a program rooted at a
//! sibling test app, with the workspace (production) app loaded as an
//! in-memory source dependency, so tests and the code they call are in one
//! graph.
//!
//! A universe is described by an immutable [`TestUniverse`]; its key
//! (`tests:<16 hex>`, [`TestUniverse::key`]) names it in the pool, the
//! session's descriptor and the answer. Its reads take the production root
//! as an injected source folder and the production root's `.alpackages` (and
//! ancestors) as an extra package root ([`TestUniverse::injected`]): nothing
//! is written into the user's test app.
//!
//! **Production stamp** ([`production_source_stamp`]): blake3 over the sorted
//! `(relative path, content hash)` of the production app's `.al` files and its
//! `app.json`, the same function on both sides: the production universe over
//! its workspace read, the test universe over the injected source folder it
//! consumed. A tests answer compares the two: different stamps mean the two
//! universes were built from different production bytes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::program::inputs::{Injected, WorkspaceInputs};

/// One test universe (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestUniverse {
    /// The test app's folder (absolute).
    pub test_root: PathBuf,
    /// Its `app.json` id, lowercased.
    pub test_app_id: String,
    /// The production (workspace) app's folder (absolute).
    pub production_root: PathBuf,
    /// Its `app.json` id, lowercased.
    pub production_app_id: String,
    /// The production app as an in-memory source dependency, as
    /// `al-sem.dependencies.json` would name it.
    pub source_overrides: Vec<PathBuf>,
    /// Package discovery after the test app's own `.alpackages` and
    /// ancestors: the production root's (CDO's layout: only
    /// `Cloud/.alpackages` exists).
    pub package_policy: Vec<PathBuf>,
    /// How the app came into scope (`call`, `configured`, `auto`).
    pub config: String,
}

impl TestUniverse {
    /// The universe of the test app at `test_root` (id `test_app_id`) over the
    /// production app at `production_root` (id `production_app_id`).
    #[must_use]
    pub fn new(
        test_root: &Path,
        test_app_id: &str,
        production_root: &Path,
        production_app_id: &str,
        config: &str,
    ) -> TestUniverse {
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let production_root = abs(production_root);
        TestUniverse {
            test_root: abs(test_root),
            test_app_id: test_app_id.to_ascii_lowercase(),
            source_overrides: vec![production_root.clone()],
            package_policy: vec![production_root.clone()],
            production_root,
            production_app_id: production_app_id.to_ascii_lowercase(),
            config: config.to_string(),
        }
    }

    /// The workspace's own tests (§4.2 rule 1): the production program itself,
    /// nothing injected; its key is `workspace`.
    #[must_use]
    pub fn workspace(root: &Path, app_id: &str) -> TestUniverse {
        let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        TestUniverse {
            test_root: abs.clone(),
            test_app_id: app_id.to_ascii_lowercase(),
            production_root: abs,
            production_app_id: app_id.to_ascii_lowercase(),
            source_overrides: Vec::new(),
            package_policy: Vec::new(),
            config: "workspace".into(),
        }
    }

    /// Whether this is the workspace's own program ([`Self::workspace`]).
    #[must_use]
    pub fn is_workspace(&self) -> bool {
        self.source_overrides.is_empty() && self.test_root == self.production_root
    }

    /// The universe key: `tests:` and the first 16 hex digits of blake3 over
    /// the descriptor's canonical JSON (`workspace` for the workspace's own).
    /// `config` is left out: how an app was found does not change what is
    /// built.
    #[must_use]
    pub fn key(&self) -> String {
        if self.is_workspace() {
            return "workspace".into();
        }
        let mut d = self.clone();
        d.config.clear();
        let bytes = serde_json::to_vec(&d).expect("a descriptor serializes");
        format!("tests:{}", &blake3::hash(&bytes).to_hex()[..16])
    }

    /// What its reads take besides the test app's own files.
    #[must_use]
    pub fn injected(&self) -> Injected {
        Injected {
            sources: self.source_overrides.clone(),
            package_roots: self.package_policy.clone(),
        }
    }
}

/// The production stamp over `(relative path, text)` pairs and the
/// `app.json` bytes (see the module doc).
fn stamp_of<'a>(app_json: &[u8], files: impl Iterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut rows: Vec<(&str, blake3::Hash)> = files.map(|(p, t)| (p, blake3::hash(t))).collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    let mut h = blake3::Hasher::new();
    let mut put = |b: &[u8]| {
        h.update(&(b.len() as u64).to_le_bytes());
        h.update(b);
    };
    put(blake3::hash(app_json).as_bytes());
    for (p, t) in rows {
        put(p.as_bytes());
        put(t.as_bytes());
    }
    format!("prod-v1:{}", &h.finalize().to_hex()[..32])
}

/// The production stamp of a read: of the workspace app itself when `of_app`
/// is its own id (the production universe), else of the source dependency
/// whose `app.json` id is `of_app` (the test universe's injected production
/// app). `None` when the read has no such app (or an unreadable file).
#[must_use]
pub fn production_source_stamp(inputs: &WorkspaceInputs, of_app: &str) -> Option<String> {
    let own = inputs
        .app_json_value
        .as_ref()
        .and_then(|v| v.as_ref().ok())
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .is_some_and(|id| id.eq_ignore_ascii_case(of_app));
    if own {
        let app_json = inputs.app_json.bytes.as_ref().ok()?;
        let mut files = Vec::with_capacity(inputs.al_files.len());
        for f in &inputs.al_files {
            files.push((f.rel_posix.as_str(), f.text.as_ref().ok()?.as_bytes()));
        }
        return Some(stamp_of(app_json, files.into_iter()));
    }
    let deps = inputs.source_dependencies.as_ref().ok()?;
    let d = deps
        .iter()
        .find(|d| d.guid().eq_ignore_ascii_case(of_app))?;
    Some(stamp_of(
        &d.app_json_bytes,
        d.files
            .iter()
            .map(|f| (f.virtual_path.as_str(), f.text.as_bytes())),
    ))
}

#[cfg(test)]
#[path = "universe_tests.rs"]
mod tests;
