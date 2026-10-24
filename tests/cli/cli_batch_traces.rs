//! #277 T6 (D10): a batch trace splits per job. Every `job.step` (resolve,
//! compute, finalize, serialize) lies inside exactly one `job.<i>.<cmd>` span,
//! its own job's; every fill (a `build.*` counter) lies inside a `view.<i>`
//! span and inside no job span. The jobs hold no cut reader (fingerprint,
//! prove and diff compose their own cut inside compute, D6.4; a stated
//! limit of the rule).

use crate::cli_batch_plan::{argv, batch, fills, is_view, spans};
use crate::cli_build_counters::{policy_yaml, write_diff, ws_dep};

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
    ];
    let run = batch(&jobs);
    assert_eq!(run.out.status.code(), Some(0));
    let job_spans = spans(&run.events, |n| {
        n.strip_prefix("job.")
            .and_then(|r| r.split('.').next())
            .is_some_and(|i| i.parse::<usize>().is_ok())
    });
    let views = spans(&run.events, is_view);
    let steps: Vec<(f64, usize, String)> = run
        .events
        .iter()
        .filter(|e| e["ph"] == "i" && e["name"] == "job.step")
        .map(|e| {
            let a = &e["args"];
            let job = usize::try_from(a["job"].as_u64().unwrap()).unwrap();
            (
                e["ts"].as_f64().unwrap(),
                job,
                a["step"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    // Precondition: every job has every step it goes through.
    for j in 0..jobs.len() {
        let mine: Vec<&str> = steps
            .iter()
            .filter(|s| s.1 == j)
            .map(|s| s.2.as_str())
            .collect();
        for step in ["compute", "finalize", "serialize"] {
            assert!(mine.contains(&step), "job {j} has no {step}: {mine:?}");
        }
    }
    for (ts, job, step) in &steps {
        let holders: Vec<&String> = job_spans
            .iter()
            .filter(|(_, b, e)| b <= ts && ts <= e)
            .map(|(n, ..)| n)
            .collect();
        assert_eq!(holders.len(), 1, "{step} of job {job} in {holders:?}");
        assert!(
            holders[0].starts_with(&format!("job.{job}.")),
            "{step} of job {job} in {}",
            holders[0]
        );
    }
    let fills = fills(&run.events);
    assert!(!fills.is_empty(), "precondition: fills");
    for t in fills {
        assert!(
            views.iter().any(|(_, b, e)| *b <= t && t <= *e),
            "fill at {t} in no view"
        );
        let inside: Vec<&String> = job_spans
            .iter()
            .filter(|(_, b, e)| *b <= t && t <= *e)
            .map(|(n, ..)| n)
            .collect();
        assert!(inside.is_empty(), "fill at {t} inside {inside:?}");
    }
}
