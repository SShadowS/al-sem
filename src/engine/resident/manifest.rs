//! The input manifest (context-server design §5.2): every input a revision's
//! build read, with the identity of what it found, and the three-way check
//! `M_before == M_consumed == M_after` over ALL of them (extended from the
//! packages' `check_consistent`).
//!
//! **Entries** (key → identity):
//!
//! | Key | Identity |
//! |---|---|
//! | `meta:universe`, `meta:engine`, `meta:grammar` | the build's universe, the engine and grammar versions |
//! | `ws:app.json`, `ws:roots.config.json`, `ws:al-sem.coverage.yaml` | the bytes (length and blake3), absent, or unreadable |
//! | `ws:app.json-paths` | every `app.json` under the root (the nested-app walk), sorted |
//! | `ws:al-walk` | the `.al` walk's listing (every file's root-relative path), or its error |
//! | `al:<rel>` | each `.al` file's decoded text (length and blake3), or its read error |
//! | `ws:source-dependencies` | `al-sem.dependencies.json` read: each dependency's spelled path and content hash (which covers its `app.json` and every file), or the error |
//! | `pkg:list`, `pkg:unreadable-folders` | the package discovery's results (each candidate `.alpackages` folder up to the `.git` boundary, as `discover_app_files` walks them) |
//! | `pkg:<path>` | each `.app`'s bytes (length and blake3), or why it is unreadable |
//!
//! **Three observations.** `M_consumed` is computed from the acquired read
//! itself ([`Manifest::consumed`]): the bytes the build actually used.
//! `M_before` and `M_after` are observations of the disk
//! ([`Manifest::observe`]), which re-run the same discovery the reader runs
//! and hash what they find. An observation may reuse the previous
//! observation's hash for a file whose length and modification time are
//! unchanged (the per-request freshness check, 1-2 ms on CDO); `verify`
//! hashes everything. Stated residual (§5.3): a rewrite that keeps length
//! and mtime, with no change notification, is missed until `verify`.
//!
//! The package-discovery probes that found nothing (an absent ancestor
//! `.alpackages`, the `.git` probes) are not listed one by one: their
//! results are (`pkg:list`, `pkg:unreadable-folders`), so a folder appearing
//! changes an entry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::program::inputs::WorkspaceInputs;

/// The stamp's scheme (§4.7).
pub const STAMP_PREFIX: &str = "al-sem-manifest-v1:";

/// What a manifest entry found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Ident {
    Absent,
    Unreadable(String),
    Bytes { len: u64, blake3: String },
    List(Vec<String>),
    Text(String),
}

impl Ident {
    fn bytes(b: &[u8]) -> Ident {
        Ident::Bytes {
            len: b.len() as u64,
            blake3: blake3::hash(b).to_hex().to_string(),
        }
    }
}

/// A file's length and modification time when it was observed: the cache
/// key of its hash, never part of its identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    len: u64,
    mtime: Option<SystemTime>,
}

fn stat(path: &Path) -> Option<Stat> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stat {
        len: m.len(),
        mtime: m.modified().ok(),
    })
}

/// One manifest (see the module doc).
#[derive(Clone, Debug, Default)]
pub struct Manifest {
    entries: BTreeMap<String, Ident>,
    /// The stat of every file entry this observation hashed or reused.
    stats: BTreeMap<String, Stat>,
}

impl PartialEq for Manifest {
    /// Two manifests name the same inputs: their identities (never their
    /// stats) are equal.
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl Eq for Manifest {}

/// How an observation hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hashing<'a> {
    /// Hash every file.
    All,
    /// Reuse `prev`'s hash for a file whose stat is unchanged, unless it is
    /// in `dirty` (a change notification).
    Cached {
        prev: &'a Manifest,
        dirty: &'a BTreeSet<PathBuf>,
    },
}

