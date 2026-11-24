//! ENG-110 (spec "`alsem batch`" and "Correctness checks"): the dependency IR cache through
//! real `alsem` processes. A plain `alsem analyze` with the cache on publishes an entry and
//! writes a `start` and an `end` report line (the positive control); `alsem batch` with two
//! workers uses it too, and its write check keeps every job write out of the cache folders
//! (TLS-116); and cargo's `[env]` default keeps a spawned engine off.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};

use crate::cli_batch_writes::{job, run_manifest_doc};
use crate::symbol_app;

/// Dependency source files: above `SMALL_PACKAGE_MIN_FILES` (8), so the package is cached.
const DEP_FILES: usize = 12;

/// A workspace calling into one embedded-source dependency, and an isolated
/// `XDG_CACHE_HOME` (ENG-113's text cache lives under it).
struct Fixture {
    _tmp: tempfile::TempDir,
    ws: PathBuf,
    xdg: PathBuf,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(
        ws.join("app.json"),
        json!({
            "id": "11111111-0000-0000-0000-000000000110",
            "name": "Ws110",
            "publisher": "probe",
            "version": "1.0.0.0",
            "runtime": "13.0",
            "idRanges": [{"from": 50100, "to": 50199}],
            "dependencies": [{"id": "22222222-0000-0000-0000-000000000110", "name": "Dep110",
                "publisher": "probe", "version": "1.0.0.0"}]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        ws.join("src/Ws.al"),
        "codeunit 50100 \"Ws Cu\"\n{\n    procedure Run()\n    var\n        D: Codeunit \"Dep Cu 0\";\n    begin\n        D.P0();\n    end;\n}\n",
    )
    .unwrap();
    let sources: Vec<(String, String)> = (0..DEP_FILES)
        .map(|i| {
            (
                format!("src/Dep{i}.Codeunit.al"),
                format!(
                    "codeunit {} \"Dep Cu {i}\"\n{{\n    procedure P{i}()\n    begin\n    end;\n}}\n",
                    60000 + i
                ),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    symbol_app::write_source_app(
        &ws.join(".alpackages/probe_Dep110_1.0.0.0.app"),
        "22222222-0000-0000-0000-000000000110",
        "Dep110",
        "1.0.0.0",
        "{}",
        &refs,
        "",
    );
    let xdg = tmp.path().join("xdg");
    let root = tmp.path().join("ir-root");
    Fixture {
        _tmp: tmp,
        ws,
        xdg,
        root,
    }
}

/// `alsem analyze <ws>` with `env` added.
fn analyze(f: &Fixture, env: &[(&str, &Path)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(["analyze", f.ws.to_str().unwrap(), "--format", "json"])
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.code().is_some(),
        "analyze ended by a signal: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn report(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON report line"))
        .collect()
}

/// Every `.bin` entry under `root`.
fn entries(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for kd in std::fs::read_dir(root).into_iter().flatten().flatten() {
        for e in std::fs::read_dir(kd.path()).into_iter().flatten().flatten() {
            if e.path().extension().is_some_and(|x| x == "bin") {
                out.push(e.path());
            }
        }
    }
    out
}

fn end_counter(lines: &[Value], name: &str) -> u64 {
    let ends: Vec<_> = lines.iter().filter(|l| l["event"] == "end").collect();
    assert_eq!(ends.len(), 1, "one end line: {lines:?}");
    ends[0]["counters"][name].as_u64().unwrap()
}

/// The positive control: the cache on, a fresh root: one entry published, a `start` and an
/// `end` line; the next run hits it.
#[test]
fn analyze_publishes_and_reports() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let on = [
        ("ALSEM_IR_CACHE", Path::new("on")),
        ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
    ];
    let cold = analyze(&f, &on);
    let lines = report(&rep);
    assert!(
        lines.iter().any(|l| l["event"] == "start"),
        "a start line: {lines:?}\n{}",
        String::from_utf8_lossy(&cold.stderr)
    );
    assert_eq!(end_counter(&lines, "published_packages"), 1, "{lines:?}");
    assert_eq!(entries(&f.root).len(), 1);
    std::fs::remove_file(&rep).unwrap();
    let warm = analyze(&f, &on);
    let lines = report(&rep);
    assert_eq!(
        end_counter(&lines, "disk_hits"),
        DEP_FILES as u64,
        "{lines:?}"
    );
    // The report's `generatedAt` differs on every run.
    let doc = |o: &Output| {
        let mut v: Value = serde_json::from_slice(&o.stdout).expect("a JSON report");
        v.as_object_mut().unwrap().remove("generatedAt");
        v
    };
    assert_eq!(doc(&warm), doc(&cold), "the same analysis warm and cold");
}

/// `alsem batch` running `analyze <ws>` in `batches` batches (1: in the parent; 2: two
/// workers) with the cache on, `f.root` and report `rep`; returns the workers started.
fn batch(f: &Fixture, rep: &Path, batches: usize) -> usize {
    let dir = tempfile::tempdir().unwrap();
    let ws = f.ws.to_str().unwrap();
    let analyze = ["analyze", ws, "--format", "json"];
    let doc = json!({ "batches": (0..batches)
        .map(|_| json!({ "jobs": [job(&analyze, None, None)] }))
        .collect::<Vec<_>>() });
    let (out, trace) = run_manifest_doc(
        dir.path(),
        &doc,
        None,
        &[
            ("ALSEM_IR_CACHE", "on"),
            ("ALSEM_IR_CACHE_DIR", f.root.to_str().unwrap()),
            ("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap()),
            ("XDG_CACHE_HOME", f.xdg.to_str().unwrap()),
        ],
        None,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    trace
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == "batch.worker.start")
        .count()
}

/// The report's `end` lines.
fn ends(lines: &[Value]) -> Vec<&Value> {
    lines.iter().filter(|l| l["event"] == "end").collect()
}

/// TLS-116: a batch run uses the cache as a plain run does. On a cold root two workers
/// publish the one entry, each writing its `start` and `end` lines through the run's
/// registered report; on the warm root each worker, and one batch in the parent, hits every
/// dependency file from disk and publishes nothing.
///
/// Discrimination (2026-11-21): the batch-off switch put back in `ir_cache::installed`
/// (`|| crate::engine::batch::claim::dir().is_some()`): fails "two workers' end lines"
/// (no report line); restored, passes.
#[test]
fn batch_with_workers_uses_the_cache() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let rep = dir.path().join("report.jsonl");
    assert_eq!(batch(&f, &rep, 2), 2, "precondition: two workers ran");
    let lines = report(&rep);
    let cold = ends(&lines);
    assert_eq!(cold.len(), 2, "two workers' end lines: {lines:?}");
    let published: u64 = cold
        .iter()
        .map(|e| e["counters"]["published_packages"].as_u64().unwrap())
        .sum();
    assert!(published >= 1, "a cold root is filled: {lines:?}");
    assert_eq!(entries(&f.root).len(), 1, "one entry");
    for (batches, workers) in [(2, 2), (1, 0)] {
        std::fs::remove_file(&rep).unwrap();
        assert_eq!(
            batch(&f, &rep, batches),
            workers,
            "precondition: {batches} batches"
        );
        let lines = report(&rep);
        let warm = ends(&lines);
        assert_eq!(warm.len(), workers.max(1), "{batches} batches: {lines:?}");
        for e in warm {
            // Every process's start line reached the registered report too (none came
            // before the parent registered it).
            assert!(
                lines
                    .iter()
                    .any(|l| l["event"] == "start" && l["pid"] == e["pid"]),
                "{batches} batches: a start line for {}: {lines:?}",
                e["pid"]
            );
            assert_eq!(
                e["counters"]["disk_hits"].as_u64(),
                Some(DEP_FILES as u64),
                "{batches} batches: {lines:?}"
            );
            assert_eq!(e["counters"]["published_packages"].as_u64(), Some(0));
        }
    }
}

/// `alsem batch` of `jobs`, its manifest in `d`, with the cache on at `f`'s roots and `env`
/// added after them (a later value wins).
fn batch_jobs(f: &Fixture, d: &Path, jobs: &[Value], env: &[(&str, &str)]) -> Output {
    batch_jobs_doc(f, d, &json!({ "jobs": jobs }), env)
}

/// [`batch_jobs`] of a whole manifest document.
fn batch_jobs_doc(f: &Fixture, d: &Path, doc: &Value, env: &[(&str, &str)]) -> Output {
    let mut all = vec![
        ("ALSEM_IR_CACHE", "on"),
        ("ALSEM_IR_CACHE_DIR", f.root.to_str().unwrap()),
        ("XDG_CACHE_HOME", f.xdg.to_str().unwrap()),
    ];
    all.extend_from_slice(env);
    run_manifest_doc(d, doc, None, &all, None).0
}

/// A run refused before any job (exit 3) with `why` on its stderr.
fn assert_refused(case: &str, out: &Output, why: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{case}: {stderr}");
    assert!(
        stderr.contains(why) && stderr.contains("TLS-116"),
        "{case}: {stderr}"
    );
}

/// A ready job of `f`'s workspace whose stdout is `stream`.
fn fanout_to(f: &Fixture, stream: &Path) -> Value {
    job(
        &["events", "fanout", f.ws.to_str().unwrap(), "--single-app"],
        Some(stream),
        None,
    )
}

/// A warm root: one IR entry under `f.root`, and the text cache entry under `f.xdg`.
fn warm(f: &Fixture) -> PathBuf {
    analyze(
        f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ],
    );
    let e = entries(&f.root);
    assert_eq!(e.len(), 1, "precondition: a warm root");
    e[0].clone()
}

/// TLS-116: no job writes in a cache folder. A stream in the IR cache folder, one in the
/// text cache folder, and one through a symlink to the IR cache folder are each refused
/// before any job, and nothing is made there.
///
/// Discrimination (2026-11-21, r1): the check's `w.under(r)` dropped (`r.under(w)` alone):
/// fails "(ir) a stream in the IR cache folder" (exit 1, not 3: the open then met the
/// claim's own cache check); restored, passes. (r2, the snapshot) the check's folder test
/// dropped (`w.chain.iter().any(..)` made `false`): fails "(ir)" the same way; restored,
/// passes.
#[test]
fn batch_refuses_a_write_in_a_cache_folder() {
    let f = fixture();
    let entry = warm(&f);
    let text = f.xdg.join("al-ch-snapshot-cache");
    assert!(text.is_dir(), "precondition: the text cache folder");
    let d = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&f.root, d.path().join("ln")).unwrap();
    for (case, stream) in [
        ("(ir) a stream in the IR cache folder", f.root.join("x.out")),
        (
            "(text) a stream in the text cache folder",
            text.join("x.out"),
        ),
        (
            "(symlink) a stream through a link to the IR cache folder",
            d.path().join("ln/y.out"),
        ),
    ] {
        let out = batch_jobs(&f, d.path(), &[fanout_to(&f, &stream)], &[]);
        assert_refused(case, &out, "is in a cache folder of the engine");
        assert!(!stream.exists(), "{case}: nothing made");
    }
    assert_eq!(entries(&f.root), vec![entry]);
}

/// TLS-116: no job write holds a cache folder. A `fingerprint --shard all --out D` whose
/// `D` holds the IR cache folder (`ALSEM_IR_CACHE_DIR=D/ir`, made by the check as the cache
/// would make it) is refused before any job; nothing but that folder is in `D`.
///
/// Discrimination (2026-11-21, r1): the check's `r.under(w)` dropped: fails (exit 0, not
/// 3); restored, passes. (r2) Again on the snapshot's check (`roots.iter().find(..)` made
/// `None`): fails (exit 0, not 3); restored, passes.
#[test]
fn batch_refuses_a_directory_holding_a_cache_folder() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let shards = d.path().join("shards");
    std::fs::create_dir(&shards).unwrap();
    let ir = shards.join("ir");
    let s = shards.to_string_lossy().into_owned();
    let out = batch_jobs(
        &f,
        d.path(),
        &[job(
            &[
                "fingerprint",
                f.ws.to_str().unwrap(),
                "--shard",
                "all",
                "--out",
                &s,
            ],
            None,
            None,
        )],
        &[("ALSEM_IR_CACHE_DIR", ir.to_str().unwrap())],
    );
    assert_refused(
        "a shard directory above the IR cache",
        &out,
        "holds the cache folder",
    );
    let names: Vec<_> = std::fs::read_dir(&shards)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["ir"], "nothing written but the cache folder");
    assert_eq!(entries(&ir), Vec::<PathBuf>::new(), "an empty cache folder");
}

