//! TLS-21: the up walk over several roots at once. Fixture:
//! `tests/fixtures/up-parallel/ws` (six roots: Alpha, Beta with two nodes, one
//! not in the graph, Gamma with a page-action caption, Delta over `RunObject`,
//! Lonely), built in-process cross-app. The reference is master `4f2459a6`'s
//! `--part up` output (`up.master.golden.json`, see `tests/cli/cli_up_parallel.rs`).
//!
//! Each worker count is its own pool (`reach_pool` keeps one per count for the
//! process), so a held task never waits on another test's tasks: 6, 5 and 2 here,
//! 3 in `revision_tests`, 7 and 9 in TLS-22's tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use serde_json::Value;

use super::{Captions, Keep, UpArgs, up};
use crate::engine::l5::digest_cli::{PassProbe, ReachWorkers};
use crate::program::impact_graph::SitedGraph;

const HOLD: Duration = Duration::from_secs(10);
const APP: &str = "4b7e2d91-6c3a-4f58-a0d2-1e9c8b5f7a36:Codeunit:51400#";
const ROOT_HASHES: [&str; 6] = [
    "81c17c9a2aeee9d00bd7820fcaccb88e39c54eb5b6f812184c979dbe654005b3",
    "f6c9f8d45c19c37415ec3eb8d87823274ae3035d8c82c5175b9778646f614b1d",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "d800ba134870623f51844fbfbb72920fc06f06a84586ecd7ce57881918fe0a9f",
    "fe1aefe8188b436f7e9aa6de6bad5c41346e321d5f78210204a8c3afdffcb0a5",
    "c723a99e303f0e7dcb2865b635356ddc3fa2671ddf23757bce1cc40012216b88",
];

pub(crate) fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The fixture in a temporary folder with a `.git` marker.
pub(crate) fn fixture_dir() -> tempfile::TempDir {
    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &to.join(e.file_name()));
            } else {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    let t = tempfile::tempdir().unwrap();
    copy(
        &repo().join("tests/fixtures/up-parallel/ws"),
        &t.path().join("ws"),
    );
    std::fs::create_dir(t.path().join(".git")).unwrap();
    t
}

pub(crate) fn root_ids() -> Vec<String> {
    ROOT_HASHES.iter().map(|h| format!("{APP}{h}")).collect()
}

struct Fx {
    _dir: tempfile::TempDir,
    g: SitedGraph,
    texts: HashMap<String, Arc<str>>,
}

fn fx() -> Fx {
    let dir = fixture_dir();
    let inputs = crate::program::inputs::WorkspaceInputs::read(
        &dir.path().join("ws"),
        &crate::engine::resident::session::REVISION_READ,
    );
    let p = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .expect("the fixture builds");
    let g = crate::program::impact_graph::sited_from_program(&p, &|| false).expect("built");
    let texts = inputs
        .al_files
        .iter()
        .filter_map(|f| {
            f.text
                .as_ref()
                .ok()
                .map(|t| (f.rel_posix.clone(), Arc::clone(t)))
        })
        .collect();
    Fx {
        _dir: dir,
        g,
        texts,
    }
}

fn run(f: &Fx, roots: &[String], workers: ReachWorkers, captions: &Captions) -> Value {
    up(
        &f.g,
        &UpArgs {
            roots,
            assume_no_dependents: false,
            keep: Keep::new(false),
            cancel: None,
            workers,
        },
        captions,
    )
}

/// The answer with the run-measuring fields blanked, as the golden holds them.
fn blanked(mut v: Value) -> Value {
    for k in ["walkMs", "walksUs", "scratchBytes"] {
        v["cost"][k] = Value::Null;
    }
    v
}

fn golden() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(repo().join("tests/fixtures/up-parallel/up.master.golden.json"))
            .unwrap(),
    )
    .unwrap()
}

fn captions(f: &Fx) -> Captions {
    Captions::new(f.texts.clone())
}

