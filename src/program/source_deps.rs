//! Source-folder dependencies (G4): dependency apps given as a folder of `.al`
//! source with its own `app.json`, not as a compiled `.app` package.
//!
//! The workspace names them in [`CONFIG_FILE`] at its root:
//!
//! ```json
//! { "sources": ["../deps/Base Application", "/abs/path/System Application"] }
//! ```
//!
//! A relative path is relative to the workspace root. Each folder must hold an
//! `app.json`; ALL its `.al` files are read ([`NestedApps::Walk`], as the
//! workspace's own walk), including folders under it with an `app.json` of their
//! own: in BC.History those are the app's modules (110 in System Application, 39
//! in Base Application), compiled into the one app. A test app is a separate
//! folder (BC.History: `System Application/Test/…`, beside the app's
//! `Source/…`), so naming the app's own folder leaves it out (G21: skipping the
//! nested folders, as G4 first did, dropped most of Base and System Application).
//! An app with no `.al` file (an umbrella app such as BC's `Application`) is
//! kept: its `app.json` still declares dependencies.
//!
//! A configured folder always supplies its app: a `.app` of the same app id in
//! the package cache is not loaded (`SnapshotBuilder`). The user named the
//! folder, so silently preferring the package would ignore the configuration.
//!
//! Everything is read once, into [`crate::program::inputs::WorkspaceInputs`], so
//! the build, the fingerprint rows and the incremental host all see the same
//! bytes. Any problem (unreadable or invalid config, a missing folder, no or
//! invalid `app.json`, an unreadable `.al` file, two folders with one app id)
//! makes the whole read an `Err` with the reason: the build then fails closed
//! with it rather than analysing without a dependency the user asked for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::snapshot::embedded::SourceFile;
use crate::source_text::{NestedApps, discover_al_files, read_al_source};

/// The config file's name at the workspace root.
pub const CONFIG_FILE: &str = "al-sem.dependencies.json";

/// The config file's shape.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    sources: Vec<String>,
}

/// One configured source folder, read.
pub struct SourceDependency {
    /// The path as the config spells it (messages and fingerprint keys).
    pub spelled: String,
    /// `spelled` against the workspace root, not canonicalized.
    pub root: PathBuf,
    /// Its `app.json`, parsed (a leading BOM is accepted).
    pub app_json: serde_json::Value,
    /// Its `app.json` bytes as read.
    pub app_json_bytes: Vec<u8>,
    /// Its `.al` files, `rel_posix` under `root`, sorted by the walk.
    pub files: Arc<Vec<SourceFile>>,
    /// Each file's lossless key ([`crate::source_text::rel_key`]), in
    /// `files` order: the resident manifest's `src:` keys (TLS-30).
    pub file_keys: Vec<String>,
    /// blake3 over the `app.json` bytes and every file's path and text, each
    /// length-prefixed, in walk order. Unlike a `.app`'s source hash it
    /// covers the paths, so a rename is a change.
    pub content_hash: String,
}

impl SourceDependency {
    /// The app id its `app.json` declares, lowercased (`""` when absent).
    pub fn guid(&self) -> String {
        self.str_field("id").to_ascii_lowercase()
    }

