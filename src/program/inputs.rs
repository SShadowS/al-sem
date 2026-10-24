//! The frozen inputs (#160 step 1, spec §4.2): every file a command reads,
//! read ONCE before the semantic build. Every reader after that reads the
//! set, never the disk, so an edit on disk during a run cannot give two
//! readers two answers.
//!
//! Two halves (#277 spec D3): [`WorkspaceInputs`], what the workspace path
//! itself gives (one `Arc` per path, which several jobs can share), and
//! [`RequestInputs`], the files one command's flags name (policy, baseline).
//!
//! One exception, by design (spec §4.2.2): the dependency `.app` files. The set
//! holds the closure's LIST (and, with `fingerprint_rows`, one hash per file),
//! but the loader still opens each `.app` itself. A `.app` replaced between the
//! two reads gives hash rows of the old bytes and a model of the new ones.
//!
//! `read` never fails: every read error is stored in its slot (the `.al` walk's
//! in `al_walk`), and each reader decides what a missing or unreadable input
//! means, exactly as it did when it read the disk itself.
//!
//! Nothing here special-cases a folder name (epic I11).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::dependencies::DiscoveredAppFile;
use crate::engine::l5::snapshot_full::SnapshotInput;
use crate::engine::perf_trace as pt;
use crate::source_text::{
    NestedApps, count_app_json_paths, dir_has_app_json, discover_al_files, read_al_source,
};

#[cfg(test)]
#[path = "../../tests/common/unreadable.rs"]
mod unreadable;

// shortcut: digest.rs test code (step 9's lane, hands-off for #277) still names
// `InputSet`; remove this alias when digest.rs is free.
pub type InputSet = WorkspaceInputs;

/// Every input the workspace path gives, read once (spec §4.2.1).
pub struct WorkspaceInputs {
    /// The workspace root as the command line spelled it (never canonicalized:
    /// `ws:` unit ids and messages use it as given).
    pub workspace: PathBuf,
    /// `<workspace>/app.json`, the raw bytes: `Err(io)` when it cannot be read.
    pub app_json: FileInput,
    /// The parse of `app_json.text()`: `None` when the bytes are `Err` OR not
    /// UTF-8; `Some(Err(serde message))` when the file is readable UTF-8 but
    /// not JSON.
    pub app_json_value: Option<Result<serde_json::Value, String>>,
    /// Every `app.json` under the root except under the skip folders
    /// (`source_text::count_app_json_paths`), sorted.
    pub app_json_paths: Vec<PathBuf>,
    /// `<workspace>/roots.config.json`: `None` = no file; `Some(Err)` = unreadable.
    pub roots_config: Option<Result<Vec<u8>, String>>,
    /// `<workspace>/al-sem.coverage.yaml` (a fingerprint input only).
    pub coverage_yaml: Option<Vec<u8>>,
    /// The dependency closure the loader reads (`dependencies::discover_app_files`).
    pub closure: ClosureEntries,
    /// The snapshot's reproducibility input rows (`derive_inputs_from_parts`),
    /// present only when [`WorkspaceNeeds::fingerprint_rows`] was asked for.
    pub snapshot_inputs: Option<Vec<SnapshotInput>>,
    /// The `.al` walk itself: `Err` on an unreadable folder or a link loop. Then
    /// `al_files` is EMPTY: a partial list must never reach the build as if it
    /// were the whole app. The slot keeps the `io::Error` itself (its `source()`
    /// chain), not its text; the `Arc` lets a shared set hand `anyhow` an owned error.
    pub al_walk: Result<(), Arc<std::io::Error>>,
    /// Every `.al` file of the `NestedApps::Walk` list, sorted by `rel_posix`.
    pub al_files: Vec<WsFile>,
    /// `driver_version()` read once.
    pub driver_version: String,
    /// `compute_gate_model_instance_id`'s value, from the fields above with the
    /// same algorithm; `None` = fail-closed.
    pub model_instance_id: Option<String>,
    /// `WorkspaceNeeds::build_only` as asked.
    pub build_only: bool,
    /// `rel_posix` -> index into `al_files`.
    index: HashMap<String, usize>,
}

/// One file's path and raw bytes.
pub struct FileInput {
    pub path: PathBuf,
    pub bytes: Result<Vec<u8>, std::io::Error>,
}

