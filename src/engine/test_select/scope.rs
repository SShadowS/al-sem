//! **Test scope discovery** (agent-tasks design §4.2): which test code a
//! test-selection answer considers, found WITHOUT building anything.
//!
//! The scope is the union of the workspace app's own `[Test]` procedures
//! (always; the caller counts them, a nested test folder is folded into the
//! primary app) and the sibling test apps of the first rule that applies:
//!
//! 1. the `testApps` the call names ([`Origin::Call`]);
//! 2. `testApps` in the workspace's `.al-sem.json` ([`Origin::Configured`]);
//!    `"testApps": []` turns sibling discovery off ([`Origin::Off`]);
//! 3. **auto-discovery** ([`Origin::Auto`]): the folders under the git top
//!    level (the nearest ancestor of the workspace holding `.git`; with none,
//!    nothing is searched and the scope is incomplete, saying so: the folders
//!    around a workspace outside any repository are no project of its own),
//!    sorted, at most [`MAX_DEPTH`] levels
//!    deep, at most [`MAX_APP_JSON`] `app.json` files read. Skipped folders:
//!    the workspace itself (its nested folders are its own app), every
//!    folder whose name starts with `.` (`.git`, `.alpackages`, `.claude`, …),
//!    `node_modules`, `target` and `target-*`. A folder holding an `app.json`
//!    is one app: its subfolders are its own (the engine's `NestedApps::Walk`),
//!    so the walk does not enter it.
//!
//! An app **qualifies** when its `app.json` lists the workspace app's `id` in
//! `dependencies` AND one of its `.al` files declares a codeunit with
//! `Subtype = Test`. A dependent with no `.al` file is skipped `no-source`
//! (CDO's `TestFR`), one without a test codeunit `no-test-codeunit` (CDO's
//! `FR`, `OnPrem`, `DemoApp`). An app that does not depend on the workspace is
//! not listed (it is counted in `searched`). A configured or named app that
//! does not depend on the workspace is skipped `not-a-dependent`.
//!
//! **Honesty.** A cap that fires makes the scope "discovery incomplete"
//! ([`Scope::complete`] `false`, with the reason), never "no app depends on
//! this one". An unreadable folder or `app.json` is incomplete too.
//!
//! The test-codeunit scan (reading `.al` files) is cached per app, keyed by
//! the `app.json`'s and every `.al` file's path, length and modification time,
//! so a resident engine asking again reads only stats.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::source_text::{NestedApps, discover_al_files, parse_app_json, read_al_source};

/// How deep under the search root auto-discovery looks for an `app.json`
/// (the root is depth 0; `root/a/b/c/app.json` is depth 3).
pub const MAX_DEPTH: usize = 3;
/// The most `app.json` files auto-discovery reads.
pub const MAX_APP_JSON: usize = 64;

/// Which rule chose the sibling test apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The call named them (`--test-app`, MCP `testApps`).
    Call,
    /// `.al-sem.json` `testApps` named them.
    Configured,
    /// `.al-sem.json` has `"testApps": []`: no sibling is considered.
    Off,
    /// Auto-discovery under the git top level.
    Auto,
}

impl Origin {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Call => "call",
            Origin::Configured => "configured",
            Origin::Off => "off",
            Origin::Auto => "auto",
        }
    }
}

/// One sibling test app in scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestApp {
    /// The app's folder (as found: absolute for auto-discovery, as given or
    /// resolved against the workspace for a named one).
    pub root: PathBuf,
    /// Its `app.json` `id`, lowercased.
    pub app_id: String,
    /// Its `app.json` `name`.
    pub name: String,
    /// Its `.al` files.
    pub files: usize,
    /// Its codeunits with `Subtype = Test`.
    pub test_codeunits: usize,
    /// Its `[Test]` attribute lines (source text, before `#if`).
    pub test_attribute_lines: usize,
    pub found: Origin,
}

/// Why a candidate is not in scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// It depends on the workspace and holds no `.al` file.
    NoSource,
    /// It depends on the workspace and holds no `Subtype = Test` codeunit.
    NoTestCodeunit,
    /// A named app that does not list the workspace app in `dependencies`.
    NotADependent,
    /// A named folder that is no app (no readable `app.json`), with the reason.
    Unreadable(String),
}