/// TLS-116 (sol r1 2): a cache folder spelled through a missing folder and `..` is made
/// before any write is resolved, so it is compared as it will be. With
/// `ALSEM_IR_CACHE_DIR=D/missing/../ir` and a shard `--out D/ir`, both missing, the cache
/// would make `D/missing` and then `D/ir`, the job's directory: refused before any job.
///
/// Discrimination (2026-11-21, r2): `make_ir_dir` neither making the folder nor requiring
/// it (`create_ir_dir` call deleted, `.then(..)` made `Some(c.ir_dir.clone())`): fails
/// (exit 0, not 3); restored, passes.
#[test]
fn batch_makes_a_cache_folder_before_comparing() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let out_dir = d.path().join("ir");
    let spelled = d.path().join("missing/../ir");
    let s = out_dir.to_string_lossy().into_owned();
    let out = batch_jobs(
        &f,
        d.path(),
        &[job(
            &[
                "fingerprint",
                f.ws.to_str().unwrap(),
                "--shard",
                "all",
                "--out",
                &s,
            ],
            None,
            None,
        )],
        &[("ALSEM_IR_CACHE_DIR", spelled.to_str().unwrap())],
    );
    assert_refused(
        "a cache folder through missing/..",
        &out,
        "is in a cache folder of the engine",
    );
    assert_eq!(entries(&out_dir), Vec::<PathBuf>::new(), "nothing written");
}

/// TLS-116: a job's `--out` that is a hard link to a cache file is that file, wherever it
/// lies in a cache folder: an IR entry (`<ir>/<key>/<digest>.bin`), a text entry (depth 1
/// in the text folder), and a file in the default `<text>/ir/` while `ALSEM_IR_CACHE_DIR`
/// points elsewhere (walked as part of the text folder). Each is refused before any job,
/// the file untouched. (A bind mount of a file is the same identity test as a hard link;
/// making one needs root, which the tests do not have, so the hard link stands for it.)
///
/// Discrimination (2026-11-21, r2): the check's file test dropped (`snap.files.contains`
/// made `false`): fails "(ir entry)" (exit 1, not 3: the open then met the claim's handle
/// check); restored, passes. The walk not entering folders (`walk(&p, snap)?` deleted):
/// fails "(ir entry)" (exit 0, not 3: neither the check nor the open knew the file);
/// restored, passes.
#[test]
fn batch_refuses_a_hard_link_to_a_cache_entry() {
    let f = fixture();
    let ir_entry = warm(&f);
    let text = f.xdg.join("al-ch-snapshot-cache");
    let text_entry = std::fs::read_dir(&text)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "bin"))
        .expect("precondition: a text entry");
    let default_ir = text.join("ir").join("k".repeat(64));
    std::fs::create_dir_all(&default_ir).unwrap();
    let default_entry = default_ir.join(format!("{}.bin", "d".repeat(64)));
    std::fs::write(&default_entry, "ENTRY").unwrap();
    for (case, entry) in [
        ("(ir entry)", &ir_entry),
        ("(text entry)", &text_entry),
        ("(default ir/ entry)", &default_entry),
    ] {
        let bytes = std::fs::read(entry).unwrap();
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out.json");
        std::fs::hard_link(entry, &out_file).unwrap();
        let o = out_file.to_string_lossy().into_owned();
        let out = batch_jobs(
            &f,
            d.path(),
            &[job(
                &[
                    "events",
                    "fanout",
                    f.ws.to_str().unwrap(),
                    "--single-app",
                    "--out",
                    &o,
                ],
                None,
                None,
            )],
            &[],
        );
        assert_refused(case, &out, "is a file of the engine's caches");
        assert_eq!(std::fs::read(entry).unwrap(), bytes, "{case}: untouched");
    }
}

/// TLS-116: an alias made after the check is caught at the open. After the check (the
/// debug seam `ALSEM_TEST_AFTER_PREFLIGHT`) the stream path becomes a hard link to the IR
/// entry, or a symlink to a missing name in the IR cache folder: the open is refused
/// (exit 1), the entry is untouched, and nothing is made in the folder (the folder test
/// runs before any file is created).
///
/// Discrimination (2026-11-21, r2): `open_guarded`'s folder test skipped
/// (`refuse_cache_path(..)` call deleted): fails "(late symlink)" (exit 0, not 1); its
/// handle test skipped (`false &&` on `if is_disk(&f)?`): fails "(late hard link)" (exit
/// 0, not 1); restored, both pass.
#[cfg(debug_assertions)]
#[test]
fn batch_claims_refuse_a_late_cache_alias() {
    let f = fixture();
    let entry = warm(&f);
    let bytes = std::fs::read(&entry).unwrap();
    let d = tempfile::tempdir().unwrap();
    let stream = d.path().join("late.out");
    let inside = f.root.join("late.out");
    for (case, seam) in [
        (
            "(late hard link)",
            format!("ln '{}' '{}'", entry.display(), stream.display()),
        ),
        (
            "(late symlink)",
            format!("ln -s '{}' '{}'", inside.display(), stream.display()),
        ),
    ] {
        let _ = std::fs::remove_file(&stream);
        let out = batch_jobs(
            &f,
            d.path(),
            &[fanout_to(&f, &stream)],
            &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{case}: {stderr}");
        assert!(
            stderr.contains("refused: ") && stderr.contains("(TLS-116)"),
            "{case}: {stderr}"
        );
        assert_eq!(
            std::fs::read(&entry).unwrap(),
            bytes,
            "{case}: entry untouched"
        );
        assert!(!inside.exists(), "{case}: nothing made in the folder");
    }
}

/// TLS-116: a shard directory is checked at run time before it is made. The shard
/// `--out D/p/sh` names a missing `D/p`; after the check (the debug seam) `D/p` becomes a
/// symlink to the IR cache folder: the directory write is refused (exit 1), and nothing is
/// made in the folder.
///
/// Discrimination (2026-11-21, r2): `output::create_dir` without its check
/// (`claim::check_dir(..)` call deleted): fails "nothing made in the folder" (`sh` made
/// there; the run still exits 1, its shard files refused at their own opens); restored,
/// passes.
#[cfg(debug_assertions)]
#[test]
fn batch_refuses_a_late_shard_directory_in_a_cache_folder() {
    let f = fixture();
    warm(&f);
    let d = tempfile::tempdir().unwrap();
    let out_dir = d.path().join("p/sh");
    let seam = format!(
        "ln -s '{}' '{}'",
        f.root.display(),
        d.path().join("p").display()
    );
    let s = out_dir.to_string_lossy().into_owned();
    let out = batch_jobs(
        &f,
        d.path(),
        &[job(
            &[
                "fingerprint",
                f.ws.to_str().unwrap(),
                "--shard",
                "all",
                "--out",
                &s,
            ],
            None,
            None,
        )],
        &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("(TLS-116)"), "{stderr}");
    assert!(!f.root.join("sh").exists(), "nothing made in the folder");
}

/// TLS-116: a trace file is published through the same guard. With `ALSEM_TRACE_FILE` in
/// the IR cache folder the run's trace is refused (one stderr line; the run itself is not
/// failed), and no file is made there.
///
/// Discrimination (2026-11-21, r2): `open_trace` opening without the guard
/// (`open_guarded(..)` made `OpenOptions::new().write(true).create(true).open(path)`): fails
/// (no refusal line: the trace was written in the folder); restored, passes.
#[test]
fn batch_refuses_a_trace_in_a_cache_folder() {
    let f = fixture();
    warm(&f);
    let d = tempfile::tempdir().unwrap();
    let trace = f.root.join("trace.json");
    let out = batch_jobs(
        &f,
        d.path(),
        &[fanout_to(&f, &d.path().join("x.out"))],
        &[("ALSEM_TRACE_FILE", trace.to_str().unwrap())],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("perf_trace: ") && stderr.contains("(TLS-116)"),
        "{stderr}"
    );
    assert!(!trace.exists(), "no trace in the cache folder");
}

/// TLS-116: a name a concurrent prune removes during the snapshot's walk is skipped, not a
/// refusal. The debug seam `ALSEM_TEST_DURING_SNAPSHOT` removes the text entry and a
/// folder of the text cache after the walk has listed that folder.
///
/// Discrimination (2026-11-21, r2): the walk's gone-name skip off (`gone(&e) && false` on
/// the entry's metadata): fails (exit 3, not 0: "cannot list the cache folder"); restored,
/// passes.
#[cfg(debug_assertions)]
#[test]
fn batch_snapshot_tolerates_a_concurrent_prune() {
    let f = fixture();
    warm(&f);
    let text = f.xdg.join("al-ch-snapshot-cache");
    std::fs::create_dir_all(text.join("sub")).unwrap();
    std::fs::write(text.join("sub/x"), "").unwrap();
    let entry = std::fs::read_dir(&text)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "bin"))
        .expect("precondition: a text entry");
    let d = tempfile::tempdir().unwrap();
    let ran = d.path().join("seam-ran");
    let seam = format!(
        "rm -rf '{}' '{}' && touch '{}'",
        entry.display(),
        text.join("sub").display(),
        ran.display()
    );
    let out = batch_jobs(
        &f,
        d.path(),
        &[fanout_to(&f, &d.path().join("x.out"))],
        &[("ALSEM_TEST_DURING_SNAPSHOT", &seam)],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(ran.exists(), "precondition: the seam removed the entry");
}