/// Rows keep root order, and equal master's answer, when the first walked
/// root finishes LAST (held until the other four are done): the pass
/// collects by index, never in completion order. Also at one worker.
///
/// DISCRIMINATION (2026-11-12): (a) collecting the rows in completion order
/// (`for_each` pushing into a `Mutex<Vec>`) fails the root-order assert (the
/// not-in-graph row first); (b) pushing the not-in-graph rows after the
/// walked ones, in both paths, fails here and in
/// `cli_up_parallel::up_part_matches_master_at_1_and_8_workers` (already at
/// 1 worker); restored, both pass.
#[test]
fn parallel_rows_keep_root_order_when_root_0_finishes_last() {
    let f = fx();
    let roots = root_ids();
    let probe = Arc::new(PassProbe::default());
    *probe.hold_first_root.lock().unwrap() = Some(HOLD);
    let got = run(
        &f,
        &roots,
        ReachWorkers::new(6).with_probe(Arc::clone(&probe)),
        &captions(&f),
    );
    assert!(probe.max() > 1, "precondition: walks ran at once");
    assert_eq!(probe.up_tasks.load(SeqCst), 5, "one task per walked root");
    let ids: Vec<&str> = got["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["stableId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, roots.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(blanked(got), golden(), "master's answer at 6 workers");
    let serial = run(&f, &roots, ReachWorkers::SERIAL, &captions(&f));
    assert_eq!(blanked(serial), golden(), "master's answer at 1 worker");
}

/// Two tasks that miss the same file at once both get its captions: the
/// first one to miss is held between its parse and its insert until the
/// second has looked the file up. Gamma is asked twice, so two tasks walk
/// it and both need `src/Up.al`'s actions for its page-action caption.
/// (Master gives no caption to the page Delta is reached from, the page an
/// action opens with `RunObject`, so only Gamma's row needs the file.)
///
/// DISCRIMINATION (2026-11-12): marking the file "being parsed" with an
/// empty entry before parsing (so the second task reads the empty entry)
/// fails (a row without its caption); restored, passes.
#[test]
fn racing_tasks_both_get_the_caption() {
    let f = fx();
    let gamma = root_ids()[3].clone();
    let roots = vec![gamma.clone(), gamma];
    let want = golden();
    let gamma_row = &want["roots"][3];
    assert!(
        gamma_row.to_string().contains("\"caption\":\"Run Gamma\""),
        "precondition: Gamma's row carries the caption on master"
    );
    let want_rows = [gamma_row.clone(), gamma_row.clone()];
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = Some((2, HOLD));
    let c = captions(&f).with_race("src/Up.al", HOLD);
    let got = run(
        &f,
        &roots,
        ReachWorkers::new(5).with_probe(Arc::clone(&probe)),
        &c,
    );
    assert_eq!(probe.max(), 2, "precondition: the two walked at once");
    let rows = got["roots"].as_array().unwrap();
    // The entry lists, where the caption sits (the program share depends on
    // which roots were asked, so the whole row is not master's here).
    assert_eq!(
        rows[0]["entries"], want_rows[0]["entries"],
        "the first task's entries"
    );
    assert_eq!(
        rows[1]["entries"], want_rows[1]["entries"],
        "the second task's entries"
    );
    assert_eq!(c.cached(), 1, "one entry for the file");
}

/// Called from inside a rayon worker the pass runs one root at a time, even
/// when asked for four: `install` from a worker could run other jobs while
/// it waits (TLS-22's reason).
///
/// DISCRIMINATION (2026-11-12): deleting the `current_thread_index` check
/// fails (4 in flight); restored, passes.
#[test]
fn inside_a_rayon_worker_the_up_walk_is_serial() {
    let f = fx();
    let roots = root_ids();
    let probe = Arc::new(PassProbe::default());
    *probe.first_wave.lock().unwrap() = Some((2, Duration::from_secs(2)));
    let outer = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let got = outer.install(|| {
        run(
            &f,
            &roots,
            ReachWorkers::new(4).with_probe(Arc::clone(&probe)),
            &captions(&f),
        )
    });
    assert_eq!(
        probe.up_tasks.load(SeqCst),
        5,
        "precondition: every root walked"
    );
    assert_eq!(probe.max(), 1, "one walk at a time");
    assert_eq!(blanked(got), golden());
}

/// No more scratches than walks in flight: at two workers over five walked
/// roots and the union, at most two are made.
///
/// DISCRIMINATION (2026-11-12): a fresh scratch per walk (never taking one
/// back from the stash) fails (6 made); restored, passes.
#[test]
fn scratches_are_reused_across_roots() {
    let f = fx();
    let roots = root_ids();
    let probe = Arc::new(PassProbe::default());
    let got = run(
        &f,
        &roots,
        ReachWorkers::new(2).with_probe(Arc::clone(&probe)),
        &captions(&f),
    );
    assert_eq!(probe.up_tasks.load(SeqCst), 5, "precondition");
    let made = probe.scratches_made.load(SeqCst);
    assert!((1..=2).contains(&made), "{made} scratches made");
    assert_eq!(blanked(got), golden());
}
