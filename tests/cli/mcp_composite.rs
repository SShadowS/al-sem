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