/// Every key folder (64 hex characters) or IR file name directly in `d`.
fn ir_names_in(d: &Path) -> Vec<String> {
    std::fs::read_dir(d)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.len() >= 64 && n[..64].bytes().all(|b| b.is_ascii_hexdigit())
                || n == "last-prune"
                || n == "prune.lock"
        })
        .collect()
}

/// TLS-116 (sol r2 1): a cache folder that is a dangling symlink is no folder the check can
/// snapshot, so that cache is off for the whole run, even once a job makes the link's
/// target. Here the target is a shard job's `--out D`: one batch writes the shard files
/// (making `D`), then builds a second view that parses the dependency. "(ir)":
/// `ALSEM_IR_CACHE_DIR` is the link; nothing of the IR cache lands in `D`, and the report
/// says the cache was off. "(text)": `<XDG_CACHE_HOME>/al-ch-snapshot-cache` is the link;
/// nothing of the text cache lands in `D`.
///
/// Discrimination (2026-11-22, r3): `cache_guard::apply` not turning the IR cache off (its
/// `ir_cache::disable()` call commented out): fails "(ir): nothing of a cache in D" (a key
/// folder, `last-prune` and `prune.lock` published there); not turning the text cache off
/// (`snapshot::cache::disable()` commented out): fails "(text)"; restored, both pass.
#[test]
fn batch_keeps_an_unsnapshotted_cache_folder_off() {
    for case in ["(ir)", "(text)"] {
        let f = fixture();
        let d = tempfile::tempdir().unwrap();
        let shards = d.path().join("shards");
        let link = d.path().join("cache-link");
        std::os::unix::fs::symlink(&shards, &link).unwrap();
        let xdg = d.path().join("xdg");
        std::fs::create_dir(&xdg).unwrap();
        let rep = d.path().join("report.jsonl");
        let mut env = vec![("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap())];
        if case == "(ir)" {
            env.push(("ALSEM_IR_CACHE_DIR", link.to_str().unwrap()));
        } else {
            std::os::unix::fs::symlink(&shards, xdg.join("al-ch-snapshot-cache")).unwrap();
            env.push(("XDG_CACHE_HOME", xdg.to_str().unwrap()));
        }
        let (ws, s) = (
            f.ws.to_str().unwrap(),
            shards.to_string_lossy().into_owned(),
        );
        let out = batch_jobs(
            &f,
            d.path(),
            &[
                job(
                    &[
                        "fingerprint",
                        ws,
                        "--single-app",
                        "--shard",
                        "all",
                        "--out",
                        &s,
                    ],
                    None,
                    None,
                ),
                job(&["analyze", ws, "--format", "json"], None, None),
            ],
            &env,
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{case}: {stderr}");
        assert!(
            shards.is_dir(),
            "{case}: precondition: the shard job made the target"
        );
        assert_eq!(
            ir_names_in(&shards),
            Vec::<String>::new(),
            "{case}: nothing of a cache in D"
        );
        if case == "(ir)" {
            let lines = report(&rep);
            assert!(
                lines
                    .iter()
                    .any(|l| l["event"] == "start" && l["enabled"] == false),
                "{case}: the cache was off: {lines:?}"
            );
        }
    }
}

/// TLS-116 (sol r2 2): a write whose NAME lies in a cache folder is refused even when the
/// name is a symlink pointing outside: the cache could replace the name (`last-prune` is
/// the prune's). The stream `<ir>/last-prune`, a symlink to a file outside, is refused
/// before any job; the symlink and its target stay as they were.
///
/// Discrimination (2026-11-22, r3): `refuses_path` without its spelled-name test (its
/// last `folder_hit` made `Ok(None)`): fails (exit 1, not 3: the run's own prune replaced
/// the symlink with its timestamp, and the open then met a file in the folder); restored,
/// passes.
#[test]
fn batch_refuses_a_cache_name_that_links_outside() {
    let f = fixture();
    warm(&f);
    let d = tempfile::tempdir().unwrap();
    let outside = d.path().join("outside.json");
    std::fs::write(&outside, "KEEP").unwrap();
    let name = f.root.join("last-prune");
    let _ = std::fs::remove_file(&name);
    std::os::unix::fs::symlink(&outside, &name).unwrap();
    let out = batch_jobs(&f, d.path(), &[fanout_to(&f, &name)], &[]);
    assert_refused("a cache name linking outside", &out, "is in a cache folder");
    assert!(
        std::fs::symlink_metadata(&name)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the name is still the symlink"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "KEEP");
}

/// TLS-116 (sol r3 1): every symlink met on the way is a name a cache could replace. With
/// `report → <ir>/last-prune → outside.jsonl` the report is refused before any job (its
/// spelling and its destination are both outside the cache; only the middle hop is not);
/// so is a job's stream of the same shape. The middle link and its target stay.
///
/// Discrimination (2026-11-23, r4): `refuses_path` without its hop walk (`link_hops(..)`
/// replaced by `Ok(None)`): fails "(report)" (exit 0, not 3); restored, passes.
#[test]
fn batch_refuses_a_symlink_hop_in_a_cache_folder() {
    let f = fixture();
    warm(&f);
    let d = tempfile::tempdir().unwrap();
    let outside = d.path().join("outside.jsonl");
    std::fs::write(&outside, "KEEP").unwrap();
    let middle = f.root.join("last-prune");
    let _ = std::fs::remove_file(&middle);
    std::os::unix::fs::symlink(&outside, &middle).unwrap();
    let report_link = d.path().join("report.jsonl");
    let stream_link = d.path().join("stream.out");
    std::os::unix::fs::symlink(&middle, &report_link).unwrap();
    std::os::unix::fs::symlink(&middle, &stream_link).unwrap();
    let x = d.path().join("x.out");
    for (case, stream, env) in [
        (
            "(report)",
            &x,
            vec![("ALSEM_IR_CACHE_REPORT", report_link.to_str().unwrap())],
        ),
        ("(stream)", &stream_link, vec![]),
    ] {
        let out = batch_jobs(&f, d.path(), &[fanout_to(&f, stream)], &env);
        assert_refused(case, &out, "is in a cache folder");
        assert!(
            std::fs::symlink_metadata(&middle)
                .unwrap()
                .file_type()
                .is_symlink(),
            "{case}: the middle link stays"
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "KEEP", "{case}");
    }
}

/// TLS-116 (sol r4): a spelling that passes through a cache folder is refused even when a
/// later `..` leaves it: `<ir>/<key>/../../report.jsonl` lands outside the cache, but the
/// run's own prune can remove `<key>` and leave the configured path unreachable. Refused
/// before any job, for the report and for a job's stream; nothing is written.
///
/// Discrimination (2026-11-23, r5): `link_hops` without its entered-folder test (`false &&`
/// before `m.is_dir()`): fails "(report)" (exit 0, not 3); restored, passes.
#[test]
fn batch_refuses_a_spelling_through_a_cache_folder() {
    let f = fixture();
    let entry = warm(&f);
    let key = entry.parent().unwrap();
    let d = tempfile::tempdir().unwrap();
    let x = d.path().join("x.out");
    let report = key.join("../../report.jsonl");
    let stream = key.join("../../stream.out");
    let landed = f.root.parent().unwrap();
    for (case, s, env) in [
        (
            "(report)",
            &x,
            vec![("ALSEM_IR_CACHE_REPORT", report.to_str().unwrap())],
        ),
        ("(stream)", &stream, vec![]),
    ] {
        let out = batch_jobs(&f, d.path(), &[fanout_to(&f, s)], &env);
        assert_refused(case, &out, "is in a cache folder");
        assert!(!landed.join("report.jsonl").exists(), "{case}: no report");
        assert!(!landed.join("stream.out").exists(), "{case}: no stream");
    }
}

/// TLS-116 (sol r3 2): a shard directory is made one component at a time, each checked
/// just before it is made. With `alias → <ir>`, the shard `--out D/missing/../alias/new`
/// passes the check while `missing` is absent; making `missing` makes the rest reach the
/// IR cache folder, so the run stops there (exit 1, a write error on that path) and makes
/// nothing inside the cache.
///
/// Discrimination (2026-11-23, r4): `claim::make_dir` as `create_dir_all` in a batch too
/// (`DIR.get().filter(|_| false)`): fails "nothing made in the cache" (`new` made there;
/// the run still exited 1, its shard files refused at their opens); restored, passes.
#[test]
fn batch_makes_a_shard_directory_one_step_at_a_time() {
    let f = fixture();
    warm(&f);
    let d = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(&f.root, d.path().join("alias")).unwrap();
    let spelled = d.path().join("missing/../alias/new");
    let s = spelled.to_string_lossy().into_owned();
    let out = batch_jobs(
        &f,
        d.path(),
        &[job(
            &[
                "fingerprint",
                f.ws.to_str().unwrap(),
                "--single-app",
                "--shard",
                "all",
                "--out",
                &s,
            ],
            None,
            None,
        )],
        &[],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("al-sem: write error: ") && stderr.contains("(TLS-116)"),
        "{stderr}"
    );
    assert!(!f.root.join("new").exists(), "nothing made in the cache");
}

/// TLS-116 (sol r2 3): the report's open never waits. A report path that is a FIFO with no
/// reader is refused at its registration (exit 3) instead of hanging the run.
///
/// Discrimination (2026-11-22, r3): `claim::options` without `O_NONBLOCK` (its
/// `custom_flags` made `0`): fails "the run ended" (it hung past the 60 s limit; the hung
/// process was then killed by its PID); restored, passes.
#[test]
fn batch_refuses_a_fifo_report_without_hanging() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let rep = d.path().join("report.fifo");
    let made = Command::new("mkfifo").arg(&rep).status().unwrap();
    assert!(made.success(), "precondition: mkfifo");
    let (tx, rx) = std::sync::mpsc::channel();
    let (dp, repc) = (d.path().to_path_buf(), rep.clone());
    std::thread::spawn(move || {
        let out = batch_jobs(
            &f,
            &dp,
            &[fanout_to(&f, &dp.join("x.out"))],
            &[("ALSEM_IR_CACHE_REPORT", repc.to_str().unwrap())],
        );
        let _ = tx.send(out);
    });
    let out = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the run ended");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("IR cache report"), "{stderr}");
}

/// A worker run by hand with `snapshot` as its claim folder's cache snapshot (`None`: none),
/// writing `out`.
fn hand_worker(f: &Fixture, snapshot: Option<&str>, out: &Path) -> Output {
    let claims = tempfile::tempdir().unwrap();
    if let Some(s) = snapshot {
        std::fs::write(claims.path().join("cache-snapshot"), s).unwrap();
    }
    let input = json!({
        "jobs": [{
            "argv": ["events", "fanout", f.ws.to_str().unwrap(), "--single-app"],
            "streams": {"stdout": out, "stderr": out.with_extension("err")},
            "changed_is_file": null,
            "diff_root": null,
        }],
        "stdin": null,
        "files": {"reads": [], "exists": []},
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["batch", "--worker", "0", "unread-manifest.json"])
        .env("ALSEM_BATCH_CLAIMS", claims.path())
        .env("XDG_CACHE_HOME", &f.xdg)
        .env("RUST_LOG", "off")
        .env_remove("ALSEM_TRACE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        input.to_string().as_bytes(),
    )
    .unwrap();
    child.wait_with_output().unwrap()
}

/// TLS-116 (Fable r2 M1, M2; sol r3): a worker whose parent's snapshot is missing or
/// damaged fails closed: it runs no job, writes nothing, and exits 4 (its claims failed).
/// Damaged: no header; a body that is not the one its blake3 names; flags changed after
/// the blake3 was taken (it covers flags and counts too); a flag that is neither `0` nor
/// `1`; counts that do not match the body (hashed as written); a cache on with no folder.
/// A valid empty snapshot runs the job (the positive control).
///
/// Discrimination (2026-11-22, r3): an empty-snapshot fallback (the worker's start ignoring
/// `load`'s error, and `get` turning it into an empty snapshot): fails "(missing)" (exit 0,
/// the job ran); the blake3 test skipped (`false &&` on its `if`): fails "(another body)"
/// (exit 0). (2026-11-23, r4) the blake3 over the body alone (`hashed` ignoring `fields`,
/// and `snapshot_file` with it, so the control still loads): fails "(flags changed)"; the
/// flag test accepting anything (`_ => Ok(false)`): fails "(a flag of 2)"; the count test's
/// dirs half off (`false &&` before it): fails "(wrong counts)"; the cache-on-with-no-folder
/// test off (`false &&`): fails "(on, no folder)"; each exit 0, the job ran; restored,
/// passes. "(no header)" has no break of its own: it is refused by the magic
/// test and, without it, by the missing fields (each a refusal on its own).
#[test]
fn a_worker_without_a_good_snapshot_fails_closed() {
    use crate::cli_batch_writes::snapshot_file;
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("ok.out");
    let ok = hand_worker(
        &f,
        Some(&snapshot_file("text=0 ir=0 dirs=0 files=0", "")),
        &out,
    );
    assert!(
        out.exists(),
        "control: {}",
        String::from_utf8_lossy(&ok.stderr)
    );
    let body = "d 1 2 0\n";
    let good = snapshot_file("text=0 ir=0 dirs=1 files=0", body);
    for (case, snapshot) in [
        ("(missing)", None),
        ("(no header)", Some(body.to_string())),
        ("(another body)", Some(good.replace("d 1 2", "d 1 3"))),
        (
            "(flags changed)",
            Some(snapshot_file("text=1 ir=0 dirs=1 files=0", body).replace("text=1", "text=0")),
        ),
        (
            "(a flag of 2)",
            Some(snapshot_file("text=2 ir=0 dirs=1 files=0", body)),
        ),
        (
            "(wrong counts)",
            Some(snapshot_file("text=0 ir=0 dirs=2 files=0", body)),
        ),
        (
            "(on, no folder)",
            Some(snapshot_file("text=1 ir=0 dirs=0 files=0", "")),
        ),
    ] {
        let out = d.path().join("refused.out");
        let run = hand_worker(&f, snapshot.as_deref(), &out);
        let stderr = String::from_utf8_lossy(&run.stderr);
        assert_eq!(run.status.code(), Some(4), "{case}: {stderr}");
        assert!(stderr.contains("cache snapshot"), "{case}: {stderr}");
        assert!(!out.exists(), "{case}: nothing written");
    }
    assert!(ok.status.success());
}

/// TLS-116: a stream that is a dangling symlink to a name outside every cache folder is
/// written through the link, as before TLS-116 (`O_EXCL` does not follow a link, so the
/// guarded open must fall back to a following create).
///
/// Discrimination (2026-11-22, r3): `open_guarded`'s fallback opening without create
/// (`.create(true)` deleted on the `AlreadyExists` arm): fails (exit 0, but the job's write
/// error, no target); restored, passes.
#[test]
fn batch_writes_through_a_dangling_symlink() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("target.out");
    let link = d.path().join("link.out");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = batch_jobs(&f, d.path(), &[fanout_to(&f, &link)], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("write error"), "{stderr}");
    assert!(target.exists(), "written through the link");
}

/// TLS-116 (Fable r2 M2): no write lands in the run's claim folder. After the check (the
/// debug seam), the stream path becomes a symlink to a new name in the claim folder, or a
/// hard link to its `cache-snapshot`: each open is refused (exit 1).
///
/// Discrimination (2026-11-22, r3): `folder_hit`'s claim-folder test off (`snap.claims ==
/// Some(id)` made `false`): fails "(a new name in the claim folder)" (exit 0, not 1); the
/// snapshot's claim in `publish` skipped (`protect` call deleted): fails "(the snapshot)"
/// (exit 0, not 1); restored, both pass.
#[cfg(debug_assertions)]
#[test]
fn batch_refuses_a_write_in_its_claim_folder() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let stream = d.path().join("late.out");
    for (case, seam) in [
        (
            "(a new name in the claim folder)",
            format!("ln -s \"$ALSEM_BATCH_CLAIMS/new\" '{}'", stream.display()),
        ),
        (
            "(the snapshot)",
            format!(
                "ln \"$ALSEM_BATCH_CLAIMS/cache-snapshot\" '{}'",
                stream.display()
            ),
        ),
    ] {
        let _ = std::fs::remove_file(&stream);
        let out = batch_jobs(
            &f,
            d.path(),
            &[fanout_to(&f, &stream)],
            &[("ALSEM_TEST_AFTER_PREFLIGHT", &seam)],
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{case}: {stderr}");
        assert!(stderr.contains("refused: "), "{case}: {stderr}");
        assert!(
            std::fs::symlink_metadata(&stream).is_ok(),
            "{case}: precondition: the seam made the alias"
        );
    }
}

/// TLS-116: the IR cache report is one of the run's writes. A job stream on the report
/// file is refused before any job.
///
/// Discrimination (2026-11-21): the report left out of the run's writes (`writes:` of
/// "the run" made empty): fails (exit 1, not 3: the stream then met the report's
/// registration at its open); restored, passes.
#[test]
fn batch_refuses_a_stream_on_the_report() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let rep = d.path().join("report.jsonl");
    let out = batch_jobs(
        &f,
        d.path(),
        &[fanout_to(&f, &rep)],
        &[("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap())],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("the run: both write"), "{stderr}");
}

/// TLS-116: every report line goes to the file the parent registered. After the
/// registration (the debug seam), the report path becomes a hard link to the workspace's
/// `app.json`. The parent appends through the handle it registered; each of the two workers
/// opens the path, finds another file and refuses its appends (a warning each, the run is
/// not failed): `app.json` is untouched, and the refusals were attempted.
///
/// Discrimination (2026-11-21, r1): the marker test dropped (`false &&` on its `if`):
/// fails "app.json untouched" (the report lines appended to it). (r2, now in
/// `claim::open_registered`) the same break: fails "an append attempted and refused" (no
/// refusal); restored, passes.
#[cfg(debug_assertions)]
#[test]
fn batch_report_appends_only_to_the_registered_file() {
    let f = fixture();
    let app = f.ws.join("app.json");
    let before = std::fs::read(&app).unwrap();
    let d = tempfile::tempdir().unwrap();
    let rep = d.path().join("report.jsonl");
    let seam = format!("rm '{0}' && ln '{1}' '{0}'", rep.display(), app.display());
    let analyze = ["analyze", f.ws.to_str().unwrap(), "--format", "json"];
    let one = json!({ "jobs": [job(&analyze, None, None)] });
    let out = batch_jobs_doc(
        &f,
        d.path(),
        &json!({ "batches": [one.clone(), one] }),
        &[
            ("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap()),
            ("ALSEM_TEST_AFTER_PREFLIGHT", &seam),
            ("RUST_LOG", "warn"),
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("IR cache report") && stderr.contains("not the file this batch run"),
        "an append attempted and refused: {stderr}"
    );
    use std::os::unix::fs::MetadataExt;
    let ino = |p: &Path| std::fs::metadata(p).unwrap().ino();
    assert_eq!(
        ino(&rep),
        ino(&app),
        "precondition: the seam linked the report"
    );
    assert_eq!(std::fs::read(&app).unwrap(), before, "app.json untouched");
}

/// The cache off through a real process: every seam file `skipped_off`, nothing read or
/// published (spec Acceptance 2's off-sample rule).
#[test]
fn cache_off_counts_every_seam_file_as_skipped_off() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("off")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
            ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
        ],
    );
    let lines = report(&rep);
    let seam = end_counter(&lines, "seam_files");
    assert!(seam >= DEP_FILES as u64, "{lines:?}");
    assert_eq!(end_counter(&lines, "skipped_off"), seam, "{lines:?}");
    assert_eq!(end_counter(&lines, "disk_hits"), 0);
    assert_eq!(end_counter(&lines, "published_packages"), 0);
    assert!(!f.root.exists(), "nothing made");
}

/// sol r1: a non-UTF-8 command line never panics the report, with or without a sink.
#[test]
fn a_non_utf8_command_line_runs() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    for with_report in [false, true] {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
        cmd.arg0(std::ffi::OsStr::from_bytes(b"alsem-\xff"))
            .args(["analyze", f.ws.to_str().unwrap(), "--format", "json"])
            .env("RUST_LOG", "off")
            .env("XDG_CACHE_HOME", &f.xdg)
            .env("ALSEM_IR_CACHE", "off");
        if with_report {
            cmd.env("ALSEM_IR_CACHE_REPORT", &rep);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.code().is_some_and(|c| c != 101),
            "report {with_report}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let lines = report(&rep);
    let start = lines
        .iter()
        .find(|l| l["event"] == "start")
        .unwrap_or_else(|| panic!("a start line: {lines:?}"));
    // sol/Fable r2: the whole command line, losslessly: the non-UTF-8 arg0 as its bytes, and
    // every argument after it (five here; a run's mode, `--low-mem` say, is provable).
    assert_eq!(
        start["argv"],
        json!([
            {"hex": "616c73656d2dff"},
            "analyze",
            f.ws.to_str().unwrap(),
            "--format",
            "json"
        ]),
        "{lines:?}"
    );
}

/// sol r1: an entry that is there but refused counts as `refused`, and the gate's report
/// rule fails the phase on it.
#[test]
fn a_refused_entry_fails_the_gate_rule() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let on = [
        ("ALSEM_IR_CACHE", Path::new("on")),
        ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
    ];
    analyze(&f, &on);
    let check = || {
        crate::clean_env::command(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/ir-cache-report-check"
        ))
        .args(["--rule", "output"])
        .arg(&rep)
        .output()
        .unwrap()
    };
    assert!(
        check().status.success(),
        "precondition: a clean fill passes"
    );
    let entry = &entries(&f.root)[0];
    let mut b = std::fs::read(entry).unwrap();
    let at = b.len() - 40;
    b[at] ^= 1;
    std::fs::write(entry, &b).unwrap();
    std::fs::remove_file(&rep).unwrap();
    analyze(&f, &on);
    let lines = report(&rep);
    assert_eq!(
        end_counter(&lines, "refused"),
        DEP_FILES as u64,
        "{lines:?}"
    );
    let out = check();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("refused"));
}

/// Fable r1: the engine child (`alsem mcp` -> `engine-host`) writes its start and end
/// lines; the front end, which parses nothing, writes none.
#[test]
fn the_engine_child_reports_start_and_end() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let out_root = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_alsem"))
        .args(["mcp", "--workspace"])
        .arg(&f.ws)
        .arg("--output-root")
        .arg(out_root.path())
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg)
        .env("ALSEM_IR_CACHE", "on")
        .env("ALSEM_IR_CACHE_DIR", &f.root)
        .env("ALSEM_IR_CACHE_REPORT", &rep)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut send = |v: Value| {
        writeln!(input, "{v}").unwrap();
        input.flush().unwrap();
    };
    let mut response = || loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "the server closed"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        if v.get("id").is_some() {
            return v;
        }
    };
    send(
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "test", "version": "1"}}}),
    );
    response();
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}));
    send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
        "name": "alsem_explain", "arguments": {"routines": ["Run"], "format": "json"}}}),
    );
    let r = response();
    assert!(r.get("error").is_none(), "{r}");
    drop(input);
    assert!(child.wait().unwrap().success());
    let lines = report(&rep);
    let starts: Vec<_> = lines.iter().filter(|l| l["event"] == "start").collect();
    assert_eq!(starts.len(), 1, "one process used the seam: {lines:?}");
    assert!(
        starts[0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "engine-host"),
        "the engine child: {lines:?}"
    );
    assert!(end_counter(&lines, "eligible_files") > 0, "{lines:?}");
    assert_eq!(lines[1]["pid"], starts[0]["pid"], "its own end line");
}

