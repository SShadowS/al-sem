//! **The base side of a before/after answer** (agent-tasks design §10.6):
//! one function both fronts call ([`base_side`]); only how the base part
//! runs differs (the CLI runs it in its own process; the MCP front end runs
//! it in a one-shot child it admitted under the pool's bound, §10.6 step 3).
//!
//! 1. The key ([`super::cache::KeyParts`]): the comparison base's tree id
//!    of the workspace folder, the current package files' digest (the base
//!    reads TODAY's packages), the source dependencies, the options.
//! 2. A cached base answers when it holds this change's base roots (by the
//!    reversed diff's hash) and a summary (or an absence) for every routine
//!    of the working tree's roots: lazy per-root summaries (astra a12), never
//!    an all-routine table.
//! 3. Otherwise: materialize ([`super::base`]), run the base part
//!    (`impact --part main --summaries` over the copy, its stdin the
//!    reversed diff, `--root-id` the working tree's roots,
//!    `--package-root` the workspace), delete the copy, merge the answer into
//!    the entry, write it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use super::base::{Environment, dependency_changes, materialize, reverse_diff, tree_of};
use super::cache::{Cache, KeyParts, Lookup};
use super::compare::Comparison;

/// How the base part runs: its command line and stdin → its JSON.
pub type RunBase<'a> = dyn FnMut(Vec<String>, String) -> Result<Value, String> + 'a;

/// What the base side needs besides the parts.
pub struct BaseCx<'a> {
    /// The cache, or `None` (`--no-base-cache`).
    pub cache: Option<Cache>,
    /// Where the copy is made (a private folder).
    pub scratch: PathBuf,
    /// The change's diff text (its reverse is the base part's stdin).
    pub diff_text: String,
    pub run: Box<RunBase<'a>>,
}

/// Where a base copy is made: the impact-base namespace's `work/` folder
/// (private per-user state), else the system's temporary folder.
#[must_use]
pub fn scratch_for(cache: Option<&Cache>) -> PathBuf {
    cache.map_or_else(std::env::temp_dir, |c| c.dir.join("work"))
}

/// The base side: its main-part document (as the base part printed it, or
/// as the cache rebuilt it) and the `base` block of the answer.
pub struct BaseOutcome {
    pub doc: Value,
    pub block: Value,
}

/// The workspace's path below the diff's root (`--diff-root`, else the git
/// top level).
fn rel_below_root(workspace: &Path, diff_root: Option<&str>) -> Result<String, String> {
    match diff_root {
        Some(root) => {
            let ws = std::fs::canonicalize(workspace).map_err(|e| e.to_string())?;
            let r = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
            ws.strip_prefix(&r)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .map_err(|_| "the workspace is not inside --diff-root".to_string())
        }
        None => {
            let out = crate::git_env::git_command()
                .arg("-C")
                .arg(workspace)
                .args(["rev-parse", "--show-prefix"])
                .output()
                .map_err(|e| format!("git could not run: {e}"))?;
            if !out.status.success() {
                return Err("the workspace is not in a git repository".into());
            }
            Ok(String::from_utf8_lossy(&out.stdout)
                .trim()
                .trim_end_matches('/')
                .to_string())
        }
    }
}

/// A folder's content hash (a source dependency read from disk): every
/// file's path and bytes, length-prefixed, in path order.
fn folder_hash(dir: &Path) -> String {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                files.push(p);
            }
        }
    }
    files.sort();
    let mut h = blake3::Hasher::new();
    for f in &files {
        let rel = f
            .strip_prefix(dir)
            .unwrap_or(f)
            .to_string_lossy()
            .into_owned();
        h.update(&(rel.len() as u64).to_le_bytes());
        h.update(rel.as_bytes());
        let bytes = std::fs::read(f).unwrap_or_default();
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    format!("blake3:{}", h.finalize().to_hex())
}