    /// A string field of its `app.json` (`""` when absent).
    pub fn str_field(&self, key: &str) -> String {
        self.app_json
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

/// What the workspace configures: `Ok(empty)` when there is no config file.
pub type SourceDependencies = Result<Vec<SourceDependency>, String>;

/// Read the workspace's [`CONFIG_FILE`] and every folder it names.
pub fn read(workspace: &Path) -> SourceDependencies {
    read_with(workspace, &[])
}

/// [`read`], then the `injected` folders (a test universe's production app,
/// agent-tasks design §4.3: named by the universe, not by a file in the
/// user's test app). See [`walk_with`].
pub fn read_with(workspace: &Path, injected: &[PathBuf]) -> SourceDependencies {
    let walked = walk_with(workspace, injected, &mut |_, _, path| {
        read_al_source(path).map_err(|e| e.to_string())
    })?;
    Ok(walked.into_iter().map(Walked::into_dependency).collect())
}

/// How an injected folder is spelled: its path as given (absolute, from the
/// universe descriptor). The manifest keys and messages use it.
#[must_use]
pub fn injected_spelling(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// One configured folder as [`walk`] found it: its `app.json` (read and
/// parsed) and, for each `.al` file in walk order, its `rel_posix` under
/// `root`, its path and what the walk's per-file step made of it.
pub struct Walked<T> {
    pub spelled: String,
    pub root: PathBuf,
    pub app_json: serde_json::Value,
    pub app_json_bytes: Vec<u8>,
    pub files: Vec<(String, PathBuf, T)>,
}

impl Walked<String> {
    /// The dependency these texts make ([`read`]).
    fn into_dependency(self) -> SourceDependency {
        let mut hasher = blake3::Hasher::new();
        let mut put = |b: &[u8]| {
            hasher.update(&(b.len() as u64).to_le_bytes());
            hasher.update(b);
        };
        put(&self.app_json_bytes);
        let mut files = Vec::with_capacity(self.files.len());
        let mut file_keys = Vec::with_capacity(self.files.len());
        for (rel, abs, text) in self.files {
            put(rel.as_bytes());
            put(text.as_bytes());
            file_keys.push(crate::source_text::rel_key(&rel, &abs));
            files.push(SourceFile {
                virtual_path: rel,
                text: text.into(),
            });
        }
        SourceDependency {
            spelled: self.spelled,
            root: self.root,
            app_json: self.app_json,
            app_json_bytes: self.app_json_bytes,
            files: Arc::new(files),
            file_keys,
            content_hash: hasher.finalize().to_hex().to_string(),
        }
    }
}

/// The config and every folder it names, with the checks [`read`] makes
/// (in its order, with its messages), each `.al` file handed to `per_file`
/// (`(spelled, rel_posix, path)`) instead of read: [`read`] reads it; the
/// resident manifest's freshness check (agent-tasks design §2.2) reuses a
/// file's earlier hash when its stat is unchanged, so only a changed file is
/// read. A `per_file` error is the file's read error.
pub fn walk<T>(
    workspace: &Path,
    per_file: &mut dyn FnMut(&str, &str, &Path) -> Result<T, String>,
) -> Result<Vec<Walked<T>>, String> {
    walk_with(workspace, &[], per_file)
}

/// [`walk`], then each `injected` folder (spelled by [`injected_spelling`]),
/// with the same checks. An injected folder whose app id a configured folder
/// already supplies is left out: the user's own configuration names that app
/// (the design's scratch test app lists the production folder itself).
pub fn walk_with<T>(
    workspace: &Path,
    injected: &[PathBuf],
    per_file: &mut dyn FnMut(&str, &str, &Path) -> Result<T, String>,
) -> Result<Vec<Walked<T>>, String> {
    let config_path = workspace.join(CONFIG_FILE);
    let sources = match std::fs::read(&config_path) {
        Ok(bytes) => {
            let text =
                std::str::from_utf8(&bytes).map_err(|_| format!("{CONFIG_FILE}: not UTF-8"))?;
            let config: Config =
                serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
                    .map_err(|e| format!("{CONFIG_FILE}: {e}"))?;
            config.sources
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{CONFIG_FILE}: {e}")),
    };
    let mut deps = walk_sources(workspace, sources, per_file)?;
    for p in injected {
        let spelled = injected_spelling(p);
        let dep = walk_one(workspace, &spelled, per_file)
            .map_err(|e| format!("injected source \"{spelled}\": {e}"))?;
        let id = str_id(&dep.app_json);
        if !id.is_empty() && deps.iter().any(|d| str_id(&d.app_json) == id) {
            continue;
        }
        deps.push(dep);
    }
    Ok(deps)
}

fn str_id(v: &serde_json::Value) -> String {
    v.get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn walk_sources<T>(
    workspace: &Path,
    sources: Vec<String>,
    per_file: &mut dyn FnMut(&str, &str, &Path) -> Result<T, String>,
) -> Result<Vec<Walked<T>>, String> {
    let config = Config { sources };
    let mut deps: Vec<Walked<T>> = Vec::with_capacity(config.sources.len());
    let guid = |v: &serde_json::Value| {
        v.get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    for spelled in config.sources {
        let dep = walk_one(workspace, &spelled, per_file)
            .map_err(|e| format!("{CONFIG_FILE}: source \"{spelled}\": {e}"))?;
        let id = guid(&dep.app_json);
        if let Some(other) = deps
            .iter()
            .find(|d| !id.is_empty() && guid(&d.app_json) == id)
        {
            return Err(format!(
                "{CONFIG_FILE}: sources \"{}\" and \"{spelled}\" are both app {id}",
                other.spelled
            ));
        }
        deps.push(dep);
    }
    Ok(deps)
}

/// The folders the config names, against the workspace root, without reading
/// them (`alsem batch`'s write check); empty when there is no config or it does
/// not load (the build then fails with the reason).
pub fn configured_roots(workspace: &Path) -> Vec<PathBuf> {
    let Ok(bytes) = std::fs::read(workspace.join(CONFIG_FILE)) else {
        return Vec::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    serde_json::from_str::<Config>(text.strip_prefix('\u{feff}').unwrap_or(text))
        .map(|c| c.sources.iter().map(|s| workspace.join(s)).collect())
        .unwrap_or_default()
}

fn walk_one<T>(
    workspace: &Path,
    spelled: &str,
    per_file: &mut dyn FnMut(&str, &str, &Path) -> Result<T, String>,
) -> Result<Walked<T>, String> {
    let root = workspace.join(spelled);
    if !root.is_dir() {
        return Err(format!("{} is not a folder", root.display()));
    }
    // A folder inside the workspace is walked as workspace source already
    // (nested apps included), so its files would load twice.
    if let (Ok(ws), Ok(r)) = (workspace.canonicalize(), root.canonicalize())
        && r.starts_with(&ws)
    {
        return Err(format!(
            "{} is inside the workspace, whose source walk already takes it",
            root.display()
        ));
    }
    let app_json_path = root.join("app.json");
    let app_json_bytes = std::fs::read(&app_json_path)
        .map_err(|e| format!("read {}: {e}", app_json_path.display()))?;
    let text = std::str::from_utf8(&app_json_bytes)
        .map_err(|_| format!("{}: not UTF-8", app_json_path.display()))?;
    let app_json = crate::source_text::parse_app_json(text)
        .map_err(|e| format!("parse {}: {e}", app_json_path.display()))?;
    if app_json
        .get("id")
        .and_then(|v| v.as_str())
        .is_none_or(str::is_empty)
    {
        return Err(format!("{} has no \"id\"", app_json_path.display()));
    }
    let found = discover_al_files(&root, NestedApps::Walk)
        .map_err(|e| format!("walking {}: {e}", root.display()))?;
    let mut files = Vec::with_capacity(found.len());
    for f in found {
        let made = per_file(spelled, &f.rel_posix, &f.abs_path)
            .map_err(|e| format!("reading {}: {e}", f.abs_path.display()))?;
        files.push((f.rel_posix, f.abs_path, made));
    }
    Ok(Walked {
        spelled: spelled.to_string(),
        root,
        app_json,
        app_json_bytes,
        files,
    })
}

/// One digest over every configured dependency (empty string for none): the
/// spelled path and content hash of each, in config order.
pub fn digest(deps: &[SourceDependency]) -> String {
    if deps.is_empty() {
        return String::new();
    }
    let mut hasher = blake3::Hasher::new();
    for d in deps {
        hasher.update(&(d.spelled.len() as u64).to_le_bytes());
        hasher.update(d.spelled.as_bytes());
        hasher.update(d.content_hash.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use crate::program::inputs::{WorkspaceInputs, WorkspaceNeeds};
    use std::path::Path;

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    /// The `dep-source` fingerprint row of `ws`, read as a fingerprinting
    /// command reads it.
    fn row(ws: &Path) -> String {
        let needs = WorkspaceNeeds {
            fingerprint_rows: true,
            ..WorkspaceNeeds::default()
        };
        let inputs = WorkspaceInputs::read(ws, &needs);
        let rows = inputs.snapshot_inputs.as_ref().expect("fingerprint rows");
        let dep: Vec<_> = rows.iter().filter(|r| r.kind == "dep-source").collect();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(dep.len(), 1, "one configured folder, one row: {kinds:?}");
        assert_eq!(dep[0].path, "../dep");
        dep[0].content_hash.clone()
    }

    /// A source-folder dependency is a fingerprint input: editing a file and
    /// renaming one (same texts, so a texts-only hash would not move) both
    /// change the row, so a snapshot or digest of the workspace changes too.
    #[test]
    fn the_fingerprint_row_follows_text_and_paths() {
        let t = tempfile::tempdir().unwrap();
        let ws = t.path().join("ws");
        write(
            &ws.join("app.json"),
            r#"{"id":"aaaaaaaa-0000-4000-8000-000000000001","name":"W","publisher":"t","version":"1.0.0.0"}"#,
        );
        write(&ws.join(super::CONFIG_FILE), r#"{ "sources": ["../dep"] }"#);
        write(
            &t.path().join("dep/app.json"),
            r#"{"id":"dddddddd-0000-4000-8000-000000000001","name":"D","publisher":"t","version":"1.0.0.0"}"#,
        );
        let al = t.path().join("dep/A.Codeunit.al");
        write(&al, "codeunit 50200 \"D A\" { }\n");

        let first = row(&ws);
        assert_eq!(row(&ws), first, "precondition: the row is deterministic");
        write(&al, "codeunit 50200 \"D A\" { procedure X() begin end; }\n");
        let edited = row(&ws);
        assert_ne!(edited, first, "an edited file changes the row");
        std::fs::rename(&al, t.path().join("dep/B.Codeunit.al")).unwrap();
        assert_ne!(row(&ws), edited, "a renamed file changes the row");
    }
}