impl FileInput {
    fn read(path: PathBuf) -> FileInput {
        let bytes = std::fs::read(&path);
        FileInput { path, bytes }
    }

    /// The bytes as UTF-8. Invalid UTF-8 is the error `std::fs::read_to_string`
    /// returns (kind `InvalidData`, `stream did not contain valid UTF-8`), so a
    /// reader that used `read_to_string` keeps its text and its read-error class.
    pub fn text(&self) -> Result<&str, std::io::Error> {
        match &self.bytes {
            Ok(b) => std::str::from_utf8(b).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                )
            }),
            Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
        }
    }
}

/// The `explicit:` / `auto:` source label of a policy file.
pub enum PolicyOrigin {
    Explicit,
    Auto,
}

pub struct PolicyInput {
    pub origin: PolicyOrigin,
    pub file: FileInput,
}

/// `discover_app_files`'s two lists: the `.app` files, and the `.alpackages`
/// folders whose listing failed.
pub struct ClosureEntries {
    pub files: Vec<DiscoveredAppFile>,
    pub unreadable: Vec<(PathBuf, std::io::Error)>,
}

/// One workspace `.al` file.
pub struct WsFile {
    pub rel_posix: String,
    pub abs_path: PathBuf,
    /// Under a child directory that holds its own `app.json`: the file the
    /// `NestedApps::Skip` walk would not list.
    pub nested_app: bool,
    /// `Err(os message)`: the slot stays so the diagnostics can warn from it.
    pub text: Result<Arc<str>, String>,
}

/// What a command reads from the workspace beyond the build's own inputs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceNeeds {
    /// Only what the snapshot build reads: `app.json`, the `.al` walk and texts,
    /// the closure. Set by `SnapshotBuilder::build*`, by no command. `read` then
    /// skips the `app.json` walk, the `nested_app` checks (all `false`), the
    /// model-instance id (`None`), `roots.config.json`, the coverage yaml and
    /// hash rows. `app_scoped_files()` and `roots_config_loaded()` assert
    /// `!build_only`, so an empty slot is never read as an answer.
    pub build_only: bool,
    pub fingerprint_rows: bool,
}

impl WorkspaceNeeds {
    /// `build_only: true`, the rest default.
    pub fn build_only() -> WorkspaceNeeds {
        WorkspaceNeeds {
            build_only: true,
            ..WorkspaceNeeds::default()
        }
    }

    /// What two jobs on one workspace read together: a set read with the
    /// union serves both (a job never reads a field it did not ask for).
    #[must_use]
    pub fn union(self, other: WorkspaceNeeds) -> WorkspaceNeeds {
        WorkspaceNeeds {
            build_only: self.build_only && other.build_only,
            fingerprint_rows: self.fingerprint_rows || other.fingerprint_rows,
        }
    }
}

/// The files one command's flags name, read once before the build (#277 D3):
/// one per job, never shared.
pub struct RequestInputs {
    /// The policy file the command asked for (see [`PolicyRequest`]). Auto-detect
    /// takes the FIRST of `al-sem.policy.yaml`, `.yml` that exists; an unreadable
    /// `.yaml` is stored as `Err` and does not fall through. The path is absolutized.
    pub policy_file: Option<PolicyInput>,
    /// The policy request read. `policy_file` is `None` both under `--no-policy`
    /// and when auto-detect finds no file; this tells them apart, so the policy
    /// mode comes from the inputs alone.
    pub policy_request: PolicyRequest,
    /// `--baseline <path>`: `None` = no flag; `Some` with `Err(NotFound)` is the
    /// "no baseline yet" case.
    pub baseline: Option<FileInput>,
}

impl RequestInputs {
    /// Read the policy file `policy` names (auto-detect looks in `workspace`)
    /// and the `baseline` file. Never fails: a read error stays in its slot.
    pub fn read(
        workspace: &Path,
        policy: PolicyRequest,
        baseline: Option<PathBuf>,
    ) -> RequestInputs {
        use crate::engine::gate::policy::pipeline::absolutize;
        let policy_file = match &policy {
            PolicyRequest::None | PolicyRequest::Disabled => None,
            PolicyRequest::Path(p) => Some(PolicyInput {
                origin: PolicyOrigin::Explicit,
                file: FileInput::read(absolutize(p)),
            }),
            PolicyRequest::AutoDetect => ["al-sem.policy.yaml", "al-sem.policy.yml"]
                .iter()
                .map(|name| workspace.join(name))
                .find(|candidate| candidate.exists())
                .map(|candidate| PolicyInput {
                    origin: PolicyOrigin::Auto,
                    file: FileInput::read(absolutize(candidate.to_string_lossy().as_ref())),
                }),
        };
        RequestInputs {
            policy_file,
            policy_request: policy,
            baseline: baseline.map(FileInput::read),
        }
    }
}

