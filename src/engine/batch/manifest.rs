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
    /// Files the job reads: diff file sides, `--baseline` without
    /// `--update-baseline`, `--renames`, `--policy`, digest's diff file.
    pub reads: Vec<PathBuf>,
    /// The workspaces it reads.
    pub workspaces: Vec<PathBuf>,
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

/// `p` [`fold`]ed, then its deepest existing ancestor canonicalized and the
/// rest appended (R8-1): two spellings of one file through a symlink, a
/// Windows case difference or an 8.3 name are one path. The part that does
/// not exist yet is compared as spelled, lowercased on Windows (a
/// case-insensitive file system).
fn resolve(cwd: &Path, p: &Path) -> PathBuf {
    let lexical = fold(cwd, p);
    let mut rest = Vec::new();
    let mut base = lexical.as_path();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(base) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |p, name: &std::ffi::OsString| {
                    if cfg!(windows) {
                        p.join(name.to_string_lossy().to_lowercase())
                    } else {
                        p.join(name)
                    }
                });
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_os_string());
                base = parent;
            }
            _ => return lexical,
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
    root: PathBuf,
    scope: crate::dependencies::CacheScope,
    taken: Taken,
}

impl WorkspaceRead {
    fn new(cwd: &Path, ws: &Path) -> WorkspaceRead {
        use crate::dependencies::{CacheScope, package_cache_scope};
        let spelled = package_cache_scope(&fold(cwd, ws));
        WorkspaceRead {
            root: resolve(cwd, ws),
            scope: CacheScope {
                candidates: spelled.candidates.iter().map(|c| resolve(cwd, c)).collect(),
                walked_dirs: vec![],
            },
            taken: taken_files(cwd, ws),
        }
    }

    /// Whether the read takes the resolved file `f`.
    fn takes(&self, f: &Path) -> bool {
        use crate::dependencies::{Role, cache_scope_role};
        let under = |root: &Path| f.starts_with(root) && is_input_name(f);
        under(&self.root)
            || cache_scope_role(&self.scope, f) == Some(Role::AppFile)
            || self.taken.files.contains(f)
            || self.taken.linked_dirs.iter().any(|d| under(d))
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
    /// Every file it takes that exists now, canonicalized.
    files: HashSet<PathBuf>,
    /// The canonical target of every linked directory the walk enters: an
    /// [`is_input_name`] file beneath one is taken, existing or not (#277
    /// final r2 R2-2).
    linked_dirs: Vec<PathBuf>,
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
        let canonical = || std::fs::canonicalize(e.path()).ok();
        if e.file_type().is_dir() && e.path_is_symlink() {
            taken.linked_dirs.extend(canonical());
        } else if e.file_type().is_file() && is_input_name(e.path()) {
            taken.files.extend(canonical());
        }
    }
    let packages = crate::dependencies::discover_app_files(&root).0;
    taken.files.extend(
        packages
            .into_iter()
            .filter_map(|a| std::fs::canonicalize(a.path).ok()),
    );
    taken
}

/// Every write target of the run checked before any build (D2.4, 12A-10):
/// two targets naming one file (across jobs, or one job's stdout and stderr),
/// a file inside a directory target or two nested directory targets, a file
/// one job reads that another writes, and a file another job writes that a
/// workspace read would take. `jobs`: `(name, paths)`, `name` as the error
/// line names the job. `Err`: the one usage line naming both jobs.
pub fn check_write_sets(jobs: &[(String, JobPaths)], cwd: &Path) -> Result<(), String> {
    let fail = |why: String| Err(format!("al-sem batch: {why}"));
    let writes: Vec<(usize, Target)> = jobs
        .iter()
        .enumerate()
        .flat_map(|(j, (_, p))| {
            p.writes.iter().map(move |t| {
                let t = match t {
                    Target::File(f) => Target::File(resolve(cwd, f)),
                    Target::Dir(d) => Target::Dir(resolve(cwd, d)),
                };
                (j, t)
            })
        })
        .collect();
    let name = |j: usize| &jobs[j].0;
    // Where each written file sits in `writes`, and every directory target
    // (few: `--out-dir`-style targets). Each check below asks these instead of
    // scanning every pair, which made a manifest of n jobs O(n²) (G14). The
    // answers, and the first clash reported, are those of the pair scan.
    let mut files: HashMap<&Path, Vec<usize>> = HashMap::new();
    let mut dirs: Vec<usize> = Vec::new();
    for (pos, (_, t)) in writes.iter().enumerate() {
        match t {
            Target::File(f) => files.entry(f.as_path()).or_default().push(pos),
            Target::Dir(_) => dirs.push(pos),
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
            Target::File(x) => {
                if let Some(&j) = files[x.as_path()].iter().find(|&&j| j > i) {
                    consider(j, format!("both write {}", x.display()));
                }
                for &j in &dirs {
                    if let Target::Dir(d) = &writes[j].1
                        && x.starts_with(d)
                    {
                        consider(
                            j,
                            format!("{} is inside the directory {}", x.display(), d.display()),
                        );
                    }
                }
            }
            Target::Dir(x) => {
                let partner = writes
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .find_map(|(j, (_, tb))| {
                        let why = match tb {
                            Target::File(f) => f.starts_with(x).then(|| {
                                format!("{} is inside the directory {}", f.display(), x.display())
                            }),
                            Target::Dir(y) => (x.starts_with(y) || y.starts_with(x)).then(|| {
                                format!("the directories {} and {} nest", x.display(), y.display())
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
                .filter(|&j| matches!(&writes[j].1, Target::Dir(d) if read.starts_with(d)));
            let hit = files
                .get(read.as_path())
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
                    read.display(),
                    name(writes[w].0)
                ));
            }
        }
        for ws in &paths.workspaces {
            let hits = taken.entry(ws).or_insert_with(|| {
                let read = WorkspaceRead::new(cwd, ws);
                writes
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, t))| matches!(t, Target::File(f) if read.takes(f)))
                    .map(|(w, _)| w)
                    .collect()
            });
            if let Some(&w) = hits.iter().find(|&&w| writes[w].0 != r) {
                let Target::File(f) = &writes[w].1 else {
                    unreachable!("only file targets are kept")
                };
                return fail(format!(
                    "{} reads the workspace {}, where {} writes {}",
                    name(r),
                    resolve(cwd, ws).display(),
                    name(writes[w].0),
                    f.display()
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