/// The base's source-dependency folders (its archived config's spellings,
/// resolved against the real workspace), read before materializing so the
/// key holds their content.
fn base_source_deps(workspace: &Path, commit: &str) -> Vec<(String, String)> {
    // `git show <commit>:<path>` names the path from the top level.
    let top_rel = crate::git_env::git_command()
        .arg("-C")
        .arg(workspace)
        .args(["rev-parse", "--show-prefix"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let spec = format!(
        "{commit}:{}{}",
        top_rel,
        crate::program::source_deps::CONFIG_FILE
    );
    let Ok(out) = crate::git_env::git_command()
        .arg("-C")
        .arg(workspace)
        .args(["show", &spec])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
        .ok()
        .and_then(|v| v["sources"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.as_str())
        .map(|s| {
            let p = workspace.join(s);
            let p = std::path::absolute(&p).unwrap_or(p);
            let h = folder_hash(&p);
            (p.to_string_lossy().into_owned(), h)
        })
        .collect()
}

fn ids(doc: &Value) -> Vec<String> {
    doc["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["stableId"].as_str().map(str::to_string))
        .collect()
}

/// The files whose diff lines do not match the base's text (a `--diff`'s
/// pre-images against the comparison base).
fn mismatches(doc: &Value) -> Vec<String> {
    doc["accounting"]["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| !f["mismatch"].is_null())
        .filter_map(|f| f["path"].as_str().map(str::to_string))
        .collect()
}

/// The base side (see the module doc). `new_main`: the working tree's main
/// part (printed with `--summaries`); `diff_text`: the change.
pub fn base_side(
    workspace: &Path,
    single_app: bool,
    diff_root: Option<&str>,
    cmp: &Comparison,
    diff_text: &str,
    new_main: &Value,
    cx: &mut BaseCx<'_>,
) -> Result<BaseOutcome, String> {
    let started = Instant::now();
    let commit = cmp
        .comparison_base
        .clone()
        .ok_or("no comparison base was resolved")?;
    let rel = rel_below_root(workspace, diff_root)?;
    let tree = tree_of(workspace, &commit)?;
    let mut env = Environment::of(workspace);
    env.source_deps = base_source_deps(workspace, &commit);
    let parts = KeyParts {
        tree: tree.clone(),
        packages_digest: env.packages_digest.clone(),
        source_deps: env.source_deps.clone(),
        options: if single_app {
            "single-app"
        } else {
            "cross-app"
        }
        .into(),
    };
    let key = parts.key();
    let reversed = reverse_diff(diff_text);
    let rdh = super::compare::diff_hash(&reversed);
    let wanted = ids(new_main);
    let mut cache_note = json!({"status": "off"});
    let mut entry: Option<Value> = None;
    if let Some(cache) = &cx.cache {
        match cache.read(&key) {
            Lookup::Hit(v) => {
                let has_diff = v["byDiff"].get(&rdh).is_some();
                let missing = wanted
                    .iter()
                    .filter(|id| {
                        v["summaries"].get(id.as_str()).is_none()
                            && !v["absent"]
                                .as_array()
                                .is_some_and(|a| a.iter().any(|x| x == id.as_str()))
                    })
                    .count();
                if has_diff && missing == 0 {
                    let doc = doc_from_entry(&v, &rdh, &wanted);
                    let (n, bytes) = cache.usage();
                    let block = block(
                        cmp,
                        &commit,
                        &tree,
                        &env,
                        new_main,
                        &doc,
                        json!({"status": "hit", "key": key, "entries": n, "bytes": bytes,
                               "dir": cache.dir.to_string_lossy()}),
                        started,
                        None,
                    );
                    return Ok(BaseOutcome { doc, block });
                }
                cache_note = json!({"status": "partial", "missingRoots": missing,
                                    "diffKnown": has_diff});
                entry = Some(v);
            }
            Lookup::Miss => cache_note = json!({"status": "miss"}),
            Lookup::Rebuilt(why) => cache_note = json!({"status": "rebuilt", "why": why}),
        }
    }
    // Build: materialize, run the base part, drop the copy.
    let m_started = Instant::now();
    let m = materialize(workspace, &commit, &cx.scratch, &rel)?;
    let materialize_ms = m_started.elapsed().as_millis() as u64;
    let abs_ws = std::path::absolute(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    let mut argv: Vec<String> = vec![
        "impact".into(),
        "--part".into(),
        "main".into(),
        "--summaries".into(),
        "--diff=-".into(),
        format!("--package-root={}", abs_ws.to_string_lossy()),
    ];
    for s in &m.source_deps {
        argv.push(format!("--source-dep={}", s.to_string_lossy()));
    }
    for id in &wanted {
        argv.push(format!("--root-id={id}"));
    }
    if single_app {
        argv.push("--single-app".into());
    }
    argv.push("--".into());
    argv.push(m.workspace.to_string_lossy().into_owned());
    let b_started = Instant::now();
    let ran = (cx.run)(argv, reversed);
    let build_ms = b_started.elapsed().as_millis() as u64;
    m.remove();
    let doc = ran?;
    // Merge into the entry and publish.
    let mut e = entry.unwrap_or_else(
        || json!({"keyParts": parts.to_json(), "summaries": {}, "absent": [], "byDiff": {}}),
    );
    for k in [
        "contract",
        "packages",
        "appDependencies",
        "sourceDependencies",
        "summarySchema",
    ] {
        e[k] = doc[k].clone();
    }
    if let (Some(dst), Some(src)) = (e["summaries"].as_object_mut(), doc["summaries"].as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    let have: std::collections::HashSet<String> = e["summaries"]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let mut absent: Vec<Value> = e["absent"].as_array().cloned().unwrap_or_default();
    for id in &wanted {
        if !have.contains(id) && !absent.iter().any(|x| x == id.as_str()) {
            absent.push(json!(id));
        }
    }
    e["absent"] = json!(absent);
    e["byDiff"][&rdh] = json!({"roots": ids(&doc), "mismatches": mismatches(&doc)});
    if let Some(cache) = &cx.cache {
        cache_note["written"] = match cache.write(&key, &e) {
            Ok(()) => json!(true),
            Err(why) => json!(why),
        };
        let (n, bytes) = cache.usage();
        cache_note["key"] = json!(key);
        cache_note["entries"] = json!(n);
        cache_note["bytes"] = json!(bytes);
        cache_note["dir"] = json!(cache.dir.to_string_lossy());
    }
    let block = block(
        cmp,
        &commit,
        &tree,
        &env,
        new_main,
        &doc,
        cache_note,
        started,
        Some((materialize_ms, build_ms)),
    );
    Ok(BaseOutcome { doc, block })
}

/// A base document rebuilt from a cache entry: the roots this diff mapped
/// to plus the wanted ids it has, their summaries, the rest as stored.
fn doc_from_entry(e: &Value, rdh: &str, wanted: &[String]) -> Value {
    let mut roots: Vec<String> = e["byDiff"][rdh]["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    for id in wanted {
        if e["summaries"].get(id.as_str()).is_some() && !roots.contains(id) {
            roots.push(id.clone());
        }
    }
    let summaries: serde_json::Map<String, Value> = roots
        .iter()
        .filter_map(|id| {
            e["summaries"]
                .get(id.as_str())
                .map(|s| (id.clone(), s.clone()))
        })
        .collect();
    let mismatch_files: Vec<Value> = e["byDiff"][rdh]["mismatches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| json!({"path": p, "mismatch": {"firstLine": null}}))
        .collect();
    json!({
        "roots": roots.iter().map(|id| json!({"stableId": id})).collect::<Vec<_>>(),
        "summaries": summaries,
        "contract": e["contract"],
        "packages": e["packages"],
        "appDependencies": e["appDependencies"],
        "sourceDependencies": e["sourceDependencies"],
        "summarySchema": e["summarySchema"],
        "accounting": {"files": mismatch_files},
    })
}

/// The answer's `base` block.
#[allow(clippy::too_many_arguments)]
fn block(
    cmp: &Comparison,
    commit: &str,
    tree: &str,
    env: &Environment,
    new_main: &Value,
    base_doc: &Value,
    cache: Value,
    started: Instant,
    built: Option<(u64, u64)>,
) -> Value {
    let dep_changes =
        dependency_changes(&base_doc["appDependencies"], &new_main["appDependencies"]);
    let packages_equal = base_doc["packages"] == new_main["packages"];
    json!({
        "commit": commit,
        "requestedRef": cmp.requested_ref,
        "tree": tree,
        "environment": env.to_json(),
        "packagesRead": {
            "base": base_doc["packages"], "new": new_main["packages"],
            "same": packages_equal,
        },
        "sourceDependencies": {"base": base_doc["sourceDependencies"], "new": new_main["sourceDependencies"]},
        "dependencyDeclarationChanged": dep_changes,
        "preimageMismatches": mismatches(base_doc),
        "cache": cache,
        "ms": {
            "total": started.elapsed().as_millis() as u64,
            "materialize": built.map(|b| b.0),
            "build": built.map(|b| b.1),
        },
    })
}
