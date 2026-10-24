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
