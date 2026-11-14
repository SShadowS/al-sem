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
//!    **A project-collection marker** ([`PROJECTS_MARKER`]) stops the search
//!    sooner: a folder holding a file named `.al-sem-projects` says each of
//!    its subfolders is a project of its own (a corpus of fixtures, a folder
//!    of unrelated apps). For a workspace under it the search root is the
//!    subfolder of the marked folder that holds the workspace, when that is
//!    nearer than the git top level. This repository marks `tests/r0-corpus/`
//!    and `tests/fixtures/`, so a fixture's scope is the fixture's own, not
//!    the 200-odd other fixtures' `app.json` files (which hit the cap and
//!    made every fixture's answer depend on the repository's layout).
//!
//!    **Other projects are not entered** (engine limits wave 0, B8): below the
//!    search root the walk does not descend into a folder that holds a
//!    [`PROJECTS_MARKER`] or a `.git` of its own (a nested clone, a worktree,
//!    a submodule) unless the workspace is inside it. Without this, a
//!    workspace elsewhere in this repository (under `target/tmp`, `docs/`)
//!    walked into `tests/r0-corpus/` from the git top level, read 64 fixture
//!    `app.json` files and answered "discovery incomplete". A folder holding
//!    an `app.json` is read as an app before the check, so a sibling test app
//!    that is its own repository is still found. The cap stays as a safety
//!    net.
//!
//!    **Ignored folders are not entered** (engine limits B8, TLS-48): a folder
//!    git ignores (`.gitignore`, `.git/info/exclude`, the user's excludes
//!    file; one `git check-ignore` per folder read) is skipped, unless it
//!    holds the workspace or a `.git` of its own (a nested clone is usually
//!    ignored by the outer repository; it is still read as an app). Below an
//!    ignored folder that holds the workspace, and when the search root itself
//!    is ignored, nothing is skipped as ignored (git reports all of it
//!    ignored, the workspace's siblings too). When git cannot answer, nothing
//!    is skipped. A folder with tracked files is not ignored. A skipped folder
//!    is not listed in the scope.
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
/// The file that marks a folder whose subfolders are separate projects (see
/// the module doc).
pub const PROJECTS_MARKER: &str = ".al-sem-projects";

/// Auto-discovery's search root for the (absolute) workspace `abs`: walking
/// up from it, the first folder that holds `.git` (`true`: the git top
/// level) or whose parent holds [`PROJECTS_MARKER`] (`false`: the project
/// folder). `None`: neither.
fn search_root(abs: &Path) -> Option<(PathBuf, bool)> {
    abs.ancestors().find_map(|a| {
        if a.join(".git").exists() {
            Some((a.to_path_buf(), true))
        } else if a
            .parent()
            .is_some_and(|p| p.join(PROJECTS_MARKER).is_file())
        {
            Some((a.to_path_buf(), false))
        } else {
            None
        }
    })
}

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
    /// The `.al-sem.json` `testApps` this discovery read
    /// ([`test_config_identity`]), also when the call named its own test
    /// apps: `alsem_status` compares it with the disk (the `test-config`
    /// category, milestone C item 3).
    pub test_config: serde_json::Value,
}

/// The identity of the `.al-sem.json` `testApps` key as
/// [`configured_test_apps`] reads it: `null` when absent (or the file does
/// not parse), else the resolved folders in order. One function for the
/// discovery that recorded it and the status check that compares it.
#[must_use]
pub fn test_config_identity(config: Option<&[PathBuf]>) -> serde_json::Value {
    match config {
        None => serde_json::Value::Null,
        Some(list) => serde_json::Value::Array(
            list.iter()
                .map(|p| serde_json::Value::String(p.to_string_lossy().into_owned()))
                .collect(),
        ),
    }
}