/// `perf_trace`'s `ALSEM_TRACE_EXIT_AFTER` exits from a span guard, past the `main`'s drop;
/// it writes the end line itself.
#[test]
fn an_exit_after_still_writes_the_end_line() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    let trace = f.root.with_extension("trace.json");
    let out = analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
            ("ALSEM_IR_CACHE_REPORT", rep.as_path()),
            ("ALSEM_TRACE", Path::new("1")),
            ("ALSEM_TRACE_FILE", trace.as_path()),
            ("ALSEM_TRACE_EXIT_AFTER", Path::new("crossapp.base_total")),
        ],
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stdout.is_empty(),
        "the exit came before the analysis printed"
    );
    let lines = report(&rep);
    assert!(end_counter(&lines, "seam_files") > 0, "{lines:?}");
}

/// `.cargo/config.toml`'s `[env]` keeps every engine a test spawns off: with no value of its
/// own, an analyze writes nothing.
#[test]
fn cargo_env_defaults_off() {
    assert_eq!(
        std::env::var("ALSEM_IR_CACHE").as_deref(),
        Ok("off"),
        "cargo's [env] sets it for tests"
    );
    let f = fixture();
    analyze(&f, &[("ALSEM_IR_CACHE_DIR", f.root.as_path())]);
    assert!(entries(&f.root).is_empty(), "nothing written");
}