/// Which policy file a command reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PolicyRequest {
    /// The command takes no policy.
    #[default]
    None,
    /// `al-sem.policy.yaml`, else `.yml`, in the workspace.
    AutoDetect,
    /// `--policy <path>`.
    Path(PathBuf),
    /// `--no-policy`.
    Disabled,
}

impl WorkspaceInputs {
    /// The ONE workspace reader (spec §4.2.2). Charges the `inputs.read` counter.
    pub fn read(workspace: &Path, needs: &WorkspaceNeeds) -> Arc<WorkspaceInputs> {
        pt::counter_delta("inputs.read", 1);
        // A command reads before its own top span (`analyze.total`) opens, so the
        // read is timed here.
        let _s = pt::span("inputs", "inputs.read");
        let build_only = needs.build_only;

        let app_json = FileInput::read(workspace.join("app.json"));
        let app_json_value = app_json
            .text()
            .ok()
            .map(|t| serde_json::from_str(t).map_err(|e| e.to_string()));
        let app_json_paths = if build_only {
            Vec::new()
        } else {
            let mut p = count_app_json_paths(workspace);
            p.sort();
            p
        };

        let (al_walk, al_files) = match discover_al_files(workspace, NestedApps::Walk) {
            Ok(found) => {
                let mut nested_dirs: HashMap<PathBuf, bool> = HashMap::new();
                let files = found
                    .into_iter()
                    .map(|f| WsFile {
                        nested_app: !build_only
                            && is_nested_app(workspace, &f.abs_path, &mut nested_dirs),
                        text: read_al_source(&f.abs_path)
                            .map(Arc::from)
                            .map_err(|e| e.to_string()),
                        rel_posix: f.rel_posix,
                        abs_path: f.abs_path,
                    })
                    .collect();
                (Ok(()), files)
            }
            Err(e) => (Err(Arc::new(e)), Vec::new()),
        };
        let index = al_files
            .iter()
            .enumerate()
            .map(|(i, f): (usize, &WsFile)| (f.rel_posix.clone(), i))
            .collect();

        let (roots_config, coverage_yaml) = if build_only {
            (None, None)
        } else {
            let roots = match std::fs::read(workspace.join("roots.config.json")) {
                Ok(b) => Some(Ok(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => Some(Err(e.to_string())),
            };
            (
                roots,
                std::fs::read(workspace.join("al-sem.coverage.yaml")).ok(),
            )
        };

        let (files, unreadable) = crate::dependencies::discover_app_files(workspace);
        let closure = ClosureEntries { files, unreadable };

        let mut set = WorkspaceInputs {
            workspace: workspace.to_path_buf(),
            app_json,
            app_json_value,
            app_json_paths,
            roots_config,
            coverage_yaml,
            closure,
            snapshot_inputs: None,
            al_walk,
            al_files,
            driver_version: crate::engine::gate::version::driver_version(),
            model_instance_id: None,
            build_only,
            index,
        };
        if !build_only {
            set.model_instance_id = set.compute_model_instance_id();
            if needs.fingerprint_rows {
                set.snapshot_inputs = Some(set.compute_snapshot_inputs());
            }
        }
        Arc::new(set)
    }

    /// The `.al` file at `rel_posix`, if the walk listed it.
    pub fn file(&self, rel_posix: &str) -> Option<&WsFile> {
        self.index.get(rel_posix).map(|&i| &self.al_files[i])
    }

    /// The files the `NestedApps::Skip` walk lists (`!nested_app`), sorted.
    pub fn app_scoped_files(&self) -> impl Iterator<Item = &WsFile> {
        assert!(
            !self.build_only,
            "WorkspaceInputs read with build_only: nested_app was not computed"
        );
        self.al_files.iter().filter(|f| !f.nested_app)
    }

    /// The root `app.json` `id`, verbatim, when it is a non-empty string.
    pub fn app_id(&self) -> Option<&str> {
        let Some(Ok(v)) = &self.app_json_value else {
            return None;
        };
        v.get("id")?.as_str().filter(|id| !id.is_empty())
    }

    /// #160 §4.1.5 / A1b.5: the `inputs` census bucket. It owns the workspace
    /// `.al` texts (the snapshot shares them by `Arc`). Every owned buffer by
    /// `len`; an `io::Error` in a slot is not walked: the census runs after a
    /// successful build, where every held error is an OS error from a read
    /// (no heap) — a failed `al_walk` fails the build first.
    #[must_use]
    pub fn census(&self) -> crate::engine::census::Tally {
        use crate::engine::census::Tally;
        fn path(t: &mut Tally, p: &Path) {
            let n = p.as_os_str().len();
            if n > 0 {
                t.bytes += n;
                t.allocs += 1;
            }
        }
        fn file(t: &mut Tally, f: &FileInput) {
            path(t, &f.path);
            if let Ok(b) = &f.bytes {
                t.buf::<u8>(b.len());
            }
        }
        fn json(t: &mut Tally, v: &serde_json::Value) {
            match v {
                serde_json::Value::String(s) => {
                    t.strs([s]);
                }
                serde_json::Value::Array(a) => {
                    t.buf::<serde_json::Value>(a.len());
                    a.iter().for_each(|v| json(t, v));
                }
                serde_json::Value::Object(m) => {
                    t.buf::<(String, serde_json::Value)>(m.len()).strs(m.keys());
                    m.values().for_each(|v| json(t, v));
                }
                _ => {}
            }
        }
        let mut t = Tally::default();
        t.arc::<WorkspaceInputs>();
        path(&mut t, &self.workspace);
        file(&mut t, &self.app_json);
        match &self.app_json_value {
            Some(Ok(v)) => json(&mut t, v),
            Some(Err(e)) => {
                t.strs([e]);
            }
            None => {}
        }
        t.buf::<PathBuf>(self.app_json_paths.len());
        self.app_json_paths.iter().for_each(|p| path(&mut t, p));
        match &self.roots_config {
            Some(Ok(b)) => {
                t.buf::<u8>(b.len());
            }
            Some(Err(e)) => {
                t.strs([e]);
            }
            None => {}
        }
        if let Some(b) = &self.coverage_yaml {
            t.buf::<u8>(b.len());
        }
        t.buf::<DiscoveredAppFile>(self.closure.files.len());
        for f in &self.closure.files {
            path(&mut t, &f.folder);
            path(&mut t, &f.path);
        }
        t.buf::<(PathBuf, std::io::Error)>(self.closure.unreadable.len());
        self.closure
            .unreadable
            .iter()
            .for_each(|(p, _)| path(&mut t, p));
        if let Some(rows) = &self.snapshot_inputs {
            t.buf::<SnapshotInput>(rows.len());
            for r in rows {
                t.strs([&r.kind, &r.path, &r.content_hash]);
            }
        }
        t.buf::<WsFile>(self.al_files.len());
        for f in &self.al_files {
            t.strs([&f.rel_posix]);
            path(&mut t, &f.abs_path);
            match &f.text {
                // `ArcInner<str>`: the two counts, then the bytes, padded to
                // the counts' alignment (as `program.snapshot_texts` counts it).
                Ok(text) => {
                    t.bytes += (2 * std::mem::size_of::<usize>() + text.len())
                        .next_multiple_of(std::mem::align_of::<usize>());
                    t.allocs += 1;
                }
                Err(e) => {
                    t.strs([e]);
                }
            }
        }
        t.strs([&self.driver_version]);
        if let Some(id) = &self.model_instance_id {
            t.strs([id]);
        }
        t.buf::<(String, usize)>(self.index.len())
            .strs(self.index.keys());
        t
    }

    /// `roots.config.json` exists and loads and validates.
    pub fn roots_config_loaded(&self) -> bool {
        assert!(
            !self.build_only,
            "WorkspaceInputs read with build_only: roots.config.json was not read"
        );
        matches!(&self.roots_config, Some(Ok(b))
            if crate::engine::root_classification::roots_config_bytes_load(b))
    }

    fn compute_model_instance_id(&self) -> Option<String> {
        let app_id = self.app_id()?;
        self.al_walk.as_ref().ok()?;
        let version = match &self.app_json_value {
            Some(Ok(v)) => v.get("version").and_then(|v| v.as_str()),
            _ => None,
        }
        .unwrap_or("0.0.0.0");
        Some(
            crate::engine::gate::model_instance_id::model_instance_id_from(
                app_id,
                version,
                self.al_files.iter().map(|f| f.rel_posix.as_str()),
            ),
        )
    }

    fn compute_snapshot_inputs(&self) -> Vec<SnapshotInput> {
        let closure = crate::program::resolve::semantic_golden::dependency_closure_entries_from(
            &self.workspace,
            &self.closure.files,
            &self.closure.unreadable,
        );
        let roots = match &self.roots_config {
            Some(Ok(b)) if self.roots_config_loaded() => Some(b.as_slice()),
            _ => None,
        };
        crate::engine::l5::snapshot_full::derive_inputs_from_parts(
            &self.workspace,
            self.app_json.bytes.as_deref().ok(),
            closure,
            roots,
            self.coverage_yaml.as_deref(),
        )
    }
}

/// True when a directory between `root` (exclusive) and the file holds its
/// own `app.json`. `cache` keeps one answer per directory. Walks the real
/// path's ancestors, never the lossy `rel_posix`, so a folder name that is not
/// valid Unicode still names a folder that exists.
fn is_nested_app(root: &Path, abs_path: &Path, cache: &mut HashMap<PathBuf, bool>) -> bool {
    abs_path
        .ancestors()
        .skip(1) // the file itself
        .take_while(|dir| *dir != root)
        .any(|dir| {
            *cache
                .entry(dir.to_path_buf())
                .or_insert_with(|| dir_has_app_json(dir))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn model_instance_id_equals_the_gate_function() {
        // durable parity with today's reader while it exists (Task 11 rewires it to this field)
        for ws in [
            "ws-d1-multi-caller",
            "ws-d8-commit-in-tx",
            "ws-d14-dead-routine",
        ] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/r0-corpus")
                .join(ws);
            let inputs = WorkspaceInputs::read(&root, &WorkspaceNeeds::default());
            assert_eq!(
                inputs.model_instance_id,
                crate::engine::gate::model_instance_id::compute_gate_model_instance_id(&root),
                "{ws}"
            );
        }
    }

    #[test]
    fn nested_app_files_are_listed_and_flagged() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#).unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/A.al"), "codeunit 50100 A { }").unwrap();
        std::fs::create_dir_all(d.path().join("sub/src")).unwrap();
        std::fs::write(d.path().join("sub/app.json"), "{}").unwrap();
        std::fs::write(d.path().join("sub/src/B.al"), "codeunit 50101 B { }").unwrap();
        let i = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        let all: Vec<_> = i
            .al_files
            .iter()
            .map(|f| (f.rel_posix.as_str(), f.nested_app))
            .collect();
        assert_eq!(all, [("src/A.al", false), ("sub/src/B.al", true)]);
        assert_eq!(
            i.app_scoped_files()
                .map(|f| f.rel_posix.as_str())
                .collect::<Vec<_>>(),
            ["src/A.al"]
        );
        assert_eq!(i.app_json_paths.len(), 2);
    }

    /// Code review CR6: a nested app under a folder whose name is not valid
    /// Unicode is still flagged. `rel_posix` spells that folder lossily, so a
    /// path rebuilt from it names a folder that does not exist.
    #[test]
    fn nested_app_under_a_non_unicode_folder_is_flagged() {
        #[cfg(windows)]
        let name: std::ffi::OsString =
            std::os::windows::ffi::OsStringExt::from_wide(&[0xD800, u16::from(b'x')]);
        #[cfg(unix)]
        let name: std::ffi::OsString =
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(b"\xffx").to_owned();
        assert!(name.to_str().is_none(), "precondition: not valid Unicode");
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#).unwrap();
        let sub = d.path().join(&name);
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("app.json"), "{}").unwrap();
        std::fs::write(sub.join("B.al"), "codeunit 50101 B { }").unwrap();
        let i = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        assert_eq!(i.al_files.len(), 1, "precondition: the walk lists B.al");
        assert!(i.al_files[0].nested_app, "{}", i.al_files[0].rel_posix);
    }

    #[test]
    fn invalid_utf8_is_the_read_to_string_error() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("app.json");
        std::fs::write(&p, [0xff, 0xfe, 0x00]).unwrap();
        let i = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        let want = std::fs::read_to_string(&p).unwrap_err().to_string();
        assert_eq!(i.app_json.text().unwrap_err().to_string(), want);
        assert!(i.app_json_value.is_none());
    }