/// [`test_config_identity`] of the workspace's `.al-sem.json` now.
#[must_use]
pub fn read_test_config_identity(workspace: &Path) -> serde_json::Value {
    test_config_identity(configured_test_apps(workspace).as_deref())
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
    // Read once: the scope records what it was discovered under.
    let configured = configured_test_apps(workspace);
    let mut scope = Scope {
        origin: Origin::Auto,
        workspace_app_id,
        apps: Vec::new(),
        skipped: Vec::new(),
        complete: true,
        incomplete: Vec::new(),
        search_root: None,
        searched: 0,
        test_config: test_config_identity(configured.as_deref()),
    };
    let named = match &req.test_apps {
        Some(list) => Some((Origin::Call, list.clone())),
        None => configured.map(|l| {
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
    let Some((root, git_top)) = search_root(&abs) else {
        scope.complete = false;
        scope.incomplete.push(
            "no git top level above the workspace, so sibling apps were not searched; name them \
             with --test-app or testApps in .al-sem.json"
                .into(),
        );
        return scope;
    };
    // A root inside an ignored folder (a marked project folder, a fake `.git`
    // in an ignored temporary folder): git reports all of it ignored, so
    // ignore rules cannot tell projects apart there.
    let honor_ignore = !root_is_ignored(&root);
    walk(&mut scope, &root, &abs, 0, honor_ignore);
    scope.search_root = Some((root, git_top));
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

/// The walk does not enter `dir` (below the search root): it is a project of
/// its own that does not hold the workspace. Either a repository of its own
/// (it holds `.git`: a nested clone, a worktree, a submodule), the git
/// boundary dependency discovery stops at too, or a project collection (it
/// holds [`PROJECTS_MARKER`]: each subfolder is an unrelated project). A
/// folder holding an `app.json` is still read as one app before this check,
/// so a test app that is its own repository is found.
fn other_project(dir: &Path, workspace: &Path) -> bool {
    !workspace.starts_with(dir)
        && (dir.join(".git").exists() || dir.join(PROJECTS_MARKER).is_file())
}

/// Depth-first, sorted, over `dir` at `depth` (see the module doc).
/// `honor_ignore` is false below an ignored folder that holds the workspace:
/// git reports everything there as ignored, the workspace's siblings too.
fn walk(scope: &mut Scope, dir: &Path, workspace: &Path, depth: usize, honor_ignore: bool) {
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
    if depth >= MAX_DEPTH || (depth > 0 && other_project(dir, workspace)) {
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
    // Engine limits B8 (TLS-48): a folder the repository's `.gitignore`
    // rules ignore is not a project folder, unless it holds the workspace.
    let ignored = if honor_ignore {
        git_ignored(dir, &subdirs)
    } else {
        std::collections::HashSet::new()
    };
    // Kept although ignored: a folder holding the workspace, and a repository
    // of its own (a nested clone is usually ignored by the outer repository;
    // it is read as an app, else `other_project` stops there).
    subdirs.retain(|d| !ignored.contains(d) || workspace.starts_with(d) || d.join(".git").exists());
    subdirs.sort();
    for d in subdirs {
        let honor = honor_ignore && !ignored.contains(&d);
        walk(scope, &d, workspace, depth + 1, honor);
        if scope.searched >= MAX_APP_JSON && !scope.complete {
            return;
        }
    }
}

/// Whether git ignores `root` itself (`git check-ignore -q .`: exit 0). No
/// repository or no git: not ignored.
fn root_is_ignored(root: &Path) -> bool {
    crate::git_env::git_command()
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", "."])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.code() == Some(0))
}

/// The folders among `subdirs` (children of `dir`) that git ignores, by one
/// `git check-ignore` over them (the repository's `.gitignore` files,
/// `.git/info/exclude` and the user's excludes file). Empty when there are
/// none, or when git cannot answer (not installed, `dir` in no repository):
/// the walk then reads every folder, as it did before this check.
fn git_ignored(dir: &Path, subdirs: &[PathBuf]) -> std::collections::HashSet<PathBuf> {
    use std::io::Write;
    let mut set = std::collections::HashSet::new();
    if subdirs.is_empty() {
        return set;
    }
    // A trailing `/`: the names are folders (git also stats a name without it).
    let mut input = Vec::new();
    for d in subdirs {
        if let Some(name) = d.file_name() {
            input.extend_from_slice(name.as_encoded_bytes());
            input.extend_from_slice(b"/\0");
        }
    }
    let child = crate::git_env::git_command()
        .arg("-C")
        .arg(dir)
        .args(["check-ignore", "--stdin", "-z"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return set;
    };
    // Write from a thread, so a large answer cannot block the write.
    let stdin = child.stdin.take();
    let writer = std::thread::spawn(move || {
        if let Some(mut s) = stdin {
            let _ = s.write_all(&input);
        }
    });
    let Ok(out) = child.wait_with_output() else {
        return set;
    };
    let _ = writer.join();
    // Exit 0: some path is ignored; 1: none; anything else: git could not tell.
    if out.status.code() != Some(0) {
        return set;
    }
    // Match git's answer to the submitted folders by their raw name bytes: a
    // lossy decode turned an ignored `tests-\xff` into a sibling `tests-\u{FFFD}`
    // and skipped that one instead (TLS-48 review sol r1).
    for name in out.stdout.split(|&b| b == 0).filter(|n| !n.is_empty()) {
        let name = name.strip_suffix(b"/").unwrap_or(name);
        if let Some(d) = subdirs
            .iter()
            .find(|d| d.file_name().is_some_and(|f| f.as_encoded_bytes() == name))
        {
            set.insert(d.clone());
        }
    }
    set
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
                "no app under the {} depends on this one with a test codeunit (searched {} app.json {})",
                if self.search_root.as_ref().is_none_or(|r| r.1) {
                    "git top level"
                } else {
                    "project folder (its parent holds .al-sem-projects)"
                },
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