/// Fable r1: `aldump --session-refresh --verify`'s cache-off reference builds run under
/// `ir_cache::Suspend`: with the cache on, `--verify` adds no seam use to the report (each
/// reference build parses its dependencies, outside the cache).
#[test]
fn session_refresh_reference_builds_bypass_the_cache() {
    let f = fixture();
    let rep = f.root.with_extension("jsonl");
    // This analyze fills ENG-113's text cache (shared) and alsem's IR key; aldump has its own
    // key, so its first run below fills that and the second hits it. The two runs' seam counts
    // are compared, which a fill or a hit does not change.
    analyze(
        &f,
        &[
            ("ALSEM_IR_CACHE", Path::new("on")),
            ("ALSEM_IR_CACHE_DIR", f.root.as_path()),
        ],
    );
    let seam = |verify: bool| {
        let _ = std::fs::remove_file(&rep);
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_aldump"));
        cmd.arg("--session-refresh").arg(&f.ws);
        if verify {
            cmd.arg("--verify");
        }
        let out = cmd
            .env("RUST_LOG", "off")
            .env("XDG_CACHE_HOME", &f.xdg)
            .env("ALSEM_IR_CACHE", "on")
            .env("ALSEM_IR_CACHE_DIR", &f.root)
            .env("ALSEM_IR_CACHE_REPORT", &rep)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "verify {verify}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        if verify {
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                text.contains("coldMs"),
                "precondition: reference builds ran: {text}"
            );
        }
        end_counter(&report(&rep), "seam_files")
    };
    let (plain, verified) = (seam(false), seam(true));
    assert!(plain > 0, "precondition: the session used the seam");
    assert_eq!(verified, plain, "the reference builds stayed off the cache");
}

