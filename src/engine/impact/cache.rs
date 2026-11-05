//! **The impact-base cache** (agent-tasks design §10.7, astra a13): a
//! bounded namespace of its own, `~/.al-sem/cache/impact-base/v1/`
//! (`state_paths`), never touched by `cache prune` (`cache_prune.rs` handles
//! only the dependency JSON artifacts at the cache folder's top level and
//! skips folders).
//!
//! - **Key**: blake3 over the base commit's tree id of the workspace folder,
//!   the package files' digest (every path and blake3), the source
//!   dependencies' content hashes, the analysis options, the engine version,
//!   the grammar commit and the summary schema ([`KeyParts`]).
//! - **Entry**: one file per base, `<key>.ib`: a header line (schema
//!   version, key, the payload's blake3 and decoded size) then the payload,
//!   gzip-compressed JSON (the base's per-root summaries, contract snapshot,
//!   environment, and the roots each diff mapped to). Written to a temporary
//!   name and renamed: a reader sees a whole entry or none. Two writers of
//!   one key both write the same content; the last rename wins.
//! - **Integrity**: a reader checks the header, the sizes and the payload
//!   hash; a bad entry is deleted (and the base rebuilt by the caller).
//! - **Limits** ([`Limits`]): at most 10 entries (owner, §13.2), 64 MB
//!   compressed and 256 MB decoded per entry (refused above), 256 MB for the
//!   folder, 30 days of age; least recently READ goes first (a read touches
//!   the file's modification time).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

/// The entry header's schema.
pub const ENTRY_SCHEMA: &str = "al-sem.impact-base";
/// The entry format's version (the folder name's `v1` too).
pub const ENTRY_VERSION: u32 = 1;
/// The namespace under `~/.al-sem/`.
pub const NAMESPACE: &str = "cache/impact-base/v1";
/// Overrides the folder (tests, probes).
pub const DIR_ENV: &str = "AL_SEM_IMPACT_BASE_CACHE";
const EXT: &str = "ib";

/// The cache's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_entries: usize,
    pub max_compressed: u64,
    pub max_decoded: u64,
    pub max_total: u64,
    pub max_age: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_entries: 10,
            max_compressed: 64 * 1024 * 1024,
            max_decoded: 256 * 1024 * 1024,
            max_total: 256 * 1024 * 1024,
            max_age: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// What a cache key is made of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyParts {
    pub tree: String,
    pub packages_digest: String,
    pub source_deps: Vec<(String, String)>,
    /// Universe and the other analysis options.
    pub options: String,
}

impl KeyParts {
    /// The key: blake3 hex over the parts, the engine version, the grammar
    /// commit and the summary schema, each length-prefixed.
    #[must_use]
    pub fn key(&self) -> String {
        let mut h = blake3::Hasher::new();
        let mut part = |s: &str| {
            h.update(&(s.len() as u64).to_le_bytes());
            h.update(s.as_bytes());
        };
        part(ENTRY_SCHEMA);
        part(&ENTRY_VERSION.to_string());
        part(&self.tree);
        part(&self.packages_digest);
        for (p, c) in &self.source_deps {
            part(p);
            part(c);
        }
        part(&self.options);
        part(&crate::engine::gate::version::driver_version());
        part(env!("CARGO_PKG_VERSION"));
        part(crate::engine::gate::cache_prune::CACHE_VERSION_GRAMMAR);
        part(&super::keys::SUMMARY_SCHEMA.to_string());
        h.finalize().to_hex().to_string()
    }

    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({"tree": self.tree, "packagesDigest": self.packages_digest,
               "sourceDependencies": self.source_deps, "options": self.options})
    }
}

/// What a read found.
#[derive(Debug, PartialEq)]
pub enum Lookup {
    Hit(Value),
    Miss,
    /// The entry was bad (and deleted): why.
    Rebuilt(String),
}

/// The cache folder.
#[derive(Clone, Debug)]
pub struct Cache {
    pub dir: PathBuf,
    pub limits: Limits,
}

impl Cache {
    /// The default folder: [`DIR_ENV`] when set, else `~/.al-sem/` +
    /// [`NAMESPACE`].
    #[must_use]
    pub fn default_dir() -> Option<PathBuf> {
        match std::env::var_os(DIR_ENV) {
            Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
            _ => crate::state_paths::state_path(NAMESPACE),
        }
    }

    #[must_use]
    pub fn new(dir: PathBuf, limits: Limits) -> Cache {
        Cache { dir, limits }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.{EXT}"))
    }

    /// Read `key`'s entry (see the module doc).
    pub fn read(&self, key: &str) -> Lookup {
        let path = self.path(key);
        let bytes = match std::fs::metadata(&path) {
            Err(_) => return Lookup::Miss,
            Ok(m) if m.len() > self.limits.max_compressed => {
                let _ = std::fs::remove_file(&path);
                return Lookup::Rebuilt(format!(
                    "the entry is {} bytes, over the {} byte cap",
                    m.len(),
                    self.limits.max_compressed
                ));
            }
            Ok(_) => match std::fs::read(&path) {
                Ok(b) => b,
                Err(_) => return Lookup::Miss,
            },
        };
        match self.decode(key, &bytes) {
            Ok(v) => {
                // Least recently read goes first.
                if let Ok(f) = std::fs::File::options().write(true).open(&path) {
                    let _ = f.set_modified(SystemTime::now());
                }
                Lookup::Hit(v)
            }
            Err(why) => {
                let _ = std::fs::remove_file(&path);
                Lookup::Rebuilt(why)
            }
        }
    }

