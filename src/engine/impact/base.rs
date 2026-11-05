//! **The comparison base, materialized** (agent-tasks design §10.2, §10.6):
//! the workspace's folder as the base commit holds it, in a private folder,
//! analyzed under TODAY's dependency environment.
//!
//! - [`materialize`]: `git archive <commit>:<workspace prefix>` extracted
//!   into a fresh folder (git is only read; nothing in the repository or the
//!   workspace changes). Packages are gitignored, so the archive has none:
//!   the base part reads the CURRENT workspace's package discovery
//!   (`--package-root <workspace>`, the test universe's injection). A folder
//!   `.git` marks the copy as its own boundary, so no folder above it adds a
//!   package. A source-dependency config in the archive names folders
//!   outside the workspace: they are read from disk (`--source-dep`, the
//!   config's spellings resolved against the real workspace) and stamped.
//! - [`reverse_diff`]: the change seen from the base: its removed lines are
//!   the base's, so the base part maps them to the base's routines (and its
//!   accounting checks a `--diff`'s pre-images against the base's text).
//! - [`Environment`]: what the base read that the archive did not hold
//!   (every package file with its blake3, the source dependencies), and
//!   whether the change edits `app.json`'s dependencies.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

/// `git <args>` in `dir`, its stdout.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The base workspace, materialized.
#[derive(Debug)]
pub struct Materialized {
    /// The folder the copy was made in (removed by [`Materialized::remove`]).
    pub holder: PathBuf,
    /// The base workspace folder inside it.
    pub workspace: PathBuf,
    /// The base commit's tree id of the workspace folder.
    pub tree: String,
    /// `--source-dep` folders (the archive's config, resolved against the
    /// real workspace), absolute.
    pub source_deps: Vec<PathBuf>,
    /// The archived `al-sem.dependencies.json`, when it had one (moved aside
    /// so the copy does not resolve it against itself).
    pub source_config: Option<String>,
}

impl Materialized {
    /// Delete the copy.
    pub fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.holder);
    }
}

/// The tree id of `workspace`'s folder at `commit` (the cache key's first
/// part), or why there is none.
pub fn tree_of(workspace: &Path, commit: &str) -> Result<String, String> {
    let prefix = git(workspace, &["rev-parse", "--show-prefix"])?;
    let spec = if prefix.is_empty() {
        format!("{commit}^{{tree}}")
    } else {
        format!("{commit}:{}", prefix.trim_end_matches('/'))
    };
    git(workspace, &["rev-parse", "--verify", "-q", &spec]).map_err(|_| {
        format!(
            "the workspace folder '{}' does not exist at the base commit {}",
            if prefix.is_empty() { "." } else { &prefix },
            &commit[..commit.len().min(10)]
        )
    })
}

/// Materialize the base: `workspace`'s folder at `commit`, extracted into a
/// new folder under `into`, at `rel` below it (the workspace's path below the
/// diff's root, so the change's paths name the copy's files as they name the
/// workspace's: the copy's top holds the `.git` marker a diff root is found
/// by). See the module doc.
pub fn materialize(
    workspace: &Path,
    commit: &str,
    into: &Path,
    rel: &str,
) -> Result<Materialized, String> {
    let tree = tree_of(workspace, commit)?;
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let holder = tempfile::Builder::new()
        .prefix("base-")
        .tempdir_in(into)
        .map_err(|e| format!("a folder in {}: {e}", into.display()))?
        .keep();
    let rel = rel.trim_matches('/');
    let ws = if rel.is_empty() {
        holder.clone()
    } else {
        holder.join(rel)
    };
    // `git archive` run in a subfolder keeps only that subfolder's paths
    // (of a tree that is already the subfolder: nothing): run it at the top.
    let top = git(workspace, &["rev-parse", "--show-toplevel"])?;
    let made = (|| {
        std::fs::create_dir_all(&ws).map_err(|e| e.to_string())?;
        let mut archive = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(["archive", "--format=tar", &tree])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("git archive could not run: {e}"))?;
        let stdout = archive.stdout.take().ok_or("git archive: no stdout")?;
        let tar = Command::new("tar")
            .arg("-x")
            .arg("-f")
            .arg("-")
            .arg("-C")
            .arg(&ws)
            .stdin(stdout)
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("tar could not run: {e}"))?;
        let git_out = archive
            .wait_with_output()
            .map_err(|e| format!("git archive: {e}"))?;
        if !git_out.status.success() {
            return Err(format!(
                "git archive {tree} failed: {}",
                String::from_utf8_lossy(&git_out.stderr).trim()
            ));
        }
        if !tar.status.success() {
            return Err(format!(
                "tar failed: {}",
                String::from_utf8_lossy(&tar.stderr).trim()
            ));
        }
        if !ws.join("app.json").is_file() {
            return Err(format!(
                "the base commit's workspace folder holds no app.json (tree {tree})"
            ));
        }
        // The copy is its own boundary: no folder above it lends a package,
        // and the diff's root is found at its top.
        std::fs::create_dir_all(holder.join(".git")).map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&holder);
        return Err(format!("the base could not be materialized: {e}"));
    }
    // A source-dependency config names folders outside the workspace:
    // resolved against the real one, read from disk.
    let config = ws.join(crate::program::source_deps::CONFIG_FILE);
    let (source_deps, source_config) = match std::fs::read_to_string(&config) {
        Ok(text) => {
            let spelled: Vec<String> =
                serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
                    .ok()
                    .and_then(|v| v["sources"].as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect();
            let _ = std::fs::remove_file(&config);
            (
                spelled
                    .iter()
                    .map(|s| {
                        let p = workspace.join(s);
                        std::path::absolute(&p).unwrap_or(p)
                    })
                    .collect(),
                Some(text),
            )
        }
        Err(_) => (Vec::new(), None),
    };
    Ok(Materialized {
        holder,
        workspace: ws,
        tree,
        source_deps,
        source_config,
    })
}

