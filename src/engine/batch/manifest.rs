//! #277 D2.1: the `alsem batch` manifest. Its shape and its stream paths;
//! each job's `argv` is parsed by the binary's own clap `Cli`, so every flag,
//! default and message is the standalone one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::JobStreams;

/// The batches of a run, in manifest order.
#[derive(Debug)]
pub struct Manifest {
    pub batches: Vec<BatchSpec>,
}

#[derive(Debug)]
pub struct BatchSpec {
    pub jobs: Vec<JobSpec>,
}

/// One job: its command line without the program name, and its two streams.
#[derive(Debug)]
pub struct JobSpec {
    pub argv: Vec<String>,
    pub streams: JobStreams,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    batches: Option<Vec<RawBatch>>,
    jobs: Option<Vec<RawJob>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBatch {
    jobs: Vec<RawJob>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    argv: Vec<String>,
    stdout: Option<PathBuf>,
    stderr: Option<PathBuf>,
}

/// A path a job writes (D2.4): a file, or a directory it writes files into
/// (fingerprint's shard directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Dir(PathBuf),
}

/// What one job writes and reads (D2.4), as its command line spells the
/// paths (relative ones resolve against the process cwd, as standalone).
#[derive(Clone, Debug, Default)]
pub struct JobPaths {
    pub writes: Vec<Target>,
    /// Files the job reads: diff file sides, `--baseline` (with
    /// `--update-baseline` too: it merges into the old file, TLS-106),
    /// `--renames`, `--policy`, digest's diff file.
    pub reads: Vec<PathBuf>,
    /// The workspaces it reads.
    pub workspaces: Vec<PathBuf>,
    /// Folders whose packages only it reads (`impact --part main
    /// --package-root`): their `.alpackages` discovery, no source.
    pub package_roots: Vec<PathBuf>,
    /// The one read it rewrites in place: `--update-baseline`'s baseline
    /// (TLS-106). Its named write of that file is the only write of a file
    /// a job reads that the check lets through (TLS-109).
    pub update: Option<PathBuf>,
    /// Targets a refused job's command line names as writes but that it will
    /// not write (ENG-168): the job's own streams may not write a kept file,
    /// or its refusal would still truncate, e.g., the baseline it named.
    /// Another job's write of one is that job's own (#277 final r1 FR-2 keeps
    /// it allowed). A kept directory (TLS-110: a refused `--shard --out D`)
    /// is the same: its own streams may not write under it.
    pub kept: Vec<Target>,
}

/// The files a workspace read takes under its workspace path
/// (`WorkspaceInputs::read`, the policy auto-detect): `*.al`, `*.app` and
/// these names.
const WORKSPACE_FILES: &[&str] = &[
    "app.json",
    "roots.config.json",
    "al-sem.coverage.yaml",
    "al-sem.policy.yaml",
    "al-sem.policy.yml",
    crate::program::source_deps::CONFIG_FILE,
];