/// TLS-120: a standalone `alsem <args>` (no batch run) with `f`'s caches (the IR cache on in
/// `f.root`, the text cache under `f.xdg`) and `env` added.
fn alone(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_alsem"));
    cmd.args(args)
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg)
        .env("ALSEM_IR_CACHE", "on")
        .env("ALSEM_IR_CACHE_DIR", &f.root);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// A standalone `events fanout` of `f`'s workspace to `--out <out>`.
fn fanout_out(f: &Fixture, out: &Path) -> Output {
    let (ws, o) = (f.ws.to_str().unwrap(), out.to_str().unwrap());
    alone(
        f,
        &["events", "fanout", ws, "--single-app", "--out", o],
        &[],
    )
}

/// `f`'s warm IR entry and its text cache entry.
fn warm_entries(f: &Fixture) -> (PathBuf, PathBuf) {
    let ir = warm(f);
    let text = std::fs::read_dir(f.xdg.join("al-ch-snapshot-cache"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|x| x == "bin"))
        .expect("precondition: a text entry");
    (ir, text)
}

/// A standalone write refused with `why`: exit 1, the write error line naming TLS-116.
fn assert_write_refused(case: &str, out: &Output, why: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{case}: {stderr}");
    assert!(
        stderr.contains("al-sem: write error: ") && stderr.contains(why),
        "{case}: {stderr}"
    );
    assert!(stderr.contains("(TLS-116)"), "{case}: {stderr}");
}

/// TLS-120: a standalone `--out` never truncates a cache file. An `--out` that is the IR
/// entry, the text entry, a hard link to either, a new name through a symlinked parent into
/// the IR folder, or a spelling through the IR folder and out again (`<key>/../../x`) is
/// refused (exit 1), each entry is untouched, and nothing is made. The positive controls: a
/// plain `--out`, and an `--out` with a second link outside the caches (which makes the open
/// look the file up in the cache folders) are both written.
///
/// Discrimination (2026-11-24): `open_for_write` outside a batch back to `File::create(path)`:
/// fails "(ir entry)" (exit 0, not 1: the entry was truncated and written); `may_alias`
/// made `false` (no lookup for a file with two links): fails "(a hard link to the ir entry)"
/// (exit 0); the standalone roots without the IR folder (`[text]` alone): fails "(ir entry)"
/// (exit 0); restored, all pass.
#[test]
fn standalone_out_refuses_a_cache_file() {
    let f = fixture();
    let (ir_entry, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let ok = d.path().join("ok.json");
    let out = fanout_out(&f, &ok);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(std::fs::metadata(&ok).unwrap().len() > 0, "a plain --out");
    let other = d.path().join("other.json");
    std::fs::write(&other, "OLD").unwrap();
    std::fs::hard_link(&other, d.path().join("other-link.json")).unwrap();
    let out = fanout_out(&f, &d.path().join("other-link.json"));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_ne!(std::fs::read(&other).unwrap(), b"OLD", "a linked --out");
    let (ir_link, text_link) = (
        d.path().join("ir-link.json"),
        d.path().join("text-link.json"),
    );
    std::fs::hard_link(&ir_entry, &ir_link).unwrap();
    std::fs::hard_link(&text_entry, &text_link).unwrap();
    std::os::unix::fs::symlink(&f.root, d.path().join("ln")).unwrap();
    let through = d.path().join("ln/new.json");
    let spelled = ir_entry.parent().unwrap().join("../../spelled.json");
    let bytes = |p: &Path| std::fs::read(p).unwrap();
    let (ir_bytes, text_bytes) = (bytes(&ir_entry), bytes(&text_entry));
    let folder = "is in a cache folder of the engine";
    let file = "is a file of the engine's caches";
    for (case, path, why) in [
        ("(ir entry)", &ir_entry, folder),
        ("(text entry)", &text_entry, folder),
        ("(a hard link to the ir entry)", &ir_link, file),
        ("(a hard link to the text entry)", &text_link, file),
        ("(a symlinked parent)", &through, folder),
        ("(a spelling through the folder)", &spelled, folder),
    ] {
        assert_write_refused(case, &fanout_out(&f, path), why);
        assert_eq!(bytes(&ir_entry), ir_bytes, "{case}: the ir entry");
        assert_eq!(bytes(&text_entry), text_bytes, "{case}: the text entry");
    }
    assert!(!through.exists() && !f.root.join("new.json").exists());
    assert!(!f.root.parent().unwrap().join("spelled.json").exists());
}

/// TLS-120: a standalone shard directory is never made in a cache folder. `fingerprint
/// --shard all --out D` with `D` in the text cache folder or the IR folder is refused, and
/// `D` is not made.
///
/// Discrimination (2026-11-24): `make_dir` outside a batch back to `create_dir_all(path)`:
/// fails "(text): nothing made" (the folder was made; its files were still refused);
/// restored, passes.
#[test]
fn standalone_shard_directory_stays_out_of_cache_folders() {
    let f = fixture();
    warm(&f);
    for (case, dir) in [
        ("(text)", f.xdg.join("al-ch-snapshot-cache/shards")),
        ("(ir)", f.root.join("shards")),
    ] {
        let (ws, o) = (f.ws.to_str().unwrap(), dir.to_str().unwrap());
        let out = alone(&f, &["fingerprint", ws, "--shard", "all", "--out", o], &[]);
        assert_write_refused(case, &out, "is in a cache folder of the engine");
        assert!(!dir.exists(), "{case}: nothing made");
    }
}

/// TLS-120: a standalone trace file is never a cache file. `ALSEM_TRACE_FILE` a hard link to
/// the text entry: the trace is refused (one stderr line, the command itself succeeds) and
/// the entry is untouched.
///
/// Discrimination (2026-11-24): `TraceWriter::create` back to `File::create(path)`: fails
/// (no refusal line); restored, passes.
#[test]
fn standalone_trace_is_never_a_cache_file() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let trace = d.path().join("trace.json");
    std::fs::hard_link(&text_entry, &trace).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let (ws, t) = (f.ws.to_str().unwrap(), trace.to_str().unwrap());
    let out = alone(
        &f,
        &["events", "fanout", ws, "--single-app"],
        &[("ALSEM_TRACE", "1"), ("ALSEM_TRACE_FILE", t)],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("perf_trace: cannot create") && stderr.contains("(TLS-116)"),
        "{stderr}"
    );
    assert!(std::fs::read(&text_entry).unwrap() == before, "untouched");
}

/// TLS-120: `aldump`'s outputs take the same guard. `aldump --switch-dump <ws> <dir>` with
/// `<dir>` in the IR folder fails and makes nothing there ("(folder)"); a dump folder outside
/// the caches whose `<name>.txt` is a hard link to the text entry fails and leaves the entry
/// as it was ("(file)").
///
/// Discrimination (2026-11-24): `switch_dump::write_dump`'s `claim::make_dir(dir)` back to
/// `create_dir_all(dir)` (its files still through `write_outside`): fails "(folder)" (the
/// folder was made); restored, passes. (r1) Its file write back to `std::fs::write`: fails
/// "(file)" (exit 0); restored, passes.
#[test]
fn aldump_switch_dump_stays_out_of_cache_folders() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let ws = f.ws.to_str().unwrap();
    let dir = f.root.join("dump");
    let out = aldump(&f, &["--switch-dump", ws, dir.to_str().unwrap()], &[]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "(folder): {stderr}");
    assert!(
        stderr.contains("is in a cache folder of the engine (TLS-116)"),
        "(folder): {stderr}"
    );
    assert!(!dir.exists(), "(folder): nothing made");
    let d = tempfile::tempdir().unwrap();
    let plain = d.path().join("plain");
    let out = aldump(&f, &["--switch-dump", ws, plain.to_str().unwrap()], &[]);
    assert!(out.status.success(), "precondition: a plain dump: {out:?}");
    let name = std::fs::read_dir(&plain)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .min()
        .expect("precondition: a dump file");
    let linked = d.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    std::fs::hard_link(&text_entry, linked.join(&name)).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let out = aldump(&f, &["--switch-dump", ws, linked.to_str().unwrap()], &[]);
    assert_aldump_refused("(file)", &out, "is a file of the engine's caches");
    assert!(
        std::fs::read(&text_entry).unwrap() == before,
        "(file): untouched"
    );
}

/// TLS-120 r1: a standalone `aldump <args>` with `f`'s caches and `env` added.
fn aldump(f: &Fixture, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aldump"));
    cmd.args(args)
        .env("RUST_LOG", "off")
        .env("XDG_CACHE_HOME", &f.xdg)
        .env("ALSEM_IR_CACHE", "on")
        .env("ALSEM_IR_CACHE_DIR", &f.root);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

/// An `aldump` run that failed with `why` and TLS-116 on its stderr.
fn assert_aldump_refused(case: &str, out: &Output, why: &str) {
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{case}: {stderr}");
    assert!(
        stderr.contains(why) && stderr.contains("(TLS-116)"),
        "{case}: {stderr}"
    );
}

/// TLS-120 r1: `aldump --snapshot-memory <ws> --pd-dump <file>` with `<file>` a hard link to
/// the text entry fails and leaves the entry as it was.
///
/// Discrimination (2026-11-24, r1): the `--pd-dump` write back to `std::fs::write`: fails
/// (exit 0); restored, passes.
#[test]
fn aldump_pd_dump_is_never_a_cache_file() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let dump = d.path().join("pd.txt");
    std::fs::hard_link(&text_entry, &dump).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let args = [
        "--snapshot-memory",
        f.ws.to_str().unwrap(),
        "--pd-dump",
        dump.to_str().unwrap(),
    ];
    let out = aldump(&f, &args, &[]);
    assert_aldump_refused("(pd-dump)", &out, "is a file of the engine's caches");
    assert!(std::fs::read(&text_entry).unwrap() == before, "untouched");
}