fn meta(entries: &mut BTreeMap<String, Ident>, single_app: bool) {
    entries.insert(
        "meta:universe".into(),
        Ident::Text(if single_app { "singleApp" } else { "crossApp" }.into()),
    );
    entries.insert(
        "meta:engine".into(),
        Ident::Text(crate::engine::gate::version::driver_version()),
    );
    entries.insert(
        "meta:grammar".into(),
        Ident::Text(crate::engine::host::grammar_sha().to_string()),
    );
}

fn source_deps_ident(deps: &crate::program::source_deps::SourceDependencies) -> Ident {
    match deps {
        Ok(deps) => Ident::List(
            deps.iter()
                .map(|d| format!("{}\u{0}{}", d.spelled, d.content_hash))
                .collect(),
        ),
        Err(e) => Ident::Unreadable(e.clone()),
    }
}

fn read_ident(r: &std::io::Result<Vec<u8>>) -> Ident {
    match r {
        Ok(b) => Ident::bytes(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ident::Absent,
        Err(e) => Ident::Unreadable(e.kind().to_string()),
    }
}

fn package_ident(state: &crate::app_image::PackageState) -> Ident {
    match state {
        crate::app_image::PackageState::Present { len, blake3 } => Ident::Bytes {
            len: *len,
            blake3: blake3.clone(),
        },
        crate::app_image::PackageState::Unreadable(why) => Ident::Unreadable(why.clone()),
    }
}

fn path_text(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// A manifest as the input descriptor carries it (agent-tasks design §1.2
/// M0-6): every entry's identity and, for a file entry, the stat its hash
/// was taken at. [`Manifest::from_descriptor_entries`] gives back a manifest equal to
/// the one written, stats included, so a cached observation against it
/// re-hashes only what moved.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub key: String,
    /// `absent`, `unreadable`, `bytes`, `list` or `text`.
    pub kind: String,
    /// The unreadable reason, the blake3 hex, or the text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Vec<String>>,
    /// The stat's length and modification time (nanoseconds since the Unix
    /// epoch), for an entry hashed off a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat_len: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat_mtime_ns: Option<u128>,
}

impl Manifest {
    /// Every entry with its stat (the input descriptor's manifest).
    #[must_use]
    pub fn descriptor_entries(&self) -> Vec<ManifestEntry> {
        self.entries
            .iter()
            .map(|(k, id)| {
                let mut e = ManifestEntry {
                    key: k.clone(),
                    kind: String::new(),
                    value: None,
                    len: None,
                    list: None,
                    stat_len: None,
                    stat_mtime_ns: None,
                };
                match id {
                    Ident::Absent => e.kind = "absent".into(),
                    Ident::Unreadable(why) => {
                        e.kind = "unreadable".into();
                        e.value = Some(why.clone());
                    }
                    Ident::Bytes { len, blake3 } => {
                        e.kind = "bytes".into();
                        e.len = Some(*len);
                        e.value = Some(blake3.clone());
                    }
                    Ident::List(l) => {
                        e.kind = "list".into();
                        e.list = Some(l.clone());
                    }
                    Ident::Text(t) => {
                        e.kind = "text".into();
                        e.value = Some(t.clone());
                    }
                }
                if let Some(s) = self.stats.get(k) {
                    e.stat_len = Some(s.len);
                    e.stat_mtime_ns = s.mtime.and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_nanos())
                    });
                }
                e
            })
            .collect()
    }

    /// The manifest [`Manifest::descriptor_entries`] wrote, or why `entries`
    /// is not one.
    pub fn from_descriptor_entries(entries: &[ManifestEntry]) -> Result<Manifest, String> {
        let mut m = Manifest::default();
        for e in entries {
            let id = match e.kind.as_str() {
                "absent" => Ident::Absent,
                "unreadable" => Ident::Unreadable(e.value.clone().unwrap_or_default()),
                "bytes" => Ident::Bytes {
                    len: e
                        .len
                        .ok_or_else(|| format!("{}: a bytes entry has no len", e.key))?,
                    blake3: e
                        .value
                        .clone()
                        .ok_or_else(|| format!("{}: a bytes entry has no hash", e.key))?,
                },
                "list" => Ident::List(e.list.clone().unwrap_or_default()),
                "text" => Ident::Text(e.value.clone().unwrap_or_default()),
                k => return Err(format!("{}: unknown entry kind {k:?}", e.key)),
            };
            m.entries.insert(e.key.clone(), id);
            if let Some(len) = e.stat_len {
                let mtime = e.stat_mtime_ns.map(|ns| {
                    std::time::UNIX_EPOCH
                        + std::time::Duration::new(
                            (ns / 1_000_000_000) as u64,
                            (ns % 1_000_000_000) as u32,
                        )
                });
                m.stats.insert(e.key.clone(), Stat { len, mtime });
            }
        }
        Ok(m)
    }

    /// The `.al` and package bytes this manifest names (the pre-model
    /// estimate's inputs, context-server design §4.5).
    #[must_use]
    pub fn input_sizes(&self) -> crate::engine::resident::budget::InputSizes {
        let mut sizes = crate::engine::resident::budget::InputSizes::default();
        for (k, v) in &self.entries {
            if let Ident::Bytes { len, .. } = v {
                if k.starts_with("al:") {
                    sizes.al_bytes += len;
                } else if k.starts_with("pkg:") {
                    sizes.package_bytes += len;
                }
            }
        }
        sizes
    }

    /// The identities, by key.
    #[must_use]
    pub fn entries(&self) -> &BTreeMap<String, Ident> {
        &self.entries
    }

    /// `M_consumed`: the manifest of what `inputs` holds (the acquired
    /// bytes), for a build in the given universe.
    #[must_use]
    pub fn consumed(inputs: &WorkspaceInputs, single_app: bool) -> Manifest {
        let mut e = BTreeMap::new();
        meta(&mut e, single_app);
        e.insert("ws:app.json".into(), read_ident(&inputs.app_json.bytes));
        e.insert(
            "ws:roots.config.json".into(),
            match &inputs.roots_config {
                None => Ident::Absent,
                Some(Ok(b)) => Ident::bytes(b),
                Some(Err(why)) => Ident::Unreadable(why.clone()),
            },
        );
        e.insert(
            "ws:al-sem.coverage.yaml".into(),
            inputs
                .coverage_yaml
                .as_deref()
                .map_or(Ident::Absent, Ident::bytes),
        );
        e.insert(
            "ws:app.json-paths".into(),
            Ident::List(inputs.app_json_paths.iter().map(|p| path_text(p)).collect()),
        );
        match &inputs.al_walk {
            Err(err) => {
                e.insert("ws:al-walk".into(), Ident::Unreadable(err.to_string()));
            }
            Ok(()) => {
                e.insert(
                    "ws:al-walk".into(),
                    Ident::List(
                        inputs
                            .al_files
                            .iter()
                            .map(|f| f.rel_posix.clone())
                            .collect(),
                    ),
                );
                for f in &inputs.al_files {
                    e.insert(
                        format!("al:{}", f.rel_posix),
                        match &f.text {
                            Ok(t) => Ident::bytes(t.as_bytes()),
                            Err(why) => Ident::Unreadable(why.clone()),
                        },
                    );
                }
            }
        }
        e.insert(
            "ws:source-dependencies".into(),
            source_deps_ident(&inputs.source_dependencies),
        );
        let consumed = inputs.closure.consumed();
        e.insert(
            "pkg:list".into(),
            Ident::List(consumed.0.iter().map(|(p, _)| path_text(p)).collect()),
        );
        e.insert(
            "pkg:unreadable-folders".into(),
            Ident::List(
                inputs
                    .closure
                    .unreadable
                    .iter()
                    .map(|(p, _)| path_text(p))
                    .collect(),
            ),
        );
        for (p, state) in &consumed.0 {
            e.insert(format!("pkg:{}", path_text(p)), package_ident(state));
        }
        Manifest {
            entries: e,
            stats: BTreeMap::new(),
        }
    }

    /// `M_before` / `M_after` / a freshness check: the disk now, through the
    /// reader's own discovery, hashed as `hashing` says.
    #[must_use]
    pub fn observe(workspace: &Path, single_app: bool, hashing: Hashing<'_>) -> Manifest {
        let _s = crate::engine::perf_trace::span("resident", "manifest.observe");
        let mut e = BTreeMap::new();
        let mut stats = BTreeMap::new();
        meta(&mut e, single_app);
        // A file's identity: the previous hash when its stat is unchanged
        // (and it is not dirty), else `read`'s.
        let mut file = |e: &mut BTreeMap<String, Ident>,
                        key: String,
                        path: &Path,
                        read: &dyn Fn() -> Ident| {
            let now = stat(path);
            let reused = match (hashing, now) {
                (Hashing::Cached { prev, dirty }, Some(now)) if !dirty.contains(path) => prev
                    .stats
                    .get(&key)
                    .filter(|s| **s == now)
                    .and_then(|_| prev.entries.get(&key).cloned()),
                _ => None,
            };
            let ident = reused.unwrap_or_else(read);
            if let Some(now) = now {
                stats.insert(key.clone(), now);
            }
            e.insert(key, ident);
        };
        let app_json = workspace.join("app.json");
        file(&mut e, "ws:app.json".into(), &app_json, &|| {
            read_ident(&std::fs::read(&app_json))
        });
        // `WorkspaceInputs::read` keeps this error's text, not its kind.
        let roots = workspace.join("roots.config.json");
        file(
            &mut e,
            "ws:roots.config.json".into(),
            &roots,
            &|| match std::fs::read(&roots) {
                Ok(b) => Ident::bytes(&b),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ident::Absent,
                Err(err) => Ident::Unreadable(err.to_string()),
            },
        );
        let yaml = workspace.join("al-sem.coverage.yaml");
        file(&mut e, "ws:al-sem.coverage.yaml".into(), &yaml, &|| {
            std::fs::read(&yaml).map_or(Ident::Absent, |b| Ident::bytes(&b))
        });
        let mut paths = crate::source_text::count_app_json_paths(workspace);
        paths.sort();
        e.insert(
            "ws:app.json-paths".into(),
            Ident::List(paths.iter().map(|p| path_text(p)).collect()),
        );
        let walk =
            crate::source_text::discover_al_files(workspace, crate::source_text::NestedApps::Walk);
        let listing = match walk {
            Err(err) => Ident::Unreadable(err.to_string()),
            Ok(found) => {
                for f in &found {
                    file(&mut e, format!("al:{}", f.rel_posix), &f.abs_path, &|| {
                        match crate::source_text::read_al_source(&f.abs_path) {
                            Ok(t) => Ident::bytes(t.as_bytes()),
                            Err(err) => Ident::Unreadable(err.to_string()),
                        }
                    });
                }
                Ident::List(found.into_iter().map(|f| f.rel_posix).collect())
            }
        };
        e.insert("ws:al-walk".into(), listing);
        let (files, unreadable) = crate::dependencies::discover_app_files(workspace);
        for f in &files {
            file(
                &mut e,
                format!("pkg:{}", path_text(&f.path)),
                &f.path,
                &|| {
                    let m =
                        crate::app_image::PackageManifest::observe(std::slice::from_ref(&f.path));
                    package_ident(&m.0[0].1)
                },
            );
        }
        e.insert(
            "ws:source-dependencies".into(),
            source_deps_ident(&crate::program::source_deps::read(workspace)),
        );
        e.insert(
            "pkg:list".into(),
            Ident::List(files.iter().map(|f| path_text(&f.path)).collect()),
        );
        e.insert(
            "pkg:unreadable-folders".into(),
            Ident::List(unreadable.iter().map(|(p, _)| path_text(p)).collect()),
        );
        Manifest { entries: e, stats }
    }

    /// The inputs stamp (§4.7): `al-sem-manifest-v1:` + blake3 over the
    /// canonical encoding of every entry.
    #[must_use]
    pub fn stamp(&self) -> String {
        let mut h = blake3::Hasher::new();
        let mut put = |b: &[u8]| {
            h.update(&(b.len() as u64).to_le_bytes());
            h.update(b);
        };
        for (k, v) in &self.entries {
            put(k.as_bytes());
            match v {
                Ident::Absent => put(b"absent"),
                Ident::Unreadable(why) => {
                    put(b"unreadable");
                    put(why.as_bytes());
                }
                Ident::Bytes { len, blake3 } => {
                    put(b"bytes");
                    put(&len.to_le_bytes());
                    put(blake3.as_bytes());
                }
                Ident::List(items) => {
                    put(b"list");
                    put(&(items.len() as u64).to_le_bytes());
                    for i in items {
                        put(i.as_bytes());
                    }
                }
                Ident::Text(t) => {
                    put(b"text");
                    put(t.as_bytes());
                }
            }
        }
        format!("{STAMP_PREFIX}{}", h.finalize().to_hex())
    }

    /// The keys whose identities differ between the two manifests, as
    /// paths where the key names one (`al:` and `pkg:` entries), sorted.
    #[must_use]
    pub fn changed_paths(&self, other: &Manifest, workspace: &Path) -> Vec<String> {
        let keys: BTreeSet<&String> = self.entries.keys().chain(other.entries.keys()).collect();
        keys.into_iter()
            .filter(|k| self.entries.get(*k) != other.entries.get(*k))
            .filter_map(|k| {
                if let Some(rel) = k.strip_prefix("al:") {
                    Some(path_text(&workspace.join(rel)))
                } else if let Some(p) = k.strip_prefix("pkg:") {
                    (p != "list" && p != "unreadable-folders").then(|| p.to_string())
                } else if let Some(name) = k.strip_prefix("ws:") {
                    match name {
                        "app.json-paths" | "al-walk" | "source-dependencies" => None,
                        _ => Some(path_text(&workspace.join(name))),
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Why a build's observations of its inputs disagree (§5.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputsUnstable {
    /// The first key whose three observations differ.
    pub key: String,
    pub before: Option<Ident>,
    pub consumed: Option<Ident>,
    pub after: Option<Ident>,
}

impl std::fmt::Display for InputsUnstable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let show = |i: &Option<Ident>| match i {
            None => "not listed".to_string(),
            Some(Ident::Absent) => "absent".to_string(),
            Some(Ident::Unreadable(why)) => format!("unreadable ({why})"),
            Some(Ident::Bytes { len, blake3 }) => {
                format!("{len} bytes, blake3 {}", &blake3[..16.min(blake3.len())])
            }
            Some(Ident::List(l)) => format!("{} entries", l.len()),
            Some(Ident::Text(t)) => t.clone(),
        };
        write!(
            f,
            "input {} changed during the build: before {}, consumed {}, after {}",
            self.key,
            show(&self.before),
            show(&self.consumed),
            show(&self.after)
        )
    }
}

/// `M_before == M_consumed == M_after` over every entry (§5.1). A file read
/// as B while the disk held A before and after fails here, though
/// `before == after`.
pub fn check_consistent(
    before: &Manifest,
    consumed: &Manifest,
    after: &Manifest,
) -> Result<(), InputsUnstable> {
    if before == consumed && consumed == after {
        return Ok(());
    }
    let keys: BTreeSet<&String> = before
        .entries
        .keys()
        .chain(consumed.entries.keys())
        .chain(after.entries.keys())
        .collect();
    let key = keys
        .into_iter()
        .find(|k| {
            let b = before.entries.get(*k);
            b != consumed.entries.get(*k) || b != after.entries.get(*k)
        })
        .cloned()
        .unwrap_or_default();
    Err(InputsUnstable {
        before: before.entries.get(&key).cloned(),
        consumed: consumed.entries.get(&key).cloned(),
        after: after.entries.get(&key).cloned(),
        key,
    })
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
pub(crate) mod tests;