    #[test]
    fn build_only_skips_command_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d1-multi-caller");
        let i = WorkspaceInputs::read(&root, &WorkspaceNeeds::build_only());
        assert!(i.build_only && i.model_instance_id.is_none() && i.app_json_paths.is_empty());
        assert!(i.al_files.iter().all(|f| !f.nested_app));
    }

    #[test]
    #[should_panic(expected = "build_only")]
    fn build_only_set_refuses_app_scoped_reads() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/r0-corpus/ws-d1-multi-caller");
        let _ = WorkspaceInputs::read(&root, &WorkspaceNeeds::build_only())
            .app_scoped_files()
            .count();
    }

    #[test]
    fn unreadable_al_file_keeps_its_slot() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#).unwrap();
        let f = d.path().join("A.al");
        std::fs::write(&f, "codeunit 50100 A { }").unwrap();
        let Some(_g) = unreadable::unreadable(&f) else {
            eprintln!("skipped: cannot make {} unreadable here", f.display());
            return;
        };
        let i = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        assert!(i.file("A.al").unwrap().text.is_err());
    }

    /// Auto-detect takes the FIRST policy name that exists, even when it cannot
    /// be read: an unreadable `.yaml` is stored as `Err`, never a fall-through
    /// to `.yml` (spec §4.2.1, `pipeline.rs:72-75`).
    #[test]
    fn auto_detect_keeps_an_unreadable_yaml() {
        let d = tempfile::tempdir().unwrap();
        let yaml = d.path().join("al-sem.policy.yaml");
        std::fs::write(&yaml, "version: 1\n").unwrap();
        std::fs::write(d.path().join("al-sem.policy.yml"), "version: 1\n").unwrap();
        let Some(_g) = unreadable::unreadable(&yaml) else {
            eprintln!("skipped: cannot make {} unreadable here", yaml.display());
            return;
        };
        let i = RequestInputs::read(d.path(), PolicyRequest::AutoDetect, None);
        let p = i.policy_file.as_ref().expect("auto-detect found a file");
        assert!(matches!(p.origin, PolicyOrigin::Auto));
        assert!(p.file.path.is_absolute());
        assert_eq!(p.file.path, yaml);
        assert!(p.file.bytes.is_err());
    }

    /// A set read with the union serves both asks: hash rows if either wants
    /// them, the full read unless both are build-only.
    #[test]
    fn needs_union() {
        let b = WorkspaceNeeds::build_only();
        let f = WorkspaceNeeds {
            fingerprint_rows: true,
            ..WorkspaceNeeds::default()
        };
        assert_eq!(b.union(b), b);
        assert_eq!(
            b.union(f),
            WorkspaceNeeds {
                build_only: false,
                fingerprint_rows: true
            }
        );
        assert_eq!(f.union(WorkspaceNeeds::default()), f);
    }

    const APP_JSON: &str = r#"{"id":"11111111-1111-1111-1111-111111111111","name":"A","publisher":"p","version":"1.0.0.0"}"#;

    /// The workspace source from the set equals the disk walk's, file for file
    /// and hash for hash: a BOM-prefixed file (BOM dropped), a lossy (invalid
    /// UTF-8) file and a nested-app file (listed: the `NestedApps::Walk` list).
    #[test]
    fn workspace_root_equals_walk_al_source() {
        use crate::snapshot::TrustTier;
        use crate::snapshot::provider::{walk_al_source, workspace_source_from_inputs};
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), APP_JSON).unwrap();
        std::fs::write(
            d.path().join("Bom.al"),
            b"\xEF\xBB\xBFcodeunit 50100 Bom { }",
        )
        .unwrap();
        std::fs::write(d.path().join("Bad.al"), b"codeunit 50101 Bad { } // \xff").unwrap();
        std::fs::create_dir_all(d.path().join("sub")).unwrap();
        std::fs::write(d.path().join("sub/app.json"), "{}").unwrap();
        std::fs::write(d.path().join("sub/C.al"), "codeunit 50102 C { }").unwrap();
        let inputs = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::build_only());
        let new = workspace_source_from_inputs(&inputs).unwrap().unwrap();
        let old = walk_al_source(d.path(), TrustTier::Workspace)
            .unwrap()
            .unwrap();
        let files = |r: &crate::snapshot::provider::SourceRoot| {
            r.files
                .iter()
                .map(|f| (f.virtual_path.clone(), f.text.to_string()))
                .collect::<Vec<_>>()
        };
        assert_eq!(files(&new).len(), 3, "precondition: three files listed");
        assert_eq!(files(&new), files(&old));
        assert_eq!(new.content_hash, old.content_hash);
        assert_eq!(new.tier, old.tier);
        // The SAME text, not a copy.
        assert!(Arc::ptr_eq(
            &new.files[0].text,
            inputs.al_files[0].text.as_ref().unwrap()
        ));
    }

    /// No `.al` file is a symbol-only workspace (`Ok(None)`), never an empty root.
    #[test]
    fn no_al_file_is_symbol_only() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), APP_JSON).unwrap();
        let inputs = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::build_only());
        assert!(
            crate::snapshot::provider::workspace_source_from_inputs(&inputs)
                .unwrap()
                .is_none()
        );
    }

    /// Case (b) of A1.7: an unreadable subfolder fails the walk, and the build
    /// fails closed with today's text, link for link (the OS text twice: the
    /// stored `io::Error` keeps its `source()`). The literals were captured from
    /// the build BEFORE it read the set (Task 9's commit, on each platform).
    #[test]
    fn walk_error_fails_the_build() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), APP_JSON).unwrap();
        let sub = d.path().join("locked");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("X.al"), "codeunit 50100 X { }").unwrap();
        let Some(_g) = unreadable::unreadable(&sub) else {
            eprintln!("skipped: cannot make {} unreadable here", sub.display());
            return;
        };
        let inputs = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        assert!(inputs.al_walk.is_err());
        assert!(inputs.al_files.is_empty());
        assert!(inputs.model_instance_id.is_none());
        let err = crate::program::resolve::full::build_program_with_coverage(&inputs)
            .err()
            .expect("build must fail closed");
        let root = d.path().display();
        #[cfg(windows)]
        let want = format!(
            "snapshot build failed: workspace source provider: walking source {root}: Access is denied. (os error 5): Access is denied. (os error 5)"
        );
        #[cfg(unix)]
        let want = format!(
            "snapshot build failed: workspace source provider: walking source {root}: IO error for operation on {p}: Permission denied (os error 13): Permission denied (os error 13)",
            p = sub.display()
        );
        assert_eq!(err, want);
        // The diagnostics return the provider rows alone (here none: one
        // readable `app.json` with an id), never an index row, even when
        // handed an objectless-file list: a failed walk parsed nothing.
        use crate::engine::gate::workspace_diagnostics::compute_workspace_diagnostics;
        assert!(compute_workspace_diagnostics(&inputs, None).is_empty());
        let listed = ["locked/X.al".to_string()];
        assert!(compute_workspace_diagnostics(&inputs, Some(&listed)).is_empty());
    }

    /// Case (a) of A1.7: a directory symlink loop at the root. Unix only: on
    /// Windows a directory link needs a privilege the test user lacks
    /// (`symlink_dir` failed when the literals were captured), so there is no
    /// Windows literal to state.
    #[cfg(unix)]
    #[test]
    fn walk_loop_fails_the_build() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("app.json"), APP_JSON).unwrap();
        std::fs::write(d.path().join("A.al"), "codeunit 50100 A { }").unwrap();
        let link = d.path().join("loop");
        if std::os::unix::fs::symlink(d.path(), &link).is_err() {
            eprintln!("skipped: cannot make {} here", link.display());
            return;
        }
        let inputs = WorkspaceInputs::read(d.path(), &WorkspaceNeeds::default());
        assert!(inputs.al_walk.is_err());
        assert!(inputs.al_files.is_empty());
        assert!(inputs.model_instance_id.is_none());
        let err = crate::program::resolve::full::build_program_with_coverage(&inputs)
            .err()
            .expect("build must fail closed");
        let root = d.path().display();
        let want = format!(
            "snapshot build failed: workspace source provider: walking source {root}: File system loop found: {root}/loop points to an ancestor {root}"
        );
        assert_eq!(err, want);
    }
}
