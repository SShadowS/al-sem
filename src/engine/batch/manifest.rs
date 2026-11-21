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
    /// Targets a refused job's command line names as writes but that it will
    /// not write (ENG-168): the job's own streams may not write a kept file,
    /// or its refusal would still truncate, e.g., the baseline it named.
    /// Another job's write of one is that job's own (#277 final r1 FR-2 keeps
    /// it allowed). A kept directory (TLS-110: a refused `--shard --out D`)
    /// takes no write under it at all, from its own streams or another job.
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

/// `p` against `cwd` with `.` and `..` folded as the OS takes them: a `..`
/// goes to the parent of where its prefix leads (a symlink's target), so an
/// existing prefix is canonicalized before it is popped (R9-3); a prefix that
/// does not exist pops as spelled.
fn fold(cwd: &Path, p: &Path) -> PathBuf {
    let mut folded = PathBuf::new();
    for c in cwd.join(p).components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if let Ok(canonical) = std::fs::canonicalize(&folded) {
                    folded = canonical;
                }
                folded.pop();
            }
            c => folded.push(c),
        }
    }
    folded
}

/// What one existing file or directory is (TLS-109): its device and inode on
/// unix, so a hard link, a bind mount or any other spelling of it is the same
/// file. Elsewhere (Windows) its canonical path, so every check there is the
/// path comparison it was before TLS-109 (a hard link is not caught there).
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = PathBuf;

