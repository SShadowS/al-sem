//! The resident session keeps nothing per refresh (context-server design §7,
//! the per-refresh RSS creep): its live heap after a run of body-only (B) and
//! declaration (S) refreshes, each followed by the four served asks (query,
//! explain, prove, digest), stays within a small bound of its live heap after
//! the first refresh, in both memory modes. On CDO the creep was measured to
//! be the allocator's (the live heap flat within 1 MiB over 30 refreshes,
//! `tools/census-probe refresh-seq`); this pins that no revision, row-cache
//! entry, dependency text, generation or other per-refresh state is kept, so
//! a retention that comes back fails here before it costs RSS.
//!
//! The bytes are read from a counting `#[global_allocator]`, so the test
//! crate is its own binary (the allocator counts the whole process) and
//! holds ONE test (nothing else allocates beside it).
//!
//! DISCRIMINATION (break with Edit, run, revert, run): the resident session
//! keeping every previous revision alive (`std::mem::forget` of a clone of
//! the released one in `ResidentSession::build_locked`) fails the bound.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

use al_sem::engine::batch::job::Job;
use al_sem::engine::resident::session::{Built, Freshness, ResidentSession};
use al_sem::engine::resident::store::Control;
use al_sem::program::inputs::WorkspaceInputs;

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: every call forwards to `System` with the same arguments; the
// counter only observes the sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Relaxed);
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            LIVE.fetch_add(new as isize - l.size() as isize, Relaxed);
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// The live-heap bound over the first refresh's. Not zero: the process's
/// live heap moves by up to about 0.7 MB between refreshes on this fixture
/// with nothing kept (measured over 30 B refreshes, no trend), the deferred
/// garbage of the rayon pools' work-stealing queues (2,064- and 2,320-byte
/// blocks that `crossbeam-epoch` frees lazily). A kept revision adds its
/// whole model and artifacts on every refresh, many times this over the
/// run (see the discrimination note above).
const BOUND: isize = 2 * 1024 * 1024;

/// Body-only and declaration refreshes after the first one.
const B_STEPS: usize = 24;
const S_STEPS: usize = 6;

const ROUTINE: &str = "CallEmbeddedSourceDep";

/// A copy of `tests/fixtures/lsp-diff-deps` (a workspace with a
/// source-bearing and a symbol-only dependency): edits never touch the
/// fixture.
fn copy_fixture() -> tempfile::TempDir {
    fn copy(from: &Path, to: &Path) {
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                std::fs::create_dir_all(&t).unwrap();
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), &t).unwrap();
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    copy(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lsp-diff-deps"),
        dir.path(),
    );
    dir
}

/// One served ask of `kind` over `inputs`.
fn job<'a>(kind: &str, inputs: &'a Arc<WorkspaceInputs>) -> Box<dyn Job + 'a> {
    use al_sem::engine::l5::digest_cli::{DiffRoot, DigestJob, DigestRequest};
    use al_sem::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    let ver = "test";
    match kind {
        "query" => Box::new(al_sem::engine::l4::effect_query_cli::QueryJob::effects(
            inputs, ROUTINE, ver, true, false,
        )),
        "explain" => Box::new(ExplainJob::new(
            ExplainRequest {
                inputs,
                mode: ExplainMode::Routines(vec![ROUTINE.to_string()]),
                single_app: false,
            },
            true,
            None,
        )),
        "prove" => Box::new(al_sem::engine::l5::prove::ProveJob {
            detectors: al_sem::engine::l5::detectors::default_detectors(),
            inputs,
            routine_selector: ROUTINE,
            question_text: "may-commit",
            alsem_ver: ver,
            deterministic: true,
            single_app: false,
            closed_world: false,
            assume_no_dependents: false,
            human: false,
            detail: false,
            out: None,
            slice: None,
            surface: Default::default(),
        }),
        _ => Box::new(DigestJob::new(
            DigestRequest {
                inputs,
                changed_files: None,
                changed_routines: Some(vec![ROUTINE.to_string()]),
                diff_text: None,
                alsem_ver: ver,
                deterministic: true,
                single_app: false,
                tests: false,
                include_reads: false,
                diff_root: DiffRoot::Workspace.into(),
            },
            false,
            None,
        )),
    }
}

/// A refresh of `session` (a `fresh` pin, as the child pins) of the class
/// `want`, then the four asks; returns the live heap after them, with the
/// previous revision released and the new one still published.
fn refresh_and_ask(session: &Arc<ResidentSession>, want: &str, what: &str) -> isize {
    let pinned = session
        .pin(Freshness::Fresh, &|| None)
        .unwrap_or_else(|e| panic!("{what}: no pin: {e:?}"));
    let Built::Revision(rev) = pinned.built.clone() else {
        panic!("{what}: the build failed")
    };
    drop(pinned);
    assert_eq!(
        session.stats().last_build_class.as_deref(),
        Some(want),
        "{what}: the refresh class"
    );
    for kind in ["query", "explain", "prove", "digest"] {
        let j = job(kind, &rev.inputs);
        let out = rev.answer(&*j, &Control::unreserved());
        assert!(out.is_ok(), "{what}: {kind} failed");
    }
    drop(rev);
    LIVE.load(Relaxed)
}

fn run(low_mem: bool) {
    let mode = if low_mem { "low-mem" } else { "default" };
    let dir = copy_fixture();
    let caller = dir.path().join("Caller.al");
    let original = std::fs::read_to_string(&caller).unwrap();
    assert_eq!(original.matches("\"Source Mgt\".DoWork(3);").count(), 1);
    let session = Arc::new(
        ResidentSession::new(dir.path().to_path_buf(), false, "test".into()).with_low_mem(low_mem),
    );
    let edit = |text: String| {
        std::fs::write(&caller, text).unwrap();
        session.notify_changed([caller.clone()]);
    };
    let body = |i: usize| {
        original.replacen(
            "\"Source Mgt\".DoWork(3);",
            &format!("\"Source Mgt\".DoWork(3);\n        // edit {i}"),
            1,
        )
    };
    let decl = |j: usize| {
        let close = original.rfind('}').unwrap();
        format!(
            "{}\n    procedure Added{j}()\n    begin\n    end;\n{}",
            &original[..close],
            &original[close..]
        )
    };

    refresh_and_ask(&session, "P", &format!("{mode} cold"));
    edit(body(0));
    let first = refresh_and_ask(&session, "B", &format!("{mode} B0"));
    let bound = BOUND;
    let mut seen = Vec::new();
    for i in 1..=B_STEPS {
        edit(body(i));
        seen.push((
            format!("B{i}"),
            refresh_and_ask(&session, "B", &format!("{mode} B{i}")),
        ));
    }
    for j in 0..S_STEPS {
        edit(decl(j));
        seen.push((
            format!("S{j}"),
            refresh_and_ask(&session, "S", &format!("{mode} S{j}")),
        ));
    }
    for (step, live) in &seen {
        assert!(
            *live <= first + bound,
            "{mode}: live heap after {step} is {live} B, more than {bound} B over the first refresh's {first} B: \
             something is kept per refresh (all: {seen:?})"
        );
    }
}

#[test]
fn live_heap_stays_bounded_across_refreshes_in_both_modes() {
    run(false);
    run(true);
}