impl SkipReason {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::NoSource => "no-source",
            SkipReason::NoTestCodeunit => "no-test-codeunit",
            SkipReason::NotADependent => "not-a-dependent",
            SkipReason::Unreadable(_) => "unreadable",
        }
    }

    /// The human words.
    #[must_use]
    pub fn human(&self) -> String {
        match self {
            SkipReason::NoSource => "no source".into(),
            SkipReason::NoTestCodeunit => "depends on the workspace, no test codeunit".into(),
            SkipReason::NotADependent => "does not depend on the workspace app".into(),
            SkipReason::Unreadable(e) => format!("unreadable: {e}"),
        }
    }
}

/// A candidate that is not in scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub root: PathBuf,
    /// Its `app.json` `name` (`""` when unread).
    pub name: String,
    pub reason: SkipReason,
}

/// The discovered scope (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    pub origin: Origin,
    /// The workspace app's `id`, lowercased (`None`: its `app.json` has none).
    pub workspace_app_id: Option<String>,
    pub apps: Vec<TestApp>,
    pub skipped: Vec<Skipped>,
    /// `false` when a cap fired or a folder could not be read.
    pub complete: bool,
    /// Why it is not complete.
    pub incomplete: Vec<String>,
    /// Auto-discovery's search root (the git top level or the workspace's
    /// parent) and whether it is the git top level.
    pub search_root: Option<(PathBuf, bool)>,
    /// The `app.json` files auto-discovery read (the workspace's excluded).
    pub searched: usize,
}

/// What the call asks (`None`: the call names no test app).
#[derive(Clone, Debug, Default)]
pub struct ScopeRequest {
    pub test_apps: Option<Vec<PathBuf>>,
}

/// The `.al-sem.json` `testApps` key: `None` when the file or the key is
/// absent (or the file does not parse: the other readers of that file ignore
/// a bad file the same way), `Some` with each entry resolved against the
/// file's folder.
#[must_use]
pub fn configured_test_apps(workspace: &Path) -> Option<Vec<PathBuf>> {
    let path = crate::state_paths::workspace_config_for_read(workspace);
    let text = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value =
        serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()?;
    let list = v.get("testApps")?.as_array()?;
    let base = path.parent().unwrap_or(workspace);
    Some(
        list.iter()
            .filter_map(|e| e.as_str())
            .map(|s| base.join(s))
            .collect(),
    )
}

/// Discover the scope of `workspace` (see the module doc).
#[must_use]
pub fn discover(workspace: &Path, req: &ScopeRequest) -> Scope {
    let workspace_app_id = read_app_json(&workspace.join("app.json"))
        .ok()
        .map(|v| str_field(&v, "id").to_ascii_lowercase())
        .filter(|s| !s.is_empty());
    let mut scope = Scope {
        origin: Origin::Auto,
        workspace_app_id,
        apps: Vec::new(),
        skipped: Vec::new(),
        complete: true,
        incomplete: Vec::new(),
        search_root: None,
        searched: 0,
    };
    let named = match &req.test_apps {
        Some(list) => Some((Origin::Call, list.clone())),
        None => configured_test_apps(workspace).map(|l| {
            if l.is_empty() {
                (Origin::Off, l)
            } else {
                (Origin::Configured, l)
            }
        }),
    };
    if let Some((origin, list)) = named {
        scope.origin = origin;
        for root in list {
            let root = if root.is_absolute() {
                root
            } else {
                workspace.join(root)
            };
            classify_named(&mut scope, &root, origin);
        }
        return scope;
    }
    if scope.workspace_app_id.is_none() {
        scope.complete = false;
        scope
            .incomplete
            .push("the workspace's app.json has no id, so no app can be matched to it".into());
        return scope;
    }
    let abs = std::path::absolute(workspace).unwrap_or_else(|_| workspace.to_path_buf());
    // No git top level: the folders around the workspace are no project of
    // its own (a temporary folder, a home folder), so nothing is searched,
    // and the scope says so rather than "none depends on it".
    let Some(root) = abs.ancestors().find(|a| a.join(".git").exists()) else {
        scope.complete = false;
        scope.incomplete.push(
            "no git top level above the workspace, so sibling apps were not searched; name them \
             with --test-app or testApps in .al-sem.json"
                .into(),
        );
        return scope;
    };
    scope.search_root = Some((root.to_path_buf(), true));
    walk(&mut scope, root, &abs, 0);
    scope
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

fn read_app_json(path: &Path) -> Result<serde_json::Value, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "not UTF-8".to_string())?;
    parse_app_json(text).map_err(|e| e.to_string())
}