#[cfg(unix)]
fn file_id(p: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(not(unix))]
fn file_id(p: &Path) -> Option<FileId> {
    std::fs::canonicalize(p).ok()
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

/// A directory target (TLS-109): its [`Ident`], and when it exists the
/// [`FileId`] of itself and of everything under it, links followed. A file
/// whose identity chain meets that set is written inside it, also a hard
/// link to a file in it or a path through a link out of it.
struct DirIdent {
    id: Ident,
    inside: HashSet<FileId>,
}

impl DirIdent {
    fn new(id: Ident) -> DirIdent {
        let inside = if id.rest.is_empty() {
            walkdir::WalkDir::new(&id.path)
                .follow_links(true)
                .into_iter()
                .flatten()
                .filter_map(|e| file_id(e.path()))
                .collect()
        } else {
            HashSet::new()
        };
        DirIdent { id, inside }
    }

    /// Whether `f` is this directory or written under it.
    fn holds(&self, f: &Ident) -> bool {
        if self.inside.is_empty() {
            f.under(&self.id)
        } else {
            f.chain.iter().any(|i| self.inside.contains(i))
        }
    }
}

/// `p` [`fold`]ed, then its deepest existing ancestor canonicalized and the
/// rest appended (R8-1), with the identity of every existing component
/// (TLS-109): two spellings of one file through a symlink, a hard link, a
/// bind mount, a Windows case difference or an 8.3 name are one [`Ident`].
fn resolve(cwd: &Path, p: &Path) -> Ident {
    let lexical = fold(cwd, p);
    let mut rest = Vec::new();
    let mut base = lexical.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(base) {
            let mut chain: Vec<FileId> = canonical.ancestors().filter_map(file_id).collect();
            chain.reverse();
            let rest: Vec<std::ffi::OsString> = rest
                .into_iter()
                .rev()
                .map(|name: std::ffi::OsString| {
                    if cfg!(windows) {
                        name.to_string_lossy().to_lowercase().into()
                    } else {
                        name
                    }
                })
                .collect();
            let path = rest.iter().fold(canonical, |p, name| p.join(name));
            return Ident { path, chain, rest };
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => {
                let rest = lexical.iter().map(|c| c.to_os_string()).collect();
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
/// folders come from the [`fold`]ed spelling and are compared [`resolve`]d
/// (R9-4). `f` is resolved; `ws` is the job's spelling. For a relative
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
    /// The loader's candidate `.alpackages` folders (`package_cache_scope`).
    candidates: Vec<Ident>,
    taken: Taken,
    /// The source-folder dependencies the read also takes (G4), resolved.
    source_roots: Vec<Ident>,
}

impl WorkspaceRead {
    fn new(cwd: &Path, ws: &Path) -> WorkspaceRead {
        let spelled = crate::dependencies::package_cache_scope(&fold(cwd, ws));
        WorkspaceRead {
            root: resolve(cwd, ws),
            candidates: spelled.candidates.iter().map(|c| resolve(cwd, c)).collect(),
            taken: taken_files(cwd, ws),
            source_roots: crate::program::source_deps::configured_roots(&fold(cwd, ws))
                .iter()
                .map(|r| resolve(cwd, r))
                .collect(),
        }
    }

    /// Whether the read takes the file `f`, by identity (TLS-109).
    fn takes(&self, f: &Ident) -> bool {
        let input = is_input_name(&f.path);
        let under = |root: &Ident| input && f.under(root);
        let app = f
            .path
            .file_name()
            .is_some_and(crate::dependencies::is_app_file);
        under(&self.root)
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
/// package candidate, which [`read_by_workspace`] resolves itself.
fn taken_files(cwd: &Path, ws: &Path) -> Taken {
    let root = fold(cwd, ws);
    let mut taken = Taken {
        files: HashSet::new(),
        linked_dirs: Vec::new(),
    };
    let walked = walkdir::WalkDir::new(&root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !crate::source_text::is_skipped_dir_name(e.file_name())
        })
        .flatten();
    for e in walked {
        if e.file_type().is_dir() && e.path_is_symlink() {
            taken.linked_dirs.extend(file_id(e.path()));
        } else if e.file_type().is_file() && is_input_name(e.path()) {
            taken.files.extend(file_id(e.path()));
        }
    }
    let packages = crate::dependencies::discover_app_files(&root).0;
    taken
        .files
        .extend(packages.into_iter().filter_map(|a| file_id(&a.path)));
    taken
}

/// A [`Target`] by identity (TLS-109).
enum Written {
    File(Ident),
    Dir(DirIdent),
}

impl Written {
    fn path(&self) -> &Path {
        match self {
            Written::File(f) => &f.path,
            Written::Dir(d) => &d.id.path,
        }
    }
}

/// Every write target of the run checked before any build (D2.4, 12A-10):
/// two targets naming one file (across jobs, or one job's stdout and stderr),
/// a file inside a directory target or two nested directory targets, a file
/// one job reads that another writes, and a file another job writes that a
/// workspace read would take, and a refused job's named file its own stream
/// writes (`JobPaths::kept`), and any write under a refused job's named
/// directory (TLS-110). Every comparison is by file identity (TLS-109, see
/// [`Ident`]): a hard link, symlink, bind mount or `..` spelling of a file is
/// that file, so no check passes a write that would truncate one. `jobs`:
/// `(name, paths)`, `name` as the error line names the job. `Err`: the one
/// usage line naming both jobs. It runs before any file is opened.
pub fn check_write_sets(jobs: &[(String, JobPaths)], cwd: &Path) -> Result<(), String> {
    let fail = |why: String| Err(format!("al-sem batch: {why}"));
    let ident = |t: &Target| match t {
        Target::File(f) => Written::File(resolve(cwd, f)),
        Target::Dir(d) => Written::Dir(DirIdent::new(resolve(cwd, d))),
    };
    let writes: Vec<(usize, Written)> = jobs
        .iter()
        .enumerate()
        .flat_map(|(j, (_, p))| p.writes.iter().map(move |t| (j, ident(t))))
        .collect();
    let name = |j: usize| &jobs[j].0;
    // Where each written file sits in `writes`, and every directory target
    // (few: `--out-dir`-style targets). Each check below asks these instead of
    // scanning every pair, which made a manifest of n jobs O(n²) (G14). The
    // answers, and the first clash reported, are those of the pair scan.
    let mut files: HashMap<(Option<&FileId>, &[std::ffi::OsString]), Vec<usize>> = HashMap::new();
    let mut dirs: Vec<usize> = Vec::new();
    for (pos, (_, t)) in writes.iter().enumerate() {
        match t {
            Written::File(f) => files.entry(f.key()).or_default().push(pos),
            Written::Dir(_) => dirs.push(pos),
        }
    }
    // Two targets that clash: the first write with a later partner, then its
    // first such partner.
    for (i, (a, ta)) in writes.iter().enumerate() {
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
                    if let Written::Dir(d) = &writes[j].1
                        && d.holds(x)
                    {
                        let (x, d) = (x.path.display(), d.id.path.display());
                        consider(j, format!("{x} is inside the directory {d}"));
                    }
                }
            }
            Written::Dir(x) => {
                let partner = writes
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find_map(|(j, (_, tb))| {
                        let xp = x.id.path.display();
                        let why = match tb {
                            Written::File(f) => x.holds(f).then(|| {
                                format!("{} is inside the directory {xp}", f.path.display())
                            }),
                            Written::Dir(y) => (y.holds(&x.id) || x.holds(&y.id)).then(|| {
                                format!("the directories {xp} and {} nest", y.id.path.display())
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
    let mut taken: BTreeMap<&PathBuf, Vec<usize>> = BTreeMap::new();
    for (r, (_, paths)) in jobs.iter().enumerate() {
        for read in &paths.reads {
            let read = resolve(cwd, read);
            let in_dir = dirs
                .iter()
                .copied()
                .filter(|&j| matches!(&writes[j].1, Written::Dir(d) if d.holds(&read)));
            let hit = files
                .get(&read.key())
                .into_iter()
                .flatten()
                .copied()
                .chain(in_dir)
                .filter(|&w| writes[w].0 != r)
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
        for kept in &paths.kept {
            match kept {
                // A refused job's named file: its own streams may not write it.
                Target::File(k) => {
                    let k = resolve(cwd, k);
                    let own = files
                        .get(&k.key())
                        .into_iter()
                        .flatten()
                        .any(|&w| writes[w].0 == r);
                    if own {
                        return fail(format!(
                            "{} names {} but is refused, so it must stay as it is; its own stream writes it",
                            name(r),
                            k.path.display()
                        ));
                    }
                }
                // Its named directory (TLS-110): no write under it, its own
                // streams' or another job's.
                Target::Dir(k) => {
                    let k = DirIdent::new(resolve(cwd, k));
                    let under = writes.iter().find(|(_, t)| match t {
                        Written::File(f) => k.holds(f),
                        Written::Dir(y) => k.holds(&y.id) || y.holds(&k.id),
                    });
                    if let Some((w, t)) = under {
                        return fail(format!(
                            "{} names the directory {} but is refused, so it must stay as it is; {} writes {} inside it",
                            name(r),
                            k.id.path.display(),
                            name(*w),
                            t.path().display()
                        ));
                    }
                }
            }
        }
        for ws in &paths.workspaces {
            let hits = taken.entry(ws).or_insert_with(|| {
                let read = WorkspaceRead::new(cwd, ws);
                writes
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, t))| matches!(t, Written::File(f) if read.takes(f)))
                    .map(|(w, _)| w)
                    .collect()
            });
            if let Some(&w) = hits.iter().find(|&&w| writes[w].0 != r) {
                return fail(format!(
                    "{} reads the workspace {}, where {} writes {}",
                    name(r),
                    resolve(cwd, ws).path.display(),
                    name(writes[w].0),
                    writes[w].1.path().display()
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
    let base = absolute(path.parent().unwrap_or(Path::new("")));
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
