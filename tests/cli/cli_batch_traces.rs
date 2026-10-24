//! #277 T6 (D10): a batch trace splits per job. Every `job.step` (resolve,
//! compute, finalize, serialize) lies inside exactly one `job.<i>.<cmd>` span,
//! its own job's; every fill (a `build.*` counter) lies inside a `view.<i>`
//! span and inside no job span, except a cut reader's own composition (D6.4:
//! fingerprint B0 composes inside its compute), whose fills lie inside its
//! own job span (D10 as amended, R7-4).
//!
//! "Inside" is decided by the order of the events in the trace file (one
//! writer behind a mutex, so file order is emission order), never by their
//! integer-microsecond timestamps, which tie (R7-2).

use serde_json::Value;

use crate::cli_batch_plan::{argv, batch, is_view};
use crate::cli_build_counters::{policy_yaml, write_diff, ws_dep};

/// A `job.<i>.<cmd>` span name.
fn is_job(name: &str) -> bool {
    name.strip_prefix("job.")
        .and_then(|r| r.split('.').next())
        .is_some_and(|i| i.parse::<usize>().is_ok())
}

/// What the trace holds at each point that matters, by event order: each
/// `job.step` with the job spans open around it, and each fill with the job
/// and view spans open around it.
struct Walk {
    steps: Vec<(usize, String, Vec<String>)>,
    fills: Vec<(Vec<String>, Vec<String>)>,
}

fn walk(events: &[Value]) -> Walk {
    let (mut jobs, mut views): (Vec<String>, Vec<String>) = (vec![], vec![]);
    let mut out = Walk {
        steps: vec![],
        fills: vec![],
    };
    for e in events {
        let Some(name) = e["name"].as_str() else {
            continue;
        };
        let open = if is_job(name) {
            Some(&mut jobs)
        } else if is_view(name) {
            Some(&mut views)
        } else {
            None
        };
        match (e["ph"].as_str(), open) {
            (Some("B"), Some(open)) => open.push(name.to_string()),
            (Some("E"), Some(open)) => {
                let at = open.iter().rposition(|n| n == name).expect("an open span");
                open.remove(at);
            }
            (Some("i"), _) if name == "job.step" => {
                let a = &e["args"];
                let job = usize::try_from(a["job"].as_u64().unwrap()).unwrap();
                let step = a["step"].as_str().unwrap().to_string();
                out.steps.push((job, step, jobs.clone()));
            }
            (Some("C"), _) if name.starts_with("build.") => {
                out.fills.push((jobs.clone(), views.clone()));
            }
            _ => {}
        }
    }
    assert!(
        jobs.is_empty() && views.is_empty(),
        "unclosed spans: {jobs:?} {views:?}"
    );
    out
}

#[test]
fn batch_trace_split_per_job() {
    let (d, ws) = ws_dep();
    let diff = write_diff(d.path(), "src/Work.al", 18, "        Customer.Insert();");
    let policy = policy_yaml();
    let jobs = vec![
        argv(&["analyze", &ws, "--format", "json"]),
        argv(&["digest", &ws, "--diff", &diff]),
        argv(&[
            "policy", "check", &ws, "--policy", &policy, "--format", "json",
        ]),
        argv(&["events", "fanout", &ws, "--format", "json"]),
        argv(&["query", "touches", &ws, "--table", "18", "--format", "json"]),
        argv(&["graph", &ws]),
        // The cut reader: fingerprint B0 on the single-app view.
        argv(&["fingerprint", &ws, "--format", "json"]),
    ];
    const CUT: usize = 6;
    let run = batch(&jobs);
    assert_eq!(run.out.status.code(), Some(0));
    let w = walk(&run.events);
    // Precondition: every job has every step it goes through.
    for j in 0..jobs.len() {
        let mine: Vec<&str> = w
            .steps
            .iter()
            .filter(|s| s.0 == j)
            .map(|s| s.1.as_str())
            .collect();
        for step in ["compute", "finalize", "serialize"] {
            assert!(mine.contains(&step), "job {j} has no {step}: {mine:?}");
        }
    }
    for (job, step, holders) in &w.steps {
        assert_eq!(holders.len(), 1, "{step} of job {job} in {holders:?}");
        assert!(
            holders[0].starts_with(&format!("job.{job}.")),
            "{step} of job {job} in {}",
            holders[0]
        );
    }
    // Precondition: the cut reader composes (its fills are in the trace).
    let cut = format!("job.{CUT}.fingerprint");
    assert!(
        w.fills
            .iter()
            .any(|(jobs, _)| jobs == std::slice::from_ref(&cut)),
        "precondition: the cut reader's own fills"
    );
    assert!(
        w.fills.iter().any(|(jobs, _)| jobs.is_empty()),
        "precondition: the views' fills"
    );
    for (i, (jobs, views)) in w.fills.iter().enumerate() {
        assert_eq!(views.len(), 1, "fill {i} in views {views:?}");
        assert!(
            jobs.is_empty() || jobs == std::slice::from_ref(&cut),
            "fill {i} inside {jobs:?}"
        );
    }
}