/// Whether `app` lists `id` (lowercased) in `dependencies` (`id` or the older
/// `appId`).
fn depends_on(app: &serde_json::Value, id: &str) -> bool {
    app.get("dependencies")
        .and_then(|d| d.as_array())
        .into_iter()
        .flatten()
        .any(|d| {
            ["id", "appId"]
                .iter()
                .any(|k| str_field(d, k).eq_ignore_ascii_case(id))
        })
}

/// The `.al` scan of one app: files, test codeunits, `[Test]` lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scan {
    files: usize,
    test_codeunits: usize,
    test_lines: usize,
}

type ScanKey = Vec<(PathBuf, u64, Option<SystemTime>)>;

static SCAN_CACHE: Mutex<Option<HashMap<PathBuf, (ScanKey, Scan)>>> = Mutex::new(None);

fn stat(p: &Path) -> (u64, Option<SystemTime>) {
    std::fs::metadata(p).map_or((0, None), |m| (m.len(), m.modified().ok()))
}

/// Scan an app's `.al` files (cached, see the module doc).
fn scan(root: &Path) -> Result<Scan, String> {
    let found = discover_al_files(root, NestedApps::Walk).map_err(|e| e.to_string())?;
    let mut key: ScanKey = Vec::with_capacity(found.len() + 1);
    let (n, t) = stat(&root.join("app.json"));
    key.push((root.join("app.json"), n, t));
    for f in &found {
        let (n, t) = stat(&f.abs_path);
        key.push((f.abs_path.clone(), n, t));
    }
    if let Some((k, s)) = SCAN_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(root))
        && *k == key
    {
        return Ok(*s);
    }
    let mut s = Scan {
        files: found.len(),
        test_codeunits: 0,
        test_lines: 0,
    };
    for f in &found {
        let text = read_al_source(&f.abs_path).map_err(|e| format!("{}: {e}", f.rel_posix))?;
        let (c, l) = count_tests(&text);
        s.test_codeunits += c;
        s.test_lines += l;
    }
    SCAN_CACHE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(root.to_path_buf(), (key, s));
    Ok(s)
}

/// `(Subtype = Test lines, [Test] attribute lines)` of one file's text, by
/// line, case-insensitively, spaces ignored: `Subtype = TestRunner` is no
/// test codeunit.
fn count_tests(text: &str) -> (usize, usize) {
    let mut codeunits = 0;
    let mut tests = 0;
    for line in text.lines() {
        let squeezed: String = line
            .chars()
            .filter(|c| !c.is_whitespace())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if squeezed.starts_with("subtype=test;") {
            codeunits += 1;
        } else if squeezed.starts_with("[test]") {
            tests += 1;
        }
    }
    (codeunits, tests)
}

/// Classify one app folder holding `app.json` against the workspace.
fn classify(scope: &mut Scope, root: &Path, app: &serde_json::Value, origin: Origin) {
    let name = str_field(app, "name");
    let ws_id = scope.workspace_app_id.clone().unwrap_or_default();
    if ws_id.is_empty() || !depends_on(app, &ws_id) {
        if origin != Origin::Auto {
            scope.skipped.push(Skipped {
                root: root.to_path_buf(),
                name,
                reason: SkipReason::NotADependent,
            });
        }
        return;
    }
    let s = match scan(root) {
        Ok(s) => s,
        Err(e) => {
            scope.complete = false;
            scope.incomplete.push(format!("{}: {e}", root.display()));
            return;
        }
    };
    let reason = if s.files == 0 {
        Some(SkipReason::NoSource)
    } else if s.test_codeunits == 0 {
        Some(SkipReason::NoTestCodeunit)
    } else {
        None
    };
    match reason {
        Some(reason) => scope.skipped.push(Skipped {
            root: root.to_path_buf(),
            name,
            reason,
        }),
        None => scope.apps.push(TestApp {
            root: root.to_path_buf(),
            app_id: str_field(app, "id").to_ascii_lowercase(),
            name,
            files: s.files,
            test_codeunits: s.test_codeunits,
            test_attribute_lines: s.test_lines,
            found: origin,
        }),
    }
}