/// The diff seen from the other side: old and new swapped (paths, hunk
/// ranges, `+`/`-` lines, file modes, renames, index ids).
#[must_use]
pub fn reverse_diff(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_old: Option<String> = None;
    let mut in_hunk = (0usize, 0usize); // lines left: old, new
    for raw in text.split_inclusive('\n') {
        let line = raw.trim_end_matches('\n');
        let eol = &raw[line.len()..];
        if in_hunk.0 > 0 || in_hunk.1 > 0 {
            let (c, rest) = line.split_at(line.len().min(1));
            match c {
                "+" => {
                    in_hunk.1 = in_hunk.1.saturating_sub(1);
                    out.push('-');
                    out.push_str(rest);
                }
                "-" => {
                    in_hunk.0 = in_hunk.0.saturating_sub(1);
                    out.push('+');
                    out.push_str(rest);
                }
                "\\" => out.push_str(line),
                _ => {
                    in_hunk.0 = in_hunk.0.saturating_sub(1);
                    in_hunk.1 = in_hunk.1.saturating_sub(1);
                    out.push_str(line);
                }
            }
            out.push_str(eol);
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@ ")
            && let Some((ranges, tail)) = rest.split_once(" @@")
            && let Some((old, new)) = ranges.split_once(' ')
            && let (Some(old), Some(new)) = (old.strip_prefix('-'), new.strip_prefix('+'))
        {
            let count =
                |r: &str| -> usize { r.split_once(',').map_or(1, |(_, n)| n.parse().unwrap_or(0)) };
            in_hunk = (count(old), count(new));
            out.push_str(&format!("@@ -{new} +{old} @@{tail}{eol}"));
            continue;
        }
        if let Some(p) = line.strip_prefix("--- ") {
            pending_old = Some(p.to_string());
            continue;
        }
        if let Some(p) = line.strip_prefix("+++ ")
            && let Some(old) = pending_old.take()
        {
            let swap = |s: &str, from: &str, to: &str| {
                s.strip_prefix(from)
                    .map_or_else(|| s.to_string(), |r| format!("{to}{r}"))
            };
            out.push_str(&format!("--- {}{eol}", swap(p, "b/", "a/")));
            out.push_str(&format!("+++ {}{eol}", swap(&old, "a/", "b/")));
            continue;
        }
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/X b/Y` → `a/Y b/X` (paths without spaces; else kept).
            let parts: Vec<&str> = rest.split(' ').collect();
            if parts.len() == 2 {
                out.push_str(&format!(
                    "diff --git a/{} b/{}{eol}",
                    parts[1].trim_start_matches("b/"),
                    parts[0].trim_start_matches("a/")
                ));
            } else {
                out.push_str(raw);
            }
            continue;
        }
        let swapped = if let Some(m) = line.strip_prefix("new file mode ") {
            format!("deleted file mode {m}")
        } else if let Some(m) = line.strip_prefix("deleted file mode ") {
            format!("new file mode {m}")
        } else if let Some(p) = line.strip_prefix("rename from ") {
            format!("rename to {p}")
        } else if let Some(p) = line.strip_prefix("rename to ") {
            format!("rename from {p}")
        } else if let Some(m) = line.strip_prefix("old mode ") {
            format!("new mode {m}")
        } else if let Some(m) = line.strip_prefix("new mode ") {
            format!("old mode {m}")
        } else if let Some(ids) = line.strip_prefix("index ") {
            let (ids, mode) = ids.split_once(' ').map_or((ids, ""), |(i, m)| (i, m));
            match ids.split_once("..") {
                Some((a, b)) if mode.is_empty() => format!("index {b}..{a}"),
                Some((a, b)) => format!("index {b}..{a} {mode}"),
                None => line.to_string(),
            }
        } else {
            line.to_string()
        };
        out.push_str(&swapped);
        out.push_str(eol);
    }
    // `rename from`/`rename to` must stay in that order after swapping.
    fix_rename_order(&out)
}

/// After [`reverse_diff`] a section reads `rename to X` before `rename from
/// Y`; git (and the parser) expects `from` first.
fn fix_rename_order(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("rename to ")
            && lines
                .get(i + 1)
                .is_some_and(|l| l.starts_with("rename from "))
        {
            out.push_str(lines[i + 1]);
            out.push_str(lines[i]);
            i += 2;
            continue;
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}

/// What the base read beyond its archive, and how the dependency
/// declarations compare.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Environment {
    /// Every package file the current workspace's discovery finds, as
    /// `(path, blake3)` (the base reads the same files).
    pub packages: Vec<(String, String)>,
    /// `closure-v2`-style digest over [`Self::packages`].
    pub packages_digest: String,
    /// The source dependencies read from disk: `(path, content hash)`.
    pub source_deps: Vec<(String, String)>,
}

impl Environment {
    /// The current workspace's package files, hashed.
    #[must_use]
    pub fn of(workspace: &Path) -> Environment {
        let (files, _) = crate::dependencies::discover_app_files(workspace);
        let mut packages: Vec<(String, String)> = files
            .iter()
            .map(|f| {
                let h = std::fs::read(&f.path).map_or_else(
                    |e| format!("unreadable: {e}"),
                    |b| format!("blake3:{}", blake3::hash(&b).to_hex()),
                );
                (f.path.to_string_lossy().into_owned(), h)
            })
            .collect();
        packages.sort();
        let mut h = blake3::Hasher::new();
        for (p, d) in &packages {
            for part in [p.as_str(), d.as_str()] {
                h.update(&(part.len() as u64).to_le_bytes());
                h.update(part.as_bytes());
            }
        }
        Environment {
            packages_digest: if packages.is_empty() {
                "packages:empty".into()
            } else {
                format!("packages:{}", h.finalize().to_hex())
            },
            packages,
            source_deps: Vec::new(),
        }
    }

    /// The JSON form (`base.environment`).
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "label": "base analyzed under the current dependency environment",
            "packages": self.packages.iter().map(|(p, h)| json!({"path": p, "hash": h})).collect::<Vec<_>>(),
            "packagesDigest": self.packages_digest,
            "sourceDependencies": self.source_deps.iter().map(|(p, h)| json!({"path": p, "contentHash": h})).collect::<Vec<_>>(),
        })
    }
}