    fn decode(&self, key: &str, bytes: &[u8]) -> Result<Value, String> {
        let nl = bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or("no header line")?;
        let header: Value = serde_json::from_slice(&bytes[..nl])
            .map_err(|e| format!("the header does not read: {e}"))?;
        if header["schema"] != ENTRY_SCHEMA || header["version"] != ENTRY_VERSION {
            return Err("another entry schema or version".into());
        }
        if header["key"] != key {
            return Err("the header names another key".into());
        }
        let decoded = header["decodedBytes"].as_u64().ok_or("no decoded size")?;
        if decoded > self.limits.max_decoded {
            return Err(format!(
                "the payload is {decoded} bytes decoded, over the {} byte cap",
                self.limits.max_decoded
            ));
        }
        let mut payload = Vec::new();
        flate2::read::GzDecoder::new(&bytes[nl + 1..])
            .take(self.limits.max_decoded + 1)
            .read_to_end(&mut payload)
            .map_err(|e| format!("the payload does not inflate: {e}"))?;
        if payload.len() as u64 != decoded {
            return Err("the payload's size is not the header's".into());
        }
        let hash = format!("blake3:{}", blake3::hash(&payload).to_hex());
        if header["payloadHash"] != hash.as_str() {
            return Err("the payload's hash is not the header's".into());
        }
        serde_json::from_slice(&payload).map_err(|e| format!("the payload does not read: {e}"))
    }

    /// Write `key`'s entry (atomic: a temporary name, then a rename), then
    /// prune. Refused when the payload is over a cap.
    pub fn write(&self, key: &str, payload: &Value) -> Result<(), String> {
        let text = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
        if text.len() as u64 > self.limits.max_decoded {
            return Err(format!(
                "not cached: the base's summaries are {} bytes, over the {} byte cap",
                text.len(),
                self.limits.max_decoded
            ));
        }
        let header = json!({
            "schema": ENTRY_SCHEMA, "version": ENTRY_VERSION, "key": key,
            "payloadHash": format!("blake3:{}", blake3::hash(&text).to_hex()),
            "decodedBytes": text.len(),
        });
        let mut bytes = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut gz = flate2::write::GzEncoder::new(bytes, flate2::Compression::new(6));
        gz.write_all(&text).map_err(|e| e.to_string())?;
        let bytes = gz.finish().map_err(|e| e.to_string())?;
        if bytes.len() as u64 > self.limits.max_compressed {
            return Err(format!(
                "not cached: the entry is {} bytes compressed, over the {} byte cap",
                bytes.len(),
                self.limits.max_compressed
            ));
        }
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let mut tmp = tempfile::Builder::new()
            .prefix(&format!("{key}.tmp."))
            .tempfile_in(&self.dir)
            .map_err(|e| format!("{}: {e}", self.dir.display()))?;
        tmp.write_all(&bytes).map_err(|e| e.to_string())?;
        tmp.persist(self.path(key))
            .map_err(|e| format!("the entry could not be published: {e}"))?;
        self.prune(Some(key));
        Ok(())
    }

    /// Every entry: `(path, bytes, modified)`.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        rd.filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == EXT))
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                m.is_file().then(|| {
                    (
                        e.path(),
                        m.len(),
                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    )
                })
            })
            .collect()
    }

    /// Drop entries over the age, then the least recently read until the
    /// count and the folder's bytes are within the limits; `keep` (the entry
    /// just written) goes last. Stale temporary files (an hour old) go too.
    /// Returns the removed paths.
    pub fn prune(&self, keep: Option<&str>) -> Vec<PathBuf> {
        let now = SystemTime::now();
        let mut removed = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.contains(".tmp.")
                    && e.metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| now.duration_since(t).ok())
                        .is_some_and(|age| age > Duration::from_secs(3600))
                {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        let keep_path = keep.map(|k| self.path(k));
        let mut entries = self.entries();
        entries.retain(|(p, _, t)| {
            let old = now
                .duration_since(*t)
                .is_ok_and(|age| age > self.limits.max_age);
            if old && Some(p) != keep_path.as_ref() {
                let _ = std::fs::remove_file(p);
                removed.push(p.clone());
                false
            } else {
                true
            }
        });
        // Oldest read first; the kept entry last.
        entries.sort_by_key(|(p, _, t)| (Some(p) == keep_path.as_ref(), *t));
        let mut total: u64 = entries.iter().map(|(_, b, _)| *b).sum();
        let mut count = entries.len();
        for (p, b, _) in &entries {
            if count <= self.limits.max_entries && total <= self.limits.max_total {
                break;
            }
            if Some(p) == keep_path.as_ref() && count == 1 {
                break;
            }
            let _ = std::fs::remove_file(p);
            removed.push(p.clone());
            count -= 1;
            total -= b;
        }
        removed
    }

    /// The entries' count and bytes (status, tests).
    #[must_use]
    pub fn usage(&self) -> (usize, u64) {
        let e = self.entries();
        (e.len(), e.iter().map(|(_, b, _)| *b).sum())
    }
}

/// `path` relative to nothing: a helper for tests to corrupt an entry.
#[must_use]
pub fn entry_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.{EXT}"))
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