fn classify_named(scope: &mut Scope, root: &Path, origin: Origin) {
    match read_app_json(&root.join("app.json")) {
        Ok(app) => classify(scope, root, &app, origin),
        Err(e) => scope.skipped.push(Skipped {
            root: root.to_path_buf(),
            name: String::new(),
            reason: SkipReason::Unreadable(format!("app.json: {e}")),
        }),
    }
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name == "node_modules"
        || name == "target"
        || name.starts_with("target-")
}

/// Depth-first, sorted, over `dir` at `depth` (see the module doc).
fn walk(scope: &mut Scope, dir: &Path, workspace: &Path, depth: usize) {
    if !scope.complete && scope.searched >= MAX_APP_JSON {
        return;
    }
    if dir == workspace {
        return;
    }
    let app_json = dir.join("app.json");
    if depth > 0 && app_json.is_file() {
        if scope.searched >= MAX_APP_JSON {
            cap(scope);
            return;
        }
        scope.searched += 1;
        match read_app_json(&app_json) {
            Ok(app) => classify(scope, dir, &app, Origin::Auto),
            Err(e) => {
                scope.complete = false;
                scope
                    .incomplete
                    .push(format!("{}: {e}", app_json.display()));
            }
        }
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let mut subdirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| !e.file_name().to_str().is_some_and(skip_dir))
            .map(|e| e.path())
            .collect(),
        Err(e) => {
            scope.complete = false;
            scope.incomplete.push(format!("{}: {e}", dir.display()));
            return;
        }
    };
    subdirs.sort();
    for d in subdirs {
        walk(scope, &d, workspace, depth + 1);
        if scope.searched >= MAX_APP_JSON && !scope.complete {
            return;
        }
    }
}

fn cap(scope: &mut Scope) {
    if scope.complete || !scope.incomplete.iter().any(|r| r.starts_with("the cap")) {
        scope.complete = false;
        scope.incomplete.push(format!(
            "the cap of {MAX_APP_JSON} app.json files was reached before the search ended"
        ));
    }
}

impl Scope {
    /// The one-line summary explain's `tests:` line uses: what was found and
    /// how, never a claim that no app depends on the workspace when the search
    /// did not finish.
    #[must_use]
    pub fn sibling_summary(&self) -> String {
        let names: Vec<&str> = self.apps.iter().map(|a| a.name.as_str()).collect();
        let found = match self.apps.len() {
            0 => None,
            1 => Some(format!("1 test app {} ({})", self.found_how(), names[0])),
            n => Some(format!(
                "{n} test apps {} ({})",
                self.found_how(),
                names.join(", ")
            )),
        };
        match (found, self.origin) {
            (Some(f), _) => {
                if self.complete {
                    f
                } else {
                    format!("{f}; discovery incomplete ({})", self.incomplete.join("; "))
                }
            }
            (None, Origin::Off) => {
                "sibling test apps turned off (testApps: [] in .al-sem.json)".into()
            }
            (None, Origin::Call | Origin::Configured) => format!(
                "no named test app qualifies ({})",
                self.skipped
                    .iter()
                    .map(|s| format!("{}: {}", s.root.display(), s.reason.human()))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            (None, Origin::Auto) if !self.complete => format!(
                "test app discovery incomplete ({}), so test apps may exist that were not found",
                self.incomplete.join("; ")
            ),
            (None, Origin::Auto) => format!(
                "no app under the git top level depends on this one with a test codeunit (searched {} app.json {})",
                self.searched,
                if self.searched == 1 { "file" } else { "files" }
            ),
        }
    }

    fn found_how(&self) -> &'static str {
        match self.origin {
            Origin::Call => "named in the call",
            Origin::Configured => "configured",
            Origin::Off => "",
            Origin::Auto => "discovered",
        }
    }
}

#[cfg(test)]
#[path = "scope_tests.rs"]
mod tests;