/// `p` against `cwd` with `.` and `..` folded as the OS takes them. On unix a
/// `..` goes to the parent of where its prefix leads (a symlink's target), so
/// an existing prefix is canonicalized before it is popped (R9-3); a `..`
/// after a prefix that is not an existing directory stays (TLS-109): the OS
/// cannot go through a missing folder or a file, so `missing/../x` and
/// `file/../x` are not `x`. Windows folds `..` by spelling before it opens a
/// path (a link's or junction's own parent, `file\..\x` is `x`), so there a
/// `..` pops as spelled; but a verbatim path (`\\?\`) goes to the file system
/// as written, so its `..` stays.
fn fold(cwd: &Path, p: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    let mut verbatim = false;
    for c in cwd.join(p).components() {
        match c {
            std::path::Component::Prefix(prefix) => {
                verbatim = prefix.kind().is_verbatim();
                folded.push(c);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir if cfg!(windows) && !verbatim => {
                folded.pop();
            }
            std::path::Component::ParentDir if cfg!(windows) => folded.push(".."),
            std::path::Component::ParentDir => match std::fs::canonicalize(&folded) {
                Ok(canonical) if canonical.is_dir() => {
                    folded = canonical;
                    folded.pop();
                }
                _ => folded.push(".."),
            },
            c => folded.push(c),
        }
    }
    folded
}

/// What one existing file or directory is (TLS-109): its device and inode on
/// unix, its volume serial number and 128-bit file id on Windows
/// ([`super::claim::handle_id`]). A hard link, a bind mount, a junction or
/// any other spelling of it is the same file.
pub type FileId = (u64, u128);

thread_local! {
    /// The first path whose identity [`file_id`] could not read during a
    /// check: [`check_write_sets`] refuses the run then.
    static ID_ERROR: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// `p`'s identity, `None` when nothing is there. Something there whose
/// identity cannot be read is no file without one: the error is kept for
/// [`check_write_sets`], which refuses the run, so no check ever takes a
/// parent's identity for the file's.
fn file_id(p: &Path) -> Option<FileId> {
    match read_file_id(p) {
        Ok(id) => id,
        Err(e) => {
            ID_ERROR.with(|err| {
                err.borrow_mut()
                    .get_or_insert_with(|| format!("{}: {e}", p.display()));
            });
            None
        }
    }
}

/// Whether `e` says there is nothing at the path.
fn absent(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

#[cfg(unix)]
pub(super) fn read_file_id(p: &Path) -> std::io::Result<Option<FileId>> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::metadata(p) {
        Ok(m) => Ok(Some((m.dev(), u128::from(m.ino())))),
        Err(e) if absent(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// From a handle the path opens with no access asked (links followed, as
/// `metadata` follows them; `FILE_FLAG_BACKUP_SEMANTICS` opens a folder too).
#[cfg(windows)]
pub(super) fn read_file_id(p: &Path) -> std::io::Result<Option<FileId>> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
    match std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(p)
    {
        Ok(f) => super::claim::handle_id(&f).map(Some),
        Err(e) if absent(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A path a check compares by the file it names (TLS-109), not by spelling.
#[derive(Clone, Debug)]
struct Ident {
    /// The [`resolve`]d spelling, for messages and file-name tests.
    path: PathBuf,
    /// The [`FileId`] of every existing component, the root first: the last
    /// is the file itself when it exists, else its deepest existing ancestor.
    chain: Vec<FileId>,
    /// The names below `chain`'s last that do not exist yet, as spelled
    /// (lowercased on Windows, a case-insensitive file system).
    rest: Vec<std::ffi::OsString>,
}

impl Ident {
    /// Equal keys: one file, existing or not.
    fn key(&self) -> (Option<&FileId>, &[std::ffi::OsString]) {
        (self.chain.last(), &self.rest)
    }

    /// Whether this is `d` or lies under it, through its existing ancestors'
    /// identities (a bind mount or a linked spelling of `d` included).
    fn under(&self, d: &Ident) -> bool {
        match d.chain.last() {
            Some(top) if d.rest.is_empty() => self.chain.contains(top),
            top => self.chain.last() == top && self.rest.starts_with(&d.rest),
        }
    }

    /// Whether this file sits directly in the directory `d`.
    fn parent_is(&self, d: &Ident) -> bool {
        match self.rest.split_last() {
            Some((_, up)) => self.chain.last() == d.chain.last() && up == d.rest,
            None => {
                d.rest.is_empty()
                    && self.chain.len() >= 2
                    && d.chain.last() == self.chain.get(self.chain.len() - 2)
            }
        }
    }
}

/// A refused job's named directory (TLS-110): its [`Ident`], and the
/// [`FileId`] of every file in it now (links not followed). Its own stream
/// that is a hard link to one of them would write it. A ready job's
/// directory needs no walk: a hard link to a file in it meets that file's
/// write at its claim ([`super::claim`]).
struct KeptDir {
    id: Ident,
    files: HashSet<FileId>,
}

impl KeptDir {
    /// `Err`: the walk failed, so the check cannot say what is inside.
    fn new(id: Ident) -> Result<KeptDir, String> {
        let mut files = HashSet::new();
        if id.rest.is_empty() {
            for e in walkdir::WalkDir::new(&id.path).follow_links(false) {
                let e = e.map_err(|e| format!("cannot list {}: {e}", id.path.display()))?;
                if e.file_type().is_file() {
                    files.extend(file_id(e.path()));
                }
            }
        }
        Ok(KeptDir { id, files })
    }

    /// Whether `f` is this directory, under it, or a file in it.
    fn holds(&self, f: &Ident) -> bool {
        f.under(&self.id)
            || (f.rest.is_empty() && f.chain.last().is_some_and(|i| self.files.contains(i)))
    }
}

/// `p` [`fold`]ed, then its deepest existing ancestor canonicalized and the
/// rest appended (R8-1), with the identity of every existing component
/// (TLS-109): two spellings of one file through a symlink, a hard link, a
/// bind mount, a Windows case difference or an 8.3 name are one [`Ident`].
/// A dangling link is followed through its text (at most 40 links), so two
/// links to one missing file are one file.
fn resolve(cwd: &Path, p: &Path) -> Ident {
    resolve_hops(cwd, p, 0)
}

/// A name that does not exist yet as the file system will match it: lowercased
/// on Windows, a case-insensitive file system. In a folder set case-sensitive
/// this can only make two names one, a refusal of a run with no clash, never
/// a clash let through; and the claim at the open compares the files.
fn spelled(name: &std::ffi::OsStr) -> std::ffi::OsString {
    if cfg!(windows) {
        name.to_string_lossy().to_lowercase().into()
    } else {
        name.to_os_string()
    }
}

fn resolve_hops(cwd: &Path, p: &Path, hops: u32) -> Ident {
    let lexical = fold(cwd, p);
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut base = lexical.as_path();
    loop {
        match std::fs::canonicalize(base) {
            Ok(canonical) => {
                let mut chain: Vec<FileId> = canonical.ancestors().filter_map(file_id).collect();
                chain.reverse();
                rest.reverse();
                let path = rest.iter().fold(canonical, |p, name| p.join(name));
                return Ident { path, chain, rest };
            }
            // A dangling link (or junction): where its text leads.
            Err(e)
                if absent(&e)
                    && hops < 40
                    && let Ok(target) = std::fs::read_link(base) =>
            {
                let mut id = resolve_hops(base.parent().unwrap_or(cwd), &target, hops + 1);
                for name in rest.into_iter().rev() {
                    id.path.push(&name);
                    id.rest.push(name);
                }
                return id;
            }
            Err(e) if absent(&e) => {}
            // Something there that the name lookup could not normalize (a
            // folder it may not list, an SMB name it cannot normalize): its
            // identity from the path itself, and every ancestor's. One that
            // cannot be read refuses the run ([`file_id`]); only absence
            // counts as missing.
            Err(_) if file_id(base).is_some() => {
                let mut chain: Vec<FileId> = base.ancestors().filter_map(file_id).collect();
                chain.reverse();
                rest.reverse();
                let path = rest.iter().fold(base.to_path_buf(), |p, name| p.join(name));
                return Ident { path, chain, rest };
            }
            Err(_) => {}
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(spelled(name));
                base = parent;
            }
            _ => {
                let rest = lexical.iter().map(spelled).collect();
                return Ident {
                    path: lexical,
                    chain: Vec::new(),
                    rest,
                };
            }
        }
    }
}

/// Whether a workspace read of `ws` takes the file `f` (`WorkspaceInputs::read`,
/// the policy auto-detect, the dependency loader): an `.al` or `.app` file or
/// one of [`WORKSPACE_FILES`] under `ws` (any case, as the loader matches),
/// or an `.app` in an ancestor `.alpackages` the loader reads (R8-3). The
/// loader walks the ancestors of the workspace AS SPELLED, so its candidate
/// folders come from the job's spelling, `..` unfolded (R9-4, TLS-109), and
/// are compared [`resolve`]d. `f` is resolved; `ws` is the job's spelling. For a relative
/// spelling the walk climbs the cwd's ancestors too, a superset of the
/// loader's (R10-5): it can refuse a write where no real clash exists, but
/// it never misses one. `taken`: the workspace's [`taken_files`], which
/// holds the targets of the links a read follows.
///
/// Everything that depends on the workspace alone is computed once, in
/// [`WorkspaceRead::new`]: it resolves paths through the file system, and
/// redoing it for every pair of a job and a write made a manifest of n jobs
/// cost n² of those calls (G14: 800 jobs 30 s, 5,488 over ten minutes).
struct WorkspaceRead {
    root: Ident,
    /// Whether the read takes the folder's sources (`false`: a package root,
    /// whose `.alpackages` discovery only is read).
    sources: bool,
    /// The loader's candidate `.alpackages` folders (`package_cache_scope`).
    candidates: Vec<Ident>,
    taken: Taken,
    /// The source-folder dependencies the read also takes (G4), resolved.
    source_roots: Vec<Ident>,
}

impl WorkspaceRead {
    /// `sources`: see the field. `Err`: a folder could not be listed, so the
    /// check cannot say what the read takes.
    fn new(cwd: &Path, ws: &Path, sources: bool) -> Result<WorkspaceRead, String> {
        // The loader reads the workspace as spelled, `..` included (TLS-109).
        let spelled = cwd.join(ws);
        let scope = crate::dependencies::package_cache_scope(&spelled);
        let roots = if sources {
            crate::program::source_deps::configured_roots(&fold(cwd, ws))
        } else {
            Vec::new()
        };
        let mut taken = Taken::default();
        if sources {
            taken = taken_files(&fold(cwd, ws))?;
        }
        // A source root's files are inputs too, under any name (TLS-109).
        for r in &roots {
            let t = taken_files(&fold(cwd, r))?;
            taken.files.extend(t.files);
            taken.linked_dirs.extend(t.linked_dirs);
        }
        taken.files.extend(
            crate::dependencies::discover_app_files(&spelled)
                .0
                .into_iter()
                .filter_map(|a| file_id(&a.path)),
        );
        Ok(WorkspaceRead {
            root: resolve(cwd, ws),
            sources,
            candidates: scope.candidates.iter().map(|c| resolve(cwd, c)).collect(),
            taken,
            source_roots: roots.iter().map(|r| resolve(cwd, r)).collect(),
        })
    }

    /// Whether the read takes the file `f`, by identity (TLS-109).
    fn takes(&self, f: &Ident) -> bool {
        let input = is_input_name(&f.path);
        let under = |root: &Ident| input && f.under(root);
        let app = f
            .path
            .file_name()
            .is_some_and(crate::dependencies::is_app_file);
        (self.sources && under(&self.root))
            || (app && self.candidates.iter().any(|c| f.parent_is(c)))
            || (f.rest.is_empty() && f.chain.last().is_some_and(|i| self.taken.files.contains(i)))
            || (input && self.taken.linked_dirs.iter().any(|d| f.chain.contains(d)))
            || self.source_roots.iter().any(under)
    }
}

/// An `.al` or `.app` file or one of [`WORKSPACE_FILES`] (any case, as the
/// loader matches).
fn is_input_name(f: &Path) -> bool {
    crate::source_text::has_al_extension(f)
        || f.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))
        || f.file_name()
            .is_some_and(|n| WORKSPACE_FILES.iter().any(|w| n.eq_ignore_ascii_case(w)))
}

/// What a workspace read of `ws` takes through links ([`taken_files`]).
#[derive(Default)]
struct Taken {
    /// The [`FileId`] of every file it takes that exists now (TLS-109: a
    /// hard link to one, under any name, is that file).
    files: HashSet<FileId>,
    /// The [`FileId`] of every linked directory the walk enters: an
    /// [`is_input_name`] file beneath one is taken, existing or not (#277
    /// final r2 R2-2).
    linked_dirs: Vec<FileId>,
}

/// What a workspace read of `ws` takes through links (#277 final r1 FR-5):
/// the source walk follows links (`source_text::discover_al_files`), and so
/// do the config reads and the package loader
/// (`dependencies::discover_app_files`), so a link inside the workspace to a
/// file or a directory outside it is read at its target. The walk takes
/// every [`is_input_name`] file under `ws`, past the folders the source walk
/// skips, a superset of what a read takes. A linked `.alpackages` is a
/// package candidate, which [`WorkspaceRead::new`] reads itself. A folder
/// that does not exist takes nothing; any other walk error is an `Err`
/// (TLS-109: a folder it cannot list may hold an input).
fn taken_files(root: &Path) -> Result<Taken, String> {
    let mut taken = Taken::default();
    if !root.exists() {
        return Ok(taken);
    }
    let walked = walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !crate::source_text::is_skipped_dir_name(e.file_name())
        });
    for e in walked {
        let e = match e {
            Ok(e) => e,
            // A dangling link: the source walk skips it too
            // (`source_text::discover_al_files`), so it is no input.
            Err(e)
                if e.io_error().map(std::io::Error::kind) == Some(std::io::ErrorKind::NotFound) =>
            {
                continue;
            }
            Err(e) => return Err(format!("cannot list the workspace {}: {e}", root.display())),
        };
        if e.file_type().is_dir() && e.path_is_symlink() {
            taken.linked_dirs.extend(file_id(e.path()));
        } else if e.file_type().is_file() && is_input_name(e.path()) {
            taken.files.extend(file_id(e.path()));
        }
    }
    Ok(taken)
}

/// A [`Target`] by identity (TLS-109).
enum Written {
    File(Ident),
    Dir(Ident),
}

impl Written {
    fn path(&self) -> &Path {
        match self {
            Written::File(f) | Written::Dir(f) => &f.path,
        }
    }
}

/// Every write target of the run checked before any build (D2.4, 12A-10):
/// two targets naming one file (across jobs, or one job's stdout and stderr),
/// a file inside a directory target or two nested directory targets, a file
/// one job reads that any write but its own `--update-baseline` writes
/// (`JobPaths::update`), a file any job writes that a workspace read would
/// take, and a refused job's named file or directory its own stream writes
/// (`JobPaths::kept`, TLS-110). Every comparison is by file identity
/// (TLS-109, see [`Ident`]): a hard link, symlink, bind mount or `..`
/// spelling of a file is that file.
///
/// Before the checks it claims every existing file the run must not write
/// ([`super::claim`]): the files jobs read and workspace inputs (a baseline
/// an update rewrites is reserved for that one write), and lists per refused
/// job the existing files it names. So a write that gets past the checks (a
/// path changed after them, a shard child) still meets them when it opens
/// its file. `jobs`: `(name, paths)`, `name` as the error line names the job.
/// `Err`: the one usage line. It runs before any job's file is opened; a
/// caller opens nothing of its own (the trace file) unless it is `Ok`. A
/// file whose identity cannot be read refuses the run ([`file_id`]).
///
/// TLS-116: first it makes the engine's cache folders and takes their
/// snapshot ([`super::cache_guard`]), before any write is resolved; a write in
/// a cache folder, one that holds one, or one that is a cache file refuses the
/// run. Once the check passes, the snapshot goes to the claim folder.
pub fn check_write_sets(jobs: &[(String, JobPaths)], cwd: &Path) -> Result<(), String> {
    let snap = super::cache_guard::take()?;
    ID_ERROR.with(|e| e.borrow_mut().take());
    let checked = check_identities(jobs, cwd, &snap);
    let checked = match ID_ERROR.with(|e| e.borrow_mut().take()) {
        Some(e) => Err(format!(
            "al-sem batch: cannot read the identity of {e}; the run cannot be checked"
        )),
        None => checked,
    };
    match super::claim::dir() {
        Some(dir) if checked.is_ok() => super::cache_guard::publish(snap, dir)
            .map_err(|e| format!("al-sem batch: cannot keep the cache snapshot: {e}")),
        _ => checked,
    }
}

/// `p`'s [`resolve`]d identities (TLS-116, a claim's cache check): every
/// existing component's, links followed, and whether the last is `p` itself.
pub(super) fn identity_chain(p: &Path) -> std::io::Result<(Vec<FileId>, bool)> {
    let cwd = std::env::current_dir()?;
    ID_ERROR.with(|e| e.borrow_mut().take());
    let id = resolve(&cwd, p);
    if let Some(e) = ID_ERROR.with(|e| e.borrow_mut().take()) {
        return Err(std::io::Error::other(format!(
            "cannot read the identity of {e}"
        )));
    }
    let exists = id.rest.is_empty() && !id.chain.is_empty();
    Ok((id.chain, exists))
}

/// [`check_write_sets`]'s checks.
fn check_identities(
    jobs: &[(String, JobPaths)],
    cwd: &Path,
    snap: &super::cache_guard::Snapshot,
) -> Result<(), String> {
    use super::claim::{keep, marker_of, protect, reserve, write_key};
    use std::collections::btree_map::Entry;
    let fail = |why: String| Err(format!("al-sem batch: {why}"));
    // Every write: its job, whether it is one of the job's two streams
    // (`JobPaths::writes` lists them first), its spelling, and what it is.
    let writes: Vec<(usize, bool, &Path, Written)> = jobs
        .iter()
        .enumerate()
        .flat_map(|(j, (_, p))| {
            p.writes.iter().enumerate().map(move |(k, t)| match t {
                Target::File(f) => (j, k < 2, f.as_path(), Written::File(resolve(cwd, f))),
                Target::Dir(d) => (j, k < 2, d.as_path(), Written::Dir(resolve(cwd, d))),
            })
        })
        .collect();
    let name = |j: usize| &jobs[j].0;
    // Where each written file sits in `writes`, and every directory target
    // (few: `--out-dir`-style targets). Each check below asks these instead of
    // scanning every pair, which made a manifest of n jobs O(n²) (G14). The
    // answers, and the first clash reported, are those of the pair scan.
    let mut files: HashMap<(Option<&FileId>, &[std::ffi::OsString]), Vec<usize>> = HashMap::new();
    let mut dirs: Vec<usize> = Vec::new();
    for (pos, (_, _, _, t)) in writes.iter().enumerate() {
        match t {
            Written::File(f) => files.entry(f.key()).or_default().push(pos),
            Written::Dir(_) => dirs.push(pos),
        }
    }
    // The writes of the file `id` is.
    let same = |id: &Ident| {
        files
            .get(&id.key())
            .cloned()
            .unwrap_or_default()
            .into_iter()
    };
    let to_err = |e: String| format!("al-sem batch: {e}");
    // Each distinct workspace read (sources too, or packages only) and kept
    // directory, built once (G14).
    let mut ws_reads: BTreeMap<(&PathBuf, bool), WorkspaceRead> = BTreeMap::new();
    let mut kept_dirs: BTreeMap<&PathBuf, KeptDir> = BTreeMap::new();
    for (_, p) in jobs {
        let folders = p.workspaces.iter().map(|w| (w, true));
        for (ws, sources) in folders.chain(p.package_roots.iter().map(|w| (w, false))) {
            if let Entry::Vacant(e) = ws_reads.entry((ws, sources)) {
                e.insert(WorkspaceRead::new(cwd, ws, sources).map_err(to_err)?);
            }
        }
        for k in &p.kept {
            if let Target::Dir(d) = k
                && let Entry::Vacant(e) = kept_dirs.entry(d)
            {
                e.insert(KeptDir::new(resolve(cwd, d)).map_err(to_err)?);
            }
        }
    }
    // Each job's `--update-baseline` write: its position in `writes`, when
    // the job names one (the one exemption of the reads check).
    let updates: Vec<Option<(Ident, usize)>> = jobs
        .iter()
        .enumerate()
        .map(|(r, (_, p))| {
            let u = resolve(cwd, p.update.as_deref()?);
            let w = same(&u).find(|&w| writes[w].0 == r && !writes[w].1)?;
            Some((u, w))
        })
        .collect();
    // The claims (see the doc), whatever the checks then say.
    let claimed = |e: std::io::Error| format!("al-sem batch: cannot claim an input: {e}");
    let existing = |id: &Ident| id.chain.last().filter(|_| id.rest.is_empty()).cloned();
    for (r, (_, p)) in jobs.iter().enumerate() {
        for read in &p.reads {
            let read = resolve(cwd, read);
            let Some(fid) = existing(&read) else { continue };
            match &updates[r] {
                Some((u, w)) if u.key() == read.key() => {
                    reserve(&marker_of(&fid), &write_key(name(r), writes[*w].2))
                }
                _ => protect(&marker_of(&fid)),
            }
            .map_err(claimed)?;
        }
        // What a refused job names: its own writes may not open it (TLS-110),
        // another job's may. Per job, so another writer lifts nothing.
        let mut kept = Vec::new();
        for k in &p.kept {
            match k {
                Target::File(f) => kept.extend(existing(&resolve(cwd, f)).map(|i| marker_of(&i))),
                Target::Dir(d) => kept.extend(kept_dirs[d].files.iter().map(marker_of)),
            }
        }
        keep(name(r), &kept).map_err(claimed)?;
    }
    for read in ws_reads.values() {
        for fid in &read.taken.files {
            protect(&marker_of(fid)).map_err(claimed)?;
        }
    }
    // TLS-116: no write in a cache folder, none that is a cache file under
    // any name, none that holds a cache folder (each claim checks its file
    // again at its open).
    let roots: Vec<Ident> = snap.roots.iter().map(|r| resolve(cwd, r)).collect();
    for (j, _, _, t) in &writes {
        let (Written::File(w) | Written::Dir(w)) = t;
        let why = if w.chain.iter().any(|id| snap.dirs.contains(id)) {
            "is in a cache folder of the engine".to_string()
        } else if existing(w).is_some_and(|id| snap.files.contains(&id)) {
            "is a file of the engine's caches".to_string()
        } else if let Some(r) = roots.iter().find(|r| r.under(w)) {
            format!("holds the cache folder {}", r.path.display())
        } else {
            continue;
        };
        return fail(format!(
            "{} writes {}, which {why}: no job may write the engine's caches (TLS-116)",
            name(*j),
            w.path.display()
        ));
    }
    // Two targets that clash: the first write with a later partner, then its
    // first such partner.
    for (i, (a, _, _, ta)) in writes.iter().enumerate() {
        let mut first: Option<(usize, String)> = None;
        let mut consider = |j: usize, why: String| {
            if j > i && first.as_ref().is_none_or(|(k, _)| j < *k) {
                first = Some((j, why));
            }
        };
        match ta {
            Written::File(x) => {
                if let Some(&j) = files[&x.key()].iter().find(|&&j| j > i) {
                    consider(j, format!("both write {}", x.path.display()));
                }
                for &j in &dirs {
                    if let Written::Dir(d) = &writes[j].3
                        && x.under(d)
                    {
                        let (x, d) = (x.path.display(), d.path.display());
                        consider(j, format!("{x} is inside the directory {d}"));
                    }
                }
            }
            Written::Dir(x) => {
                let partner =
                    writes
                        .iter()
                        .enumerate()
                        .skip(i + 1)
                        .find_map(|(j, (_, _, _, tb))| {
                            let xp = x.path.display();
                            let why = match tb {
                                Written::File(f) => f.under(x).then(|| {
                                    format!("{} is inside the directory {xp}", f.path.display())
                                }),
                                Written::Dir(y) => (x.under(y) || y.under(x)).then(|| {
                                    format!("the directories {xp} and {} nest", y.path.display())
                                }),
                            };
                            why.map(|why| (j, why))
                        });
                if let Some((j, why)) = partner {
                    consider(j, why);
                }
            }
        }
        if let Some((j, why)) = first {
            return fail(format!("{} and {}: {why}", name(*a), name(writes[j].0)));
        }
    }
    // Per distinct workspace: the writes (positions in `writes`, in order) its
    // read takes. Whether a read takes a file does not depend on which job
    // reads, so each pair of a workspace and a write is asked once.
    let mut taken: BTreeMap<(&PathBuf, bool), Vec<usize>> = BTreeMap::new();
    for (r, (_, paths)) in jobs.iter().enumerate() {
        for kept in &paths.kept {
            match kept {
                // A refused job's named file: its own streams may not write it.
                Target::File(k) => {
                    let k = resolve(cwd, k);
                    if same(&k).any(|w| writes[w].0 == r) {
                        return fail(format!(
                            "{} names {} but is refused, so it must stay as it is; its own stream writes it",
                            name(r),
                            k.path.display()
                        ));
                    }
                }
                // Its named directory (TLS-110): its own streams may not write
                // in it either. Another job's write there is its own, as for a
                // kept file: the refused job writes nothing it could lose.
                Target::Dir(d) => {
                    let kd = &kept_dirs[d];
                    let own = writes.iter().find(|(j, _, _, t)| {
                        *j == r
                            && match t {
                                Written::File(f) => kd.holds(f),
                                Written::Dir(y) => kd.holds(y) || kd.id.under(y),
                            }
                    });
                    if let Some((_, _, _, t)) = own {
                        return fail(format!(
                            "{} names the directory {} but is refused, so it must stay as it is; its own stream {} is in it",
                            name(r),
                            kd.id.path.display(),
                            t.path().display()
                        ));
                    }
                }
            }
        }
        for read in &paths.reads {
            let read = resolve(cwd, read);
            let in_dir = dirs
                .iter()
                .copied()
                .filter(|&j| matches!(&writes[j].3, Written::Dir(d) if read.under(d)));
            // Any write but the job's own `--update-baseline` of this file
            // (TLS-109): a diff output on its own input would destroy it.
            let update = updates[r]
                .as_ref()
                .filter(|(u, _)| u.key() == read.key())
                .map(|(_, w)| *w);
            let hit = same(&read)
                .chain(in_dir)
                .filter(|&w| Some(w) != update)
                .min();
            if let Some(w) = hit {
                return fail(format!(
                    "{} reads {}, which {} writes",
                    name(r),
                    read.path.display(),
                    name(writes[w].0)
                ));
            }
        }
        let folders = paths.workspaces.iter().map(|w| (w, true));
        for key in folders.chain(paths.package_roots.iter().map(|w| (w, false))) {
            let read = &ws_reads[&key];
            let hits = taken.entry(key).or_insert_with(|| {
                writes
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, _, _, t))| matches!(t, Written::File(f) if read.takes(f)))
                    .map(|(w, _)| w)
                    .collect()
            });
            // Any write, its own job's too (TLS-109): a stream or `--out` on a
            // source of its own workspace would destroy it.
            if let Some(&w) = hits.first() {
                return fail(format!(
                    "{} reads the workspace {}, where {} writes {}",
                    name(r),
                    read.root.path.display(),
                    name(writes[w].0),
                    writes[w].3.path().display()
                ));
            }
        }
    }
    Ok(())
}

