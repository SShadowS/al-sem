//! M0-T7 (agent-tasks design §1.3, §1.2 M0-6): a composite request over the
//! engine pool. Real engine children (`alsem engine-host`), each sleeping on
//! every ask (`ALSEM_ENGINE_TEST_HOOK=sleep-on-ask:<ms>`, debug builds).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use al_sem::mcp::Decide;
use al_sem::mcp::output::OutputSink;
use al_sem::mcp::pool::{ChildConfig, Pool, PoolConfig, Request, SubOutcome, Transport};
use al_sem::mcp::tools::{CommandCall, Freshness, Universe};
use serde_json::{Value, json};

const ALSEM: &str = env!("CARGO_BIN_EXE_alsem");

fn corpus(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/r0-corpus")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// Frames the pool sent, decoded.
#[derive(Default)]
struct Recorder(Mutex<Vec<Value>>);

impl Transport for Recorder {
    fn response(&self, id: &Value, body: &Value, is_error: bool, _: Option<Value>) -> Value {
        json!({"id": id, "body": body, "isError": is_error})
    }
    fn send(&self, frame: &[u8]) {
        let v: Value = serde_json::from_slice(frame).unwrap_or(Value::Null);
        self.0.lock().unwrap().push(v);
    }
    fn cancelled(&self, _: &Value, _: Option<&[u8]>) {}
    fn progress(&self, _: &Value, _: u32, _: &str) {}
}

struct NoDecisions;

impl Decide for NoDecisions {
    fn decide(&self, _: &[String], _: Option<&str>) -> Option<al_sem::engine::host::Decisions> {
        None
    }
}

fn call(workspace: &str, universe: Universe) -> CommandCall {
    let mut argv: Vec<String> = ["query", "effects", "--routine=Archive", "--format=human"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    if universe == Universe::SingleApp {
        argv.push("--single-app".into());
    }
    argv.push("--".into());
    argv.push(workspace.to_string());
    CommandCall {
        tool: "alsem_query_effects",
        workspace: workspace.to_string(),
        universe,
        freshness: Freshness::Fresh,
        argv,
        diff_text: None,
        out: None,
        max_inline_bytes: None,
        always_file: false,
        client_budget_chars: None,
    }
}

/// Both children busy (max-children 2): the composite's part on one of
/// their engines waits in that engine's own queue and is answered; its part
/// on a third engine cannot be admitted and says why. Nothing deadlocks:
/// the whole request ends well inside the timeout.
///
/// DISCRIMINATION (2026-11-05): an admission that waits (before a later
/// part's `enqueue` in `Pool::run_composite`, sleeping while every worker
/// has calls pending) fails "a resource reason": the second part is
/// answered once a child frees, 10 s in, instead of being refused at once.
#[test]
fn a_composite_with_both_children_busy_returns_the_main_part_and_a_reason() {
    let out_root = tempfile::tempdir().unwrap();
    let sink = OutputSink::create(out_root.path(), "composite-test").unwrap();
    let transport = Arc::new(Recorder::default());
    let pool = Arc::new(Pool::new(
        PoolConfig {
            program: PathBuf::from(ALSEM),
            child_env: vec![("ALSEM_ENGINE_TEST_HOOK".into(), "sleep-on-ask:2500".into())],
            children: ChildConfig {
                budgets: (6 << 30, 2 << 30),
                low_mem: false,
                idle_timeout_secs: 0,
                max_children: 2,
            },
            sink,
            decide: Arc::new(NoDecisions),
        },
        Arc::clone(&transport) as Arc<dyn Transport>,
    ));
    let a = corpus("ws-explain");
    let c = corpus("ws-api-page");
    // Two long calls keep both children busy.
    pool.submit(
        json!("busy-1"),
        Request::Command {
            call: call(&a, Universe::CrossApp),
            progress: None,
        },
    );
    // The second child stays busy longer than the composite's first part
    // takes (five asks queued against that part's two).
    for i in 0..5 {
        pool.submit(
            json!(format!("busy-2-{i}")),
            Request::Command {
                call: call(&a, Universe::SingleApp),
                progress: None,
            },
        );
    }
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let pool = Arc::clone(&pool);
        let (a, c) = (a.clone(), c.clone());
        std::thread::spawn(move || {
            let parts = vec![call(&a, Universe::CrossApp), call(&c, Universe::CrossApp)];
            let _ = tx.send(pool.run_composite(&json!("composite"), parts));
        });
    }
    // Queued behind the composite's first part, so the first engine is
    // still busy (not idle, not evictable) when that part has been
    // answered and the second part asks for a child.
    std::thread::sleep(Duration::from_millis(300));
    pool.submit(
        json!("busy-3"),
        Request::Command {
            call: call(&a, Universe::CrossApp),
            progress: None,
        },
    );
    let outcomes = rx
        .recv_timeout(Duration::from_secs(90))
        .expect("the composite request ends (no deadlock)");
    assert!(start.elapsed() < Duration::from_secs(90));
    assert_eq!(outcomes.len(), 2);
    match &outcomes[0] {
        SubOutcome::Answered(v) => {
            assert_eq!(v["isError"], json!(false), "{v}");
            assert_eq!(v["body"]["kind"], "cli", "{v}");
        }
        o => panic!("the main part is answered: {o:?}"),
    }
    match &outcomes[1] {
        SubOutcome::NotComputed(why) => {
            assert!(
                why.contains("could not start") && why.contains("max-children 2"),
                "a resource reason: {why}"
            );
        }
        o => panic!("a resource reason: {o:?}"),
    }
    // M0-6's descriptor: the worker asked its child for the revision's input
    // descriptor after the call and holds it, read here without the (busy)
    // front end.
    let d = pool
        .descriptor(&a, Universe::CrossApp)
        .expect("the worker published the revision's descriptor");
    assert_eq!(d["universe"], "crossApp", "{d}");
    assert!(d["generation"].as_u64().is_some(), "{d}");
    assert!(
        d["manifest"]
            .as_array()
            .is_some_and(|m| m.iter().any(|e| e["statLen"].is_u64())),
        "file entries carry their stat: {d}"
    );
    pool.shutdown(true);
}

/// A pool of `max` children whose every ask sleeps `sleep_ms`.
fn sleepy_pool(max: usize, sleep_ms: u64) -> (tempfile::TempDir, Arc<Pool>) {
    let out_root = tempfile::tempdir().unwrap();
    let sink = OutputSink::create(out_root.path(), "admission-test").unwrap();
    let pool = Arc::new(Pool::new(
        PoolConfig {
            program: PathBuf::from(ALSEM),
            child_env: vec![(
                "ALSEM_ENGINE_TEST_HOOK".into(),
                format!("sleep-on-ask:{sleep_ms}").into(),
            )],
            children: ChildConfig {
                budgets: (6 << 30, 2 << 30),
                low_mem: false,
                idle_timeout_secs: 0,
                max_children: max,
            },
            sink,
            decide: Arc::new(NoDecisions),
        },
        Arc::new(Recorder::default()) as Arc<dyn Transport>,
    ));
    (out_root, pool)
}

/// B-11 (agent-tasks §10.6 step 3, owner: no oversubscription): with the
/// one child busy (max-children 1), an impact base child is not admitted:
/// after its wait it is refused with the reason, and the bound's count never
/// passes one. Once the child is idle, the base child takes its place (the
/// idle child ends), and while the base child holds the place a call for
/// another engine is refused rather than started beside it.
///
/// Break: admit whenever asked (`admit_rule` returning `Now`) → the first
/// admission succeeds while the child is busy.
#[test]
fn b11_a_base_child_is_never_admitted_over_the_bound() {
    use std::sync::atomic::AtomicBool;
    let (_out, pool) = sleepy_pool(1, 3000);
    let a = corpus("ws-explain");
    pool.submit(
        json!("busy"),
        Request::Command {
            call: call(&a, Universe::CrossApp),
            progress: None,
        },
    );
    std::thread::sleep(Duration::from_millis(500));
    let cancel = AtomicBool::new(false);
    let refused = pool.admit_oneshot(Duration::from_millis(400), &cancel);
    let why = refused.err().expect("refused while the one child is busy");
    assert!(
        why.contains("base not built") && why.contains("max-children 1"),
        "{why}"
    );
    // The busy call ends; the idle child gives its place.
    let start = Instant::now();
    let slot = pool
        .admit_oneshot(Duration::from_secs(60), &cancel)
        .expect("admitted once the child is idle");
    assert!(start.elapsed() < Duration::from_secs(60));
    // While the base child holds the only place, another engine's call
    // cannot start a child beside it.
    let c = corpus("ws-api-page");
    let parts = pool.run_composite(&json!("beside"), vec![call(&c, Universe::CrossApp)]);
    match &parts[0] {
        SubOutcome::NotComputed(why) => assert!(why.contains("max-children 1"), "{why}"),
        o => panic!("refused beside the base child: {o:?}"),
    }
    drop(slot);
    pool.shutdown(true);
}

/// The admission rule, case by case.
#[test]
fn the_admission_rule() {
    use al_sem::mcp::pool::{Admit, admit_rule};
    assert_eq!(admit_rule(0, 0, 0, 2), Admit::Now);
    assert_eq!(admit_rule(1, 1, 0, 2), Admit::Now);
    assert_eq!(admit_rule(2, 1, 0, 2), Admit::Evict);
    assert_eq!(admit_rule(1, 0, 1, 2), Admit::Wait);
    assert_eq!(admit_rule(2, 0, 0, 2), Admit::Wait);
}
