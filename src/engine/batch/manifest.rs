//! #277 D2.1: the `alsem batch` manifest. Its shape and its stream paths;
//! each job's `argv` is parsed by the binary's own clap `Cli`, so every flag,
//! default and message is the standalone one.

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

/// `p` against `cwd`, `.` and `..` folded lexically (no link is followed:
/// two spellings through a symlink are two paths, stated).
fn resolve(cwd: &Path, p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in cwd.join(p).components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
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
    for (i, (a, ta)) in writes.iter().enumerate() {
        for (b, tb) in &writes[i + 1..] {
            let clash = match (ta, tb) {
                (Target::File(x), Target::File(y)) => {
                    (x == y).then(|| format!("both write {}", x.display()))
                }
                (Target::File(f), Target::Dir(d)) | (Target::Dir(d), Target::File(f)) => f
                    .starts_with(d)
                    .then(|| format!("{} is inside the directory {}", f.display(), d.display())),
                (Target::Dir(x), Target::Dir(y)) => (x.starts_with(y) || y.starts_with(x))
                    .then(|| format!("the directories {} and {} nest", x.display(), y.display())),
            };
            if let Some(why) = clash {
                return fail(format!("{} and {}: {why}", name(*a), name(*b)));
            }
        }
    }
    for (r, (_, paths)) in jobs.iter().enumerate() {
        for read in &paths.reads {
            let read = resolve(cwd, read);
            for (w, t) in writes.iter().filter(|(w, _)| *w != r) {
                let hit = match t {
                    Target::File(f) => *f == read,
                    Target::Dir(d) => read.starts_with(d),
                };
                if hit {
                    return fail(format!(
                        "{} reads {}, which {} writes",
                        name(r),
                        read.display(),
                        name(*w)
                    ));
                }
            }
        }
        for ws in &paths.workspaces {
            let ws = resolve(cwd, ws);
            for (w, t) in writes.iter().filter(|(w, _)| *w != r) {
                let Target::File(f) = t else { continue };
                let read_by_ws = f.extension().is_some_and(|e| e == "al" || e == "app")
                    || f.file_name()
                        .is_some_and(|n| WORKSPACE_FILES.iter().any(|w| n == *w));
                if f.starts_with(&ws) && read_by_ws {
                    return fail(format!(
                        "{} reads the workspace {}, where {} writes {}",
                        name(r),
                        ws.display(),
                        name(*w),
                        f.display()
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