/// Every stream's directory exists, `--out-dir` too (D2.1; #277 final r1
/// FR-13): a missing one is a usage error before any build, not a write
/// failure of every job after the whole batch has run.
pub fn check_stream_dirs(manifest: &Manifest, out_dir: Option<&Path>) -> Result<(), String> {
    if let Some(dir) = out_dir.filter(|d| !d.is_dir()) {
        return Err(format!(
            "al-sem batch: --out-dir {}: not a directory",
            dir.display()
        ));
    }
    for (b, batch) in manifest.batches.iter().enumerate() {
        for (i, job) in batch.jobs.iter().enumerate() {
            for stream in [&job.streams.stdout, &job.streams.stderr] {
                if stream.parent().is_some_and(|d| !d.is_dir()) {
                    return Err(format!(
                        "al-sem batch: batch {b} job {i}: no directory for {}",
                        stream.display()
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Read the manifest at `path`. `{"jobs": [...]}` is one batch. A job's
/// streams default to `DIR/<batch>.<job>.stdout` / `.stderr` (`DIR`: `out_dir`,
/// else the manifest's directory); a relative path given in the manifest
/// resolves against the manifest's directory. `Err`: the one usage line
/// (exit 3, nothing runs), also for a manifest, or a batch, with no job.
pub fn load(path: &Path, out_dir: Option<&Path>) -> Result<Manifest, String> {
    let fail = |why: String| format!("al-sem batch: {}: {why}", path.display());
    let text = std::fs::read_to_string(path).map_err(|e| fail(e.to_string()))?;
    let raw: RawManifest = serde_json::from_str(&text).map_err(|e| fail(e.to_string()))?;
    let batches = match (raw.batches, raw.jobs) {
        (Some(batches), None) => batches,
        (None, Some(jobs)) => vec![RawBatch { jobs }],
        _ => return Err(fail("give either \"batches\" or \"jobs\"".to_string())),
    };
    if batches.is_empty() {
        return Err(fail("no batch".to_string()));
    }
    let absolute = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    // A bare `m.json` has the parent "", which `absolute` refuses: it is ".".
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    let base = absolute(parent.unwrap_or(Path::new(".")));
    let dir = out_dir.map_or_else(|| base.clone(), absolute);
    batches
        .into_iter()
        .enumerate()
        .map(|(b, batch)| {
            if batch.jobs.is_empty() {
                return Err(format!("al-sem batch: batch {b}: no job"));
            }
            let jobs = batch.jobs.into_iter().enumerate().map(|(i, job)| {
                let stream = |given: Option<PathBuf>, ext: &str| {
                    given.map_or_else(|| dir.join(format!("{b}.{i}.{ext}")), |p| base.join(p))
                };
                JobSpec {
                    argv: job.argv,
                    streams: JobStreams {
                        stdout: stream(job.stdout, "stdout"),
                        stderr: stream(job.stderr, "stderr"),
                    },
                }
            });
            Ok(BatchSpec {
                jobs: jobs.collect(),
            })
        })
        .collect::<Result<_, _>>()
        .map(|batches| Manifest { batches })
}