/// TLS-120 r1: `aldump --dump-dependency-source <ws> <out>` whose first source file's name
/// under `<out>` is a hard link to the text entry fails and leaves the entry as it was (its
/// folders, outside the caches, pass).
///
/// Discrimination (2026-11-24, r1): the source write back to `std::fs::write`: fails (exit
/// 0); restored, passes.
#[test]
fn aldump_dependency_source_is_never_a_cache_file() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let ws = f.ws.to_str().unwrap();
    let d = tempfile::tempdir().unwrap();
    let plain = d.path().join("plain");
    let out = aldump(
        &f,
        &["--dump-dependency-source", ws, plain.to_str().unwrap()],
        &[],
    );
    assert!(out.status.success(), "precondition: a plain dump: {out:?}");
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&plain)
        .into_iter()
        .map(|e| e.unwrap().into_path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    let first = files.first().expect("precondition: a dumped file");
    let linked = d
        .path()
        .join("linked")
        .join(first.strip_prefix(&plain).unwrap());
    std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
    std::fs::hard_link(&text_entry, &linked).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let out_dir = d.path().join("linked");
    let out = aldump(
        &f,
        &["--dump-dependency-source", ws, out_dir.to_str().unwrap()],
        &[],
    );
    assert_aldump_refused("(source)", &out, "is a file of the engine's caches");
    assert!(std::fs::read(&text_entry).unwrap() == before, "untouched");
}

/// TLS-120 r1: `aldump --session-refresh` edits its workspace copy through the guard. "(source
/// edit)": a copy inside the IR folder is refused at its first edit of the source file.
/// "(app.json)": a copy outside the caches whose `app.json` is a hard link to a file in the
/// impact-base cache's `work/` is refused at its `app.json` edit (the source edits before it
/// pass), and that file is as it was.
///
/// Discrimination (2026-11-24, r1): the copy's source `write` back to `std::fs::write`: fails
/// "(source edit)" (the refusal came at `write app.json` instead); restored, passes. The
/// `app.json` edit back to `std::fs::write`: fails "(app.json)" (the refusal came at its
/// restore instead); restored, passes.
#[test]
fn aldump_session_refresh_edits_stay_out_of_cache_folders() {
    let f = fixture();
    warm(&f);
    let copy_to = |copy: &Path| {
        std::fs::create_dir_all(copy.join("src")).unwrap();
        std::fs::create_dir_all(copy.join(".alpackages")).unwrap();
        std::fs::copy(f.ws.join("src/Ws.al"), copy.join("src/Ws.al")).unwrap();
        for e in std::fs::read_dir(f.ws.join(".alpackages")).unwrap() {
            let p = e.unwrap().path();
            std::fs::copy(&p, copy.join(".alpackages").join(p.file_name().unwrap())).unwrap();
        }
    };
    let copy = f.root.join("ws-copy");
    copy_to(&copy);
    std::fs::copy(f.ws.join("app.json"), copy.join("app.json")).unwrap();
    let out = aldump(&f, &["--session-refresh", copy.to_str().unwrap()], &[]);
    assert_aldump_refused("(source edit)", &out, "is in a cache folder of the engine");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("write the workspace copy"),
        "(source edit): {stderr}"
    );
    let d = tempfile::tempdir().unwrap();
    let ib = d.path().join("ib");
    std::fs::create_dir_all(ib.join("work/base")).unwrap();
    let held = ib.join("work/base/app.json");
    std::fs::copy(f.ws.join("app.json"), &held).unwrap();
    let before = std::fs::read(&held).unwrap();
    let copy = d.path().join("ws-copy");
    copy_to(&copy);
    std::fs::hard_link(&held, copy.join("app.json")).unwrap();
    let out = aldump(
        &f,
        &["--session-refresh", copy.to_str().unwrap()],
        &[("AL_SEM_IMPACT_BASE_CACHE", ib.to_str().unwrap())],
    );
    assert_aldump_refused("(app.json)", &out, "is a file of the engine's caches");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("write app.json"), "(app.json): {stderr}");
    assert!(
        std::fs::read(&held).unwrap() == before,
        "(app.json): untouched"
    );
}

/// TLS-120 r1: the IR cache report of a standalone run never lands on a cache file. With
/// `ALSEM_IR_CACHE_REPORT` a hard link to the text entry, `analyze` succeeds and the entry is
/// as it was (the refused report is a warning).
///
/// Discrimination (2026-11-24, r1): `append_shared` outside a batch back to a plain append
/// (`OpenOptions::new().append(true).create(true).open(path)`): fails "untouched" (the lines
/// were appended to the entry); restored, passes.
#[test]
fn standalone_ir_report_is_never_a_cache_file() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let rep = d.path().join("report.jsonl");
    std::fs::hard_link(&text_entry, &rep).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let ws = f.ws.to_str().unwrap();
    let out = alone(
        &f,
        &["analyze", ws, "--format", "json"],
        &[("ALSEM_IR_CACHE_REPORT", rep.to_str().unwrap())],
    );
    assert!(out.status.code().is_some(), "{out:?}");
    assert!(std::fs::read(&text_entry).unwrap() == before, "untouched");
}

/// TLS-120 r1: the cones census's schedule file is never a cache file. With
/// `ALSEM_CONES_CENSUS=1`, a plain `ALSEM_CONES_SCHEDULE_FILE` is written (the positive
/// control: the walk reaches it); one that is a hard link to the text entry is refused (its
/// stderr line) and the entry is as it was.
///
/// Discrimination (2026-11-24, r1): the schedule's open back to a plain append: fails (no
/// refusal line); restored, passes.
#[test]
fn standalone_cones_schedule_is_never_a_cache_file() {
    let f = fixture();
    let (_, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let ws = f.ws.to_str().unwrap();
    let run = |file: &Path| {
        alone(
            &f,
            &["analyze", ws, "--format", "json"],
            &[
                ("ALSEM_CONES_CENSUS", "1"),
                ("ALSEM_CONES_SCHEDULE_FILE", file.to_str().unwrap()),
            ],
        )
    };
    let plain = d.path().join("plain.txt");
    run(&plain);
    assert!(
        std::fs::read_to_string(&plain).is_ok_and(|s| s.starts_with("walk ")),
        "precondition: the schedule is written"
    );
    let linked = d.path().join("linked.txt");
    std::fs::hard_link(&text_entry, &linked).unwrap();
    let before = std::fs::read(&text_entry).unwrap();
    let out = run(&linked);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[cones-census] schedule file") && stderr.contains("(TLS-116)"),
        "{stderr}"
    );
    assert!(std::fs::read(&text_entry).unwrap() == before, "untouched");
}

/// TLS-120 r1: a cache subfolder that cannot be listed (mode 000) blocks no write that needs
/// no lookup: a plain `--out` (one link) is written. A write that needs the lookup (an
/// `--out` with a second link) cannot be checked and is refused, naming the folder.
///
/// Discrimination (2026-11-24, r1): the folders-only walk strict again (`|| !files` dropped
/// from `walk`'s listing error): fails "(one link)" (exit 1, not 0); restored, passes. A
/// failed lookup taken as "no cache file" (`holds_file` returning `false` on `whole`'s
/// error): fails "(two links)" (exit 0); restored, passes.
#[test]
fn standalone_unlistable_folder_blocks_only_a_lookup() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    warm(&f);
    let locked = f.xdg.join("al-ch-snapshot-cache/locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(
        std::fs::read_dir(&locked).is_err(),
        "precondition: the folder cannot be listed"
    );
    let d = tempfile::tempdir().unwrap();
    let one = d.path().join("one.json");
    let out_one = fanout_out(&f, &one);
    let two = d.path().join("two.json");
    std::fs::write(&two, "OLD").unwrap();
    std::fs::hard_link(&two, d.path().join("two-link.json")).unwrap();
    let out_two = fanout_out(&f, &two);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(out_one.status.code(), Some(0), "(one link): {out_one:?}");
    assert!(
        std::fs::metadata(&one).unwrap().len() > 0,
        "(one link): written"
    );
    let stderr = String::from_utf8_lossy(&out_two.stderr);
    assert_eq!(out_two.status.code(), Some(1), "(two links): {stderr}");
    assert!(
        stderr.contains("cannot list the cache folder") && stderr.contains("locked"),
        "(two links): {stderr}"
    );
    assert_eq!(
        std::fs::read(&two).unwrap(),
        b"OLD",
        "(two links): untouched"
    );
}

/// TLS-120 r1: a missing cache folder is made by no write. With the IR folder missing (the
/// cache off, so nothing makes it), a plain `--out` makes no folder there, and a shard
/// directory spelled inside it (or as it) is refused and not made.
///
/// Discrimination (2026-11-24, r1): the missing-folder test off (`.any(..)` made `false` in
/// `link_hops`): fails "(inside)" (the folder was made); restored, passes.
#[test]
fn standalone_write_makes_no_missing_cache_folder() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let ir = d.path().join("ir-missing");
    let env = [
        ("ALSEM_IR_CACHE", "off"),
        ("ALSEM_IR_CACHE_DIR", ir.to_str().unwrap()),
    ];
    let ws = f.ws.to_str().unwrap();
    let ok = d.path().join("ok.json");
    let o = ok.to_str().unwrap();
    let out = alone(
        &f,
        &["events", "fanout", ws, "--single-app", "--out", o],
        &env,
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(!ir.exists(), "a plain --out makes no cache folder");
    for (case, dir) in [("(inside)", ir.join("sh")), ("(as it)", ir.clone())] {
        let o = dir.to_str().unwrap();
        let out = alone(&f, &["fingerprint", ws, "--shard", "all", "--out", o], &env);
        assert_write_refused(case, &out, "is in a cache folder of the engine");
        assert!(!ir.exists(), "{case}: nothing made");
    }
}