/// `app.json`'s declared dependencies as comparable rows (`id@version`,
/// lowercased ids), and the rows that differ between two declarations.
#[must_use]
pub fn dependency_changes(base: &Value, new: &Value) -> Vec<String> {
    let rows = |v: &Value| -> std::collections::BTreeMap<String, String> {
        v.as_array()
            .into_iter()
            .flatten()
            .map(|d| {
                let id = d["id"]
                    .as_str()
                    .or_else(|| d["appId"].as_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let name = d["name"].as_str().unwrap_or("").to_string();
                let version = d["version"].as_str().unwrap_or("").to_string();
                (
                    if id.is_empty() {
                        name.to_lowercase()
                    } else {
                        id
                    },
                    format!("{name} {version}"),
                )
            })
            .collect()
    };
    let (b, n) = (rows(base), rows(new));
    let mut out = Vec::new();
    for (id, shown) in &b {
        match n.get(id) {
            None => out.push(format!("removed {shown}")),
            Some(s) if s != shown => out.push(format!("{shown} → {s}")),
            _ => {}
        }
    }
    for (id, shown) in &n {
        if !b.contains_key(id) {
            out.push(format!("added {shown}"));
        }
    }
    out
}

#[cfg(test)]
#[path = "base_tests.rs"]
mod tests;