/// TLS-120 r1: `cache prune --dep-cache-dir` never runs in a cache folder: the text folder,
/// or a folder inside the IR folder, is refused (exit 1) and every entry stays; an ordinary
/// folder is pruned as before.
///
/// Discrimination (2026-11-24, r1): the refusal skipped (`if let Some(dir)` made `if false`
/// in `run_cache_prune_cmd`): fails "(text)" (exit 0, entries removed); restored, passes.
#[test]
fn cache_prune_refuses_a_cache_folder() {
    let f = fixture();
    let (ir_entry, text_entry) = warm_entries(&f);
    let d = tempfile::tempdir().unwrap();
    let prune = |dir: &Path| {
        alone(
            &f,
            &["cache", "prune", "--dep-cache-dir", dir.to_str().unwrap()],
            &[],
        )
    };
    let out = prune(d.path());
    assert_eq!(out.status.code(), Some(0), "an ordinary folder: {out:?}");
    for (case, dir) in [
        ("(text)", text_entry.parent().unwrap()),
        ("(inside ir)", ir_entry.parent().unwrap()),
    ] {
        let out = prune(dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "{case}: {stderr}");
        assert!(stderr.contains("(TLS-120)"), "{case}: {stderr}");
        assert!(
            ir_entry.exists() && text_entry.exists(),
            "{case}: entries kept"
        );
    }
}

/// TLS-120 r1: the impact-base cache's `work/` (live base-workspace copies) takes the same
/// guard: an `--out` inside it, or a hard link to a file in it, is refused and the file is as
/// it was.
///
/// Discrimination (2026-11-24, r1): `work/` left out of the standalone roots: fails
/// "(inside)" (exit 0); restored, passes.
#[test]
fn standalone_out_stays_out_of_impact_work() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let ib = d.path().join("ib");
    let copy = ib.join("work/base-1");
    std::fs::create_dir_all(&copy).unwrap();
    let held = copy.join("A.al");
    std::fs::write(&held, "codeunit 1 A { }").unwrap();
    let link = d.path().join("link.json");
    std::fs::hard_link(&held, &link).unwrap();
    let ws = f.ws.to_str().unwrap();
    for (case, path, why) in [
        (
            "(inside)",
            copy.join("x.json"),
            "is in a cache folder of the engine",
        ),
        ("(a hard link)", link, "is a file of the engine's caches"),
    ] {
        let o = path.to_str().unwrap();
        let out = alone(
            &f,
            &["events", "fanout", ws, "--single-app", "--out", o],
            &[("AL_SEM_IMPACT_BASE_CACHE", ib.to_str().unwrap())],
        );
        assert_write_refused(case, &out, why);
        assert_eq!(std::fs::read(&held).unwrap(), b"codeunit 1 A { }", "{case}");
    }
    assert!(!copy.join("x.json").exists());
}

/// TLS-120 r2: a cache folder missing at the snapshot and made later is protected from then
/// on. The trace file (`ALSEM_TRACE_FILE`) takes the snapshot at the process's start, with an
/// empty cache home.
/// The debug seam `ALSEM_TEST_DURING_SNAPSHOT` makes the folder while that first snapshot
/// walks (the impact-base `work/` is the folder it walks), as a first run's engine makes its
/// caches after the trace file opened. (In this fixture the engine itself makes the text
/// folder only after the `--out` write, which the missing-folder test covers.)
/// - "(entry)": the seam makes the text folder and an entry in it; an `--out` that is the entry
///   is refused and the entry is as it was. (A NEW name there is refused even without the
///   re-check: the missing-folder test meets the missing file under the remembered path.)
/// - "(link)": the seam makes the text folder, an entry in it and an outside hard link to the
///   entry; an `--out` on the link is refused and the entry is as it was.
///
/// Discrimination (2026-11-24, r2): the re-check of missing folders off (`.any(|m|
/// m.is_dir())` made `false` in `standalone`): fails "(entry)" (exit 0: the entry clobbered);
/// restored, passes.
#[cfg(debug_assertions)]
#[test]
fn standalone_protects_a_cache_folder_made_after_its_snapshot() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let ib = d.path().join("ib");
    std::fs::create_dir_all(ib.join("work")).unwrap();
    let text = f.xdg.join("al-ch-snapshot-cache");
    assert!(!text.exists(), "precondition: an empty cache home");
    let trace = d.path().join("trace.json");
    let ws = f.ws.to_str().unwrap();
    let run = |out: &Path, seam: &str| {
        let o = out.to_str().unwrap();
        alone(
            &f,
            &["events", "fanout", ws, "--out", o],
            &[
                ("ALSEM_TRACE", "1"),
                ("ALSEM_TRACE_FILE", trace.to_str().unwrap()),
                ("AL_SEM_IMPACT_BASE_CACHE", ib.to_str().unwrap()),
                ("ALSEM_TEST_DURING_SNAPSHOT", seam),
            ],
        )
    };
    let (entry, link) = (text.join("e.bin"), d.path().join("link.json"));
    let made = format!(
        "mkdir -p '{}' && printf ENTRY > '{}'",
        text.display(),
        entry.display()
    );
    let out = run(&entry, &made);
    assert!(entry.exists(), "(entry): precondition: the seam made it");
    assert_write_refused("(entry)", &out, "is in a cache folder of the engine");
    assert_eq!(
        std::fs::read(&entry).unwrap(),
        b"ENTRY",
        "(entry): untouched"
    );
    std::fs::remove_dir_all(&f.xdg).unwrap();
    let seam = format!(
        "mkdir -p '{}' && printf ENTRY > '{}' && ln '{}' '{}'",
        text.display(),
        entry.display(),
        entry.display(),
        link.display()
    );
    let out = run(&link, &seam);
    assert!(
        link.exists(),
        "(link): precondition: the seam made the link"
    );
    assert_write_refused("(link)", &out, "is a file of the engine's caches");
    assert_eq!(
        std::fs::read(&entry).unwrap(),
        b"ENTRY",
        "(link): untouched"
    );
}

/// TLS-120 r3: a cache folder deleted and made again after the snapshot is protected again.
/// The text folder exists at the snapshot (made before the run); the debug seam
/// `ALSEM_TEST_DURING_SNAPSHOT` deletes it after the walk listed it and makes it again with
/// an entry in it (new identities). An `--out` that is that entry is refused and the entry is
/// as it was.
///
/// Discrimination (2026-11-24, r3): the identity re-check of present folders off (`!holds(..)`
/// made `false` in `standalone`): fails (exit 0: the entry clobbered); restored, passes.
#[cfg(debug_assertions)]
#[test]
fn standalone_protects_a_cache_folder_made_again() {
    let f = fixture();
    let d = tempfile::tempdir().unwrap();
    let text = f.xdg.join("al-ch-snapshot-cache");
    std::fs::create_dir_all(&text).unwrap();
    let entry = text.join("e.bin");
    let seam = format!(
        "rm -rf '{t}' && mkdir -p '{t}' && printf ENTRY > '{e}'",
        t = text.display(),
        e = entry.display()
    );
    let ws = f.ws.to_str().unwrap();
    let out = alone(
        &f,
        &[
            "events",
            "fanout",
            ws,
            "--single-app",
            "--out",
            entry.to_str().unwrap(),
        ],
        &[
            (
                "AL_SEM_IMPACT_BASE_CACHE",
                d.path().join("ib").to_str().unwrap(),
            ),
            ("ALSEM_TEST_DURING_SNAPSHOT", &seam),
        ],
    );
    assert!(entry.exists(), "precondition: the seam made the entry");
    assert_write_refused("(made again)", &out, "is in a cache folder of the engine");
    assert_eq!(std::fs::read(&entry).unwrap(), b"ENTRY", "untouched");
}

/// TLS-120 r4: a cache path that is no folder at the snapshot (a file, or a dangling symlink)
/// is watched like a missing one: the debug seam `ALSEM_TEST_DURING_SNAPSHOT` (run while the
/// snapshot walks the impact-base `work/`) makes it a folder with an entry in it, and an
/// `--out` that is that entry is refused, the entry as it was.
///
/// Discrimination (2026-11-24, r4): `first_standalone` dropping such a path again (only a
/// path whose `symlink_metadata` fails taken as missing): fails "(a file)" (exit 0: the entry
/// clobbered); restored, passes.
#[cfg(debug_assertions)]
#[test]
fn standalone_protects_a_cache_path_that_was_no_folder() {
    for case in ["(a file)", "(a dangling link)"] {
        let f = fixture();
        let d = tempfile::tempdir().unwrap();
        let ib = d.path().join("ib");
        std::fs::create_dir_all(ib.join("work")).unwrap();
        std::fs::create_dir_all(&f.xdg).unwrap();
        let text = f.xdg.join("al-ch-snapshot-cache");
        let target = d.path().join("target");
        let entry = text.join("e.bin");
        let seam = if case == "(a file)" {
            std::fs::write(&text, "not a folder").unwrap();
            format!(
                "rm '{t}' && mkdir '{t}' && printf ENTRY > '{e}'",
                t = text.display(),
                e = entry.display()
            )
        } else {
            std::os::unix::fs::symlink(&target, &text).unwrap();
            format!(
                "mkdir '{g}' && printf ENTRY > '{e}'",
                g = target.display(),
                e = entry.display()
            )
        };
        let ws = f.ws.to_str().unwrap();
        let out = alone(
            &f,
            &[
                "events",
                "fanout",
                ws,
                "--single-app",
                "--out",
                entry.to_str().unwrap(),
            ],
            &[
                ("AL_SEM_IMPACT_BASE_CACHE", ib.to_str().unwrap()),
                ("ALSEM_TEST_DURING_SNAPSHOT", &seam),
            ],
        );
        assert!(
            entry.exists(),
            "{case}: precondition: the seam made the entry"
        );
        assert_write_refused(case, &out, "is in a cache folder of the engine");
        assert_eq!(
            std::fs::read(&entry).unwrap(),
            b"ENTRY",
            "{case}: untouched"
        );
    }
}
